use {
    std::io,
    base64::engine::{
        Engine as _,
        general_purpose::STANDARD as BASE64,
    },
    itertools::Itertools as _,
    ootr_utils::spoiler::SpoilerLog,
    rand::prelude::*,
    rocket::{
        Request,
        Rocket,
        State,
        config::SecretKey,
        fairing::Fairing,
        fs::FileServer,
        http::{
            Header,
            Status,
            StatusClass,
            hyper::header::{
                CONTENT_DISPOSITION,
                LINK,
            },
        },
        response::{
            Response,
            content::{
                RawHtml,
                RawText,
            },
        },
        uri,
    },
    rocket_oauth2::{
        OAuth2,
        OAuthConfig,
    },
    rocket_util::{
        Doctype,
        Origin,
        Suffix,
        ToHtml,
        html,
    },
    serenity::client::Context as DiscordCtx,
    serenity_utils::RwFuture,
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
    tokio::process::Command,
    crate::{
        Environment,
        config::Config,
        api,
        auth,
        cal,
        event::{
            self,
            Series,
        },
        favicon::{
            self,
            ChestAppearances,
            ChestTextures,
        },
        notification::{
            self,
            Notification,
        },
        seed,
        user::{
            self,
            User,
        },
        util::{
            DateTimeFormat,
            Id,
            format_date_range,
            format_datetime,
        },
    },
};

include!(concat!(env!("OUT_DIR"), "/static_files.rs"));

pub(crate) use static_url;

pub(crate) enum PageKind {
    Index,
    Banner,
    Center,
    Login,
    MyProfile,
    Notifications,
    Other,
}

pub(crate) struct PageStyle {
    pub(crate) kind: PageKind,
    pub(crate) chests: ChestAppearances,
}

