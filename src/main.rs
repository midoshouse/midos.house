#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::{
        mem,
        time::Duration,
    },
    anyhow::Result,
    futures::stream::TryStreamExt as _,
    horrorshow::{
        RenderBox,
        RenderOnce,
        TemplateBuffer,
        box_html,
        helper::doctype,
        html,
        rocket::TemplateExt as _, //TODO use a rocket_util wrapper instead?
    },
    itertools::Itertools as _,
    rand::prelude::*,
    rocket::{
        FromForm,
        FromFormField,
        Request,
        Responder,
        State,
        config::SecretKey,
        form::{
            self,
            Context,
            Contextual,
            Form,
        },
        fs::FileServer,
        response::{
            Redirect,
            content::Html,
        },
        uri,
    },
    rocket_oauth2::{
        OAuth2,
        OAuthConfig,
    },
    rocket_util::Suffix,
    sqlx::{
        PgPool,
        postgres::{
            PgConnectOptions,
            PgTypeInfo,
        },
    },
    crate::{
        auth::User,
        config::Config,
        favicon::{
            ChestAppearances,
            ChestTextures,
        },
    },
};

mod auth;
mod config;
mod favicon;

enum PageKind {
    Index,
    Login,
    Other,
}

struct PageStyle {
    kind: PageKind,
    chests: ChestAppearances,
    is_banner: bool,
}

