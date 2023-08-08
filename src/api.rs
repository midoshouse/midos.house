use {
    std::{
        cmp::Ordering::{
            self,
            *,
        },
        sync::Arc,
    },
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
    async_trait::async_trait,
    chrono::prelude::*,
    enum_iterator::all,
    itertools::Itertools as _,
    log_lock::{
        Mutex,
        lock,
    },
    rocket::{
        State,
        http::{
            ContentType,
            Status,
        },
        request::{
            self,
            FromRequest,
            Request,
        },
        response::content::RawHtml,
    },
    serde::Serialize,
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
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
            teams,
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

macro_rules! db {
    ($ctx:expr) => {{
        &mut *lock!($ctx.data_unchecked::<ArcTransaction>())
    }};
}

#[derive(Default, PartialEq, Eq)]
struct Scopes {
    entrants_read: bool,
    user_search: bool,
}

impl Scopes {
    async fn validate(&self, transaction: &mut Transaction<'_, Postgres>, api_key: &str) -> sqlx::Result<Option<user::User>> {
        let Some(key_scope) = sqlx::query_as!(Self, "SELECT entrants_read, user_search FROM api_keys WHERE key = $1", api_key).fetch_optional(&mut **transaction).await? else { return Ok(None) };
        if key_scope >= *self {
            let user_id = sqlx::query_scalar!(r#"SELECT user_id AS "user_id: Id" FROM api_keys WHERE key = $1"#, api_key).fetch_one(&mut **transaction).await?;
            user::User::from_id(&mut **transaction, user_id).await
        } else {
            Ok(None)
        }
    }
}

impl PartialOrd for Scopes {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let mut any_less = false;
        let mut any_greater = false;

        macro_rules! compare_fields {
            ($($field:ident,)+) => {
                let Self { $($field),+ } = *self;
                $(
                    match ($field, other.$field) {
                        (false, false) | (true, true) => {}
                        (false, true) => any_less = true,
                        (true, false) => any_greater = true,
                    }
                )+
            };
        }

        compare_fields![
            entrants_read,
            user_search,
        ];
        match (any_less, any_greater) {
            (false, false) => Some(Equal),
            (false, true) => Some(Greater),
            (true, false) => Some(Less),
            (true, true) => None,
        }
    }
}

#[async_trait]
impl Guard for Scopes {
    async fn check(&self, ctx: &Context<'_>) -> Result<()> {
        if ctx.data::<ApiKey>()?.scopes >= *self {
            Ok(())
        } else {
            Err("API key missing scopes".into())
        }
    }
}

struct ShowRestreamConsent<'a>(&'a cal::Race);

#[async_trait]
impl Guard for ShowRestreamConsent<'_> {
    async fn check(&self, ctx: &Context<'_>) -> Result<()> {
        let me = &ctx.data::<ApiKey>()?.user;
        let event = self.0.event(db!(ctx)).await?;
        if event.organizers(db!(ctx)).await?.contains(me) || event.restreamers(db!(ctx)).await?.contains(me) {
            Ok(())
        } else {
            Err("Only event organizers and restreamers can view restream consent info".into())
        }
    }
}

type ArcTransaction = Arc<Mutex<Transaction<'static, Postgres>>>;

struct UtcTimestamp(DateTime<Utc>);

