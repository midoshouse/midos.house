use {
    std::{
        borrow::Cow,
        fmt,
        io,
        str::FromStr,
    },
    anyhow::anyhow,
    chrono::prelude::*,
    futures::stream::{
        self,
        StreamExt as _,
        TryStreamExt as _,
    },
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
    serenity::model::prelude::*,
    sqlx::{
        Decode,
        Encode,
        postgres::{
            Postgres,
            PgArgumentBuffer,
            PgPool,
            PgTypeInfo,
            PgValueRef,
        },
    },
    url::Url,
    crate::{
        auth,
        favicon::ChestAppearances,
        http::{
            PageError,
            PageStyle,
            page,
        },
        notification::SimpleNotificationKind,
        user::User,
        util::{
            DateTimeFormat,
            EmptyForm,
            Id,
            IdTable,
            RedirectOrContent,
            StatusOrError,
            favicon,
            format_datetime,
            parse_duration,
        },
    },
};

pub(crate) mod mw;
pub(crate) mod pic;

#[derive(Debug, sqlx::Type)]
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
    /// “runner” in Pictionary
    Sheikah,
    /// “pilot” in Pictionary
    Gerudo,
    /// world 1
    Power,
    /// world 2
    Wisdom,
    /// world 3
    Courage,
}

impl Role {
    fn css_class(&self) -> &'static str {
        match self {
            Self::Sheikah => "sheikah",
            Self::Gerudo => "gerudo",
            Self::Power => "power",
            Self::Wisdom => "wisdom",
            Self::Courage => "courage",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Series {
    Multiworld,
    Pictionary,
}

impl Series {
    fn to_str(&self) -> &'static str {
        match self {
            Self::Multiworld => "mw",
            Self::Pictionary => "pic",
        }
    }
}

impl FromStr for Series {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "mw" => Ok(Self::Multiworld),
            "pic" => Ok(Self::Pictionary),
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

pub(crate) enum TeamConfig {
    Pictionary,
    Multiworld,
}

impl TeamConfig {
    fn roles(&self) -> Vec<(Role, &'static str)> {
        match self {
            Self::Multiworld => vec![
                (Role::Power, "Player 1"),
                (Role::Wisdom, "Player 2"),
                (Role::Courage, "Player 3"),
            ],
            Self::Pictionary => vec![
                (Role::Sheikah, "Runner"),
                (Role::Gerudo, "Pilot"),
            ],
        }
    }
}

pub(crate) struct Data<'a> {
    pool: PgPool,
    pub(crate) series: Series,
    pub(crate) event: Cow<'a, str>,
    pub(crate) display_name: String,
    pub(crate) start: Option<DateTime<Utc>>,
    pub(crate) end: Option<DateTime<Utc>>,
    url: Option<Url>,
    teams_url: Option<Url>,
    video_url: Option<Url>,
    discord_guild: Option<GuildId>,
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum DataError {
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
}

impl<'a> Data<'a> {
    pub(crate) async fn new(pool: PgPool, series: Series, event: impl Into<Cow<'a, str>>) -> Result<Option<Data<'a>>, DataError> {
        let event = event.into();
        sqlx::query!(r#"SELECT display_name, start, end_time, url, teams_url, video_url, discord_guild AS "discord_guild: Id" FROM events WHERE series = $1 AND event = $2"#, series as _, &event).fetch_optional(&pool).await?
            .map(|row| Ok::<_, DataError>(Self {
                display_name: row.display_name,
                start: row.start,
                end: row.end_time,
                url: row.url.map(|url| url.parse()).transpose()?,
                teams_url: row.teams_url.map(|url| url.parse()).transpose()?,
                video_url: row.video_url.map(|url| url.parse()).transpose()?,
                discord_guild: row.discord_guild.map(|Id(id)| id.into()),
                pool, series, event,
            }))
            .transpose()
    }

    pub(crate) fn team_config(&self) -> TeamConfig {
        match self.series {
            Series::Multiworld => TeamConfig::Multiworld,
            Series::Pictionary => TeamConfig::Pictionary,
        }
    }

    fn is_started(&self) -> bool {
        self.start.map_or(false, |start| start <= Utc::now())
    }

    fn is_ended(&self) -> bool {
        self.end.map_or(false, |end| end <= Utc::now())
    }

    pub(crate) fn chests(&self) -> ChestAppearances {
        match (self.series, &*self.event) {
            (Series::Multiworld, "2") => ChestAppearances::VANILLA, // CAMC off or classic and no keys in overworld
            (Series::Multiworld, "3") => ChestAppearances::random(), //TODO update after preliminary base settings exist
            (Series::Multiworld, _) => unimplemented!(),
            (Series::Pictionary, _) => ChestAppearances::VANILLA, // no CAMC in Pictionary
        }
    }

    async fn header(&self, me: Option<&User>, tab: Tab) -> sqlx::Result<RawHtml<String>> {
        let signed_up = if let Some(me) = me {
            sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                id = team
                AND series = $1
                AND event = $2
                AND member = $3
                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            ) AS "exists!""#, self.series.to_str(), &self.event, i64::from(me.id)).fetch_one(&self.pool).await?
        } else {
            false
        };
        Ok(html! {
            h1 {
                a(class = "nav", href? = (!matches!(tab, Tab::Info)).then(|| uri!(info(self.series, &*self.event)).to_string())) : &self.display_name;
            }
            @if let Some(start) = self.start {
                h2 : format_datetime(start, DateTimeFormat { long: true, running_text: false });
            }
            div(class = "button-row") {
                @if let Tab::Info = tab {
                    span(class = "button selected") : "Info";
                } else {
                    a(class = "button", href = uri!(info(self.series, &*self.event)).to_string()) : "Info";
                }
                @if let Tab::Teams = tab {
                    span(class = "button selected") : "Teams";
                } else if let Some(ref teams_url) = self.teams_url {
                    a(class = "button", href = teams_url.to_string()) {
                        : favicon(teams_url);
                        : "Teams";
                    }
                } else {
                    a(class = "button", href = uri!(teams(self.series, &*self.event)).to_string()) : "Teams";
                }
                @if signed_up {
                    @if let Tab::MyStatus = tab {
                        span(class = "button selected") : "My Status";
                    } else {
                        a(class = "button", href = uri!(status(self.series, &*self.event)).to_string()) : "My Status";
                    }
                } else if !self.is_started() {
                    @if let Tab::Enter = tab {
                        span(class = "button selected") : "Enter";
                    } else {
                        a(class = "button", href = uri!(enter(self.series, &*self.event, _, _)).to_string()) : "Enter";
                    }
                    @if let Tab::FindTeam = tab {
                        span(class = "button selected") : "Find Teammates";
                    } else {
                        a(class = "button", href = uri!(find_team(self.series, &*self.event)).to_string()) : "Find Teammates";
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
                            Some("start.gg" | "www.start.gg") => : "Brackets";
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
    MyStatus,
    Enter,
    FindTeam,
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

impl From<DataError> for StatusOrError<Error> {
    fn from(e: DataError) -> Self {
        Self::Err(Error::Data(e))
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
    let data = Data::new((**pool).clone(), series, event).await.map_err(InfoError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(me.as_ref(), Tab::Info).await.map_err(InfoError::Sql)?;
    let content = match data.series {
        Series::Multiworld => mw::info(pool, event).await?,
        Series::Pictionary => pic::info(pool, event).await?,
    };
    page(pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &data.display_name, html! {
        : header;
        : content;
    }).await.map_err(|e| StatusOrError::Err(InfoError::Page(e)))
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum TeamsError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("team with nonexistent user")]
    NonexistentUser,
}

#[rocket::get("/event/<series>/<event>/teams")]
pub(crate) async fn teams(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<TeamsError>> {
    let data = Data::new((**pool).clone(), series, event).await.map_err(TeamsError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(me.as_ref(), Tab::Teams).await.map_err(TeamsError::Sql)?;
    let mut signups = Vec::default();
    let mut teams_query = sqlx::query!(r#"SELECT id AS "id!: Id", name, racetime_slug FROM teams WHERE
        series = $1
        AND event = $2
        AND (
            EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
            OR NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        )
    "#, series.to_str(), event, me.as_ref().map(|me| i64::from(me.id))).fetch(&**pool);
    let roles = data.team_config().roles();
    while let Some(team) = teams_query.try_next().await.map_err(TeamsError::Sql)? {
        let members = stream::iter(&roles)
            .then(|&(role, _)| async move {
                let row = sqlx::query!(r#"SELECT member AS "id: Id", status AS "status: SignupStatus" FROM team_members WHERE team = $1 AND role = $2"#, i64::from(team.id), role as _).fetch_one(&**pool).await?;
                let is_confirmed = row.status.is_confirmed();
                let user = User::from_id(&**pool, row.id).await?.ok_or(TeamsError::NonexistentUser)?;
                Ok::<_, TeamsError>((role, user, is_confirmed))
            })
            .try_collect::<Vec<_>>().await?;
        signups.push((team.id, team.name, team.racetime_slug, members));
    }
    page(pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Teams — {}", data.display_name), html! {
        : header;
        table {
            thead {
                tr {
                    th : "Team Name";
                    @for &(role, display_name) in &roles {
                        th(class = role.css_class()) : display_name;
                    }
                }
            }
            tbody {
                @if signups.is_empty() {
                    tr {
                        td(colspan = roles.len() + 1) {
                            i : "(no signups yet)";
                        }
                    }
                } else {
                    @for (team_id, team_name, racetime_slug, members) in signups {
                        tr {
                            @if let Some(racetime_slug) = racetime_slug {
                                td {
                                    a(href = format!("https://racetime.gg/team/{racetime_slug}")) {
                                        @if let Some(team_name) = team_name {
                                            : team_name;
                                        } else {
                                            i : "(unnamed)";
                                        }
                                    }
                                }
                            } else {
                                td : team_name.unwrap_or_default();
                            }
                            @for (role, user, is_confirmed) in &members {
                                td(class = role.css_class()) {
                                    : user;
                                    @if *is_confirmed {
                                        @if me.as_ref().map_or(false, |me| me == user) && members.iter().any(|(_, _, is_confirmed)| !is_confirmed) {
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
                                }
                            }
                        }
                    }
                }
            }
        }
    }).await.map_err(|e| StatusOrError::Err(TeamsError::Page(e)))
}

async fn status_page(pool: &PgPool, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, context: Context<'_>) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let data = Data::new(pool.clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(me.as_ref(), Tab::MyStatus).await?;
    Ok(page(pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("My Status — {}", data.display_name), {
        if let Some(ref me) = me {
            if let Some(row) = sqlx::query!(r#"SELECT id AS "id: Id", name, racetime_slug, role AS "role: Role" FROM teams, team_members WHERE
                id = team
                AND series = $1
                AND event = $2
                AND member = $3
                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            "#, series.to_str(), event, i64::from(me.id)).fetch_optional(pool).await? {
                html! {
                    : header;
                    p {
                        : "You are signed up as part of ";
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
                    @match data.series {
                        Series::Multiworld => : mw::status(pool, csrf, &data, row.id, context).await?;
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
                    }
                    h2 : "Options";
                    p : "More options coming soon"; //TODO options to change team name, swap roles, or opt in/out for restreaming
                    @if !data.is_ended() {
                        p {
                            a(href = uri!(resign(series, event, row.id)).to_string()) : "Resign";
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
        }
    }).await?)
}

#[rocket::get("/event/<series>/<event>/status")]
pub(crate) async fn status(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    status_page(pool, me, uri, csrf, series, event, Context::default()).await
}

#[rocket::get("/event/<series>/<event>/enter?<my_role>&<teammate>")]
pub(crate) async fn enter(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, client: &State<reqwest::Client>, series: Series, event: &str, my_role: Option<crate::event::pic::Role>, teammate: Option<Id>) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(match data.team_config() {
        TeamConfig::Multiworld => mw::enter_form(me, uri, csrf, data, Context::default(), client).await?,
        TeamConfig::Pictionary => pic::enter_form(me, uri, csrf, data, pic::EnterFormDefaults::Values { my_role, teammate }).await?,
    })
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
    let data = Data::new((**pool).clone(), series, event).await.map_err(FindTeamError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(match data.team_config() {
        TeamConfig::Multiworld => unimplemented!(), //TODO “find team” form for multiworld, without invite feature
        TeamConfig::Pictionary => pic::find_team_form(me, uri, csrf, data, Context::default()).await?,
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum AcceptError {
    #[error(transparent)] Csrf(#[from] rocket_csrf::VerificationFailure),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("you can no longer enter this event since it has already started")]
    EventStarted,
    #[error("you haven't been invited to this team")]
    NotInTeam,
    #[error("a racetime.gg account is required to enter as runner")]
    RaceTimeAccountRequired,
}

#[rocket::post("/event/<series>/<event>/confirm/<team>", data = "<form>")]
pub(crate) async fn confirm_signup(pool: &State<PgPool>, me: User, team: Id, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<EmptyForm>) -> Result<Redirect, StatusOrError<AcceptError>> {
    let data = Data::new((**pool).clone(), series, event).await.map_err(AcceptError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    form.verify(&csrf).map_err(AcceptError::Csrf)?; //TODO option to resubmit on error page (with some “are you sure?” wording)
    if data.is_started() { return Err(AcceptError::EventStarted.into()) }
    let mut transaction = pool.begin().await.map_err(AcceptError::Sql)?;
    if let Some(role) = sqlx::query_scalar!(r#"SELECT role AS "role: Role" FROM team_members WHERE team = $1 AND member = $2 AND status = 'unconfirmed'"#, i64::from(team), i64::from(me.id)).fetch_optional(&mut transaction).await.map_err(AcceptError::Sql)? {
        if role == Role::Sheikah && me.racetime_id.is_none() {
            return Err(AcceptError::RaceTimeAccountRequired.into())
        }
        for member in sqlx::query_scalar!(r#"SELECT member AS "id: Id" FROM team_members WHERE team = $1 AND (status = 'created' OR status = 'confirmed')"#, i64::from(team)).fetch_all(&mut transaction).await.map_err(AcceptError::Sql)? {
            let id = Id::new(&mut transaction, IdTable::Notifications).await.map_err(AcceptError::Sql)?;
            sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, 'accept', $3, $4, $5)", id as _, member as _, series as _, event, me.id as _).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
        }
        sqlx::query!("UPDATE team_members SET status = 'confirmed' WHERE team = $1 AND member = $2", i64::from(team), i64::from(me.id)).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
        // if this confirms the team, remove all members from looking_for_team
        sqlx::query!("DELETE FROM looking_for_team WHERE
            EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND member = user_id)
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND status = 'unconfirmed')
        ", i64::from(team)).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
        //TODO also remove all other teams with member overlap, and notify
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
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    if data.is_ended() {
        return Err(StatusOrError::Status(Status::Forbidden))
    }
    Ok(page(pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Resign — {}", data.display_name), html! {
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
    let data = Data::new((**pool).clone(), series, event).await.map_err(ResignError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    form.verify(&csrf).map_err(ResignError::Csrf)?; //TODO option to resubmit on error page (with some “are you sure?” wording)
    if data.is_ended() { return Err(ResignError::EventEnded.into()) }
    //TODO if the event has started, only mark the team as resigned, don't delete data
    let mut transaction = pool.begin().await.map_err(ResignError::Sql)?;
    let delete = sqlx::query!(r#"DELETE FROM team_members WHERE team = $1 RETURNING member AS "id: Id", status AS "status: SignupStatus""#, i64::from(team)).fetch_all(&mut transaction).await.map_err(ResignError::Sql)?;
    let mut me_in_team = false;
    let mut notification_kind = SimpleNotificationKind::Resign;
    for member in &delete {
        if member.id == me.id {
            me_in_team = true;
            if !member.status.is_confirmed() { notification_kind = SimpleNotificationKind::Decline }
            break
        }
    }
    if me_in_team {
        for member in delete {
            if member.id != me.id && member.status.is_confirmed() {
                let id = Id::new(&mut transaction, IdTable::Notifications).await.map_err(ResignError::Sql)?;
                sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, $3, $4, $5, $6)", id as _, member.id as _, notification_kind as _, series as _, event, me.id as _).execute(&mut transaction).await.map_err(ResignError::Sql)?;
            }
        }
        sqlx::query!("DELETE FROM teams WHERE id = $1", i64::from(team)).execute(&mut transaction).await.map_err(ResignError::Sql)?;
        transaction.commit().await.map_err(ResignError::Sql)?;
        Ok(Redirect::to(uri!(teams(series, event))))
    } else {
        transaction.rollback().await.map_err(ResignError::Sql)?;
        Err(ResignError::NotInTeam.into())
    }
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct RequestAsyncForm {
    #[field(default = String::new())]
    csrf: String,
    confirm: bool,
}

#[rocket::post("/event/<series>/<event>/request-async", data = "<form>")]
pub(crate) async fn request_async(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, RequestAsyncForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started() {
        form.context.push_error(form::Error::validation("You can no longer request the qualifier async since the event has already started."));
    }
    let mut transaction = pool.begin().await?;
    //TODO error if async already requested
    Ok(if let Some(ref value) = form.value {
        let team_id = sqlx::query_scalar!(r#"SELECT team AS "team: Id" FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, series.to_str(), event, i64::from(me.id)).fetch_optional(&mut transaction).await?;
        if team_id.is_none() {
            form.context.push_error(form::Error::validation("You are not signed up for this event."));
        }
        if !value.confirm {
            form.context.push_error(form::Error::validation("This field is required.").with_name("confirm"));
        }
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(status_page(pool, Some(me), uri, csrf, series, event, form.context).await?)
        } else {
            let team_id = team_id.expect("validated");
            sqlx::query!("INSERT INTO async_teams (team, requested) VALUES ($1, $2)", team_id as _, Utc::now()).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(status(series, event))))
        }
    } else {
        transaction.rollback().await?;
        RedirectOrContent::Content(status_page(pool, Some(me), uri, csrf, series, event, form.context).await?)
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
pub(crate) async fn submit_async(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, SubmitAsyncForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started() {
        form.context.push_error(form::Error::validation("You can no longer submit the qualifier async since the event has already started."));
    }
    let mut transaction = pool.begin().await?;
    //TODO error if async not yet requested
    Ok(if let Some(ref value) = form.value {
        let team_id = sqlx::query_scalar!(r#"SELECT team AS "team: Id" FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, series.to_str(), event, i64::from(me.id)).fetch_optional(&mut transaction).await?;
        if team_id.is_none() {
            form.context.push_error(form::Error::validation("You are not signed up for this event."));
        }
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
            RedirectOrContent::Content(status_page(pool, Some(me), uri, csrf, series, event, form.context).await?)
        } else {
            let team_id = team_id.expect("validated");
            sqlx::query!("UPDATE async_teams SET submitted = $1, fpa = $2 WHERE team = $3", Utc::now(), (!value.fpa.is_empty()).then(|| &value.fpa), i64::from(team_id)).execute(&mut transaction).await?;
            let player1 = sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1 AND role = 'power'"#, i64::from(team_id)).fetch_one(&mut transaction).await?;
            sqlx::query!("INSERT INTO async_players (series, event, player, time, vod) VALUES ($1, $2, $3, $4, $5)", series as _, event, player1 as _, time1 as _, (!value.vod1.is_empty()).then(|| &value.vod1)).execute(&mut transaction).await?;
            let player2 = sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1 AND role = 'wisdom'"#, i64::from(team_id)).fetch_one(&mut transaction).await?;
            sqlx::query!("INSERT INTO async_players (series, event, player, time, vod) VALUES ($1, $2, $3, $4, $5)", series as _, event, player2 as _, time2 as _, (!value.vod2.is_empty()).then(|| &value.vod2)).execute(&mut transaction).await?;
            let player3 = sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1 AND role = 'courage'"#, i64::from(team_id)).fetch_one(&mut transaction).await?;
            sqlx::query!("INSERT INTO async_players (series, event, player, time, vod) VALUES ($1, $2, $3, $4, $5)", series as _, event, player3 as _, time3 as _, (!value.vod3.is_empty()).then(|| &value.vod3)).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(status(series, event))))
        }
    } else {
        transaction.rollback().await?;
        RedirectOrContent::Content(status_page(pool, Some(me), uri, csrf, series, event, form.context).await?)
    })
}
