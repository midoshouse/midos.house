use {
    std::sync::Arc,
    async_graphql::{
        *,
        http::{
            GraphQLPlaygroundConfig,
            playground_source,
        },
        types::ID as GqlId,
    },
    async_graphql_rocket::{
        GraphQLQuery,
        GraphQLRequest,
        GraphQLResponse,
    },
    chrono::prelude::*,
    enum_iterator::all,
    itertools::Itertools as _,
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
    tokio::sync::Mutex,
    wheel::traits::ReqwestResponseExt as _,
    crate::{
        Config,
        Environment,
        auth::Discriminator,
        cal::{
            self,
            RaceSchedule,
        },
        event::{
            self,
            TeamConfig,
        },
        racetime_bot,
        team,
        user,
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
    async fn validate(&self, transaction: &mut Transaction<'_, Postgres>, api_key: &str) -> sqlx::Result<Option<user::User>> {
        let Some(row) = sqlx::query!(r#"SELECT user_id AS "user_id: Id", entrants_read FROM api_keys WHERE key = $1"#, api_key).fetch_optional(&mut *transaction).await? else { return Ok(None) };
        let Self { entrants_read } = *self;
        if entrants_read && !row.entrants_read { return Ok(None) }
        user::User::from_id(transaction, row.user_id).await
    }
}

type ArcTransaction = Arc<Mutex<Transaction<'static, Postgres>>>;

macro_rules! db {
    ($ctx:expr) => {{
        &mut *$ctx.data_unchecked::<ArcTransaction>().lock().await
    }};
}

struct UtcTimestamp(DateTime<Utc>);

impl<Tz: TimeZone> From<DateTime<Tz>> for UtcTimestamp {
    fn from(value: DateTime<Tz>) -> Self {
        Self(value.with_timezone(&Utc))
    }
}

#[Scalar]
/// A date and time in UTC, formatted per ISO 8601.
impl ScalarType for UtcTimestamp {
    fn parse(value: Value) -> InputValueResult<Self> {
        if let Value::String(s) = value {
            Ok(Self(DateTime::parse_from_rfc3339(&s).map_err(InputValueError::custom)?.with_timezone(&Utc)))
        } else {
            Err(InputValueError::expected_type(value))
        }
    }

    fn to_value(&self) -> Value {
        Value::String(self.0.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }

    fn is_valid(value: &Value) -> bool {
        if let Value::String(s) = value {
            DateTime::parse_from_rfc3339(s).is_ok()
        } else {
            false
        }
    }
}

type MidosHouseSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub(crate) struct Query;

#[Object] impl Query {
    /// Custom racetime.gg goals in the OoTR category handled by Mido instead of RandoBot.
    async fn goal_names(&self) -> Vec<&'static str> {
        all::<racetime_bot::Goal>().filter(|goal| goal.is_custom()).map(|goal| goal.as_str()).collect()
    }

    /// Returns a series (group of events) by its URL part.
    async fn series(&self, name: String) -> Option<Series> {
        name.parse().ok().map(Series)
    }
}

struct Series(event::Series);

#[Object] impl Series {
    /// Returns an event by its URL part.
    async fn event(&self, ctx: &Context<'_>, name: String) -> Result<Option<Event>, event::DataError> {
        Ok(event::Data::new(db!(ctx), self.0, name).await?.map(Event))
    }
}

struct Event(event::Data<'static>);

#[Object] impl Event {
    /// All past, upcoming, and unscheduled races for this event, sorted chronologically.
    async fn races(&self, ctx: &Context<'_>) -> Result<Vec<Race>, cal::Error> {
        Ok(cal::Race::for_event(db!(ctx), ctx.data_unchecked(), ctx.data_unchecked(), ctx.data_unchecked(), &self.0).await?.into_iter().map(Race).collect())
    }
}

struct Race(cal::Race);