impl<Z: TimeZone> From<DateTime<Z>> for UtcTimestamp {
    fn from(value: DateTime<Z>) -> Self {
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

#[derive(Debug, thiserror::Error)]
enum UserFromDiscordError {
    #[error(transparent)] DiscordUserIdParse(#[from] serenity::all::UserIdParseError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

#[Object] impl Query {
    /// Custom racetime.gg goals in the OoTR category handled by Mido instead of RandoBot.
    async fn goal_names(&self) -> Vec<&'static str> {
        all::<racetime_bot::Goal>().filter(|goal| goal.is_custom()).map(|goal| goal.as_str()).collect()
    }

    /// Returns a series (group of events) by its URL part.
    async fn series(&self, name: String) -> Option<Series> {
        name.parse().ok().map(Series)
    }

    /// Returns the Mido's House user connected to the given racetime.gg user ID, if any.
    /// Requires an API key with `user_search` scope.
    #[graphql(guard = "Scopes { user_search: true, ..Scopes::default() }")]
    async fn user_from_racetime(&self, ctx: &Context<'_>, id: GqlId) -> sqlx::Result<Option<User>> {
        Ok(user::User::from_racetime(&mut **db!(ctx), id.as_str()).await?.map(User))
    }

    /// Returns the Mido's House user connected to the given Discord user snowflake ID, if any.
    /// Requires an API key with `user_search` scope.
    #[graphql(guard = "Scopes { user_search: true, ..Scopes::default() }")]
    async fn user_from_discord(&self, ctx: &Context<'_>, id: GqlId) -> Result<Option<User>, UserFromDiscordError> {
        Ok(user::User::from_discord(&mut **db!(ctx), id.parse()?).await?.map(User))
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
        Ok(cal::Race::for_event(db!(ctx), ctx.data_unchecked(), *ctx.data_unchecked(), ctx.data_unchecked(), &self.0).await?.into_iter().map(Race).collect())
    }
}

struct Race(cal::Race);

#[Object] impl Race {
    /// The race's internal ID. Unique across all series, but only for races (e.g. a user may have the same ID as a race).
    async fn id(&self) -> GqlId { self.0.id.into() }

    /// The scheduled starting time. Null if this race is asynced or not yet scheduled.
    async fn start(&self) -> Option<UtcTimestamp> {
        if let RaceSchedule::Live { start, .. } = self.0.schedule {
            Some(start.into())
        } else {
            None
        }
    }

    /// The race room URL. Null if no room has been opened yet or if this race is asynced.
    async fn room(&self) -> Option<&str> {
        if let RaceSchedule::Live { ref room, .. } = self.0.schedule {
            room.as_ref().map(|url| url.as_str())
        } else {
            None
        }
    }

    /// A categorization of races within the event, e.g. “Swiss”, “Challenge Cup”, “Live Qualifier”, “Top 8”, “Groups”, or “Bracket”. Combine with round, entrants, and game for a human-readable description of the race.
    /// Null if this event only has one phase or for the main phase of the event (e.g. Standard top 64 as opposed to Challenge Cup).
    async fn phase(&self) -> Option<&str> { self.0.phase.as_deref() }

    /// A categorization of races within the phase, e.g. “Round 1”, “Openers”, or “Losers Quarterfinal”. Combine with phase, entrants, and game for a human-readable description of the race.
    /// Null if this phase only has one match or if all matches in this phase are equivalent (e.g. a leaderboard phase).
    async fn round(&self) -> Option<&str> { self.0.round.as_deref() }

    /// If this race is part of a best-of-N-races match, the ordinal of the race within the match, counting from 1. Null for best-of-1 matches.
    async fn game(&self) -> Option<i16> { self.0.game }

    /// All teams participating in this race. For solo events, these will be single-member teams.
    /// Null if the race is open (not invitational) or if the event does not use Mido's House to manage entrants.
    async fn teams(&self, ctx: &Context<'_>) -> Result<Option<Vec<Team>>, event::DataError> {
        let event = self.0.event(db!(ctx)).await?;
        Ok(self.0.teams_opt().map(|teams| teams.map(|team| Team { inner: team.clone(), event: event.clone() }).collect()))
    }

    /// Whether all teams in this race have consented to be restreamed.
    /// Null if the race is open (not invitational) or if the event does not use Mido's House to manage entrants.
    /// Requires permission to view restream consent and an API key with `entrants_read` scope.
    #[graphql(guard = "Scopes { entrants_read: true, ..Scopes::default() }.and(ShowRestreamConsent(&self.0))")]
    async fn restream_consent(&self) -> Option<bool> {
        self.0.teams_opt().map(|mut teams| teams.all(|team| team.restream_consent))
    }
}

