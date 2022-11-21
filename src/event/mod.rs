use {
    std::{
        borrow::Cow,
        collections::HashSet,
        convert::identity,
        fmt,
        io,
        pin::Pin,
        str::FromStr,
        time::Duration,
    },
    anyhow::anyhow,
    chrono::prelude::*,
    futures::{
        future::Future,
        stream::TryStreamExt as _,
    },
    once_cell::sync::Lazy,
    rand::prelude::*,
    rocket::{
        FromForm,
        State,
        form::{
            self,
            Context,
            Contextual,
            Form,
        },
        http::{
            Status,
            impl_from_uri_param_identity,
            uri::{
                self,
                fmt::{
                    Path,
                    UriDisplay,
                },
            },
        },
        request::FromParam,
        response::{
            Redirect,
            content::RawHtml,
        },
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        ContextualExt as _,
        CsrfForm,
        Origin,
        ToHtml,
        html,
    },
    serenity::{
        client::Context as DiscordCtx,
        model::prelude::*,
        utils::MessageBuilder,
    },
    serenity_utils::RwFuture,
    sqlx::{
        Decode,
        Encode,
        Transaction,
        postgres::{
            Postgres,
            PgArgumentBuffer,
            PgPool,
            PgTypeInfo,
            PgValueRef,
        },
    },
    url::Url,
    wheel::traits::ReqwestResponseExt as _,
    crate::{
        Environment,
        auth,
        cal::{
            self,
            Race,
            RaceSchedule,
        },
        config::Config,
        favicon::ChestAppearances,
        http::{
            PageError,
            PageStyle,
            page,
        },
        notification::SimpleNotificationKind,
        seed,
        team::Team,
        user::User,
        util::{
            DateTimeFormat,
            EmptyForm,
            Id,
            IdTable,
            MessageBuilderExt as _,
            RedirectOrContent,
            StatusOrError,
            decode_pginterval,
            favicon,
            form_field,
            format_datetime,
            format_duration,
            parse_duration,
            render_form_error,
        },
    },
};

pub(crate) mod mw;
mod ndos;
pub(crate) mod pic;
mod rsl;
mod s;

#[derive(Debug, Clone, Copy, sqlx::Type)]
#[sqlx(type_name = "signup_status", rename_all = "snake_case")]
pub(crate) enum SignupStatus {
    Created,
    Confirmed,
    Unconfirmed,
}

impl SignupStatus {
    fn is_confirmed(&self) -> bool {
        matches!(self, Self::Created | Self::Confirmed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "team_role", rename_all = "snake_case")]
pub(crate) enum Role {
    /// For solo events.
    None,
    /// Player 1 of 2. “Runner” in Pictionary.
    Sheikah,
    /// Player 2 of 2. “Pilot” in Pictionary.
    Gerudo,
    /// Player 1 of 3.
    Power,
    /// Player 2 of 3.
    Wisdom,
    /// Player 3 of 3.
    Courage,
}

impl Role {
    fn from_css_class(css_class: &str) -> Option<Self> {
        match css_class {
            "sheikah" => Some(Self::Sheikah),
            "gerudo" => Some(Self::Gerudo),
            "power" => Some(Self::Power),
            "wisdom" => Some(Self::Wisdom),
            "courage" => Some(Self::Courage),
            _ => None,
        }
    }

    fn css_class(&self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Sheikah => Some("sheikah"),
            Self::Gerudo => Some("gerudo"),
            Self::Power => Some("power"),
            Self::Wisdom => Some("wisdom"),
            Self::Courage => Some("courage"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Series {
    Multiworld,
    NineDaysOfSaws,
    Pictionary,
    Rsl,
    Standard,
}

impl Series {
    fn to_str(&self) -> &'static str {
        match self {
            Self::Multiworld => "mw",
            Self::NineDaysOfSaws => "9dos",
            Self::Pictionary => "pic",
            Self::Rsl => "rsl",
            Self::Standard => "s",
        }
    }
}

impl FromStr for Series {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "9dos" => Ok(Self::NineDaysOfSaws),
            "mw" => Ok(Self::Multiworld),
            "pic" => Ok(Self::Pictionary),
            "rsl" => Ok(Self::Rsl),
            "s" => Ok(Self::Standard),
            _ => Err(()),
        }
    }
}

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.to_str(), f)
    }
}

impl<'r> Decode<'r, Postgres> for Series {
    fn decode(value: PgValueRef<'r>) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let series = <&str>::decode(value)?;
        series.parse().map_err(|()| anyhow!("unknown series: {series}").into())
    }
}

impl<'q> Encode<'q, Postgres> for Series {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> sqlx::encode::IsNull {
        self.to_str().encode(buf)
    }

    fn encode(self, buf: &mut PgArgumentBuffer) -> sqlx::encode::IsNull {
        self.to_str().encode(buf)
    }

    fn produces(&self) -> Option<PgTypeInfo> {
        self.to_str().produces()
    }

    fn size_hint(&self) -> usize {
        Encode::size_hint(&self.to_str())
    }
}

impl sqlx::Type<Postgres> for Series {
    fn type_info() -> PgTypeInfo {
        <&str>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str>::compatible(ty)
    }
}

impl<'a> FromParam<'a> for Series {
    type Error = &'a str;

    fn from_param(param: &'a str) -> Result<Self, Self::Error> {
        param.parse().map_err(|()| param)
    }
}

impl UriDisplay<Path> for Series {
    fn fmt(&self, f: &mut uri::fmt::Formatter<'_, Path>) -> fmt::Result {
        UriDisplay::fmt(self.to_str(), f) // assume all series names are URI safe
    }
}

impl_from_uri_param_identity!([Path] Series);

enum EnterFlow {
    RaceTime,
    RaceTimeDiscord,
    Extern,
}

pub(crate) enum TeamConfig {
    Solo,
    CoOp,
    Pictionary,
    Multiworld,
}

impl TeamConfig {
    fn roles(&self) -> &'static [(Role, &'static str)] {
        match self {
            Self::Solo => &[
                (Role::None, "Runner"),
            ],
            Self::CoOp => &[
                (Role::Sheikah, "Player 1"),
                (Role::Gerudo, "Player 2"),
            ],
            Self::Pictionary => &[
                (Role::Sheikah, "Runner"),
                (Role::Gerudo, "Pilot"),
            ],
            Self::Multiworld => &[
                (Role::Power, "World 1"),
                (Role::Wisdom, "World 2"),
                (Role::Courage, "World 3"),
            ],
        }
    }
}

pub(crate) struct Data<'a> {
    pub(crate) series: Series,
    pub(crate) event: Cow<'a, str>,
    pub(crate) display_name: String,
    /// The event's originally scheduled starting time, not accounting for the 24-hour deadline extension in the event of an odd number of teams for events with qualifier asyncs.
    pub(crate) base_start: Option<DateTime<Utc>>,
    pub(crate) end: Option<DateTime<Utc>>,
    url: Option<Url>,
    hide_teams_tab: bool,
    teams_url: Option<Url>,
    enter_url: Option<Url>,
    video_url: Option<Url>,
    pub(crate) discord_guild: Option<GuildId>,
    pub(crate) discord_race_room_channel: Option<ChannelId>,
    pub(crate) discord_race_results_channel: Option<ChannelId>,
    pub(crate) discord_organizer_channel: Option<ChannelId>,
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum DataError {
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error("no event with this series and identifier")]
    Missing,
}

impl<'a> Data<'a> {
    pub(crate) async fn new(transaction: &mut Transaction<'_, Postgres>, series: Series, event: impl Into<Cow<'a, str>>) -> Result<Option<Data<'a>>, DataError> {
        let event = event.into();
        sqlx::query!(r#"SELECT
            display_name,
            start,
            end_time,
            url,
            hide_teams_tab,
            teams_url,
            enter_url,
            video_url,
            discord_guild AS "discord_guild: Id",
            discord_race_room_channel AS "discord_race_room_channel: Id",
            discord_race_results_channel AS "discord_race_results_channel: Id",
            discord_organizer_channel AS "discord_organizer_channel: Id"
        FROM events WHERE series = $1 AND event = $2"#, series as _, &event).fetch_optional(transaction).await?
            .map(|row| Ok::<_, DataError>(Self {
                display_name: row.display_name,
                base_start: row.start,
                end: row.end_time,
                url: row.url.map(|url| url.parse()).transpose()?,
                hide_teams_tab: row.hide_teams_tab,
                teams_url: row.teams_url.map(|url| url.parse()).transpose()?,
                enter_url: row.enter_url.map(|url| url.parse()).transpose()?,
                video_url: row.video_url.map(|url| url.parse()).transpose()?,
                discord_guild: row.discord_guild.map(|Id(id)| id.into()),
                discord_race_room_channel: row.discord_race_room_channel.map(|Id(id)| id.into()),
                discord_race_results_channel: row.discord_race_results_channel.map(|Id(id)| id.into()),
                discord_organizer_channel: row.discord_organizer_channel.map(|Id(id)| id.into()),
                series, event,
            }))
            .transpose()
    }