impl Default for PageStyle {
    fn default() -> Self {
        Self {
            kind: PageKind::Other,
            chests: ChestAppearances::random(),
            is_banner: false,
        }
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
enum PageError {
    #[error(transparent)] Horrorshow(#[from] horrorshow::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("missing user data for Fenhl")]
    FenhlUserData,
}

type PageResult = Result<Html<String>, PageError>;

async fn page(pool: &PgPool, user: &Option<User>, style: PageStyle, title: &str, content: impl RenderOnce) -> PageResult {
    let (banner_content, content) = if style.is_banner {
        (Some(content), None)
    } else {
        (None, Some(content))
    };
    let fenhl = User::from_id(pool, -3874943390487736167).await?.ok_or(PageError::FenhlUserData)?;
    Ok(html! {
        : doctype::HTML;
        html {
            head {
                meta(charset = "utf-8");
                meta(name = "viewport", content = "width=device-width, initial-scale=1, shrink-to-fit=no");
                title : title;
                link(rel = "icon", sizes = "512x512", type = "image/png", href = uri!(favicon::favicon_png(style.chests.textures(), Suffix(512, "png"))).to_string());
                link(rel = "icon", sizes = "1024x1024", type = "image/png", href = uri!(favicon::favicon_png(style.chests.textures(), Suffix(1024, "png"))).to_string());
                link(rel = "stylesheet", href = "/static/common.css");
            }
            body(class = style.is_banner.then(|| "fullscreen")) {
                div {
                    nav(class? = matches!(style.kind, PageKind::Index).then(|| "index")) {
                        a(class = "nav", href? = (!matches!(style.kind, PageKind::Index)).then(|| uri!(index).to_string())) {
                            //TODO get 128px images, use those (with 256 as a 2x srcset)
                            div(class = "logo") {
                                @for chest in style.chests.0 {
                                    img(class = "chest", src = format!("/static/chest/{}256.png", char::from(chest.texture)));
                                }
                            }
                            h1 : "Mido's House";
                        }
                        @if !matches!(style.kind, PageKind::Login) {
                            div(id = "login") {
                                @if let Some(user) = user {
                                    : format!("signed in as {}", user.display_name);
                                    br;
                                    //TODO links to profile and preferences
                                    a(href = uri!(auth::logout).to_string()) : "Sign out";
                                } else {
                                    a(href = uri!(auth::login).to_string()) : "Sign in / Create account";
                                }
                            }
                        }
                    }
                    : content;
                }
                : banner_content;
                footer {
                    p {
                        : "hosted by ";
                        : fenhl.to_html();
                        : " • ";
                        a(href = "https://fenhl.net/disc") : "disclaimer";
                        : " • ";
                        a(href = "https://github.com/midoshouse/midos.house") : "source code";
                    }
                    p : "Special thanks to Maplestar for the chest icons used in the logo!";
                }
            }
        }
    }.write_to_html()?)
}

#[rocket::get("/")]
async fn index(pool: &State<PgPool>, user: Option<User>) -> PageResult {
    page(&pool, &user, PageStyle { kind: PageKind::Index, ..PageStyle::default() }, "Mido's House", html! {
        h1 : "Events";
        ul {
            li {
                a(href = uri!(pictionary_random_settings).to_string()) : "1st Random Settings Pictionary Spoiler Log Race";
            }
        }
    }).await
}

enum Tab {
    Info,
    Teams,
    Enter,
}

fn event_header(tab: Tab) -> Box<dyn RenderBox> {
    box_html! {
        h1 {
            a(class = "nav", href? = (!matches!(tab, Tab::Info)).then(|| uri!(pictionary_random_settings).to_string())) : "1st Random Settings Pictionary Spoiler Log Race";
        }
        h2 : "Saturday, May 14, 2021 • 20:00 CEST • 18:00 UTC • 2PM EDT";
        div(class = "button-row") {
            @if let Tab::Info = tab {
                span(class = "button selected") : "Info";
            } else {
                a(class = "button", href = uri!(pictionary_random_settings).to_string()) : "Info";
            }
            @if let Tab::Teams = tab {
                span(class = "button selected") : "Teams";
            } else {
                a(class = "button", href = uri!(pictionary_random_settings_teams).to_string()) : "Teams";
            }
            //a(class = "button") : "My Status"; //TODO (if in any teams, including unconfirmed ones)
            //TODO hide “Enter” and “Find Teammates” if in a confirmed team
            @if let Tab::Enter = tab {
                span(class = "button selected") : "Enter";
            } else {
                a(class = "button", href = uri!(pictionary_random_settings_enter).to_string()) : "Enter";
            }
            //a(class = "button") : "Find Teammates"; //TODO
            //a(class = "button") : "Volunteer"; //TODO
            //a(class = "button") : "Watch"; //TODO
        }
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
enum PictionaryRandomSettingsError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("missing user data for a race organizer")]
    OrganizerUserData,
}

#[rocket::get("/event/pic/rs1")]
async fn pictionary_random_settings(pool: &State<PgPool>, user: Option<User>) -> Result<Html<String>, PictionaryRandomSettingsError> {
    let tj = User::from_id(pool, -3874943390487736167).await?.ok_or(PictionaryRandomSettingsError::OrganizerUserData)?;
    let fenhl = User::from_id(pool, 5961629664912637980).await?.ok_or(PictionaryRandomSettingsError::OrganizerUserData)?;
    Ok(page(&pool, &user, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, "1st Random Settings Pictionary Spoiler Log Race", html! {
        main {
            : event_header(Tab::Info);
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
                    : "We will be using ";
                    a(href = "https://github.com/fenhl/plando-random-settings/blob/dev-fenhl/weights/pictionary_override.json") : "a special weights override";
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
                        a(href = "https://github.com/fenhl/OoT-Randomizer") : "Fenhl's branch";
                        : ", so some settings that aren't in Dev-R are added:";
                        ul {
                            li : "Heart container requirements for rainbow bridge and/or Ganon boss key (50% chance each to replace a skulltula token requirement)";
                            li : "Full one-way entrance randomization (owls, warp songs, and spawns can lead to more destinations; 25% chance each)";
                            li : "One bonk KO (5% chance)";
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
                //TODO sample seeds?
                h2 : "Further information";
                p {
                    : "The race is organized by ";
                    : tj.to_html();
                    : ", ksinjah, ";
                    : fenhl.to_html();
                    : ", melqwii, and TeaGrenadier. We will answer questions and inform about recent events on The Silver Gauntlets Discord in the #pictionary-spoiler-log channel (";
                    a(href = "https://discord.gg/m8z8ZqtN8H") : "invite link";
                    : " • ";
                    a(href = "https://discord.com/channels/663207960432082944/865206020015128586") : "direct channel link";
                    : "). If you have any questions, feel free to ask there!";
                }
                p : "Special thanks to winniedemon who will be helping us keep important posts from getting lost in the Discord!";
            }
        }
    }).await?)
}

#[derive(Debug, sqlx::Type)]
#[sqlx(type_name = "signup_player")]
struct SignupPlayer {
    id: i64,
    confirmed: bool,
}

/// This type is a workaround for https://github.com/launchbadge/sqlx/issues/298.
#[derive(Debug, sqlx::Encode, sqlx::Decode)]
struct SignupPlayers(Vec<SignupPlayer>);

impl sqlx::Type<sqlx::Postgres> for SignupPlayers {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("_signup_player")
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
enum PictionaryRandomSettingsTeamsError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("team with nonexistent user")]
    NonexistentUser,
}

#[rocket::get("/event/pic/rs1/teams")]
async fn pictionary_random_settings_teams(pool: &State<PgPool>, user: Option<User>) -> Result<Html<String>, PictionaryRandomSettingsTeamsError> {
    let mut signups = Vec::default();
    let mut signups_query = sqlx::query!(r#"SELECT team_name, players AS "players!: SignupPlayers" FROM signups WHERE
        series = 'pic'
        AND event = 'rs1'
        AND (
            EXISTS (SELECT 1 FROM UNNEST(players) AS player WHERE player.id = $1)
            OR NOT EXISTS (SELECT 1 FROM UNNEST(players) AS player WHERE NOT player.confirmed)
        )
    "#, user.as_ref().map(|user| user.id)).fetch(&**pool); //TODO don't show unconfirmed teams even if the viewer is in them, add a “My Status” page instead
    while let Some(row) = signups_query.try_next().await? {
        let [runner, pilot] = <[_; 2]>::try_from(row.players.0).expect("found Pictionary spoiler log team not consisting of 2 players");
        let runner = User::from_id(&pool, runner.id).await?.ok_or(PictionaryRandomSettingsTeamsError::NonexistentUser)?;
        let pilot = User::from_id(&pool, pilot.id).await?.ok_or(PictionaryRandomSettingsTeamsError::NonexistentUser)?;
        signups.push((row.team_name, runner, pilot));
    }
    Ok(page(&pool, &user, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, "Teams — 1st Random Settings Pictionary Spoiler Log Race", html! {
        main {
            : event_header(Tab::Teams);
            table {
                thead {
                    tr {
                        th : "Team Name";
                        th(class = "sheikah") : "Runner";
                        th(class = "gerudo") : "Pilot";
                    }
                }
                tbody {
                    @if signups.is_empty() {
                        tr {
                            td(colspan = "3") {
                                i : "(no signups yet)";
                            }
                        }
                    } else {
                        @for (team_name, runner, pilot) in signups {
                            tr {
                                td : team_name.unwrap_or_default();
                                td(class = "sheikah") : runner.to_html();
                                td(class = "gerudo") : pilot.to_html();
                            }
                        }
                    }
                }
            }
        }
    }).await?)
}

fn render_form_error(tmpl: &mut TemplateBuffer<'_>, error: &form::Error<'_>) {
    tmpl << html! {
        p(class = "error") : error.to_string();
    };
}

fn field_errors(tmpl: &mut TemplateBuffer<'_>, errors: &mut Vec<&form::Error<'_>>, name: &str) {
    let field_errors;
    (field_errors, *errors) = mem::take(errors).into_iter().partition(|error| error.is_for(name));
    tmpl << html! {
        @for error in field_errors {
            |tmpl| render_form_error(tmpl, error);
        }
    };
}

async fn pictionary_random_settings_enter_form(pool: &PgPool, user: Option<User>, context: Context<'_>) -> PageResult {
    page(pool, &user, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, "Enter — 1st Random Settings Pictionary Spoiler Log Race", if user.is_some() {
        let mut errors = context.errors().collect_vec();
        let form_content = html! {
            //TODO CSRF protection (rocket_csrf crate?)
            legend {
                : "Fill out this form to enter the race as a team.";
                /*
                : " If you don't have a teammate yet, you can ";
                a(href = unimplemented!(/*TODO*/)) : "look for a teammate";
                : " instead.";
                */
            }
            fieldset {
                |tmpl| field_errors(tmpl, &mut errors, "my_role");
                label(for = "my_role") : "My Role:";
                input(id = "my_role-sheikah", class = "sheikah", type = "radio", name = "my_role", value = "sheikah", checked? = context.field_value("my_role") == Some("sheikah"));
                label(class = "sheikah", for = "my_role-sheikah") : "Runner";
                input(id = "my_role-gerudo", class = "gerudo", type = "radio", name = "my_role", value = "gerudo", checked? = context.field_value("my_role") == Some("gerudo"));
                label(class = "gerudo", for = "my_role-gerudo") : "Pilot";
            }
            fieldset {
                |tmpl| field_errors(tmpl, &mut errors, "teammate");
                label(for = "teammate") : "Teammate:";
                input(type = "text", name = "teammate", value? = context.field_value("teammate"));
                label(class = "help") : "(Enter your teammate's Mido's House user ID.)"; //TODO instructions on where to find the ID, add JS-based user search?
            }
            fieldset {
                input(type = "submit", value = "Submit");
            }
        }.write_to_html()?;
        html! {
            main {
                : event_header(Tab::Enter);
                form(action = uri!(pictionary_random_settings_enter_post).to_string(), method = "post") {
                    @for error in errors {
                        |tmpl| render_form_error(tmpl, error);
                    }
                    : form_content;
                }
            }
        }.write_to_html()?
    } else {
        html! {
            main {
                : event_header(Tab::Enter);
                article {
                    p {
                        a(href = uri!(auth::login).to_string()) : "Sign in or create a Mido's House account";
                        : " to enter this race.";
                    }
                }
            }
        }.write_to_html()?
    }).await
}