struct Team {
    inner: team::Team,
    event: event::Data<'static>,
}

#[Object] impl Team {
    /// The team's internal ID. Unique across all series, but only for teams (e.g. a race may have the same ID as a team).
    async fn id(&self) -> GqlId { self.inner.id.into() }

    /// The team's display name. Null for solo events or if the team did not specify a name.
    async fn name(&self) -> Option<&str> { self.inner.name.as_deref() }

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

    /// The user's Mido's House display name.
    async fn display_name(&self) -> &str { self.0.display_name() }


    /// Returns the user's connected racetime.gg user ID, if any.
    async fn racetime_id(&self) -> Option<&str> {
        self.0.racetime.as_ref().map(|racetime| &*racetime.id)
    }

    /// Returns the user's connected Discord user snowflake ID, if any.
    async fn discord_id(&self) -> Option<ID> {
        self.0.discord.as_ref().map(|discord| discord.id.into())
    }
}

pub(crate) fn schema(db_pool: PgPool) -> MidosHouseSchema {
    Schema::build(Query, EmptyMutation, EmptySubscription)
        .data(db_pool)
        .finish()
}

pub(crate) struct ApiKey {
    scopes: Scopes,
    user: user::User,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ApiKeyFromRequestError {
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("failed to get database connection pool")]
    DbPool,
    #[error("the X-API-Key header was not specified")]
    MissingHeader,
    #[error("the X-API-Key header was specified multiple times")]
    MultipleHeaders,
    #[error("the given API key does not exist")]
    NoSuchApiKey,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for ApiKey {
    type Error = ApiKeyFromRequestError;

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        let db_pool = match <&State<PgPool>>::from_request(req).await {
            request::Outcome::Success(db_pool) => db_pool,
            request::Outcome::Forward(()) => return request::Outcome::Forward(()),
            request::Outcome::Failure((status, ())) => return request::Outcome::Failure((status, ApiKeyFromRequestError::DbPool)),
        };
        match req.headers().get("X-API-Key").at_most_one() {
            Ok(Some(api_key)) => match sqlx::query!(r#"SELECT
                entrants_read,
                user_search,
                user_id AS "user_id: Id"
            FROM api_keys WHERE key = $1"#, api_key).fetch_optional(&**db_pool).await {
                Ok(Some(row)) => request::Outcome::Success(Self {
                    scopes: Scopes {
                        entrants_read: row.entrants_read,
                        user_search: row.user_search,
                    },
                    user: match user::User::from_id(&**db_pool, row.user_id).await {
                        Ok(user) => user.expect("database constraint validated: API keys belong to existing users"),
                        Err(e) => return request::Outcome::Failure((Status::InternalServerError, ApiKeyFromRequestError::Sql(e))),
                    },
                }),
                Ok(None) => request::Outcome::Failure((Status::Unauthorized, ApiKeyFromRequestError::NoSuchApiKey)),
                Err(e) => request::Outcome::Failure((Status::InternalServerError, ApiKeyFromRequestError::Sql(e))),
            },
            Ok(None) => request::Outcome::Failure((Status::Unauthorized, ApiKeyFromRequestError::MissingHeader)),
            Err(_) => request::Outcome::Failure((Status::Unauthorized, ApiKeyFromRequestError::MultipleHeaders)),
        }
    }
}

#[rocket::get("/api/v1/graphql?<query..>")]
pub(crate) async fn graphql_query(env: &State<Environment>, config: &State<Config>, db_pool: &State<PgPool>, http_client: &State<reqwest::Client>, schema: &State<MidosHouseSchema>, api_key: Option<ApiKey>, query: GraphQLQuery) -> Result<GraphQLResponse, rocket_util::Error<sqlx::Error>> {
    let transaction = Arc::new(Mutex::new(db_pool.begin().await?));
    let mut request = GraphQLRequest::from(query)
        .data::<Environment>(**env)
        .data::<Config>((*config).clone())
        .data::<ArcTransaction>(transaction.clone())
        .data::<reqwest::Client>((*http_client).clone());
    if let Some(api_key) = api_key {
        request = request.data::<ApiKey>(api_key);
    }
    let response = request.execute(&**schema).await;
    Arc::try_unwrap(transaction).expect("query data still live after execution").into_inner().commit().await?;
    Ok(response)
}

