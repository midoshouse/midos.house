#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::{
        io,
        time::Duration,
    },
    horrorshow::{
        RenderOnce,
        helper::doctype,
        html,
        rocket::TemplateExt as _, //TODO use a rocket_util wrapper instead?
    },
    rocket::{
        Request,
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
        postgres::PgConnectOptions,
    },
    crate::{
        config::Config,
        favicon::{
            ChestAppearances,
            ChestTextures,
        },
        notification::Notification,
        seed::SpoilerLog,
        user::User,
        util::Id,
    },
};

mod auth;
mod config;
mod event;
mod favicon;
mod notification;
mod seed;
mod user;
mod util;

enum PageKind {
    Index,
    Banner,
    Login,
    MyProfile,
    Notifications,
    Other,
}

struct PageStyle {
    kind: PageKind,
    chests: ChestAppearances,
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
enum PageError {
    #[error(transparent)] Horrorshow(#[from] horrorshow::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("missing user data for Fenhl")]
    FenhlUserData,
}

type PageResult = Result<Html<String>, PageError>;

async fn page(pool: &PgPool, me: &Option<User>, style: PageStyle, title: &str, content: impl RenderOnce) -> PageResult {
    let notifications = if let Some(me) = me {
        if let PageKind::Notifications = style.kind {
            Vec::default()
        } else {
            Notification::get(pool, me).await?
        }
    } else {
        Vec::default()
    };
    let (banner_content, content) = if let PageKind::Banner = style.kind {
        (Some(content), None)
    } else {
        (None, Some(content))
    };
    let fenhl = User::from_id(pool, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
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
            body(class = matches!(style.kind, PageKind::Banner).then(|| "fullscreen")) {
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
                        div(id = "login") {
                            @if !matches!(style.kind, PageKind::Login) {
                                @if let Some(me) = me {
                                    : "signed in as ";
                                    @if let PageKind::MyProfile = style.kind {
                                        : me.display_name();
                                    } else {
                                        : me.to_html();
                                    }
                                    br;
                                    //TODO link to preferences
                                    a(href = uri!(auth::logout).to_string()) : "Sign out";
                                } else {
                                    a(href = uri!(auth::login).to_string()) : "Sign in / Create account";
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
                        main {
                            : content;
                        }
                    }
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
async fn index(pool: &State<PgPool>, me: Option<User>) -> PageResult {
    page(pool, &me, PageStyle { kind: PageKind::Index, ..PageStyle::default() }, "Mido's House", html! {
        p {
            : "Mido's House is a platform where ";
            a(href = "https://ootrandomizer.com/") : "Ocarina of Time randomizer";
            : " events like tournaments or community races can be organized.";
        }
        h1 : "Upcoming events";
        ul {
            li {
                a(href = uri!(event::info("pic", "rs1")).to_string()) : "1st Random Settings Pictionary Spoiler Log Race";
            }
        }
        p {
            a(href = uri!(new_event).to_string()) : "Planning an event?";
        }
    }).await
}

#[rocket::get("/new")]
async fn new_event(pool: &State<PgPool>, me: Option<User>) -> PageResult {
    let fenhl = User::from_id(pool, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
    page(pool, &me, PageStyle::default(), "New Event — Mido's House", html! {
        p {
            : "If you are planning a tournament, community race, or other event for the Ocarina of Time randomizer community, or if you would like Mido's House to archive data about a past event you organized, please contact ";
            : fenhl.into_html();
            : " to determine the specific needs of the event.";
        }
    }).await
}

#[rocket::catch(404)]
async fn not_found(request: &Request<'_>) -> PageResult {
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    page(pool, &me, PageStyle { kind: PageKind::Banner, ..PageStyle::default() }, "Not Found — Mido's House", html! {
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
    let me = request.guard::<User>().await.succeeded();
    page(pool, &me, PageStyle::default(), "Internal Server Error — Mido's House", html! {
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
                    if let Ok(log) = serde_json::from_str::<SpoilerLog>(&body) {
                        let textures = ChestAppearances::from(log).textures();
                        res.adjoin_header(Header::new(LINK.as_str(), format!(r#"<{}>; rel="icon"; sizes="512x512""#, uri!(favicon::favicon_png(textures, Suffix(512, "png"))))));
                        res.adjoin_header(Header::new(LINK.as_str(), format!(r#"<{}>; rel="icon"; sizes="1024x1024""#, uri!(favicon::favicon_png(textures, Suffix(1024, "png"))))));
                    } else {
                        //TODO notify about JSON parse failure
                    }
                    res.set_sized_body(body.len(), io::Cursor::new(body))
                } else {
                    res.set_status(Status::InternalServerError);
                }
            }
        }
    }
}

#[derive(clap::Parser)]
struct Args {
    #[clap(long = "dev")]
    is_dev: bool,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)] Any(#[from] anyhow::Error),
    #[error(transparent)] Base64(#[from] base64::DecodeError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Rocket(#[from] rocket::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

#[wheel::main(rocket, debug)]
async fn main(Args { is_dev }: Args) -> Result<(), Error> {
    let config = Config::load().await?;
    rocket::custom(rocket::Config {
        port: if is_dev { 24814 } else { 24812 },
        secret_key: SecretKey::from(&base64::decode(config.secret_key)?),
        ..rocket::Config::default()
    })
    .mount("/", rocket::routes![
        index,
        new_event,
        auth::racetime_callback,
        auth::discord_callback,
        auth::login,
        auth::logout,
        auth::racetime_login,
        auth::discord_login,
        auth::register_racetime,
        auth::register_discord,
        event::info,
        event::teams,
        event::status,
        event::enter,
        event::enter_post,
        event::find_team,
        event::find_team_post,
        event::confirm_signup,
        event::resign,
        event::resign_post,
        favicon::favicon_ico,
        favicon::favicon_png,
        notification::notifications,
        notification::dismiss,
        user::profile,
    ])
    .mount("/seed", FileServer::new(seed::DIR, rocket::fs::Options::None))
    .mount("/static", FileServer::new("assets/static", rocket::fs::Options::None))
    .register("/", rocket::catchers![
        not_found,
        internal_server_error,
    ])
    .attach(rocket_csrf::Fairing::default())
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
    .attach(OAuth2::<auth::Discord>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider {
            auth_uri: "https://discord.com/api/oauth2/authorize".into(),
            token_uri: "https://discord.com/api/oauth2/token".into(),
        },
        if is_dev { config.discord.dev_client_id } else { config.discord.client_id },
        if is_dev { config.discord.dev_client_secret } else { config.discord.client_secret },
        Some(if is_dev {
            uri!("https://dev.midos.house", auth::discord_callback)
        } else {
            uri!("https://midos.house", auth::discord_callback)
        }.to_string()),
    )))
    .attach(SeedDownloadFairing)
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
