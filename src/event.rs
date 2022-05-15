use {
    std::{
        borrow::Cow,
        cmp::Ordering::*,
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
    horrorshow::{
        RenderBox,
        box_html,
        html,
        rocket::TemplateExt as _,
    },
    itertools::Itertools as _,
    rocket::{
        FromForm,
        FromFormField,
        State,
        UriDisplayQuery,
        form::{
            self,
            Context,
            Contextual,
            Form,
        },
        http::Status,
        response::{
            Redirect,
            content::Html,
        },
        uri,
    },
    rocket_csrf::CsrfToken,
    sqlx::PgPool,
    url::Url,
    crate::{
        PageError,
        PageResult,
        PageStyle,
        auth,
        favicon::ChestAppearances,
        notification::SimpleNotificationKind,
        page,
        seed,
        user::User,
        util::{
            ContextualExt as _,
            CsrfForm,
            CsrfTokenExt as _,
            EmptyForm,
            Id,
            IdTable,
            Origin,
            RedirectOrContent,
            StatusOrError,
            field_errors,
            natjoin,
            render_form_error,
        },
    },
};

#[derive(Debug, sqlx::Decode)]
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

impl From<PictionaryRole> for Role {
    fn from(role: PictionaryRole) -> Self {
        match role {
            PictionaryRole::Sheikah => Self::Sheikah,
            PictionaryRole::Gerudo => Self::Gerudo,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, FromFormField, UriDisplayQuery)]
pub(crate) enum PictionaryRole {
    #[field(value = "sheikah")]
    Sheikah,
    #[field(value = "gerudo")]
    Gerudo,
}

impl PictionaryRole {
    pub(crate) fn to_html(&self) -> Box<dyn RenderBox> {
        match self {
            Self::Sheikah => box_html! {
                span(class = "sheikah") : "runner";
            },
            Self::Gerudo => box_html! {
                span(class = "gerudo") : "pilot";
            },
        }
    }
}

impl TryFrom<Role> for PictionaryRole {
    type Error = ();

    fn try_from(role: Role) -> Result<Self, ()> {
        match role {
            Role::Sheikah => Ok(Self::Sheikah),
            Role::Gerudo => Ok(Self::Gerudo),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, sqlx::Type, FromFormField)]
#[sqlx(type_name = "role_preference", rename_all = "snake_case")]
pub(crate) enum RolePreference {
    #[field(value = "sheikah_only")]
    SheikahOnly,
    #[field(value = "sheikah_preferred")]
    SheikahPreferred,
    #[field(value = "no_preference")]
    NoPreference,
    #[field(value = "gerudo_preferred")]
    GerudoPreferred,
    #[field(value = "gerudo_only")]
    GerudoOnly,
}

impl RolePreference {
    pub(crate) fn to_html(&self) -> Box<dyn RenderBox> {
        match self {
            Self::SheikahOnly => box_html! {
                span(class = "sheikah") : "runner only";
            },
            Self::SheikahPreferred => box_html! {
                span(class = "sheikah") : "runner preferred";
            },
            Self::NoPreference => box_html! {
                : "no preference";
            },
            Self::GerudoPreferred => box_html! {
                span(class = "gerudo") : "pilot preferred";
            },
            Self::GerudoOnly => box_html! {
                span(class = "gerudo") : "pilot only";
            },
        }
    }
}

enum TeamConfig {
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

struct Data<'a> {
    pool: PgPool,
    series: &'a str,
    event: &'a str,
    display_name: String,
    start: Option<DateTime<Utc>>,
    url: Option<Url>,
    video_url: Option<Url>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DataError {
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
}

impl<'a> Data<'a> {
    async fn new(pool: PgPool, series: &'a str, event: &'a str) -> Result<Option<Data<'a>>, DataError> {
        Ok(
            sqlx::query!("SELECT display_name, start, url, video_url FROM events WHERE series = $1 AND event = $2", &series, &event).fetch_optional(&pool).await?
                .map(|row| Ok::<_, DataError>(Self {
                    display_name: row.display_name,
                    start: row.start,
                    url: row.url.map(|url| url.parse()).transpose()?,
                    video_url: row.video_url.map(|url| url.parse()).transpose()?,
                    pool, series, event,
                }))
                .transpose()?
        )
    }

    fn team_config(&self) -> TeamConfig {
        match self.series {
            "mw" => TeamConfig::Multiworld,
            "pic" => TeamConfig::Pictionary,
            _ => unimplemented!(),
        }
    }

    async fn header(&self, me: &Option<User>, tab: Tab) -> sqlx::Result<Box<dyn RenderBox + Send + '_>> {
        let signed_up = if let Some(me) = me {
            sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                id = team
                AND series = $1
                AND event = $2
                AND member = $3
                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            ) AS "exists!""#, self.series, self.event, i64::from(me.id)).fetch_one(&self.pool).await?
        } else {
            false
        };
        let start = self.start.map(|start| (
            start,
            start.with_timezone(&Europe::Berlin),
            start.with_timezone(&America::New_York),
        ));
        Ok(box_html! {
            h1 {
                a(class = "nav", href? = (!matches!(tab, Tab::Info)).then(|| uri!(info(self.series, self.event)).to_string())) : &self.display_name;
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
                    a(class = "button", href = uri!(info(self.series, self.event)).to_string()) : "Info";
                }
                @if let Tab::Teams = tab {
                    span(class = "button selected") : "Teams";
                } else {
                    a(class = "button", href = uri!(teams(self.series, self.event)).to_string()) : "Teams";
                }
                @if signed_up {
                    @if let Tab::MyStatus = tab {
                        span(class = "button selected") : "My Status";
                    } else {
                        a(class = "button", href = uri!(status(self.series, self.event)).to_string()) : "My Status";
                    }
                } else {
                    @if let Tab::Enter = tab {
                        span(class = "button selected") : "Enter";
                    } else {
                        a(class = "button", href = uri!(enter(self.series, self.event, _, _)).to_string()) : "Enter";
                    }
                    @if let Tab::FindTeam = tab {
                        span(class = "button selected") : "Find Teammates";
                    } else {
                        a(class = "button", href = uri!(find_team(self.series, self.event)).to_string()) : "Find Teammates";
                    }
                }
                //a(class = "button") : "Volunteer"; //TODO
                @if let Some(ref video_url) = self.video_url {
                    a(class = "button", href = video_url.to_string()) {
                        @if video_url.host_str() == Some("twitch.tv") {
                            img(class = "favicon", alt = "external link (twitch.tv)", src = "https://static.twitchcdn.net/assets/favicon-16-52e571ffea063af7a7f4.png", width = "16", height = "16", srcset = "https://static.twitchcdn.net/assets/favicon-32-e29e246c157142c94346.png 2x");
                        } else {
                            //TODO generic “external link” image?
                        }
                        : "Watch";
                    }
                }
                @if let Some(ref url) = self.url {
                    a(class = "button", href = url.to_string()) {
                        @if url.host_str() == Some("racetime.gg") {
                            img(class = "favicon", alt = "external link (racetime.gg)", src = "https://racetime.gg/favicon.ico", width = "16", height = "16");
                            : "Race Room";
                        } else {
                            //TODO generic “external link” image?
                            : "Website";
                        }
                    }
                }
            }
        })
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
pub(crate) async fn info(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, series: &str, event: &str) -> Result<Html<String>, StatusOrError<InfoError>> {
    let content = match (series, event) {
        ("mw", "3") => {
            let organizers = stream::iter([
                11983715422555811980, // ACreativeUsername
                10663518306823692018, // Alaszun
                11587964556511372305, // Bliven
                6374388881117205057, // felixoide4
                14571800683221815449, // Fenhl
                12937129924130129092, // Hamsda
                2315005348393237449, // rockchalk
                5305697930717211129, // tenacious_toad
            ])
                .map(Id)
                .then(|id| async move { User::from_id(pool, id).await?.ok_or(InfoError::OrganizerUserData) })
                .try_collect::<Vec<_>>().await?;
            let organizers = natjoin(organizers.into_iter().map(|organizer| organizer.into_html()));
            (box_html! {
                article {
                    p {
                        : "This is a placeholder page for the third Ocarina of Time randomizer multiworld tournament, organized by ";
                        : organizers;
                        : ". More infos coming soon.";
                    }
                }
            }) as Box<dyn RenderBox + Send>
        }
        ("pic", "rs1") => {
            let sample_seeds = seed::table(stream::iter(vec![
                seed::Data { web: Some(seed::OotrWebData { id: 1079630, gen_time: Utc.ymd(2022, 4, 22).and_hms(15, 59, 36) }), file_stem: Cow::Borrowed("OoTR_1079630_V6516H22IW") },
                seed::Data { web: Some(seed::OotrWebData { id: 1079637, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 1, 5) }), file_stem: Cow::Borrowed("OoTR_1079637_HAH75EOAHQ") },
                seed::Data { web: Some(seed::OotrWebData { id: 1079645, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 3, 19) }), file_stem: Cow::Borrowed("OoTR_1079645_6XZJOSDCRW") },
                seed::Data { web: Some(seed::OotrWebData { id: 1079646, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 3, 53) }), file_stem: Cow::Borrowed("OoTR_1079646_AJZWAB1X3U") },
                seed::Data { web: Some(seed::OotrWebData { id: 1079648, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 4, 11) }), file_stem: Cow::Borrowed("OoTR_1079648_1DHCCQB5AC") },
            ])).await.map_err(InfoError::Io)?;
            let organizers = stream::iter([
                5961629664912637980, // Tjongejonge_
                2689982510832487907, // ksinjah
                14571800683221815449, // Fenhl
                3722744861553903438, // melqwii
                14099802746436324950, // TeaGrenadier
            ])
                .map(Id)
                .then(|id| async move { User::from_id(pool, id).await?.ok_or(InfoError::OrganizerUserData) })
                .try_collect::<Vec<_>>().await?;
            let organizers = natjoin(organizers.into_iter().map(|organizer| organizer.into_html()));
            box_html! {
                article {
                    h2 : "What is a Pictionary Spoiler Log Race?";
                    p : "Each team consists of one Runner and one Spoiler Log Pilot who is drawing. The pilot has to figure out a way through the seed and how to tell their runner in drawing what checks they need to do. Hints are obviously disabled.";
                    p : "This time, we are doing something slightly different: The settings will be random, with weights based on the Random Settings League but adjusted for Pictionary. To compensate for the additional complexity, the preparation time for the pilot will be 30 minutes instead of the usual 15.";
                    p {
                        : "Before the race we will provide a room on ";
                        a(href = "https://aggie.io/") : "aggie.io";
                        : " to each team. The canvas will be set to 660×460 for restream purposes.";
                    }
                    p {
                        strong : "At the ±0 minute mark:";
                        : " The pilot is now allowed to look at the spoiler log and can start figuring out the route.";
                    }
                    p {
                        strong : "At the +30 minute mark:";
                        : " The pilot is allowed to start drawing and the runner is allowed to start the file.";
                    }
                    h2 : "Rules";
                    p {
                        : "The race uses the ";
                        a(href = "https://rsl-leaderboard.web.app/rules") : "Random Settings League";
                        : " ruleset.";
                    }
                    p : "The pilot is allowed to communicate to their partner only via drawing and may watch and hear the stream of the runner. Runners may talk to their pilot. We would prefer if the pilot did not directly respond to questions, as figuring things out is supposed to be part of the challenge, but in the end it's up to the individual teams.";
                    p {
                        strong : "Allowed:";
                        : " Arrows, Question marks, ingame symbols, check marks, “X” for crossing out stuff.";
                    }
                    p {
                        strong : "Not allowed:";
                        : " Any kind of numbers or letters.";
                    }
                    h3 : "Examples";
                    p : "For having a better idea what we mean in regards with the rules / communication, here are some examples:";
                    ol {
                        li {
                            : "The pilot draws 3 spiders and a bow. The runner then asks if there is a bow on 30 skulls. The pilot then draws a smiley or a checkmark for confirmation or a sad face for “no” — that is ";
                            strong : "allowed";
                            : ".";
                        }
                        li {
                            : "The runner just asks without a drawing if it's AD or if a specific check is required — that is ";
                            strong : "not allowed";
                            : ".";
                        }
                        li {
                            : "The team has prepared a language for specific checks to avoid the requirement to draw the check (like morse code etc.) — that is ";
                            strong : "not allowed";
                            : ".";
                        }
                        li {
                            : "The runner says “if I need to do the toilet check, draw a heart” — that is ";
                            strong : "not allowed";
                            : ".";
                        }
                        li {
                            : "The runner says: “since you didn't draw anything in the Lost Woods, I'm gonna skip all the checks there and go immediately to the Sacred Forest Meadow” — that is ";
                            strong : "allowed";
                            : ".";
                        }
                    }
                    h2 : "Settings";
                    p {
                        : "The seed will be rolled on ";
                        a(href = "https://github.com/fenhl/plando-random-settings/tree/a08223927138c6f039c1aa3603130d8bd900fb48") : "version 2.2.10 Fenhl-5";
                        : " of the random settings script. We will be using ";
                        a(href = "https://github.com/fenhl/plando-random-settings/blob/a08223927138c6f039c1aa3603130d8bd900fb48/weights/pictionary_override.json") : "a special weights override";
                        : " for Pictionary spoiler log races. Changes include:";
                    }
                    ul {
                        li : "To reduce complexity for the pilot, overworld ER is disabled.";
                        li : "Master Quest dungeons are disabled due to a lack of documentation for spoiler log location names.";
                        li {
                            : "Some of the settings and combinations of settings that are disabled in RSL for information-related reasons are turned back on, since they're not an issue if you have the spoiler log:";
                            ul {
                                li : "Triforce hunt + minimal item pool";
                                li : "Ice trap mayhem/onslaught + quad damage/OHKO";
                                li : "Separate keysanity setting for the Thieves' Hideout";
                                li : "Random scrub prices without a starting wallet";
                                li : "All goals reachable (33% chance)";
                            }
                        }
                        li {
                            : "The seed will be rolled on ";
                            a(href = "https://github.com/fenhl/OoT-Randomizer/tree/d7d16553252b96bd0f50ef96c2af250b7bfbba58") : "Fenhl's branch";
                            : ", so some settings that aren't in Dev-R are added:";
                            ul {
                                li : "Heart container requirements for rainbow bridge and/or Ganon boss key (50% chance each to replace a skulltula token requirement)";
                                li : "Full one-way entrance randomization (owls, warp songs, and spawns can lead to more destinations; 25% chance each)";
                                li : "One bonk KO (5% chance)";
                                li : "Closed Kokiri Forest exit (50% chance, independent of Closed/Open Deku)";
                            }
                        }
                        li {
                            : "Some newer settings that are not yet included in RSL due to the ongoing tournament are enabled:";
                            ul {
                                li : "Planted magic beans (50% chance)";
                                li : "Key rings for all dungeons (20% chance)";
                            }
                        }
                        li {
                            : "The following settings that would give the runner hints or similar information are disabled:";
                            ul {
                                li : "Maps & compasses give info";
                                li : "Chest appearance matches contents";
                                li : "Gossip stone hints";
                                li : "Temple of Time altar hints";
                                li : "Ganondorf light arrows hint";
                                li : "Warp song text boxes hinting destinations";
                            }
                        }
                    }
                    p {
                        : "Everything else is the same as ";
                        a(href = "https://rsl-leaderboard.web.app/weights") : "the usual RSL weights";
                        : ".";
                    }
                    h2 : "Sample seeds";
                    p : "Since the random settings script isn't available online, we've prepared some sample seeds:";
                    : sample_seeds;
                    h2 : "Further information";
                    p {
                        : "The race is organized by ";
                        : organizers;
                        : ". We will answer questions and inform about recent events on The Silver Gauntlets Discord in the #pictionary-spoiler-log channel (";
                        a(href = "https://discord.gg/m8z8ZqtN8H") : "invite link";
                        : " • ";
                        a(href = "https://discord.com/channels/663207960432082944/865206020015128586") : "direct channel link";
                        : "). If you have any questions, feel free to ask there!";
                    }
                    p : "Special thanks to winniedemon who will be helping us keep important posts from getting lost in the Discord!";
                }
            }
        }
        (_, _) => unimplemented!(),
    };
    let data = Data::new((**pool).clone(), series, event).await.map_err(InfoError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&me, Tab::Info).await.map_err(InfoError::Sql)?;
    page(pool, &me, &uri, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, &data.display_name, html! {
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
pub(crate) async fn teams(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, series: &str, event: &str) -> Result<Html<String>, StatusOrError<TeamsError>> {
    let data = Data::new((**pool).clone(), series, event).await.map_err(TeamsError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&me, Tab::Teams).await.map_err(TeamsError::Sql)?;
    let mut signups = Vec::default();
    let mut teams_query = sqlx::query!(r#"SELECT id AS "id!: Id", name FROM teams WHERE
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
                let user = User::from_id(pool, row.id).await?.ok_or(TeamsError::NonexistentUser)?;
                Ok::<_, TeamsError>((role, user, is_confirmed))
            })
            .try_collect::<Vec<_>>().await?;
        signups.push((team.name, members));
    }
    page(pool, &me, &uri, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, &format!("Teams — {}", data.display_name), html! {
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
                    @for (team_name, members) in signups {
                        tr {
                            td : team_name.unwrap_or_default();
                            @for (role, user, is_confirmed) in members {
                                td(class = role.css_class()) {
                                    : user.to_html();
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
pub(crate) async fn status(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, series: &str, event: &str) -> Result<Html<String>, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&me, Tab::MyStatus).await?;
    Ok(page(pool, &me, &uri, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, &format!("My Status — {}", data.display_name), {
        if let Some(ref me) = me {
            if let Some(row) = sqlx::query!(r#"SELECT id AS "id: Id", name FROM teams, team_members WHERE
                id = team
                AND series = $1
                AND event = $2
                AND member = $3
                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            "#, series, event, i64::from(me.id)).fetch_optional(&**pool).await? {
                box_html! {
                    : header;
                    p {
                        : "You are signed up as part of ";
                        @if let Some(name) = row.name {
                            i : name;
                        } else {
                            : "an unnamed team";
                        }
                        //TODO list teammates
                        : ".";
                    }
                    p : "More options coming soon"; //TODO options to change team name, swap roles, or opt in/out for restreaming
                    p {
                        a(href = uri!(resign(series, event, row.id)).to_string()) : "Resign"; //TODO hide if the event is over
                    }
                }
            } else {
                (box_html! {
                    : header;
                    article {
                        p : "You are not signed up for this race.";
                        //p : "You can accept, decline, or retract unconfirmed team invitations on the teams page."; //TODO
                    }
                } as Box<dyn RenderBox + Send>)
            }
        } else {
            box_html! {
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

enum PictionaryRandomSettingsEnterFormDefaults<'a> {
    Context(Context<'a>),
    Values {
        my_role: Option<PictionaryRole>,
        teammate: Option<Id>,
    },
}

impl<'v> PictionaryRandomSettingsEnterFormDefaults<'v> {
    fn errors(&self) -> Vec<&form::Error<'v>> {
        match self {
            Self::Context(ctx) => ctx.errors().collect(),
            Self::Values { .. } => Vec::default(),
        }
    }

    fn team_name(&self) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value("team_name"),
            Self::Values { .. } => None,
        }
    }

    fn my_role(&self) -> Option<PictionaryRole> {
        match self {
            Self::Context(ctx) => match ctx.field_value("my_role") {
                Some("sheikah") => Some(PictionaryRole::Sheikah),
                Some("gerudo") => Some(PictionaryRole::Gerudo),
                _ => None,
            },
            &Self::Values { my_role, .. } => my_role,
        }
    }

    fn teammate(&self) -> Option<Id> {
        self.teammate_text().and_then(|text| text.parse().ok())
    }

    fn teammate_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Context(ctx) => ctx.field_value("teammate").map(Cow::Borrowed),
            &Self::Values { teammate, .. } => teammate.map(|id| Cow::Owned(id.0.to_string())),
        }
    }
}

async fn pictionary_random_settings_enter_form(me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, defaults: PictionaryRandomSettingsEnterFormDefaults<'_>) -> PageResult {
    let header = data.header(&me, Tab::Enter).await?;
    page(&data.pool, &me, &uri, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, &format!("Enter — {}", data.display_name), if me.is_some() {
        let mut errors = defaults.errors();
        let form_content = html! {
            : csrf.to_html();
            legend {
                : "Fill out this form to enter the race as a team. Your teammate will receive an invitation they have to accept to confirm the signup. If you don't have a team yet, you can ";
                a(href = uri!(find_team(data.series, data.event)).to_string()) : "look for a teammate";
                : " instead.";
            }
            fieldset {
                |tmpl| field_errors(tmpl, &mut errors, "team_name");
                label(for = "team_name") : "Team Name:";
                input(type = "text", name = "team_name", value? = defaults.team_name());
                label(class = "help") : "(Optional unless you want to be on restream. Can be changed later. Organizers may remove inappropriate team names.)";
            }
            fieldset {
                |tmpl| field_errors(tmpl, &mut errors, "my_role");
                label(for = "my_role") : "My Role:";
                input(id = "my_role-sheikah", class = "sheikah", type = "radio", name = "my_role", value = "sheikah", checked? = defaults.my_role() == Some(PictionaryRole::Sheikah));
                label(class = "sheikah", for = "my_role-sheikah") : "Runner";
                input(id = "my_role-gerudo", class = "gerudo", type = "radio", name = "my_role", value = "gerudo", checked? = defaults.my_role() == Some(PictionaryRole::Gerudo));
                label(class = "gerudo", for = "my_role-gerudo") : "Pilot";
            }
            fieldset {
                |tmpl| field_errors(tmpl, &mut errors, "teammate");
                label(for = "teammate") : "Teammate:";
                input(type = "text", name = "teammate", value? = defaults.teammate_text().as_deref());
                label(class = "help") : "(Enter your teammate's Mido's House user ID. It can be found on their profile page.)"; //TODO add JS-based user search?
            }
            fieldset {
                input(type = "submit", value = "Submit");
            }
        }.write_to_html()?;
        html! {
            : header;
            form(action = uri!(enter_post(data.series, data.event)).to_string(), method = "post") {
                @for error in errors {
                    |tmpl| render_form_error(tmpl, error);
                }
                : form_content;
            }
        }.write_to_html()?
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(enter(data.series, data.event, defaults.my_role(), defaults.teammate()))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to enter this race.";
                }
            }
        }.write_to_html()?
    }).await
}

#[rocket::get("/event/<series>/<event>/enter?<my_role>&<teammate>")]
pub(crate) async fn enter(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: &str, event: &str, my_role: Option<PictionaryRole>, teammate: Option<Id>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(pictionary_random_settings_enter_form(me, uri, csrf, data, PictionaryRandomSettingsEnterFormDefaults::Values { my_role, teammate }).await?))
}

#[derive(FromForm)]
pub(crate) struct PictionaryEnterForm {
    #[field(default = String::new())]
    csrf: String,
    team_name: String,
    my_role: PictionaryRole,
    teammate: Id,
}

impl CsrfForm for PictionaryEnterForm { //TODO derive
    fn csrf(&self) -> &String { &self.csrf }
}

#[rocket::post("/event/<series>/<event>/enter", data = "<form>")]
pub(crate) async fn enter_post(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: &str, event: &str, form: Form<Contextual<'_, PictionaryEnterForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    //TODO deny action if the event has started
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if let Some(ref value) = form.value {
        let mut transaction = pool.begin().await?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $4)
        ) AS "exists!""#, series, event, i64::from(me.id), i64::from(value.teammate)).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("A team with these members is already proposed for this race. Check your notifications to accept the invite, or ask your teammate to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, series, event, i64::from(me.id)).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("You are already signed up for this race."));
        }
        if !value.team_name.is_empty() && sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
            series = $1
            AND event = $2
            AND name = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, series, event, value.team_name).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("A team with this name is already signed up for this race.").with_name("team_name"));
        }
        if value.my_role == PictionaryRole::Sheikah && me.racetime_id.is_none() {
            form.context.push_error(form::Error::validation("A racetime.gg account is required to enter as runner. Go to your profile and select “Connect a racetime.gg account”.").with_name("my_role")); //TODO direct link?
        }
        if value.teammate == me.id {
            form.context.push_error(form::Error::validation("You cannot be your own teammate.").with_name("teammate"));
        }
        if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, i64::from(value.teammate)).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("There is no user with this ID.").with_name("teammate"));
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, series, event, i64::from(value.teammate)).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("This user is already signed up for this race.").with_name("teammate"));
        }
        //TODO check to make sure the teammate hasn't blocked the user submitting the form (or vice versa) or the event
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(pictionary_random_settings_enter_form(Some(me), uri, csrf, data, PictionaryRandomSettingsEnterFormDefaults::Context(form.context)).await?)
        } else {
            let id = Id::new(&mut transaction, IdTable::Teams).await?;
            sqlx::query!("INSERT INTO teams (id, series, event, name) VALUES ($1, $2, $3, $4)", i64::from(id), series, event, (!value.team_name.is_empty()).then(|| &value.team_name)).execute(&mut transaction).await?;
            sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', $3)", i64::from(id), i64::from(me.id), Role::from(value.my_role) as _).execute(&mut transaction).await?;
            sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'unconfirmed', $3)", i64::from(id), i64::from(value.teammate), match value.my_role { PictionaryRole::Sheikah => Role::Gerudo, PictionaryRole::Gerudo => Role::Sheikah } as _).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(teams(series, event))))
        }
    } else {
        RedirectOrContent::Content(pictionary_random_settings_enter_form(Some(me), uri, csrf, data, PictionaryRandomSettingsEnterFormDefaults::Context(form.context)).await?)
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum PictionaryRandomSettingsFindTeamError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Horrorshow(#[from] horrorshow::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("unknown user")]
    UnknownUser,
}

async fn pictionary_random_settings_find_team_form(me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, context: Context<'_>) -> Result<Html<String>, PictionaryRandomSettingsFindTeamError> {
    let header = data.header(&me, Tab::FindTeam).await?;
    let mut my_role = None;
    let mut looking_for_team = Vec::default();
    let mut looking_for_team_query = sqlx::query!(r#"SELECT user_id AS "user!: Id", role AS "role: RolePreference" FROM looking_for_team WHERE series = $1 AND event = $2"#, data.series, data.event).fetch(&data.pool);
    while let Some(row) = looking_for_team_query.try_next().await? {
        let user = User::from_id(&data.pool, row.user).await?.ok_or(PictionaryRandomSettingsFindTeamError::UnknownUser)?;
        if me.as_ref().map_or(false, |me| user.id == me.id) { my_role = Some(row.role) }
        let can_invite = me.as_ref().map_or(true, |me| user.id != me.id) && true /*TODO not already in a team with that user */;
        looking_for_team.push((user, row.role, can_invite));
    }
    let form = if me.is_some() {
        let mut errors = context.errors().collect_vec();
        if my_role.is_none() {
            let form_content = html! {
                : csrf.to_html();
                legend {
                    : "Fill out this form to add yourself to the list below.";
                }
                fieldset {
                    |tmpl| field_errors(tmpl, &mut errors, "role");
                    label(for = "role") : "Role:";
                    input(id = "role-sheikah_only", class = "sheikah", type = "radio", name = "role", value = "sheikah_only", checked? = context.field_value("role") == Some("sheikah_only"));
                    label(class = "sheikah", for = "role-sheikah_only") : "Runner only";
                    input(id = "role-sheikah_preferred", class = "sheikah", type = "radio", name = "role", value = "sheikah_preferred", checked? = context.field_value("role") == Some("sheikah_preferred"));
                    label(class = "sheikah", for = "role-sheikah_preferred") : "Runner preferred";
                    input(id = "role-no_preference", type = "radio", name = "role", value = "no_preference", checked? = context.field_value("role").map_or(true, |role| role == "no_preference"));
                    label(for = "role-no_preference") : "No preference";
                    input(id = "role-gerudo_preferred", class = "gerudo", type = "radio", name = "role", value = "gerudo_preferred", checked? = context.field_value("role") == Some("gerudo_preferred"));
                    label(class = "gerudo", for = "role-gerudo_preferred") : "Pilot preferred";
                    input(id = "role-gerudo_only", class = "gerudo", type = "radio", name = "role", value = "gerudo_only", checked? = context.field_value("role") == Some("gerudo_only"));
                    label(class = "gerudo", for = "role-gerudo_only") : "Pilot only";
                }
                fieldset {
                    input(type = "submit", value = "Submit");
                }
            }.write_to_html()?;
            Some(html! {
                form(action = uri!(find_team_post(data.series, data.event)).to_string(), method = "post") {
                    @for error in errors {
                        |tmpl| render_form_error(tmpl, error);
                    }
                    : form_content;
                }
            }.write_to_html()?)
        } else {
            None
        }
    } else {
        Some(html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(find_team(data.series, data.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to add yourself to this list.";
                }
            }
        }.write_to_html()?)
    };
    let can_invite_any = looking_for_team.iter().any(|&(_, _, can_invite)| can_invite);
    let looking_for_team = looking_for_team.into_iter()
        .map(|(user, role, can_invite)| (user, role, can_invite.then(|| match (my_role, role) {
            // if I haven't signed up looking for team, default to the role opposite the invitee's preference
            (None, RolePreference::SheikahOnly | RolePreference::SheikahPreferred) => Some(PictionaryRole::Gerudo),
            (None, RolePreference::GerudoOnly | RolePreference::GerudoPreferred) => Some(PictionaryRole::Sheikah),
            (None, RolePreference::NoPreference) => None,
            // if I have signed up looking for team, take the role that's more preferred by me than by the invitee
            (Some(my_role), _) => match my_role.cmp(&role) {
                Less => Some(PictionaryRole::Sheikah),
                Equal => None,
                Greater => Some(PictionaryRole::Gerudo),
            },
        })))
        .collect_vec();
    Ok(page(&data.pool, &me, &uri, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, "Find Teammates — 1st Random Settings Pictionary Spoiler Log Race", html! {
        : header;
        : form;
        table {
            thead {
                tr {
                    th : "User";
                    th : "Role";
                    @if can_invite_any {
                        th;
                    }
                }
            }
            tbody {
                @if looking_for_team.is_empty() {
                    tr {
                        td(colspan = "3") {
                            i : "(no one currently looking for teammates)";
                        }
                    }
                } else {
                    @for (user, role, invite) in looking_for_team {
                        tr {
                            td : user.to_html();
                            td : role.to_html();
                            @if can_invite_any {
                                td {
                                    @if let Some(my_role) = invite {
                                        a(class = "button", href = uri!(enter(data.series, data.event, my_role, Some(user.id))).to_string()) : "Invite";
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }).await?)
}

#[rocket::get("/event/<series>/<event>/find-team")]
pub(crate) async fn find_team(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: &str, event: &str) -> Result<RedirectOrContent, StatusOrError<PictionaryRandomSettingsFindTeamError>> {
    let data = Data::new((**pool).clone(), series, event).await.map_err(PictionaryRandomSettingsFindTeamError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(pictionary_random_settings_find_team_form(me, uri, csrf, data, Context::default()).await?))
}

#[derive(FromForm)]
pub(crate) struct FindTeamForm {
    #[field(default = String::new())]
    csrf: String,
    role: RolePreference,
}

impl CsrfForm for FindTeamForm { //TODO derive
    fn csrf(&self) -> &String { &self.csrf }
}

#[rocket::post("/event/<series>/<event>/find-team", data = "<form>")]
pub(crate) async fn find_team_post(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: &str, event: &str, form: Form<Contextual<'_, FindTeamForm>>) -> Result<RedirectOrContent, StatusOrError<PictionaryRandomSettingsFindTeamError>> {
    let data = Data::new((**pool).clone(), series, event).await.map_err(PictionaryRandomSettingsFindTeamError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    //TODO deny action if the event has started
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if let Some(ref value) = form.value {
        let mut transaction = pool.begin().await.map_err(PictionaryRandomSettingsFindTeamError::Sql)?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM looking_for_team WHERE
            series = $1
            AND event = $2
            AND user_id = $3
        ) AS "exists!""#, series, event, i64::from(me.id)).fetch_one(&mut transaction).await.map_err(PictionaryRandomSettingsFindTeamError::Sql)? {
            form.context.push_error(form::Error::validation("You are already on the list."));
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, series, event, i64::from(me.id)).fetch_one(&mut transaction).await.map_err(PictionaryRandomSettingsFindTeamError::Sql)? {
            form.context.push_error(form::Error::validation("You are already signed up for this race."));
        }
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(pictionary_random_settings_find_team_form(Some(me), uri, csrf, data, form.context).await?)
        } else {
            sqlx::query!("INSERT INTO looking_for_team (series, event, user_id, role) VALUES ($1, $2, $3, $4)", series, event, i64::from(me.id), value.role as _).execute(&mut transaction).await.map_err(PictionaryRandomSettingsFindTeamError::Sql)?;
            transaction.commit().await.map_err(PictionaryRandomSettingsFindTeamError::Sql)?;
            RedirectOrContent::Redirect(Redirect::to(uri!(find_team(series, event))))
        }
    } else {
        RedirectOrContent::Content(pictionary_random_settings_find_team_form(Some(me), uri, csrf, data, form.context).await?)
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum AcceptError {
    #[error(transparent)] Csrf(#[from] rocket_csrf::VerificationFailure),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("you haven't been invited to this team")]
    NotInTeam,
    #[error("a racetime.gg account is required to enter as runner")]
    RaceTimeAccountRequired,
}

#[rocket::post("/event/<series>/<event>/confirm/<team>", data = "<form>")]
pub(crate) async fn confirm_signup(pool: &State<PgPool>, me: User, team: Id, csrf: Option<CsrfToken>, series: &str, event: &str, form: Form<EmptyForm>) -> Result<Redirect, AcceptError> {
    form.verify(&csrf)?; //TODO option to resubmit on error page (with some “are you sure?” wording)
    //TODO deny action if the event has started
    let mut transaction = pool.begin().await?;
    if let Some(role) = sqlx::query_scalar!(r#"SELECT role AS "role: Role" FROM team_members WHERE team = $1 AND member = $2 AND status = 'unconfirmed'"#, i64::from(team), i64::from(me.id)).fetch_optional(&mut transaction).await? {
        if role == Role::Sheikah && me.racetime_id.is_none() {
            return Err(AcceptError::RaceTimeAccountRequired)
        }
        for member in sqlx::query_scalar!(r#"SELECT member AS "id: Id" FROM team_members WHERE team = $1 AND (status = 'created' OR status = 'confirmed')"#, i64::from(team)).fetch_all(&mut transaction).await? {
            let id = Id::new(&mut transaction, IdTable::Notifications).await?;
            sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, 'accept', $3, $4, $5)", i64::from(id), i64::from(member), series, event, i64::from(me.id)).execute(&mut transaction).await?;
        }
        sqlx::query!("UPDATE team_members SET status = 'confirmed' WHERE team = $1 AND member = $2", i64::from(team), i64::from(me.id)).execute(&mut transaction).await?;
        // if this confirms the team, remove all members from looking_for_team
        sqlx::query!("DELETE FROM looking_for_team WHERE
            EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND member = user_id)
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND status = 'unconfirmed')
        ", i64::from(team)).execute(&mut transaction).await?;
        //TODO also remove all other teams with member overlap, and notify
        transaction.commit().await?;
        Ok(Redirect::to(uri!(teams(series, event))))
    } else {
        transaction.rollback().await?;
        Err(AcceptError::NotInTeam)
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum ResignError {
    #[error(transparent)] Csrf(#[from] rocket_csrf::VerificationFailure),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("can't delete teams you're not part of")]
    NotInTeam,
}

#[rocket::get("/event/<series>/<event>/resign/<team>")]
pub(crate) async fn resign(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: &str, event: &str, team: Id) -> Result<Html<String>, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    //TODO display error message if the event is over
    Ok(page(pool, &me, &uri, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, &format!("Resign — {}", data.display_name), html! {
        //TODO different wording if the event has started
        p {
            : "Are you sure you want to retract your team's registration from ";
            a(href = uri!(info(series, event)).to_string()) : "the 1st Random Settings Pictionary Spoiler Log Race"; //TODO don't hardcode event name
            : "? If you change your mind later, you will need to invite your teammates again.";
        }
        div(class = "button-row") {
            form(action = uri!(crate::event::resign_post(series, event, team)).to_string(), method = "post") {
                : csrf.to_html();
                input(type = "submit", value = "Yes, resign");
            }
        }
    }).await?)
}

#[rocket::post("/event/<series>/<event>/resign/<team>", data = "<form>")]
pub(crate) async fn resign_post(pool: &State<PgPool>, me: User, csrf: Option<CsrfToken>, series: &str, event: &str, team: Id, form: Form<EmptyForm>) -> Result<Redirect, ResignError> {
    form.verify(&csrf)?; //TODO option to resubmit on error page (with some “are you sure?” wording)
    //TODO deny action if the event is over
    //TODO if the event has started, only mark the team as resigned, don't delete data
    let mut transaction = pool.begin().await?;
    let delete = sqlx::query!(r#"DELETE FROM team_members WHERE team = $1 RETURNING member AS "id: Id", status AS "status: SignupStatus""#, i64::from(team)).fetch_all(&mut transaction).await?;
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
                let id = Id::new(&mut transaction, IdTable::Notifications).await?;
                sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, $3, $4, $5, $6)", i64::from(id), i64::from(member.id), notification_kind as _, series, event, i64::from(me.id)).execute(&mut transaction).await?;
            }
        }
        sqlx::query!("DELETE FROM teams WHERE id = $1", i64::from(team)).execute(&mut transaction).await?;
        transaction.commit().await?;
        Ok(Redirect::to(uri!(teams(series, event))))
    } else {
        transaction.rollback().await?;
        Err(ResignError::NotInTeam)
    }
}
