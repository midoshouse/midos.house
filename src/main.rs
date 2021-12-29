#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::{
        io,
        time::Duration,
    },
    anyhow::Result,
    horrorshow::{
        RenderOnce,
        helper::doctype,
        html,
        rocket::{
            Result as HtmlResult,
            TemplateExt as _,
        },
    },
    rocket::{
        config::SecretKey,
        fs::{
            FileServer,
            NamedFile,
        },
        uri,
    },
    rocket_oauth2::{
        OAuth2,
        OAuthConfig,
    },
    sqlx::{
        PgPool,
        postgres::PgConnectOptions,
    },
    structopt::StructOpt,
    crate::{
        auth::User,
        config::Config,
    },
};

mod auth;
mod config;

fn page(user: &Option<User>, title: &str, content: impl RenderOnce) -> HtmlResult {
    html! {
        : doctype::HTML;
        html {
            head {
                meta(charset = "utf-8");
                meta(name = "viewport", content = "width=device-width, initial-scale=1, shrink-to-fit=no");
                title : title;
                //TODO description, favicon
                link(rel = "stylesheet", href = "/static/common.css");
            }
            body {
                nav {
                    div(id = "login") { //TODO hide if already on /login?
                        @if let Some(user) = user {
                            p : format!("signed in as {}", user.display_name);
                            //TODO display profile/preferences/logout menu
                        } else {
                            a(href = uri!(auth::login).to_string()) : "Sign in / Create account";
                        }
                    }
                    a(href = uri!(index).to_string()) { //TODO don't link to index if already on index
                        //TODO randomize chest textures/sizes depending on page
                        //TODO get 128px images, use those (with 256 as a 2x srcset)
                        img(class = "small-chest", src = "/static/chest/s256.png");
                        img(class = "small-chest", src = "/static/chest/s256.png");
                        img(class = "small-chest", src = "/static/chest/s256.png");
                        img(class = "small-chest", src = "/static/chest/s256.png");
                        h1 : "Mido's House";
                    }
                }
                : content;
                footer {
                    a(href = uri!("https://fenhl.net/disc").to_string()) : "disclaimer / Impressum";
                }
            }
        }
    }.write_to_html()
}

#[rocket::get("/")]
fn index(user: Option<User>) -> HtmlResult {
    page(&user, "Mido's House", html! {
        p : "Coming soon™"; //TODO
    })
}

#[rocket::get("/favicon.ico")]
async fn favicon() -> io::Result<NamedFile> {
    //TODO random chest configurations based on current RSL weights except CSMC is replaced with CTMC?
    NamedFile::open("assets/favicon.ico").await
}

#[derive(StructOpt)]
struct Args {
    #[structopt(long = "dev")]
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
        favicon,
        auth::login,
        auth::racetime_login,
        auth::racetime_callback,
        auth::register_racetime,
        auth::logout,
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
    .manage(PgPool::connect_with(PgConnectOptions::default().database("midos_house").application_name("midos-house")).await?)
    .manage(reqwest::Client::builder()
        .user_agent(concat!("MidosHouse/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .use_rustls_tls()
        .trust_dns(true)
        .https_only(true)
        .build()?
    )
    .mount("/static", FileServer::new("assets/static", rocket::fs::Options::None))
    .launch().await?;
    Ok(())
}