#[rocket::post("/api/v1/graphql", data = "<request>", format = "application/json")]
pub(crate) async fn graphql_request(env: &State<Environment>, config: &State<Config>, db_pool: &State<PgPool>, http_client: &State<reqwest::Client>, schema: &State<MidosHouseSchema>, api_key: Option<ApiKey>, request: GraphQLRequest) -> Result<GraphQLResponse, rocket_util::Error<sqlx::Error>> {
    let transaction = Arc::new(Mutex::new(db_pool.begin().await?));
    let mut request = request
        .data::<Environment>(**env)
        .data::<Config>((*config).clone())
        .data::<ArcTransaction>(transaction.clone())
        .data::<reqwest::Client>((*http_client).clone());
    if let Some(api_key) = api_key {
        request = request.data::<ApiKey>(api_key);
    }
    let response = request.execute(&**schema).await;
    Arc::try_unwrap(transaction).expect("query data still live after execution").into_inner().commit().await?;
    Ok(response)
}

#[rocket::get("/api/v1/graphql")]
pub(crate) fn graphql_playground() -> RawHtml<String> {
    RawHtml(playground_source(GraphQLPlaygroundConfig::new("/api/v1/graphql")))
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum CsvError {
    #[error(transparent)] Cal(#[from] cal::Error),
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
pub(crate) async fn entrants_csv(db_pool: &State<PgPool>, http_client: &State<reqwest::Client>, env: &State<Environment>, config: &State<Config>, series: event::Series, event: &str, api_key: &str) -> Result<(ContentType, Vec<u8>), StatusOrError<CsvError>> {
    let mut transaction = db_pool.begin().await?;
    let me = Scopes { entrants_read: true, ..Scopes::default() }.validate(&mut transaction, api_key).await?.ok_or(StatusOrError::Status(Status::Forbidden))?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    if !event.organizers(&mut transaction).await?.contains(&me) && !event.restreamers(&mut transaction).await?.contains(&me) {
        return Err(StatusOrError::Status(Status::Forbidden))
    }
    let qualifier_kind = teams::QualifierKind::Single { //TODO adjust to match teams::get?
        show_times: event.show_qualifier_times && event.is_started(&mut transaction).await?,
    };
    let signups = teams::signups_sorted(&mut transaction, http_client, **env, config, None, &event, qualifier_kind).await?;
    let mut csv = csv::Writer::from_writer(Vec::default());
    for (i, teams::SignupsTeam { team, .. }) in signups.into_iter().enumerate() {
        if let Some(team) = team {
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
                    discord_username: Option<&'a str>,
                }

                csv.serialize(Row {
                    id: member.id,
                    display_name: member.display_name(),
                    twitch_display_name: member.racetime_user_data(**env, http_client).await?.and_then(|racetime_user_data| racetime_user_data.twitch_display_name),
                    discord_display_name: member.discord.as_ref().map(|discord| &*discord.display_name),
                    discord_discriminator: member.discord.as_ref().and_then(|discord| discord.username_or_discriminator.as_ref().right()).copied(),
                    racetime_id: member.racetime.as_ref().map(|racetime| &*racetime.id),
                    qualifier_rank: i + 1,
                    restream_consent: team.restream_consent,
                    discord_username: member.discord.as_ref().and_then(|discord| discord.username_or_discriminator.as_ref().left()).map(|username| &**username),
                })?;
            }
        }
    }
    Ok((ContentType::CSV, csv.into_inner()?))
}
