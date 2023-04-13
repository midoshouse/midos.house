use {
    async_graphql::{
        *,
        http::{
            GraphQLPlaygroundConfig,
            playground_source,
        },
    },
    async_graphql_rocket::{
        GraphQLQuery,
        GraphQLRequest,
        GraphQLResponse,
    },
    enum_iterator::all,
    rocket::{
        State,
        http::{
            ContentType,
            Status,
        },
        response::content::RawHtml,
    },
    serde::Serialize,
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
    wheel::traits::ReqwestResponseExt as _,
    crate::{
        Environment,
        auth::Discriminator,
        event::{
            self,
            Series,
        },
        racetime_bot,
        user::User,
        util::{
            Id,
            StatusOrError,
        },
    },
};

#[derive(Default)]
struct Scopes {
    entrants_read: bool,
}

impl Scopes {
    async fn validate(&self, transaction: &mut Transaction<'_, Postgres>, api_key: &str) -> sqlx::Result<Option<User>> {
        let Some(row) = sqlx::query!(r#"SELECT user_id AS "user_id: Id", entrants_read FROM api_keys WHERE key = $1"#, api_key).fetch_optional(&mut *transaction).await? else { return Ok(None) };
        let Self { entrants_read } = *self;
        if entrants_read && !row.entrants_read { return Ok(None) }
        User::from_id(transaction, row.user_id).await
    }
}

type MidosHouseSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub(crate) struct Query;

#[Object] impl Query {
    async fn goal_names(&self) -> Vec<&'static str> {
        all::<racetime_bot::Goal>().filter(|goal| goal.is_custom()).map(|goal| goal.as_str()).collect()
    }
}

#[rocket::get("/api/v1/graphql?<query..>")]
pub(crate) async fn graphql_query(schema: &State<MidosHouseSchema>, query: GraphQLQuery) -> GraphQLResponse {
    query.execute(&**schema).await
}

#[rocket::post("/api/v1/graphql", data = "<request>", format = "application/json")]
pub(crate) async fn graphql_request(schema: &State<MidosHouseSchema>, request: GraphQLRequest) -> GraphQLResponse {
    request.execute(&**schema).await
}

#[rocket::get("/api/v1/graphql")]
pub(crate) fn graphql_playground() -> RawHtml<String> {
    RawHtml(playground_source(GraphQLPlaygroundConfig::new("/api/v1/graphql")))
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum CsvError {
    #[error(transparent)] Csv(#[from] csv::Error),
    #[error(transparent)] Event(#[from] event::Error),
    #[error(transparent)] EventData(#[from] event::DataError),
    #[error(transparent)] IntoInner(#[from] csv::IntoInnerError<csv::Writer<Vec<u8>>>),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
}

impl<E: Into<CsvError>> From<E> for StatusOrError<CsvError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::get("/api/v1/event/<series>/<event>/entrants.csv?<api_key>")]
pub(crate) async fn entrants_csv(db_pool: &State<PgPool>, http_client: &State<reqwest::Client>, env: &State<Environment>, series: Series, event: &str, api_key: &str) -> Result<(ContentType, Vec<u8>), StatusOrError<CsvError>> {
    let mut transaction = db_pool.begin().await?;
    let me = Scopes { entrants_read: true, ..Scopes::default() }.validate(&mut transaction, api_key).await?.ok_or(StatusOrError::Status(Status::Forbidden))?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    if !event.organizers(&mut transaction).await?.contains(&me) && !event.restreamers(&mut transaction).await?.contains(&me) {
        return Err(StatusOrError::Status(Status::Forbidden))
    }
    let show_qualifier_times = event.show_qualifier_times && event.is_started(&mut transaction).await?;
    let signups = event.signups_sorted(&mut transaction, None, show_qualifier_times).await?;
    let mut csv = csv::Writer::from_writer(Vec::default());
    for (i, (team, _, _, _)) in signups.into_iter().enumerate() {
        for member in team.members(&mut transaction).await? {
            #[derive(Serialize)]
            struct Row<'a> {
                id: Id,
                display_name: &'a str,
                twitch_display_name: Option<String>,
                discord_display_name: Option<&'a str>,
                discord_discriminator: Option<Discriminator>,
                racetime_id: Option<&'a str>,
                qualifier_rank: usize,
                restream_consent: bool,
            }

            let twitch_display_name = if let Some(ref racetime_id) = member.racetime_id {
                http_client.get(format!("https://{}/user/{racetime_id}/data", env.racetime_host()))
                    .send().await?
                    .detailed_error_for_status().await?
                    .json_with_text_in_error::<racetime::model::UserData>().await?
                    .twitch_display_name
            } else {
                None
            };
            csv.serialize(Row {
                id: member.id,
                display_name: member.display_name(),
                discord_display_name: member.discord_display_name.as_deref(),
                discord_discriminator: member.discord_discriminator,
                racetime_id: member.racetime_id.as_deref(),
                qualifier_rank: i + 1,
                restream_consent: team.restream_consent,
                twitch_display_name,
            })?;
        }
    }
    Ok((ContentType::CSV, csv.into_inner()?))
}