impl Default for PageStyle {
    fn default() -> Self {
        Self {
            kind: PageKind::Other,
            chests: ChestAppearances::random(),
        }
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum PageError {
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("missing user data for Fenhl")]
    FenhlUserData,
    #[error("missing user data for Xopar")]
    XoparUserData,
}

pub(crate) type PageResult = Result<RawHtml<String>, PageError>;

pub(crate) async fn page(mut transaction: Transaction<'_, Postgres>, me: &Option<User>, uri: &Origin<'_>, style: PageStyle, title: &str, content: impl ToHtml) -> PageResult {
    let notifications = if let Some(me) = me {
        if let PageKind::Notifications = style.kind {
            Vec::default()
        } else {
            Notification::get(&mut transaction, me).await?
        }
    } else {
        Vec::default()
    };
    let (banner_content, content) = if let PageKind::Banner = style.kind {
        (Some(content), None)
    } else {
        (None, Some(content))
    };
    let fenhl = User::from_id(&mut transaction, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
    let xopar = User::from_id(&mut transaction, Id(17762941071474623984)).await?.ok_or(PageError::XoparUserData)?;
    transaction.commit().await?;
    Ok(html! {
        : Doctype;
        html {
            head {
                meta(charset = "utf-8");
                title : title;
                meta(name = "viewport", content = "width=device-width, initial-scale=1, shrink-to-fit=no");
                link(rel = "icon", sizes = "1024x1024", type = "image/png", href = uri!(favicon::favicon_png(style.chests.textures(), Suffix(1024, "png"))).to_string());
                link(rel = "stylesheet", href = static_url!("common.css"));
                script(defer, src = static_url!("common.js"));
            }
            body(class = matches!(style.kind, PageKind::Banner).then(|| "fullscreen")) {
                div {
                    nav(class? = matches!(style.kind, PageKind::Index).then(|| "index")) {
                        a(class = "nav", href? = (!matches!(style.kind, PageKind::Index)).then(|| uri!(index).to_string())) {
                            div(class = "logo") {
                                @for chest in style.chests.0 {
                                    : chest.texture;
                                }
                            }
                            h1 : "Mido's House";
                        }
                        div(id = "login") {
                            @if !matches!(style.kind, PageKind::Login) {
                                @if let Some(me) = me {
                                    : "signed in as ";
                                    @if let PageKind::MyProfile = style.kind {
                                        : me.display_name();
                                    } else {
                                        : me;
                                    }
                                    br;
                                    //TODO link to preferences
                                    a(href = uri!(auth::logout(Some(uri))).to_string()) : "Sign out";
                                } else {
                                    a(href = uri!(auth::login(Some(uri))).to_string()) : "Sign in / Create account";
                                }
                                @if !notifications.is_empty() {
                                    br;
                                }
                            }
                            @if !notifications.is_empty() {
                                a(href = uri!(notification::notifications).to_string()) {
                                    : notifications.len().to_string();
                                    @if notifications.len() == 1 {
                                        : " notification";
                                    } else {
                                        : " notifications";
                                    }
                                }
                            }
                        }
                    }
                    @if let Some(content) = content {
                        main(class? = matches!(style.kind, PageKind::Center).then(|| "center")) {
                            : content;
                        }
                    }
                }
                : banner_content;
                footer {
                    p {
                        : "hosted by ";
                        : fenhl;
                        : " • ";
                        a(href = "https://fenhl.net/disc") : "disclaimer";
                        : " • ";
                        a(href = "https://github.com/midoshouse/midos.house") : "source code";
                    }
                    p {
                        : "Special thanks to Maplestar for some of the chest icons used in the logo, and to ";
                        : xopar;
                        : " and shiroaeli for some of the seed hash icons!";
                    }
                }
            }
        }
    })
}

#[rocket::get("/")]
async fn index(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> Result<RawHtml<String>, event::Error> {
    let mut transaction = pool.begin().await?;
    let mut upcoming_events = Vec::default();
    for row in sqlx::query!(r#"SELECT series AS "series!: Series", event FROM events WHERE listed AND (end_time IS NULL OR end_time > NOW()) ORDER BY start ASC NULLS LAST"#).fetch_all(&mut transaction).await.map_err(event::Error::from)? {
        upcoming_events.push(event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during transaction"));
    }
    let chests = upcoming_events.choose(&mut thread_rng()).map_or_else(|| ChestAppearances::random(), |event| event.chests());
    let mut ongoing_events = Vec::default();
    for event in upcoming_events.drain(..).collect_vec() {
        if event.is_started(&mut transaction).await? { &mut ongoing_events } else { &mut upcoming_events }.push(event);
    }
    let page_content = html! {
        p {
            : "Mido's House is a platform where ";
            a(href = "https://ootrandomizer.com/") : "Ocarina of Time randomizer";
            : " events like tournaments or community races can be organized.";
        }
        h1 : "Ongoing events";
        ul {
            @if ongoing_events.is_empty() {
                i : "(none currently)";
            } else {
                @for event in ongoing_events {
                    li : event;
                }
            }
        }
        h1 : "Upcoming events";
        ul {
            @if upcoming_events.is_empty() {
                i : "(none currently)";
            } else {
                @for event in upcoming_events {
                    li {
                        : event;
                        @if let Some(start) = event.start(&mut transaction).await? {
                            : " — ";
                            : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                        }
                    }
                }
            }
        }
        p {
            a(href = uri!(archive).to_string()) : "Past events";
            : " • ";
            a(href = uri!(new_event).to_string()) : "Planning an event?";
        }
        h1 : "Calendar";
        p {
            : "A calendar of all races across all events can be found at ";
            code : uri!("https://midos.house", cal::index).to_string();
            : " — by pasting this link into most calendar apps' “subscribe” feature instead of downloading it, you can get automatic updates as races are scheduled:";
        }
        ul {
            li {
                : "In Google Calendar, select ";
                a(href = "https://calendar.google.com/calendar/u/0/r/settings/addbyurl") : "Add calendar → From URL";
            }
            li {
                : "In Apple Calendar, press ";
                kbd : "⌥";
                kbd : "⌘";
                kbd : "S";
                : " or select File → New Calendar Subscription";
            }
            li : "In Mozilla Thunderbird, select New Calendar → On the Network. Paste the link into the “Location” field and click “Find Calendars”, then “Properties”. Enable “Read Only” and click “OK”, then “Subscribe”.";
        }
        //p : "You can also find calendar links for individual events on their pages."; //TODO figure out where to put these calendar links (below the date for single races, “Schedule” tab for tournaments?)
    };
    Ok(page(transaction, &me, &uri, PageStyle { kind: PageKind::Index, chests, ..PageStyle::default() }, "Mido's House", page_content).await?)
}

#[rocket::get("/archive")]
async fn archive(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> Result<RawHtml<String>, event::Error> {
    let mut transaction = pool.begin().await?;
    let mut past_events = Vec::default();
    for row in sqlx::query!(r#"SELECT series AS "series!: Series", event FROM events WHERE listed AND end_time IS NOT NULL AND end_time <= NOW() ORDER BY end_time DESC"#).fetch_all(&mut transaction).await.map_err(event::Error::from)? {
        past_events.push(event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during transaction"));
    }
    let chests = past_events.choose(&mut thread_rng()).map_or_else(|| ChestAppearances::random(), |event| event.chests());
    let page_content = html! {
        h1 : "Past events";
        ul {
            @if past_events.is_empty() {
                i : "(none currently)";
            } else {
                @for event in past_events {
                    li {
                        : event;
                        : " — ";
                        : format_date_range(event.start(&mut transaction).await?.expect("ended event with no start date"), event.end.expect("checked above"));
                    };
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests, ..PageStyle::default() }, "Event Archive — Mido's House", page_content).await?)
}

#[rocket::get("/new")]
async fn new_event(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> PageResult {
    let mut transaction = pool.begin().await?;
    let fenhl = User::from_id(&mut transaction, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
    page(transaction, &me, &uri, PageStyle::default(), "New Event — Mido's House", html! {
        p {
            : "If you are planning a tournament, community race, or other event for the Ocarina of Time randomizer community, or if you would like Mido's House to archive data about a past event you organized, please contact ";
            : fenhl;
            : " to determine the specific needs of the event.";
        }
    }).await
}

#[rocket::get("/mw")]
async fn mw(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> PageResult {
    let transaction = pool.begin().await?;
    page(transaction, &me, &uri, PageStyle { kind: PageKind::Center, ..PageStyle::default() }, "Mido's House Multiworld", html! {
        h1 : "Mido's House Multiworld";
        img(class = "banner icon", src = static_url!("mw.png"));
        p {
            : "Mido's House Multiworld is a tool that can be used to play ";
            a(href = "https://wiki.ootrandomizer.com/index.php?title=Multiworld") : "multiworld";
            : " seeds of the ";
            a(href = "https://ootrandomizer.com/") : "Ocarina of Time randomizer";
            : ". It supports both ";
            a(href = "https://tasvideos.org/BizHawk") : "BizHawk";
            : " and ";
            a(href = "https://pj64-emu.com/") : "Project64";
            : ", and does not require port forwarding.";
        }
        div(class = "button-row") {
            a(class = "button", href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer.exe") : "Download";
        }
        p {
            : "If you can't or don't want to use the installer, you can follow the ";
            a(href = "https://github.com/midoshouse/ootr-multiworld#manual") : "manual install instructions";
            : " instead.";
        }
        p {
            a(href = "https://github.com/midoshouse/ootr-multiworld") : "The source code for Mido's House Multiworld";
            : " is available on GitHub.";
        }
    }).await
}

#[rocket::get("/robots.txt")]
async fn robots_txt() -> RawText<&'static str> {
    RawText("User-agent: *\nDisallow: /seed/\nDisallow: /static/\n")
}

#[rocket::catch(400)]
async fn bad_request(request: &Request<'_>) -> PageResult {
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    let uri = request.guard::<Origin<'_>>().await.succeeded().unwrap_or_else(|| Origin(uri!(index)));
    page(pool.begin().await?, &me, &uri, PageStyle { chests: ChestAppearances::SMALL_KEYS, ..PageStyle::default() }, "Bad Request — Mido's House", html! {
        h1 : "Error 400: Bad Request";
        p : "Login failed. If you need help, contact Fenhl on Discord.";
    }).await
}

#[rocket::catch(404)]
async fn not_found(request: &Request<'_>) -> PageResult {
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    let uri = request.guard::<Origin<'_>>().await.succeeded().unwrap_or_else(|| Origin(uri!(index)));
    page(pool.begin().await?, &me, &uri, PageStyle { kind: PageKind::Banner, chests: ChestAppearances::INVISIBLE, ..PageStyle::default() }, "Not Found — Mido's House", html! {
        div(style = "flex-grow: 0;") {
            h1 : "Error 404: Not Found";
        }
        img(style = "flex-grow: 1;", class = "banner nearest-neighbor", src = "https://cdn.discordapp.com/attachments/512048482677424138/905673263005433866/unknown.png");
    }).await
}

#[rocket::catch(500)]
async fn internal_server_error(request: &Request<'_>) -> PageResult {
    let _ = Command::new("sudo").arg("-u").arg("fenhl").arg("/opt/night/bin/nightd").arg("report").arg("/net/midoshouse/error").spawn(); //TODO include error details in report
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    let uri = request.guard::<Origin<'_>>().await.succeeded().unwrap_or_else(|| Origin(uri!(index)));
    page(pool.begin().await?, &me, &uri, PageStyle { chests: ChestAppearances::TOKENS, ..PageStyle::default() }, "Internal Server Error — Mido's House", html! {
        h1 : "Error 500: Internal Server Error";
        p : "Sorry, something went wrong. Please notify Fenhl on Discord.";
    }).await
}

struct SeedDownloadFairing;

#[rocket::async_trait]
impl Fairing for SeedDownloadFairing {
    fn info(&self) -> rocket::fairing::Info {
        rocket::fairing::Info {
            name: "SeedDownloadFairing",
            kind: rocket::fairing::Kind::Singleton | rocket::fairing::Kind::Response,
        }
    }

    async fn on_response<'r>(&self, req: &'r Request<'_>, res: &mut Response<'r>) {
        if res.status().class() == StatusClass::Success {
            let path = req.uri().path();
            if path.ends_with(".zpf") || path.ends_with(".zpfz") {
                res.set_header(Header::new(CONTENT_DISPOSITION.as_str(), "attachment"));
            } else if path.ends_with(".json") {
                res.set_header(Header::new(CONTENT_DISPOSITION.as_str(), "inline"));
                if let Ok(body) = res.body_mut().to_string().await {
                    match serde_json::from_str::<SpoilerLog>(&body) {
                        Ok(log) => {
                            let textures = ChestAppearances::from(log).textures();
                            res.adjoin_header(Header::new(LINK.as_str(), format!(r#"<{}>; rel="icon"; sizes="1024x1024""#, uri!(favicon::favicon_png(textures, Suffix(1024, "png"))))));
                        }
                        Err(e) => {
                            eprintln!("failed to add favicon to {path}: {e} ({e:?})");
                        }
                    }
                    res.set_sized_body(body.len(), io::Cursor::new(body))
                } else {
                    res.set_status(Status::InternalServerError);
                }
            }
        }
    }
}

pub(crate) async fn rocket(pool: PgPool, discord_ctx: RwFuture<DiscordCtx>, http_client: reqwest::Client, config: Config, env: Environment, port: u16) -> Result<Rocket<rocket::Ignite>, crate::Error> {
    let discord_config = if env.is_dev() { &config.discord_dev } else { &config.discord_production };
    let racetime_config = if env.is_dev() { &config.racetime_oauth_dev } else { &config.racetime_oauth_production };
    Ok(rocket::custom(rocket::Config {
        secret_key: SecretKey::from(&BASE64.decode(&config.secret_key)?),
        log_level: rocket::config::LogLevel::Critical,
        port,
        ..rocket::Config::default()
    })
    .mount("/", rocket::routes![
        index,
        archive,
        new_event,
        mw,
        robots_txt,
        api::graphql_request,
        api::graphql_query,
        api::graphql_playground,
        auth::racetime_callback,
        auth::discord_callback,
        auth::login,
        auth::logout,
        auth::racetime_login,
        auth::discord_login,
        auth::register_racetime,
        auth::register_discord,
        auth::merge_accounts,
        cal::index,
        cal::for_series,
        cal::for_event,
        cal::edit_race,
        cal::edit_race_post,
        event::info,
        event::teams,
        event::races,
        event::status,
        event::enter,
        event::enter_post,
        event::find_team,
        event::find_team_post,
        event::confirm_signup,
        event::resign,
        event::resign_post,
        event::request_async,
        event::submit_async,
        favicon::favicon_ico,
        favicon::favicon_png,
        notification::notifications,
        notification::dismiss,
        user::profile,
    ])
    .mount("/seed", FileServer::new(seed::DIR, rocket::fs::Options::None))
    .mount("/static", FileServer::new("assets/static", rocket::fs::Options::None))
    .register("/", rocket::catchers![
        bad_request,
        not_found,
        internal_server_error,
    ])
    .attach(rocket_csrf::Fairing::default())
    .attach(OAuth2::<auth::RaceTime>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider {
            auth_uri: format!("https://{}/o/authorize", env.racetime_host()).into(),
            token_uri: format!("https://{}/o/token", env.racetime_host()).into(),
        },
        racetime_config.client_id.clone(),
        racetime_config.client_secret.clone(),
        Some(match env {
            Environment::Local => uri!("http://localhost:24814", auth::racetime_callback),
            Environment::Dev => uri!("https://dev.midos.house", auth::racetime_callback),
            Environment::Production => uri!("https://midos.house", auth::racetime_callback),
        }.to_string()),
    )))
    .attach(OAuth2::<auth::Discord>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider { //TODO use built-in constant once https://github.com/jebrosen/rocket_oauth2/pull/42 is merged and released
            auth_uri: "https://discord.com/api/oauth2/authorize".into(),
            token_uri: "https://discord.com/api/oauth2/token".into(),
        },
        discord_config.client_id.to_string(),
        discord_config.client_secret.to_string(),
        Some(match env {
            Environment::Local => uri!("http://localhost:24814", auth::discord_callback),
            Environment::Dev => uri!("https://dev.midos.house", auth::discord_callback),
            Environment::Production => uri!("https://midos.house", auth::discord_callback),
        }.to_string()),
    )))
    .attach(SeedDownloadFairing)
    .manage(config)
    .manage(env)
    .manage(pool)
    .manage(discord_ctx)
    .manage(http_client)
    .manage(async_graphql::Schema::build(api::Query, async_graphql::EmptyMutation, async_graphql::EmptySubscription).finish())
    .ignite().await?)
}