    pub(crate) fn chests(&self) -> ChestAppearances {
        match (self.series, &*self.event) {
            (Series::Multiworld, "2") => ChestAppearances::VANILLA, // CAMC off or classic and no keys in overworld
            (Series::Multiworld, "3") => mw::S3Settings::random(&mut thread_rng()).chests(),
            (Series::Multiworld, _) => unimplemented!(),
            (Series::NineDaysOfSaws, _) => ChestAppearances::VANILLA, // no CAMC in SAWS
            (Series::Pictionary, _) => ChestAppearances::VANILLA, // no CAMC in Pictionary
            (Series::Rsl, "2") => {
                static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../../assets/event/rsl/chests-2-7028072.json")).expect("failed to parse chest weights"));

                WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
            }
            (Series::Rsl, "3") => {
                static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../../assets/event/rsl/chests-3-a0f568b.json")).expect("failed to parse chest weights"));

                WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
            }
            (Series::Rsl, "4") => {
                static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../../assets/event/rsl/chests-4-da4dae5.json")).expect("failed to parse chest weights"));

                WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
            }
            (Series::Rsl, "5") => {
                // rsl/5 moved from version 20cd31a of the RSL script to version 05bfcd2 after the first two races of the first Swiss round.
                // For the sake of simplicity, only the new version is used for chests weights right now.
                //TODO After the event, the version should be randomized based on the total number of races played on each version.
                static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../../assets/event/rsl/chests-5-05bfcd2.json")).expect("failed to parse chest weights"));

                WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
            }
            (Series::Rsl, _) => unimplemented!(),
            (Series::Standard, "6") => {
                static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../../assets/event/s/chests-6-6.9.10.json")).expect("failed to parse chest weights"));

                WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
            }
            (Series::Standard, _) => unimplemented!(),
        }
    }

    pub(crate) fn is_single_race(&self) -> bool {
        match self.series {
            Series::Multiworld => false,
            Series::NineDaysOfSaws => true,
            Series::Pictionary => true,
            Series::Rsl => false,
            Series::Standard => false,
        }
    }

    fn enter_flow(&self) -> EnterFlow {
        match self.series {
            Series::Multiworld => EnterFlow::RaceTimeDiscord,
            Series::NineDaysOfSaws => EnterFlow::RaceTime,
            Series::Pictionary => EnterFlow::RaceTime,
            Series::Rsl => EnterFlow::Extern,
            Series::Standard => EnterFlow::Extern,
        }
    }

    pub(crate) fn team_config(&self) -> TeamConfig {
        match self.series {
            Series::Multiworld => TeamConfig::Multiworld,
            Series::NineDaysOfSaws => match &*self.event {
                "1" => TeamConfig::Solo,
                "2" => TeamConfig::CoOp,
                "3" => TeamConfig::Solo,
                "4" => TeamConfig::Solo,
                "5" => TeamConfig::Solo,
                "6" => TeamConfig::Multiworld,
                "7" => TeamConfig::Solo,
                "8" => TeamConfig::CoOp,
                "9" => TeamConfig::Solo,
                _ => unimplemented!(),
            },
            Series::Pictionary => TeamConfig::Pictionary,
            Series::Rsl => TeamConfig::Solo,
            Series::Standard => TeamConfig::Solo,
        }
    }

    pub(crate) async fn start(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Option<DateTime<Utc>>> {
        Ok(if let Some(mut start) = self.base_start {
            if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM asyncs WHERE series = $1 AND event = $2 AND kind = 'qualifier') AS "exists!""#, self.series as _, &self.event).fetch_one(&mut *transaction).await? {
                let mut num_qualified_teams = 0;
                let mut last_submission_time = None::<DateTime<Utc>>;
                let mut teams = sqlx::query_scalar!(r#"SELECT submitted AS "submitted!" FROM teams LEFT OUTER JOIN async_teams ON (id = team) WHERE
                    series = $1
                    AND event = $2
                    AND NOT resigned
                    AND submitted IS NOT NULL
                    AND kind = 'qualifier'
                "#, self.series as _, &self.event).fetch(transaction);
                while let Some(submitted) = teams.try_next().await? {
                    num_qualified_teams += 1;
                    last_submission_time = Some(if let Some(last_submission_time) = last_submission_time {
                        last_submission_time.max(submitted)
                    } else {
                        submitted
                    });
                }
                if num_qualified_teams % 2 == 0 {
                    if let Some(last_submission_time) = last_submission_time {
                        start = start.max(last_submission_time);
                    }
                } else {
                    if start <= Utc::now() {
                        start += chrono::Duration::days(1);
                    }
                }
            }
            Some(start)
        } else {
            None
        })
    }

    pub(crate) async fn is_started(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<bool> {
        Ok(self.start(transaction).await?.map_or(false, |start| start <= Utc::now()))
    }

    fn is_ended(&self) -> bool {
        self.end.map_or(false, |end| end <= Utc::now())
    }

    async fn active_async(&self, transaction: &mut Transaction<'_, Postgres>, team: &Team) -> sqlx::Result<Option<AsyncKind>> {
        for kind in sqlx::query_scalar!(r#"SELECT kind AS "kind: AsyncKind" FROM asyncs WHERE series = $1 AND event = $2"#, self.series as _, &self.event).fetch_all(&mut *transaction).await? {
            match kind {
                AsyncKind::Qualifier => if !self.is_started(&mut *transaction).await? {
                    return Ok(Some(kind))
                },
                AsyncKind::Tiebreaker1 | AsyncKind::Tiebreaker2 => if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM async_teams WHERE team = $1 AND kind = $2) AS "exists!""#, i64::from(team.id), kind as _).fetch_one(&mut *transaction).await? {
                    return Ok(Some(kind))
                }
            }
        }
        Ok(None)
    }

    async fn header(&self, transaction: &mut Transaction<'_, Postgres>, me: Option<&User>, tab: Tab) -> sqlx::Result<RawHtml<String>> {
        let signed_up = if let Some(me) = me {
            sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                id = team
                AND series = $1
                AND event = $2
                AND member = $3
                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            ) AS "exists!""#, self.series as _, &self.event, i64::from(me.id)).fetch_one(&mut *transaction).await?
        } else {
            false
        };
        Ok(html! {
            h1 {
                a(class = "nav", href? = (!matches!(tab, Tab::Info)).then(|| uri!(info(self.series, &*self.event)).to_string())) : &self.display_name;
            }
            @if let Some(start) = self.start(&mut *transaction).await? {
                h2 : format_datetime(start, DateTimeFormat { long: true, running_text: false });
            }
            div(class = "button-row") {
                @if let Tab::Info = tab {
                    span(class = "button selected") : "Info";
                } else {
                    a(class = "button", href = uri!(info(self.series, &*self.event)).to_string()) : "Info";
                }
                @let teams_label = if let TeamConfig::Solo = self.team_config() { "Entrants" } else { "Teams" };
                @if !self.hide_teams_tab {
                    @if let Tab::Teams = tab {
                        span(class = "button selected") : teams_label;
                    } else if let Some(ref teams_url) = self.teams_url {
                        a(class = "button", href = teams_url.to_string()) {
                            : favicon(teams_url);
                            : teams_label;
                        }
                    } else {
                        a(class = "button", href = uri!(teams(self.series, &*self.event)).to_string()) : teams_label;
                    }
                }
                @if !self.is_single_race() { //TODO also hide for past events with no race list
                    @if let Tab::Races = tab {
                        span(class = "button selected") : "Races";
                    } else {
                        a(class = "button", href = uri!(races(self.series, &*self.event)).to_string()) : "Races";
                    }
                }
                @if signed_up {
                    @if let Tab::MyStatus = tab {
                        span(class = "button selected") : "My Status";
                    } else {
                        a(class = "button", href = uri!(status(self.series, &*self.event)).to_string()) : "My Status";
                    }
                } else if !self.is_started(transaction).await? {
                    @if let Tab::Enter = tab {
                        span(class = "button selected") : "Enter";
                    } else if let Some(ref enter_url) = self.enter_url {
                        a(class = "button", href = enter_url.to_string()) {
                            : favicon(enter_url);
                            : "Enter";
                        }
                    } else {
                        a(class = "button", href = uri!(enter(self.series, &*self.event, _, _)).to_string()) : "Enter";
                    }
                    @if !matches!(self.team_config(), TeamConfig::Solo) {
                        @if let Tab::FindTeam = tab {
                            span(class = "button selected") : "Find Teammates";
                        } else {
                            a(class = "button", href = uri!(find_team(self.series, &*self.event)).to_string()) : "Find Teammates";
                        }
                    }
                }
                //a(class = "button") : "Volunteer"; //TODO
                @if let Some(ref video_url) = self.video_url {
                    a(class = "button", href = video_url.to_string()) {
                        : favicon(video_url);
                        : "Watch";
                    }
                }
                @if let Some(ref url) = self.url {
                    a(class = "button", href = url.to_string()) {
                        : favicon(url);
                        @match url.host_str() {
                            Some("racetime.gg") => : "Race Room";
                            Some("challonge.com" | "www.challonge.com" | "start.gg" | "www.start.gg") => : "Brackets";
                            _ => : "Website";
                        }
                    }
                }
            }
        })
    }
}

