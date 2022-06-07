use {
    std::{
        borrow::Cow,
        io,
    },
    chrono::prelude::*,
    chrono_tz::{
        America,
        Europe,
    },
    futures::stream::{
        self,
        StreamExt as _,
        TryStreamExt as _,
    },
    rocket::{
        State,
        form::{
            Context,
            Form,
        },
        http::Status,
        response::{
            Redirect,
            content::RawHtml,
        },
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        Origin,
        ToHtml,
        html,
    },
    sqlx::PgPool,
    url::Url,
    crate::{
        PageError,
        PageStyle,
        auth,
        favicon::ChestAppearances,
        notification::SimpleNotificationKind,
        page,
        user::User,
        util::{
            EmptyForm,
            Id,
            IdTable,
            StatusOrError,
            favicon,
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
    /// “pilot” in Pictionary
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
    pub(crate) series: Cow<'a, str>,
    pub(crate) event: Cow<'a, str>,
    pub(crate) display_name: String,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    url: Option<Url>,
    teams_url: Option<Url>,
    video_url: Option<Url>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DataError {
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
}

impl<'a> Data<'a> {
    pub(crate) async fn new(pool: PgPool, series: impl Into<Cow<'a, str>>, event: impl Into<Cow<'a, str>>) -> Result<Option<Data<'a>>, DataError> {
        let series = series.into();
        let event = event.into();
        Ok(
            sqlx::query!(r#"SELECT display_name, start, end_time, url, teams_url, video_url FROM events WHERE series = $1 AND event = $2"#, &series, &event).fetch_optional(&pool).await?
                .map(|row| Ok::<_, DataError>(Self {
                    display_name: row.display_name,
                    start: row.start,
                    end: row.end_time,
                    url: row.url.map(|url| url.parse()).transpose()?,
                    teams_url: row.teams_url.map(|url| url.parse()).transpose()?,
                    video_url: row.video_url.map(|url| url.parse()).transpose()?,
                    pool, series, event,
                }))
                .transpose()?
        )
    }

    pub(crate) fn team_config(&self) -> TeamConfig {
        match &*self.series {
            "mw" => TeamConfig::Multiworld,
            "pic" => TeamConfig::Pictionary,
            _ => unimplemented!(),
        }
    }

    fn is_started(&self) -> bool {
        self.start.map_or(false, |start| start <= Utc::now())
    }

    fn is_ended(&self) -> bool {
        self.end.map_or(false, |end| end <= Utc::now())
    }

    pub(crate) fn chests(&self) -> ChestAppearances {
        match (&*self.series, &*self.event) {
            ("mw", "3") => ChestAppearances::random(), //TODO update after preliminary base settings exist
            ("pic", _) => ChestAppearances::VANILLA, // no CAMC in Pictionary
            (_, _) => unimplemented!(),
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
            ) AS "exists!""#, &self.series, &self.event, i64::from(me.id)).fetch_one(&self.pool).await?
        } else {
            false
        };
        let start = self.start.map(|start| (
            start,
            start.with_timezone(&Europe::Berlin),
            start.with_timezone(&America::New_York),
        ));
        Ok(html! {
            h1 {
                a(class = "nav", href? = (!matches!(tab, Tab::Info)).then(|| uri!(info(&*self.series, &*self.event)).to_string())) : &self.display_name;
            }
            @if let Some((start_utc, start_berlin, start_new_york)) = start {
                h2 {
                    : start_utc.format("%A, %B %-d, %Y, %H:%M UTC").to_string();
                    : " • ";
                    : start_berlin.format(if start_berlin.date() == start_utc.date() { "%H:%M %Z" } else { "%A %H:%M %Z" }).to_string();
                    : " • ";
                    : start_new_york.format(if start_new_york.date() == start_utc.date() { "%-I:%M %p %Z" } else { "%A %-I:%M %p %Z" }).to_string(); //TODO omit minutes if 0
                    //TODO allow users to set timezone and format preferences, fall back to JS APIs
                }
            }
            div(class = "button-row") {
                @if let Tab::Info = tab {
                    span(class = "button selected") : "Info";
                } else {
                    a(class = "button", href = uri!(info(&*self.series, &*self.event)).to_string()) : "Info";
                }
                @if let Tab::Teams = tab {
                    span(class = "button selected") : "Teams";
                } else if let Some(ref teams_url) = self.teams_url {
                    a(class = "button", href = teams_url.to_string()) {
                        : favicon(teams_url);
                        : "Teams";
                    }
                } else {
                    a(class = "button", href = uri!(teams(&*self.series, &*self.event)).to_string()) : "Teams";
                }
                @if signed_up {
                    @if let Tab::MyStatus = tab {
                        span(class = "button selected") : "My Status";
                    } else {
                        a(class = "button", href = uri!(status(&*self.series, &*self.event)).to_string()) : "My Status";
                    }
                } else if !self.is_started() {
                    @if let Tab::Enter = tab {
                        span(class = "button selected") : "Enter";
                    } else {
                        a(class = "button", href = uri!(enter(&*self.series, &*self.event, _, _)).to_string()) : "Enter";
                    }
                    @if let Tab::FindTeam = tab {
                        span(class = "button selected") : "Find Teammates";
                    } else {
                        a(class = "button", href = uri!(find_team(&*self.series, &*self.event)).to_string()) : "Find Teammates";
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
                        @if url.host_str() == Some("racetime.gg") {
                            : "Race Room";
                        } else {
                            : "Website";
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
            a(href = uri!(info(&*self.series, &*self.event)).to_string()) : self.display_name;
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
    #[error("missing user data for a race organizer")]
    OrganizerUserData,
}

#[rocket::get("/event/<series>/<event>")]
pub(crate) async fn info(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, series: &str, event: &str) -> Result<RawHtml<String>, StatusOrError<InfoError>> {
    let content = match series {
        "mw" => mw::info(pool, event).await?,
        "pic" => pic::info(pool, event).await?,
        _ => unimplemented!(),
    };
    let data = Data::new((**pool).clone(), series, event).await.map_err(InfoError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(me.as_ref(), Tab::Info).await.map_err(InfoError::Sql)?;
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
pub(crate) async fn teams(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, series: &str, event: &str) -> Result<RawHtml<String>, StatusOrError<TeamsError>> {
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
    "#, series, event, me.as_ref().map(|me| i64::from(me.id))).fetch(&**pool);
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
        signups.push((team.name, team.racetime_slug, members));
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
                    @for (team_name, racetime_slug, members) in signups {
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
                            @for (role, user, is_confirmed) in members {
                                td(class = role.css_class()) {
                                    : user;
                                    @if !is_confirmed {
                                        : " (unconfirmed)";
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

#[rocket::get("/event/<series>/<event>/status")]
pub(crate) async fn status(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, series: &str, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(me.as_ref(), Tab::MyStatus).await?;
    Ok(page(pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("My Status — {}", data.display_name), {
        if let Some(ref me) = me {
            if let Some(row) = sqlx::query!(r#"SELECT id AS "id: Id", name, racetime_slug FROM teams, team_members WHERE
                id = team
                AND series = $1
                AND event = $2
                AND member = $3
                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            "#, series, event, i64::from(me.id)).fetch_optional(&**pool).await? {
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
                        p : "You are not signed up for this race.";
                        //p : "You can accept, decline, or retract unconfirmed team invitations on the teams page."; //TODO
                    }
                }
            }
        } else {
            html! {
                : header;
                article {
                    p {
                        a(href = uri!(auth::login(Some(uri!(status(series, event))))).to_string()) : "Sign in or create a Mido's House account";
                        : " to view your status for this race.";
                    }
                }
            }
        }
    }).await?)
}

#[rocket::get("/event/<series>/<event>/enter?<my_role>&<teammate>")]
pub(crate) async fn enter(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, client: &State<reqwest::Client>, series: &str, event: &str, my_role: Option<crate::event::pic::Role>, teammate: Option<Id>) -> Result<RawHtml<String>, StatusOrError<Error>> {
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
pub(crate) async fn find_team(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: &str, event: &str) -> Result<RawHtml<String>, StatusOrError<FindTeamError>> {
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
pub(crate) async fn confirm_signup(pool: &State<PgPool>, me: User, team: Id, csrf: Option<CsrfToken>, series: &str, event: &str, form: Form<EmptyForm>) -> Result<Redirect, StatusOrError<AcceptError>> {
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
            sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, 'accept', $3, $4, $5)", i64::from(id), i64::from(member), series, event, i64::from(me.id)).execute(&mut transaction).await.map_err(AcceptError::Sql)?;
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
pub(crate) async fn resign(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: &str, event: &str, team: Id) -> Result<RawHtml<String>, StatusOrError<Error>> {
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
pub(crate) async fn resign_post(pool: &State<PgPool>, me: User, csrf: Option<CsrfToken>, series: &str, event: &str, team: Id, form: Form<EmptyForm>) -> Result<Redirect, StatusOrError<ResignError>> {
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
                sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, $3, $4, $5, $6)", i64::from(id), i64::from(member.id), notification_kind as _, series, event, i64::from(me.id)).execute(&mut transaction).await.map_err(ResignError::Sql)?;
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