#[Object] impl Race {
    /// The race's internal ID. Unique across all series, but only for races (e.g. a user may have the same ID as a race).
    async fn id(&self) -> GqlId { self.0.id.unwrap().into() }

    /// The scheduled starting time. Null if this race is asynced or not yet scheduled.
    async fn start(&self) -> Option<UtcTimestamp> {
        if let RaceSchedule::Live { start, .. } = self.0.schedule {
            Some(start.into())
        } else {
            None
        }
    }

    /// A categorization of races within the event, e.g. “Swiss”, “Challenge Cup”, “Live Qualifier”, “Top 8”, “Groups”, or “Bracket”. Combine with round, entrants, and game for a human-readable description of the race.
    /// Null if this event only has one phase or for the main phase of the event (e.g. Standard top 64 as opposed to Challenge Cup).
    async fn phase(&self) -> Option<&str> { self.0.phase.as_deref() }

    /// A categorization of races within the phase, e.g. “Round 1”, “Openers”, or “Losers Quarterfinal”. Combine with phase, entrants, and game for a human-readable description of the race.
    /// Null if this phase only has one match.
    async fn round(&self) -> Option<&str> { self.0.round.as_deref() }

    /// If this race is part of a best-of-N-races match, the ordinal of the race within the match. Null for best-of-1 matches.
    async fn game(&self) -> Option<i16> { self.0.game }

    /// All teams participating in this race. For solo events, these will be single-member teams.
    /// Null if the race is open (not invitational) or if the event does not use Mido's House to manage entrants.
    async fn teams(&self, ctx: &Context<'_>) -> Result<Option<Vec<Team>>, event::DataError> {
        let event = self.0.event(db!(ctx)).await?;
        Ok(self.0.teams_opt().map(|teams| teams.map(|team| Team { inner: team.clone(), event: event.clone() }).collect()))
    }
}

struct Team {
    inner: team::Team,
    event: event::Data<'static>,
}

#[Object] impl Team {
    /// The team's internal ID. Unique across all series, but only for teams (e.g. a race may have the same ID as a team).
    async fn id(&self) -> GqlId { self.inner.id.into() }

    /// Members are guaranteed to be listed in a consistent order depending on the team configuration of the event, e.g. pictionary events will always list the runner first and the pilot second.
    async fn members(&self, ctx: &Context<'_>) -> sqlx::Result<Vec<TeamMember>> {
        let team_config = self.event.team_config();
        let roles = team_config.roles();
        let mut members = self.inner.members_roles(db!(ctx)).await?;
        members.sort_unstable_by_key(|(_, role)| roles.iter().position(|(iter_role, _)| iter_role == role));
        Ok(
            members.into_iter().zip_eq(roles)
                .map(|((user, _), (_, display_name))| TeamMember {
                    role: (!matches!(team_config, TeamConfig::Solo)).then(|| (*display_name).to_owned()),
                    user: User(user),
                })
                .collect()
        )
    }
}

#[derive(SimpleObject)]
struct TeamMember {
    /// The role of the player within this team for team formats with distinct roles (e.g. multiworld, pictionary).
    /// For team formats without distinct roles (e.g. co-op), this can be used to get a consistent ordering of the members of a team.
    /// Null for solo events.
    role: Option<String>,
    user: User,
}

struct User(user::User);

#[Object] impl User {
    /// The user's internal ID. Only unique for users (e.g. a team may have the same ID as a user).
    async fn id(&self) -> GqlId { self.0.id.into() }
}

pub(crate) fn schema(db_pool: PgPool) -> MidosHouseSchema {
    Schema::build(Query, EmptyMutation, EmptySubscription)
        .data(db_pool)
        .finish()
}

#[rocket::get("/api/v1/graphql?<query..>")]
pub(crate) async fn graphql_query(env: &State<Environment>, config: &State<Config>, db_pool: &State<PgPool>, http_client: &State<reqwest::Client>, schema: &State<MidosHouseSchema>, query: GraphQLQuery) -> Result<GraphQLResponse, rocket_util::Error<sqlx::Error>> {
    let transaction = Arc::new(Mutex::new(db_pool.begin().await?));
    let response = GraphQLRequest::from(query)
        .data::<Environment>(**env)
        .data::<Config>((*config).clone())
        .data::<ArcTransaction>(transaction.clone())
        .data::<reqwest::Client>((*http_client).clone())
        .execute(&**schema).await;
    Arc::try_unwrap(transaction).expect("query data still live after execution").into_inner().commit().await?;
    Ok(response)
}

#[rocket::post("/api/v1/graphql", data = "<request>", format = "application/json")]
pub(crate) async fn graphql_request(env: &State<Environment>, config: &State<Config>, db_pool: &State<PgPool>, http_client: &State<reqwest::Client>, schema: &State<MidosHouseSchema>, request: GraphQLRequest) -> Result<GraphQLResponse, rocket_util::Error<sqlx::Error>> {
    let transaction = Arc::new(Mutex::new(db_pool.begin().await?));
    let response = request
        .data::<Environment>(**env)
        .data::<Config>((*config).clone())
        .data::<ArcTransaction>(transaction.clone())
        .data::<reqwest::Client>((*http_client).clone())
        .execute(&**schema).await;
    Arc::try_unwrap(transaction).expect("query data still live after execution").into_inner().commit().await?;
    Ok(response)
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
pub(crate) async fn entrants_csv(db_pool: &State<PgPool>, http_client: &State<reqwest::Client>, env: &State<Environment>, series: event::Series, event: &str, api_key: &str) -> Result<(ContentType, Vec<u8>), StatusOrError<CsvError>> {
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