impl ToHtml for Data<'_> {
    fn to_html(&self) -> RawHtml<String> {
        html! {
            a(href = uri!(info(self.series, &*self.event)).to_string()) : self.display_name;
        }
    }
}

enum Tab {
    Info,
    Teams,
    Races,
    MyStatus,
    Enter,
    FindTeam,
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Calendar(#[from] cal::Error),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Discord(#[from] serenity::Error),
    #[error(transparent)] Io(#[from] io::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error(transparent)] Wheel(#[from] wheel::Error),
}

impl From<cal::Error> for StatusOrError<Error> {
    fn from(e: cal::Error) -> Self {
        Self::Err(Error::Calendar(e))
    }
}

impl From<DataError> for StatusOrError<Error> {
    fn from(e: DataError) -> Self {
        Self::Err(Error::Data(e))
    }
}

impl From<serenity::Error> for StatusOrError<Error> {
    fn from(e: serenity::Error) -> Self {
        Self::Err(Error::Discord(e))
    }
}

impl From<PageError> for StatusOrError<Error> {
    fn from(e: PageError) -> Self {
        Self::Err(Error::Page(e))
    }
}

impl From<reqwest::Error> for StatusOrError<Error> {
    fn from(e: reqwest::Error) -> Self {
        Self::Err(Error::Reqwest(e))
    }
}

impl From<sqlx::Error> for StatusOrError<Error> {
    fn from(e: sqlx::Error) -> Self {
        Self::Err(Error::Sql(e))
    }
}

impl From<url::ParseError> for StatusOrError<Error> {
    fn from(e: url::ParseError) -> Self {
        Self::Err(Error::Url(e))
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum InfoError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Io(#[from] io::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("missing user data for an event organizer")]
    OrganizerUserData,
}

#[rocket::get("/event/<series>/<event>")]
pub(crate) async fn info(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<InfoError>> {
    let mut transaction = pool.begin().await.map_err(InfoError::Sql)?;
    let data = Data::new(&mut transaction, series, event).await.map_err(InfoError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, me.as_ref(), Tab::Info).await.map_err(InfoError::Sql)?;
    let content = match data.series {
        Series::Multiworld => mw::info(pool, event).await?,
        Series::NineDaysOfSaws => ndos::info(pool, &data).await?,
        Series::Pictionary => pic::info(pool, event).await?,
        Series::Rsl => rsl::info(event),
        Series::Standard => s::info(event),
    };
    page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &data.display_name, html! {
        : header;
        : content;
    }).await.map_err(|e| StatusOrError::Err(InfoError::Page(e)))
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum TeamsError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] PgInterval(#[from] crate::util::PgIntervalDecodeError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("team with nonexistent user")]
    NonexistentUser,
}

#[rocket::get("/event/<series>/<event>/teams")]
pub(crate) async fn teams(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<TeamsError>> {
    let mut transaction = pool.begin().await.map_err(TeamsError::Sql)?;
    let data = Data::new(&mut transaction, series, event).await.map_err(TeamsError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, me.as_ref(), Tab::Teams).await.map_err(TeamsError::Sql)?;
    let mut signups = Vec::default();
    let has_qualifier = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM asyncs WHERE series = $1 AND event = $2 AND kind = 'qualifier') AS "exists!""#, series as _, event).fetch_one(&mut transaction).await.map_err(TeamsError::Sql)?;
    let show_qualifier_times =
        sqlx::query_scalar!(r#"SELECT submitted IS NOT NULL AS "qualified!" FROM async_teams, team_members WHERE async_teams.team = team_members.team AND member = $1 AND kind = 'qualifier'"#, me.as_ref().map(|me| i64::from(me.id))).fetch_optional(&mut *transaction).await.map_err(TeamsError::Sql)?.unwrap_or(false)
        || data.is_started(&mut transaction).await.map_err(TeamsError::Sql)?;
    let teams = sqlx::query!(r#"SELECT id AS "id!: Id", name, racetime_slug, submitted IS NOT NULL AS "qualified!" FROM teams LEFT OUTER JOIN async_teams ON (id = team) WHERE
        series = $1
        AND event = $2
        AND NOT resigned
        AND (
            EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
            OR NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        )
        AND kind = 'qualifier'
    "#, series as _, event, me.as_ref().map(|me| i64::from(me.id))).fetch_all(&mut transaction).await.map_err(TeamsError::Sql)?;
    let roles = data.team_config().roles();
    for team in teams {
        let mut members = Vec::with_capacity(roles.len());
        for &(role, _) in roles {
            let row = sqlx::query!(r#"
                SELECT member AS "id: Id", status AS "status: SignupStatus", time, vod
                FROM team_members LEFT OUTER JOIN async_players ON (member = player)
                WHERE team = $1 AND role = $2 AND kind = 'qualifier'
            "#, i64::from(team.id), role as _).fetch_one(&mut transaction).await.map_err(TeamsError::Sql)?;
            let is_confirmed = row.status.is_confirmed();
            let user = User::from_id(&mut transaction, row.id).await.map_err(TeamsError::Sql)?.ok_or(TeamsError::NonexistentUser)?;
            members.push((role, user, is_confirmed, row.time.map(decode_pginterval).transpose().map_err(TeamsError::PgInterval)?, row.vod));
        }
        signups.push((team.id, team.name, team.racetime_slug, members, team.qualified));
    }
    if show_qualifier_times {
        signups.sort_unstable_by(|(id1, name1, _, members1, qualified1), (id2, name2, _, members2, qualified2)| {
            #[derive(PartialEq, Eq, PartialOrd, Ord)]
            enum Qualification {
                Finished(Duration),
                DidNotFinish,
                NotYetQualified,
            }

            impl Qualification {
                fn new(qualified: bool, members: &[(Role, User, bool, Option<Duration>, Option<String>)]) -> Self {
                    if qualified {
                        if let Some(time) = members.iter().try_fold(Duration::default(), |acc, &(_, _, _, time, _)| Some(acc + time?)) {
                            Self::Finished(time)
                        } else {
                            Self::DidNotFinish
                        }
                    } else {
                        Self::NotYetQualified
                    }
                }
            }

            Qualification::new(*qualified1, members1).cmp(&Qualification::new(*qualified2, members2))
            .then_with(|| name1.cmp(name2))
            .then_with(|| id1.cmp(id2))
        });
    } else {
        signups.sort_unstable_by(|(id1, name1, _, _, qualified1), (id2, name2, _, _, qualified2)|
            qualified2.cmp(qualified1) // reversed to list qualified teams first
            .then_with(|| name1.cmp(name2))
            .then_with(|| id1.cmp(id2))
        );
    }
    let mut footnotes = Vec::default();
    let teams_label = if let TeamConfig::Solo = data.team_config() { "Entrants" } else { "Teams" };
    page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("{teams_label} — {}", data.display_name), html! {
        : header;
        table {
            thead {
                tr {
                    @if !matches!(data.team_config(), TeamConfig::Solo) {
                        th : "Team Name";
                    }
                    @for &(role, display_name) in roles {
                        th(class = role.css_class()) : display_name;
                    }
                    @if has_qualifier && !show_qualifier_times {
                        th : "Qualified";
                    }
                }
            }
            tbody {
                @if signups.is_empty() {
                    tr {
                        td(colspan = if let TeamConfig::Solo = data.team_config() { 0 } else { 1 } + roles.len() + if has_qualifier && !show_qualifier_times { 1 } else { 0 }) {
                            i : "(no signups yet)";
                        }
                    }
                } else {
                    @for (team_id, team_name, racetime_slug, members, qualified) in signups {
                        tr {
                            @if !matches!(data.team_config(), TeamConfig::Solo) {
                                td {
                                    //TODO use Team type
                                    @if let Some(racetime_slug) = racetime_slug {
                                        a(href = format!("https://racetime.gg/team/{racetime_slug}")) {
                                            @if let Some(team_name) = team_name {
                                                : team_name;
                                            } else {
                                                i : "(unnamed)";
                                            }
                                        }
                                    } else {
                                        : team_name.unwrap_or_default();
                                    }
                                    @if show_qualifier_times && qualified {
                                        br;
                                        small {
                                            @if let Some(time) = members.iter().try_fold(Duration::default(), |acc, &(_, _, _, time, _)| Some(acc + time?)) {
                                                : format_duration(time / u32::try_from(members.len()).expect("too many team members"), false);
                                            } else {
                                                : "DNF";
                                            }
                                        }
                                    }
                                }
                            }
                            @for (role, user, is_confirmed, qualifier_time, qualifier_vod) in &members {
                                td(class? = role.css_class()) {
                                    : user;
                                    @if *is_confirmed {
                                        @if me.as_ref().map_or(false, |me| me == user) && members.iter().any(|(_, _, is_confirmed, _, _)| !is_confirmed) {
                                            : " ";
                                            span(class = "button-row") {
                                                form(action = uri!(resign_post(series, event, team_id)).to_string(), method = "post") {
                                                    : csrf;
                                                    input(type = "submit", value = "Retract");
                                                }
                                            }
                                        }
                                    } else {
                                        : " ";
                                        @if me.as_ref().map_or(false, |me| me == user) {
                                            span(class = "button-row") {
                                                form(action = uri!(confirm_signup(series, event, team_id)).to_string(), method = "post") {
                                                    : csrf;
                                                    input(type = "submit", value = "Accept");
                                                }
                                                form(action = uri!(resign_post(series, event, team_id)).to_string(), method = "post") {
                                                    : csrf;
                                                    input(type = "submit", value = "Decline");
                                                }
                                                //TODO options to block sender or event
                                            }
                                        } else {
                                            : "(unconfirmed)";
                                        }
                                    }
                                    @if show_qualifier_times && qualified {
                                        br;
                                        small {
                                            @let time = if let Some(time) = qualifier_time { format_duration(*time, false) } else { format!("DNF") };
                                            @if let Some(vod) = qualifier_vod {
                                                @if let Some(Ok(vod_url)) = (!vod.contains(' ')).then(|| Url::parse(vod)) {
                                                    a(href = vod_url.to_string()) : time;
                                                } else {
                                                    : time;
                                                    sup {
                                                        @let footnote_id = { footnotes.push(vod.clone()); footnotes.len() };
                                                        a(href = format!("#footnote{footnote_id}")) {
                                                            : "[";
                                                            : footnote_id;
                                                            : "]";
                                                        }
                                                    };
                                                }
                                            } else {
                                                : time;
                                            }
                                        }
                                    }
                                }
                            }
                            @if has_qualifier && !show_qualifier_times {
                                td {
                                    @if qualified {
                                        : "✓";
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        @for (i, footnote) in footnotes.into_iter().enumerate() {
            p(id = format!("footnote{}", i + 1)) {
                : "[";
                : i + 1;
                : "]";
                @for word in footnote.split(' ') {
                    : " ";
                    @if let Ok(word_url) = Url::parse(word) {
                        a(href = word_url.to_string()) : word;
                    } else {
                        : word;
                    }
                }
            }
        }
    }).await.map_err(|e| StatusOrError::Err(TeamsError::Page(e)))
}

#[rocket::get("/event/<series>/<event>/races")]
pub(crate) async fn races(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, me.as_ref(), Tab::Races).await?;
    let (past_races, ongoing_and_upcoming_races) = Race::for_event(&mut transaction, http_client, startgg_token, series, event).await?
        .into_iter()
        .partition::<Vec<_>, _>(|race| race.schedule.is_ended());
    let any_races_ongoing_or_upcoming = !ongoing_and_upcoming_races.is_empty();
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Races — {}", data.display_name), html! {
        : header;
        //TODO copiable calendar link (with link to index for explanation?)
        @if any_races_ongoing_or_upcoming {
            table {
                thead {
                    tr {
                        th : "Start";
                        th : "Round";
                        th(colspan = "2") : "Entrants";
                        th : "Links";
                    }
                }
                tbody {
                    @for race in ongoing_and_upcoming_races {
                        tr {
                            td {
                                @match race.schedule {
                                    RaceSchedule::Unscheduled => {}
                                    RaceSchedule::Live { start, .. } => : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                    RaceSchedule::Async { .. } => : "(async)";
                                }
                            }
                            td {
                                : race.phase;
                                : " ";
                                : race.round;
                            }
                            td(class = "vs1") {
                                : race.team1.to_html(false);
                                @if let RaceSchedule::Async { start1: Some(start), .. } = race.schedule {
                                    br;
                                    small {
                                        : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                    }
                                }
                            }
                            td(class = "vs2") {
                                : race.team2.to_html(false);
                                @if let RaceSchedule::Async { start2: Some(start), .. } = race.schedule {
                                    br;
                                    small {
                                        : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                    }
                                }
                            }
                            td {
                                a(class = "favicon", href = race.startgg_set_url()?.to_string()) : favicon(&race.startgg_set_url()?);
                                @for room in race.rooms() {
                                    a(class = "favicon", href = room.to_string()) : favicon(&room);
                                }
                            }
                        }
                    }
                }
            }
        }
        @if !past_races.is_empty() {
            @if any_races_ongoing_or_upcoming {
                h2 : "Past races";
            }
            table {
                thead {
                    tr {
                        th : "Start";
                        th : "Round";
                        th(colspan = "2") : "Entrants";
                        th : "Links";
                        : seed::table_header_cells(true);
                    }
                }
                tbody {
                    @let now = Utc::now();
                    @for race in past_races.into_iter().rev() {
                        tr {
                            td {
                                @match race.schedule {
                                    RaceSchedule::Unscheduled => {}
                                    RaceSchedule::Live { start, .. } => : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                    RaceSchedule::Async { .. } => : "(async)";
                                }
                            }
                            td {
                                : race.phase;
                                : " ";
                                : race.round;
                            }
                            td(class = "vs1") {
                                : race.team1.to_html(false);
                                @if let RaceSchedule::Async { start1: Some(start), .. } = race.schedule {
                                    br;
                                    small {
                                        : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                    }
                                }
                            }
                            td(class = "vs2") {
                                : race.team2.to_html(false);
                                @if let RaceSchedule::Async { start2: Some(start), .. } = race.schedule {
                                    br;
                                    small {
                                        : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                    }
                                }
                            }
                            td {
                                a(class = "favicon", href = race.startgg_set_url()?.to_string()) : favicon(&race.startgg_set_url()?);
                                @for room in race.rooms() {
                                    a(class = "favicon", href = room.to_string()) : favicon(&room);
                                }
                            }
                            @if let Some(ref seed) = race.seed {
                                : seed::table_cells(now, seed, true).await.map_err(Error::Io)?;
                            } else {
                                : seed::table_empty_cells(true);
                            }
                        }
                    }
                }
            }
        }
    }).await?)
}

async fn status_page(pool: &PgPool, discord_ctx: &DiscordCtx, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, context: Context<'_>) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, me.as_ref(), Tab::MyStatus).await?;
    let content = if let Some(ref me) = me {
        if let Some(row) = sqlx::query!(r#"SELECT id AS "id: Id", name, racetime_slug, role AS "role: Role", resigned FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, series as _, event, i64::from(me.id)).fetch_optional(&mut transaction).await? {
            html! {
                : header;
                p {
                    : "You are signed up as part of ";
                    //TODO use Team type
                    @if let Some(racetime_slug) = row.racetime_slug {
                        a(href = format!("https://racetime.gg/team/{racetime_slug}")) {
                            @if let Some(name) = row.name {
                                i : name;
                            } else {
                                : "an unnamed team";
                            }
                        }
                    } else {
                        @if let Some(name) = row.name {
                            i : name;
                        } else {
                            : "an unnamed team";
                        }
                    }
                    //TODO list teammates
                    : ".";
                }
                @if row.resigned {
                    p : "You have resigned from this event.";
                } else {
                    @match data.series {
                        Series::Multiworld => : mw::status(&mut transaction, discord_ctx, csrf, &data, row.id, context).await?;
                        Series::NineDaysOfSaws => @if data.is_ended() {
                            p : "This race has been completed."; //TODO ranking and finish time
                        } else if let Some(ref race_room) = data.url {
                            p {
                                : "Please join ";
                                a(href = race_room.to_string()) : "the race room";
                                : " as soon as possible. You will receive further instructions there.";
                            }
                        } else {
                            : "Waiting for the race room to be opened, which should happen around 30 minutes before the scheduled starting time. Keep an eye out for an announcement on Discord.";
                        }
                        Series::Pictionary => @if data.is_ended() {
                            p : "This race has been completed."; //TODO ranking and finish time
                        } else if let Some(ref race_room) = data.url {
                            @match row.role.try_into().expect("non-Pictionary role in Pictionary team") {
                                pic::Role::Sheikah => p {
                                    : "Please join ";
                                    a(href = race_room.to_string()) : "the race room";
                                    : " as soon as possible. You will receive further instructions there.";
                                }
                                pic::Role::Gerudo => p {
                                    : "Please keep an eye on ";
                                    a(href = race_room.to_string()) : "the race room";
                                    : " (but do not join). The spoiler log will be posted there.";
                                }
                            }
                        } else {
                            : "Waiting for the race room to be opened, which should happen around 30 minutes before the scheduled starting time. Keep an eye out for an announcement on Discord.";
                        }
                        Series::Rsl => @unimplemented // no signups on Mido's House
                        Series::Standard => @unimplemented // no signups on Mido's House
                    }
                    h2 : "Options";
                    p : "More options coming soon"; //TODO options to change team name, swap roles, or opt in/out for restreaming
                    @if !data.is_ended() {
                        p {
                            a(href = uri!(resign(series, event, row.id)).to_string()) : "Resign";
                        }
                    }
                }
            }
        } else {
            html! {
                : header;
                article {
                    p : "You are not signed up for this event.";
                    p : "You can accept, decline, or retract unconfirmed team invitations on the teams page.";
                }
            }
        }
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(status(series, event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to view your status for this event.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("My Status — {}", data.display_name), content).await?)
}

#[rocket::get("/event/<series>/<event>/status")]
pub(crate) async fn status(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    status_page(pool, &*discord_ctx.read().await, me, uri, csrf, series, event, Context::default()).await
}

async fn validate_team(me: &User, client: &reqwest::Client, context: &mut Context<'_>, team_slug: &str, team_config: TeamConfig) -> Result<Option<mw::RaceTimeTeamData>, Error> {
    Ok(if let Some(ref racetime_id) = me.racetime_id {
        let user = client.get(format!("https://racetime.gg/user/{racetime_id}/data"))
            .send().await?
            .detailed_error_for_status().await?
            .json_with_text_in_error::<mw::RaceTimeUser>().await?;
        if user.teams.iter().any(|team| team.slug == team_slug) {
            let team = client.get(format!("https://racetime.gg/team/{team_slug}/data"))
                .send().await?
                .detailed_error_for_status().await?
                .json_with_text_in_error::<mw::RaceTimeTeamData>().await?;
            let expected_size = team_config.roles().len();
            if team.members.len() != expected_size {
                context.push_error(form::Error::validation(format!("Teams for this event must have exactly {expected_size} members, but this team has {}", team.members.len())))
            }
            //TODO get each team member's Mido's House account for displaying in step 2
            Some(team)
        } else {
            context.push_error(form::Error::validation("This racetime.gg team does not exist or you're not in it.").with_name("racetime_team"));
            None
        }
    } else {
        context.push_error(form::Error::validation("A racetime.gg account is required to enter this tournament. Go to your profile and select “Connect a racetime.gg account”.")); //TODO direct link?
        None
    })
}

async fn enter_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, client: &State<reqwest::Client>, data: Data<'_>, defaults: pic::EnterFormDefaults<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::Enter).await?;
    Ok(match data.enter_flow() {
        EnterFlow::RaceTime => match data.team_config() {
            TeamConfig::Solo => page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), html! {
                : header;
                @if data.is_single_race() {
                    article {
                        p {
                            @if let Some(ref url) = data.url {
                                : "Enter ";
                                a(href = url.to_string()) : "the race room";
                                : " to participate in this race.";
                            } else {
                                : "The race room will be opened around 30 minutes before the scheduled starting time. ";
                                @if me.as_ref().map_or(false, |me| me.racetime_id.is_some()) {
                                    : "You don't need to sign up beforehand.";
                                } else {
                                    : "You will need a ";
                                    a(href = "https://racetime.gg/") : "racetime.gg";
                                    : " account to participate.";
                                }
                            }
                        }
                    }
                } else {
                    form(action = uri!(enter_post(data.series, &*data.event)).to_string(), method = "post") {
                        : csrf;
                        fieldset {
                            input(type = "submit", value = "Enter");
                        }
                    }
                }
            }).await?,
            TeamConfig::CoOp => unimplemented!(), //TODO like old MW enter flow but without Discord account check and without role choice
            TeamConfig::Pictionary => pic::enter_form(transaction, me, uri, csrf, data, defaults).await?,
            TeamConfig::Multiworld => unimplemented!(), //TODO like old MW enter flow but without Discord account check
        },
        EnterFlow::RaceTimeDiscord => if let TeamConfig::Solo = data.team_config() {
            unimplemented!() //TODO like EnterFlow::RaceTime but also check to make sure the user has a Discord account connected
        } else {
            mw::enter_form(transaction, me, uri, csrf, data, Context::default(), client).await?
        },
        EnterFlow::Extern => match data.series {
            Series::Multiworld => unimplemented!(),
            Series::NineDaysOfSaws => unimplemented!(),
            Series::Pictionary => unimplemented!(),
            Series::Rsl => rsl::enter_form(transaction, me, uri, data).await?,
            Series::Standard => s::enter_form(transaction, me, uri, data).await?,
        },
    })
}

fn enter_form_step2<'a, 'b: 'a, 'c: 'a, 'd: 'a>(mut transaction: Transaction<'a, Postgres>, me: Option<User>, uri: Origin<'b>, client: &reqwest::Client, csrf: Option<CsrfToken>, data: Data<'c>, defaults: mw::EnterFormStep2Defaults<'d>) -> Pin<Box<dyn Future<Output = Result<RawHtml<String>, Error>> + Send + 'a>> {
    let team_members = defaults.racetime_members(client);
    Box::pin(async move {
        let header = data.header(&mut transaction, me.as_ref(), Tab::Enter).await?;
        let page_content = {
            let team_members = team_members.await?;
            let mut errors = defaults.errors();
            let form_content = html! {
                : csrf;
                : form_field("racetime_team", &mut errors, html! {
                    label(for = "racetime_team") {
                        : "racetime.gg Team: ";
                        a(href = format!("https://racetime.gg/team/{}", defaults.racetime_team_slug().expect("missing racetime team slug"))) : defaults.racetime_team_name().expect("missing racetime team name");
                        : " • ";
                        a(href = uri!(enter(data.series, &*data.event, _, _)).to_string()) : "Change";
                    }
                    input(type = "hidden", name = "racetime_team", value = defaults.racetime_team_slug());
                    input(type = "hidden", name = "racetime_team_name", value = defaults.racetime_team_name());
                });
                @for team_member in team_members {
                    : form_field(&format!("world_number[{}]", team_member.id), &mut errors, html! {
                        label(for = &format!("world_number[{}]", team_member.id)) : &team_member.name; //TODO Mido's House display name, falling back to racetime display name if no Mido's House account
                        @for (role, display_name) in data.team_config().roles() {
                            @let css_class = role.css_class().expect("tried to render enter_form_step2 for a solo event");
                            input(id = &format!("world_number[{}]-{css_class}", team_member.id), class = css_class, type = "radio", name = &format!("world_number[{}]", team_member.id), value = css_class, checked? = defaults.role(&team_member.id) == Some(*role));
                            label(class = css_class, for = &format!("world_number[{}]-{css_class}", team_member.id)) : display_name;
                        }
                    });
                    : form_field(&format!("startgg_id[{}]", team_member.id), &mut errors, html! {
                        label(for = &format!("startgg_id[{}]", team_member.id)) : "start.gg User ID:";
                        input(type = "text", name = &format!("startgg_id[{}]", team_member.id), value? = defaults.startgg_id(&team_member.id));
                        label(class = "help") {
                            : "(Optional. Can be found by going to your ";
                            a(href = "https://start.gg/") : "start.gg";
                            : " profile and clicking your name.)";
                        }
                    });
                }
                : form_field("restream_consent", &mut errors, html! {
                    input(type = "checkbox", id = "restream_consent", name = "restream_consent", checked? = defaults.restream_consent());
                    label(for = "restream_consent") {
                        @if data.is_single_race() {
                            : "We are okay with being restreamed. (Optional. Can be changed later.)";
                        } else {
                            //TODO allow changing on Status page during Swiss, except revoking while a restream is planned
                            //TODO change text depending on tournament structure
                            : "We are okay with being restreamed. (Optional for Swiss, required for top 8. Can be changed later.)";
                        }
                    }
                });
                fieldset {
                    input(type = "submit", value = "Submit");
                }
            };
            html! {
                : header;
                form(action = uri!(mw::enter_post_step2(&*data.event)).to_string(), method = "post") { //TODO compatibility with other team configs
                    @for error in errors {
                        : render_form_error(error);
                    }
                    : form_content;
                }
            }
        };
        Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), page_content).await?)
    })
}

#[rocket::get("/event/<series>/<event>/enter?<my_role>&<teammate>")]
pub(crate) async fn enter(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, client: &State<reqwest::Client>, series: Series, event: &str, my_role: Option<crate::event::pic::Role>, teammate: Option<Id>) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(enter_form(transaction, me, uri, csrf, client, data, pic::EnterFormDefaults::Values { my_role, teammate }).await?)
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct EnterForm {
    #[field(default = String::new())]
    csrf: String,
    racetime_team: Option<String>,
    #[field(default = String::new())]
    team_name: String,
    my_role: Option<pic::Role>,
    teammate: Option<Id>,
}

#[rocket::post("/event/<series>/<event>/enter", data = "<form>")]
pub(crate) async fn enter_post(pool: &State<PgPool>, me: User, uri: Origin<'_>, client: &State<reqwest::Client>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, EnterForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started(&mut transaction).await? {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    if let Some(ref value) = form.value {
        match data.enter_flow() {
            EnterFlow::RaceTime => match data.team_config() {
                TeamConfig::Solo => {
                    if me.racetime_id.is_none() {
                        form.context.push_error(form::Error::validation("A racetime.gg account is required to enter this event. Go to your profile and select “Connect a racetime.gg account”.")); //TODO direct link?
                    }
                    //TODO if no errors, sign up and redirect to teams page
                }
                TeamConfig::Pictionary => {
                    let (my_role, teammate) = match (value.my_role, value.teammate) {
                        (Some(my_role), Some(teammate)) => {
                            if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                                id = team
                                AND series = 'pic'
                                AND event = $1
                                AND member = $2
                                AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                            ) AS "exists!""#, event, i64::from(me.id), i64::from(teammate)).fetch_one(&mut transaction).await? {
                                form.context.push_error(form::Error::validation("A team with these members is already proposed for this race. Check your notifications to accept the invite, or ask your teammate to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
                            }
                            if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                                id = team
                                AND series = 'pic'
                                AND event = $1
                                AND member = $2
                                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                            ) AS "exists!""#, event, i64::from(me.id)).fetch_one(&mut transaction).await? {
                                form.context.push_error(form::Error::validation("You are already signed up for this race."));
                            }
                            if !value.team_name.is_empty() && sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
                                series = 'pic'
                                AND event = $1
                                AND name = $2
                                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                            ) AS "exists!""#, event, value.team_name).fetch_one(&mut transaction).await? {
                                form.context.push_error(form::Error::validation("A team with this name is already signed up for this race.").with_name("team_name"));
                            }
                            if my_role == pic::Role::Sheikah && me.racetime_id.is_none() {
                                form.context.push_error(form::Error::validation("A racetime.gg account is required to enter as runner. Go to your profile and select “Connect a racetime.gg account”.").with_name("my_role")); //TODO direct link?
                            }
                            if teammate == me.id {
                                form.context.push_error(form::Error::validation("You cannot be your own teammate.").with_name("teammate"));
                            }
                            if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, i64::from(teammate)).fetch_one(&mut transaction).await? {
                                form.context.push_error(form::Error::validation("There is no user with this ID.").with_name("teammate"));
                            }
                            if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                                id = team
                                AND series = 'pic'
                                AND event = $1
                                AND member = $2
                                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                            ) AS "exists!""#, event, i64::from(teammate)).fetch_one(&mut transaction).await? {
                                form.context.push_error(form::Error::validation("This user is already signed up for this race.").with_name("teammate"));
                            }
                            //TODO check to make sure the teammate hasn't blocked the user submitting the form (or vice versa) or the event
                            (Some(my_role), Some(teammate))
                        }
                        (Some(_), None) => {
                            form.context.push_error(form::Error::validation("This field is required.").with_name("teammate"));
                            (None, None)
                        }
                        (None, Some(_)) => {
                            form.context.push_error(form::Error::validation("This field is required.").with_name("my_role"));
                            (None, None)
                        }
                        (None, None) => {
                            form.context.push_error(form::Error::validation("This field is required.").with_name("my_role"));
                            form.context.push_error(form::Error::validation("This field is required.").with_name("teammate"));
                            (None, None)
                        }
                    };
                    if form.context.errors().next().is_none() {
                        let id = Id::new(&mut transaction, IdTable::Teams).await?;
                        sqlx::query!("INSERT INTO teams (id, series, event, name) VALUES ($1, 'pic', $2, $3)", id as _, event, (!value.team_name.is_empty()).then(|| &value.team_name)).execute(&mut transaction).await?;
                        sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', $3)", id as _, me.id as _, Role::from(my_role.expect("validated")) as _).execute(&mut transaction).await?;
                        sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'unconfirmed', $3)", id as _, teammate.expect("validated") as _, match my_role.expect("validated") { pic::Role::Sheikah => Role::Gerudo, pic::Role::Gerudo => Role::Sheikah } as _).execute(&mut transaction).await?;
                        transaction.commit().await?;
                        return Ok(RedirectOrContent::Redirect(Redirect::to(uri!(teams(series, event)))))
                    }
                }
                _ => {
                    let racetime_team = if let Some(ref racetime_team) = value.racetime_team {
                        validate_team(&me, client, &mut form.context, racetime_team, data.team_config()).await?
                    } else {
                        form.context.push_error(form::Error::validation("This field is required.").with_name("racetime_team"));
                        None
                    };
                    if form.context.errors().next().is_none() { //TODO check if step2 data is already present
                        return Ok(RedirectOrContent::Content(enter_form_step2(transaction, Some(me), uri, client, csrf, data, mw::EnterFormStep2Defaults::Values { racetime_team: racetime_team.expect("validated") }).await?))
                    }
                }
            },
            EnterFlow::RaceTimeDiscord => match data.team_config() {
                TeamConfig::Solo => {
                    if me.racetime_id.is_none() {
                        form.context.push_error(form::Error::validation("A racetime.gg account is required to enter this event. Go to your profile and select “Connect a racetime.gg account”.")); //TODO direct link?
                    }
                    if me.discord_id.is_none() {
                        form.context.push_error(form::Error::validation("A Discord account is required to enter this event. Go to your profile and select “Connect a Discord account”.")); //TODO direct link?
                    }
                }
                TeamConfig::Pictionary => unimplemented!(), //TODO (only check racetime.gg connection for runner, but check Discord connection for both?)
                _ => {
                    let racetime_team = if let Some(ref racetime_team) = value.racetime_team {
                        validate_team(&me, client, &mut form.context, racetime_team, data.team_config()).await?
                    } else {
                        form.context.push_error(form::Error::validation("This field is required.").with_name("racetime_team"));
                        None
                    };
                    if form.context.errors().next().is_none() { //TODO check if step2 data is already present
                        return Ok(RedirectOrContent::Content(enter_form_step2(transaction, Some(me), uri, client, csrf, data, mw::EnterFormStep2Defaults::Values { racetime_team: racetime_team.expect("validated") }).await?))
                    }
                }
            },
            EnterFlow::Extern => form.context.push_error(form::Error::validation("Mido's House does not handle signups for this event.")),
        }
    }
    Ok(RedirectOrContent::Content(enter_form(transaction, Some(me), uri, csrf, client, data, pic::EnterFormDefaults::Context(form.context)).await?))
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum FindTeamError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("unknown user")]
    UnknownUser,
}