#[rocket::get("/event/pic/rs1/enter")]
async fn pictionary_random_settings_enter(pool: &State<PgPool>, user: Option<User>) -> PageResult {
    pictionary_random_settings_enter_form(&pool, user, Context::default()).await
}

#[derive(FromFormField)]
enum Role {
    Sheikah,
    Gerudo,
}

#[derive(FromForm)]
struct EnterForm {
    my_role: Role,
    teammate: i64,
}

#[derive(Responder)]
pub(crate) enum PictionaryRandomSettingsEnterPostResponse {
    Redirect(Redirect),
    Content(Html<String>),
}

#[rocket::post("/event/pic/rs1/enter", data = "<form>")]
async fn pictionary_random_settings_enter_post(pool: &State<PgPool>, user: User, form: Form<Contextual<'_, EnterForm>>) -> Result<PictionaryRandomSettingsEnterPostResponse, PageError> {
    let mut form = form.into_inner();
    if let Some(ref value) = form.value {
        let mut transaction = pool.begin().await?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM signups WHERE
            series = 'pic'
            AND event = 'rs1'
            AND EXISTS (SELECT 1 FROM UNNEST(players) AS player WHERE player.id = $1)
            AND NOT EXISTS (SELECT 1 FROM UNNEST(players) AS player WHERE NOT player.confirmed)
        ) as "exists!""#, user.id).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("You are already signed up for this race."));
        }
        if value.teammate == user.id {
            form.context.push_error(form::Error::validation("You cannot be your own teammate.").with_name("teammate"));
        } else {
            if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) as "exists!""#, value.teammate).fetch_one(&mut transaction).await? {
                form.context.push_error(form::Error::validation("There is no user with this ID.").with_name("teammate"));
            } else {
                if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM signups WHERE
                    series = 'pic'
                    AND event = 'rs1'
                    AND EXISTS (SELECT 1 FROM UNNEST(players) AS player WHERE player.id = $1)
                    AND NOT EXISTS (SELECT 1 FROM UNNEST(players) AS player WHERE NOT player.confirmed)
                ) as "exists!""#, value.teammate).fetch_one(&mut transaction).await? {
                    form.context.push_error(form::Error::validation("This user is already signed up for this race."));
                }
                //TODO check to make sure the teammate hasn't blocked the user submitting the form (or vice versa)
            }
        }
        if form.context.errors().next().is_some() {
            pictionary_random_settings_enter_form(&pool, Some(user), form.context).await
                .map(PictionaryRandomSettingsEnterPostResponse::Content)
        } else {
            let me = SignupPlayer {
                id: user.id,
                confirmed: true,
            };
            let teammate = SignupPlayer {
                id: value.teammate,
                confirmed: false,
            };
            let signup_players = match value.my_role {
                Role::Sheikah => vec![me, teammate],
                Role::Gerudo => vec![teammate, me],
            };
            let id = loop {
                let id = thread_rng().gen::<i64>();
                if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM signups WHERE id = $1) AS "exists!""#, id).fetch_one(&mut transaction).await? {
                    break id
                }
            };
            sqlx::query!("INSERT INTO signups (id, series, event, players) VALUES ($1, 'pic', 'rs1', $2)", id, SignupPlayers(signup_players) as _).execute(&mut transaction).await?;
            transaction.commit().await?;
            Ok(PictionaryRandomSettingsEnterPostResponse::Redirect(Redirect::to(uri!(pictionary_random_settings_teams)))) //TODO redirect to “My Status” page instead
        }
    } else {
        pictionary_random_settings_enter_form(&pool, Some(user), form.context).await
            .map(PictionaryRandomSettingsEnterPostResponse::Content)
    }
}

