#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::time::Duration,
    anyhow::Result,
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
        fs::FileServer,
        response::content::Html,
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
        auth::User,
        config::Config,
        favicon::{
            ChestAppearances,
            ChestTextures,
        },
        util::Id,
    },
};

mod auth;
mod config;
mod event;
mod favicon;
mod util;

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
                a(href = uri!(event::pictionary_random_settings).to_string()) : "1st Random Settings Pictionary Spoiler Log Race";
            }
        }
    }).await
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
        auth::racetime_callback,
        auth::login,
        auth::logout,
        auth::racetime_login,
        auth::register_racetime,
        event::pictionary_random_settings,
        event::pictionary_random_settings_teams,
        event::pictionary_random_settings_enter,
        event::pictionary_random_settings_enter_post,
        favicon::favicon_ico,
        favicon::favicon_png,
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