#[rocket::get("/event/<series>/<event>/find-team")]
pub(crate) async fn find_team(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<FindTeamError>> {
    let mut transaction = pool.begin().await.map_err(FindTeamError::Sql)?;
    let data = Data::new(&mut transaction, series, event).await.map_err(FindTeamError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(match data.team_config() {
        TeamConfig::Solo => {
            let header = data.header(&mut transaction, me.as_ref(), Tab::FindTeam).await.map_err(FindTeamError::Sql)?;
            page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Find Teammates — {}", data.display_name), html! {
                : header;
                : "This is a solo event.";
            }).await.map_err(FindTeamError::Page)?
        }
        TeamConfig::CoOp => unimplemented!(), //TODO
        TeamConfig::Pictionary => pic::find_team_form(transaction, me, uri, csrf, data, Context::default()).await?,
        TeamConfig::Multiworld => mw::find_team_form(transaction, me, uri, csrf, data, Context::default()).await?,
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum AcceptError {
    #[error(transparent)] Csrf(#[from] rocket_csrf::VerificationFailure),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Discord(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("you can no longer enter this event since it has already started")]
    EventStarted,
    #[error("you haven't been invited to this team")]
    NotInTeam,
    #[error("a racetime.gg account is required to enter as runner")]
    RaceTimeAccountRequired,
}

#[rocket::post("/event/<series>/<event>/confirm/<team>", data = "<form>")]
pub(crate) async fn confirm_signup(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, team: Id, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<EmptyForm>) -> Result<Redirect, StatusOrError<AcceptError>> {
    let mut transaction = pool.begin().await.map_err(AcceptError::Sql)?;
    let data = Data::new(&mut transaction, series, event).await.map_err(AcceptError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    form.verify(&csrf).map_err(AcceptError::Csrf)?; //TODO option to resubmit on error page (with some “are you sure?” wording)
    if data.is_started(&mut transaction).await.map_err(AcceptError::Sql)? { return Err(AcceptError::EventStarted.into()) }
    if let Some(role) = sqlx::query_scalar!(r#"SELECT role AS "role: Role" FROM team_members WHERE team = $1 AND member = $2 AND status = 'unconfirmed'"#, i64::from(team), i64::from(me.id)).fetch_optional(&mut transaction).await.map_err(AcceptError::Sql)? {
        if role == Role::Sheikah && me.racetime_id.is_none() {
            return Err(AcceptError::RaceTimeAccountRequired.into())
        }
        for member in sqlx::query_scalar!(r#"SELECT member AS "id: Id" FROM team_members WHERE team = $1 AND (status = 'created' OR status = 'confirmed')"#, i64::from(team)).fetch_all(&mut transaction).await.map_err(AcceptError::Sql)? {
            let id = Id::new(&mut transaction, IdTable::Notifications).await.map_err(AcceptError::Sql)?;
            sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, 'accept', $3, $4, $5)", id as _, member as _, series as _, event, me.id as _).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
        }
        sqlx::query!("UPDATE team_members SET status = 'confirmed' WHERE team = $1 AND member = $2", i64::from(team), i64::from(me.id)).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
        if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND status = 'unconfirmed') AS "exists!""#, i64::from(team)).fetch_one(&mut transaction).await.map_err(AcceptError::Sql)? {
            // this confirms the team
            // remove all members from looking_for_team
            sqlx::query!("DELETE FROM looking_for_team WHERE EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND member = user_id)", i64::from(team)).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
            //TODO also remove all other teams with member overlap, and notify
            // create and assign Discord roles
            if let Some(discord_guild) = data.discord_guild {
                let discord_ctx = discord_ctx.read().await;
                for row in sqlx::query!(r#"SELECT discord_id AS "discord_id!: Id", role AS "role: Role" FROM users, team_members WHERE id = member AND discord_id IS NOT NULL AND team = $1"#, i64::from(team)).fetch_all(&mut transaction).await.map_err(AcceptError::Sql)? {
                    if let Ok(member) = discord_guild.member(&*discord_ctx, UserId(row.discord_id.0)).await {
                        let mut roles_to_assign = member.roles.iter().copied().collect::<HashSet<_>>();
                        if let Some(Id(participant_role)) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND series = $2 AND event = $3"#, i64::from(discord_guild), series as _, event).fetch_optional(&mut transaction).await.map_err(AcceptError::Sql)? {
                            roles_to_assign.insert(RoleId(participant_role));
                        }
                        if let Some(Id(role_role)) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND role = $2"#, i64::from(discord_guild), row.role as _).fetch_optional(&mut transaction).await.map_err(AcceptError::Sql)? {
                            roles_to_assign.insert(RoleId(role_role));
                        }
                        if let Some(racetime_slug) = sqlx::query_scalar!("SELECT racetime_slug FROM teams WHERE id = $1", i64::from(team)).fetch_one(&mut transaction).await.map_err(AcceptError::Sql)? {
                            if let Some(Id(team_role)) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND racetime_team = $2"#, i64::from(discord_guild), racetime_slug).fetch_optional(&mut transaction).await.map_err(AcceptError::Sql)? {
                                roles_to_assign.insert(RoleId(team_role));
                            } else {
                                let team_name = sqlx::query_scalar!(r#"SELECT name AS "name!" FROM teams WHERE id = $1"#, i64::from(team)).fetch_one(&mut transaction).await.map_err(AcceptError::Sql)?;
                                let team_role = discord_guild.create_role(&*discord_ctx, |r| r.hoist(false).mentionable(true).name(team_name).permissions(Permissions::empty())).await.map_err(AcceptError::Discord)?.id;
                                sqlx::query!("INSERT INTO discord_roles (id, guild, racetime_team) VALUES ($1, $2, $3)", i64::from(team_role), i64::from(discord_guild), racetime_slug).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
                                roles_to_assign.insert(team_role);
                            }
                        }
                        member.edit(&*discord_ctx, |m| m.roles(roles_to_assign)).await.map_err(AcceptError::Discord)?;
                    }
                }
            }
        }
        transaction.commit().await.map_err(AcceptError::Sql)?;
        Ok(Redirect::to(uri!(teams(series, event))))
    } else {
        transaction.rollback().await.map_err(AcceptError::Sql)?;
        Err(AcceptError::NotInTeam.into())
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum ResignError {
    #[error(transparent)] Csrf(#[from] rocket_csrf::VerificationFailure),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("you can no longer resign from this event since it has already ended")]
    EventEnded,
    #[error("can't delete teams you're not part of")]
    NotInTeam,
}

#[rocket::get("/event/<series>/<event>/resign/<team>")]
pub(crate) async fn resign(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, team: Id) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    if data.is_ended() {
        return Err(StatusOrError::Status(Status::Forbidden))
    }
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Resign — {}", data.display_name), html! {
        //TODO different wording if the event has started
        p {
            : "Are you sure you want to retract your team's registration from ";
            : data;
            : "? If you change your mind later, you will need to invite your teammates again.";
        }
        div(class = "button-row") {
            form(action = uri!(crate::event::resign_post(series, event, team)).to_string(), method = "post") {
                : csrf;
                input(type = "submit", value = "Yes, resign");
            }
        }
    }).await?)
}