#[rocket::catch(404)]
async fn not_found(request: &Request<'_>) -> PageResult {
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let user = request.guard::<User>().await.succeeded();
    page(&pool, &user, PageStyle { is_banner: true, ..PageStyle::default() }, "Not Found — Mido's House", html! {
        div(style = "flex-grow: 0;") {
            h1 : "Error 404: Not Found";
        }
        img(style = "flex-grow: 1;", class = "banner nearest-neighbor", src = "https://cdn.discordapp.com/attachments/512048482677424138/905673263005433866/unknown.png");
    }).await
}

#[rocket::catch(500)]
async fn internal_server_error(request: &Request<'_>) -> PageResult {
    //TODO report
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let user = request.guard::<User>().await.succeeded();
    page(&pool, &user, PageStyle::default(), "Internal Server Error — Mido's House", html! {
        h1 : "Error 500: Internal Server Error";
        p : "Sorry, something went wrong. Please notify Fenhl on Discord.";
    }).await
}

#[derive(clap::Parser)]
struct Args {
    #[clap(long = "dev")]
    is_dev: bool,
}

#[wheel::main(rocket)]
async fn main(Args { is_dev }: Args) -> Result<()> {
    let config = Config::load().await?;
    rocket::custom(rocket::Config {
        port: if is_dev { 24814 } else { 24812 },
        secret_key: SecretKey::from(&base64::decode(config.secret_key)?),
        ..rocket::Config::default()
    })
    .mount("/", rocket::routes![
        index,
        pictionary_random_settings,
        pictionary_random_settings_teams,
        pictionary_random_settings_enter,
        pictionary_random_settings_enter_post,
        favicon::favicon_ico,
        favicon::favicon_png,
        auth::login,
        auth::racetime_login,
        auth::racetime_callback,
        auth::register_racetime,
        auth::logout,
    ])
    .mount("/static", FileServer::new("assets/static", rocket::fs::Options::None))
    .register("/", rocket::catchers![
        not_found,
        internal_server_error,
    ])
    .attach(OAuth2::<auth::RaceTime>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider {
            auth_uri: "https://racetime.gg/o/authorize".into(),
            token_uri: "https://racetime.gg/o/token".into(),
        },
        config.racetime.client_id,
        config.racetime.client_secret,
        Some(if is_dev {
            uri!("https://dev.midos.house", auth::racetime_callback)
        } else {
            uri!("https://midos.house", auth::racetime_callback)
        }.to_string()),
    )))
    .manage(PgPool::connect_with(PgConnectOptions::default().username("mido").database("midos_house").application_name("midos-house")).await?)
    .manage(reqwest::Client::builder()
        .user_agent(concat!("MidosHouse/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .use_rustls_tls()
        .trust_dns(true)
        .https_only(true)
        .build()?
    )
    .launch().await?;
    Ok(())
}