#[rocket::post("/event/<series>/<event>/resign/<team>", data = "<form>")]
pub(crate) async fn resign_post(pool: &State<PgPool>, me: User, csrf: Option<CsrfToken>, series: Series, event: &str, team: Id, form: Form<EmptyForm>) -> Result<Redirect, StatusOrError<ResignError>> {
    let mut transaction = pool.begin().await.map_err(ResignError::Sql)?;
    let data = Data::new(&mut transaction, series, event).await.map_err(ResignError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    form.verify(&csrf).map_err(ResignError::Csrf)?; //TODO option to resubmit on error page (with some “are you sure?” wording)
    if data.is_ended() { return Err(ResignError::EventEnded.into()) }
    let is_started = data.is_started(&mut transaction).await.map_err(ResignError::Sql)?;
    let members = if is_started {
        sqlx::query!(r#"UPDATE teams SET resigned = TRUE WHERE id = $1"#, i64::from(team)).execute(&mut transaction).await.map_err(ResignError::Sql)?;
        sqlx::query!(r#"SELECT member AS "id: Id", status AS "status: SignupStatus" FROM team_members WHERE team = $1"#, i64::from(team)).fetch(&mut transaction)
            .map_ok(|row| (row.id, row.status))
            .try_collect::<Vec<_>>().await.map_err(ResignError::Sql)?
    } else {
        sqlx::query!(r#"DELETE FROM team_members WHERE team = $1 RETURNING member AS "id: Id", status AS "status: SignupStatus""#, i64::from(team)).fetch(&mut transaction)
            .map_ok(|row| (row.id, row.status))
            .try_collect().await.map_err(ResignError::Sql)?
    };
    let mut me_in_team = false;
    let mut notification_kind = SimpleNotificationKind::Resign;
    for &(member_id, status) in &members {
        if member_id == me.id {
            me_in_team = true;
            if !status.is_confirmed() { notification_kind = SimpleNotificationKind::Decline }
            break
        }
    }
    if me_in_team {
        for (member_id, status) in members {
            if member_id != me.id && status.is_confirmed() {
                let notification_id = Id::new(&mut transaction, IdTable::Notifications).await.map_err(ResignError::Sql)?;
                sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, $3, $4, $5, $6)", notification_id as _, member_id as _, notification_kind as _, series as _, event, me.id as _).execute(&mut transaction).await.map_err(ResignError::Sql)?;
            }
        }
        if !is_started {
            sqlx::query!("DELETE FROM teams WHERE id = $1", i64::from(team)).execute(&mut transaction).await.map_err(ResignError::Sql)?;
        }
        transaction.commit().await.map_err(ResignError::Sql)?;
        Ok(Redirect::to(uri!(teams(series, event))))
    } else {
        transaction.rollback().await.map_err(ResignError::Sql)?;
        Err(ResignError::NotInTeam.into())
    }
}

#[derive(sqlx::Type)]
#[sqlx(type_name = "async_kind", rename_all = "lowercase")]
enum AsyncKind {
    Qualifier,
    /// The tiebreaker for the highest Swiss points group with more than one team.
    Tiebreaker1,
    /// The tiebreaker for the 2nd-highest Swiss points group with more than one team.
    Tiebreaker2,
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct RequestAsyncForm {
    #[field(default = String::new())]
    csrf: String,
    confirm: bool,
}

#[rocket::post("/event/<series>/<event>/request-async", data = "<form>")]
pub(crate) async fn request_async(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, RequestAsyncForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started(&mut transaction).await? {
        form.context.push_error(form::Error::validation("You can no longer request the qualifier async since the event has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        let team = sqlx::query_as!(Team, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT resigned
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, series as _, event, i64::from(me.id)).fetch_optional(&mut transaction).await?;
        let async_kind = if let Some(ref team) = team {
            if let Some(async_kind) = data.active_async(&mut transaction, team).await? {
                let requested = sqlx::query_scalar!(r#"SELECT requested IS NOT NULL AS "requested!" FROM async_teams WHERE team = $1 AND kind = $2"#, i64::from(team.id), async_kind as _).fetch_optional(&mut transaction).await?;
                if requested.map_or(false, identity) {
                    form.context.push_error(form::Error::validation("Your team has already requested this async."));
                }
                Some(async_kind)
            } else {
                form.context.push_error(form::Error::validation("There is no active async for your team."));
                None
            }
        } else {
            form.context.push_error(form::Error::validation("You are not signed up for this event."));
            None
        };
        if !value.confirm {
            form.context.push_error(form::Error::validation("This field is required.").with_name("confirm"));
        }
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(status_page(pool, &*discord_ctx.read().await, Some(me), uri, csrf, series, event, form.context).await?)
        } else {
            let team = team.expect("validated");
            let async_kind = async_kind.expect("validated");
            sqlx::query!("INSERT INTO async_teams (team, kind, requested) VALUES ($1, $2, $3) ON CONFLICT (team, kind) DO UPDATE SET requested = EXCLUDED.requested", team.id as _, async_kind as _, Utc::now()).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(status(series, event))))
        }
    } else {
        transaction.rollback().await?;
        RedirectOrContent::Content(status_page(pool, &*discord_ctx.read().await, Some(me), uri, csrf, series, event, form.context).await?)
    })
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct SubmitAsyncForm {
    #[field(default = String::new())]
    csrf: String,
    time1: String,
    vod1: String,
    time2: String,
    vod2: String,
    time3: String,
    vod3: String,
    fpa: String,
}

#[rocket::post("/event/<series>/<event>/submit-async", data = "<form>")]
pub(crate) async fn submit_async(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, SubmitAsyncForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if let Some(ref value) = form.value {
        let team = sqlx::query_as!(Team, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT resigned
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, series as _, event, i64::from(me.id)).fetch_optional(&mut transaction).await?;
        let async_kind = if let Some(ref team) = team {
            if let Some(async_kind) = data.active_async(&mut transaction, team).await? {
                let row = sqlx::query!(r#"SELECT requested IS NOT NULL AS "requested!", submitted IS NOT NULL AS "submitted!" FROM async_teams WHERE team = $1 AND kind = $2"#, i64::from(team.id), async_kind as _).fetch_optional(&mut transaction).await?;
                if row.as_ref().map_or(false, |row| row.submitted) {
                    form.context.push_error(form::Error::validation("You have already submitted times for this async. To make a correction or add vods, please contact the tournament organizers.")); //TODO allow adding vods via form but no other edits
                }
                if !row.map_or(false, |row| row.requested) {
                    form.context.push_error(form::Error::validation("You have not requested this async yet."));
                }
                Some(async_kind)
            } else {
                form.context.push_error(form::Error::validation("There is no active async for your team."));
                None
            }
        } else {
            form.context.push_error(form::Error::validation("You are not signed up for this event."));
            None
        };
        let time1 = if value.time1.is_empty() {
            None
        } else if let Some(time) = parse_duration(&value.time1) {
            Some(time)
        } else {
            form.context.push_error(form::Error::validation("Duration must be formatted like “1:23:45” or “1h 23m 45s”.").with_name("time1"));
            None
        };
        let time2 = if value.time2.is_empty() {
            None
        } else if let Some(time) = parse_duration(&value.time2) {
            Some(time)
        } else {
            form.context.push_error(form::Error::validation("Duration must be formatted like “1:23:45” or “1h 23m 45s”.").with_name("time2"));
            None
        };
        let time3 = if value.time3.is_empty() {
            None
        } else if let Some(time) = parse_duration(&value.time3) {
            Some(time)
        } else {
            form.context.push_error(form::Error::validation("Duration must be formatted like “1:23:45” or “1h 23m 45s”.").with_name("time3"));
            None
        };
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(status_page(pool, &*discord_ctx.read().await, Some(me), uri, csrf, series, event, form.context).await?)
        } else {
            let team = team.expect("validated");
            let async_kind = async_kind.expect("validated");
            sqlx::query!("UPDATE async_teams SET submitted = $1, fpa = $2 WHERE team = $3 AND kind = $4", Utc::now(), (!value.fpa.is_empty()).then(|| &value.fpa), i64::from(team.id), async_kind as _).execute(&mut transaction).await?;
            let player1 = sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1 AND role = 'power'"#, i64::from(team.id)).fetch_one(&mut transaction).await?;
            sqlx::query!("INSERT INTO async_players (series, event, player, kind, time, vod) VALUES ($1, $2, $3, $4, $5, $6)", series as _, event, player1 as _, async_kind as _, time1 as _, (!value.vod1.is_empty()).then(|| &value.vod1)).execute(&mut transaction).await?;
            let player2 = sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1 AND role = 'wisdom'"#, i64::from(team.id)).fetch_one(&mut transaction).await?;
            sqlx::query!("INSERT INTO async_players (series, event, player, kind, time, vod) VALUES ($1, $2, $3, $4, $5, $6)", series as _, event, player2 as _, async_kind as _, time2 as _, (!value.vod2.is_empty()).then(|| &value.vod2)).execute(&mut transaction).await?;
            let player3 = sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1 AND role = 'courage'"#, i64::from(team.id)).fetch_one(&mut transaction).await?;
            sqlx::query!("INSERT INTO async_players (series, event, player, kind, time, vod) VALUES ($1, $2, $3, $4, $5, $6)", series as _, event, player3 as _, async_kind as _, time3 as _, (!value.vod3.is_empty()).then(|| &value.vod3)).execute(&mut transaction).await?;
            if let Some(discord_guild) = data.discord_guild {
                let asyncs_row = sqlx::query!(r#"SELECT discord_role AS "discord_role: Id", discord_channel AS "discord_channel: Id" FROM asyncs WHERE series = $1 AND event = $2 AND kind = $3"#, series as _, event, async_kind as _).fetch_one(&mut transaction).await?;
                let members = sqlx::query_scalar!(r#"SELECT discord_id AS "discord_id!: Id" FROM users, team_members WHERE id = member AND discord_id IS NOT NULL AND team = $1"#, i64::from(team.id)).fetch_all(&mut transaction).await?;
                if let Some(Id(discord_role)) = asyncs_row.discord_role {
                    for &Id(user_id) in &members {
                        if let Ok(mut member) = discord_guild.member(&*discord_ctx.read().await, user_id).await {
                            member.add_role(&*discord_ctx.read().await, discord_role).await?;
                        }
                    }
                }
                if let Some(Id(discord_channel)) = asyncs_row.discord_channel {
                    let mut message = MessageBuilder::default();
                    message.push("Please welcome ");
                    message.mention_team(&mut transaction, discord_guild, &team).await?;
                    if let (Some(time1), Some(time2), Some(time3)) = (time1, time2, time3) {
                        message.push(" who finished with a time of ");
                        message.push(format_duration((time1 + time2 + time3) / 3, true));
                        message.push_line('!');
                    } else {
                        message.push_line(" who did not finish.");
                    };
                    if let Some(player1) = User::from_id(&mut transaction, player1).await? {
                        message.mention_user(&player1);
                    } else {
                        message.push("player 1");
                    }
                    message.push(": ");
                    if let Some(time1) = time1 {
                        message.push(format_duration(time1, false));
                    } else {
                        message.push("DNF");
                    }
                    if value.vod1.is_empty() {
                        message.push_line("");
                    } else {
                        message.push(' ');
                        message.push_line_safe(&value.vod1);
                    }
                    if let Some(player2) = User::from_id(&mut transaction, player2).await? {
                        message.mention_user(&player2);
                    } else {
                        message.push("player 2");
                    }
                    message.push(": ");
                    if let Some(time2) = time2 {
                        message.push(format_duration(time2, false));
                    } else {
                        message.push("DNF");
                    }
                    if value.vod2.is_empty() {
                        message.push_line("");
                    } else {
                        message.push(' ');
                        message.push_line_safe(&value.vod2);
                    }
                    if let Some(player3) = User::from_id(&mut transaction, player3).await? {
                        message.mention_user(&player3);
                    } else {
                        message.push("player 3");
                    }
                    message.push(": ");
                    if let Some(time3) = time3 {
                        message.push(format_duration(time3, false));
                    } else {
                        message.push("DNF");
                    }
                    if !value.vod3.is_empty() {
                        message.push(' ');
                        message.push_safe(&value.vod3);
                    }
                    ChannelId(discord_channel).send_message(&*discord_ctx.read().await, |m| m
                        .content(message)
                        .flags(MessageFlags::SUPPRESS_EMBEDS)
                    ).await?;
                }
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(status(series, event))))
        }
    } else {
        transaction.rollback().await?;
        RedirectOrContent::Content(status_page(pool, &*discord_ctx.read().await, Some(me), uri, csrf, series, event, form.context).await?)
    })
}
