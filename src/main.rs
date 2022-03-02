#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::time::Duration,
    anyhow::Result,
    horrorshow::{
        RenderOnce,
        helper::doctype,
        html,
        rocket::{ //TODO use rocket_util wrappers instead?
            Result as HtmlResult,
            TemplateExt as _,
        },
    },
    rocket::{
        Request,
        config::SecretKey,
        fs::FileServer,
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
    },
};

mod auth;
mod config;
mod favicon;

fn page(user: &Option<User>, title: &str, content: impl RenderOnce) -> HtmlResult {
    let appearances = ChestAppearances::random(); //TODO change based on page (tournament settings, seed spoiler log)
    html! {
        : doctype::HTML;
        html {
            head {
                meta(charset = "utf-8");
                meta(name = "viewport", content = "width=device-width, initial-scale=1, shrink-to-fit=no");
                title : title;
                link(rel = "icon", sizes = "512x512", type = "image/png", href = uri!(favicon::favicon_png(appearances.textures(), Suffix(512, "png"))).to_string());
                link(rel = "icon", sizes = "1024x1024", type = "image/png", href = uri!(favicon::favicon_png(appearances.textures(), Suffix(1024, "png"))).to_string());
                link(rel = "stylesheet", href = "/static/common.css");
            }
            body {
                nav {
                    a(href = uri!(index).to_string()) {
                        //TODO get 128px images, use those (with 256 as a 2x srcset)
                        div(class = "logo") {
                            @for chest in appearances.0 {
                                img(class = "chest", src = format!("/static/chest/{}256.png", char::from(chest.texture)));
                            }
                        }
                        h1 : "Mido's House";
                    }
                    div(id = "login") { //TODO hide if already on /login?
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

#[rocket::catch(404)]
async fn not_found(request: &Request<'_>) -> HtmlResult {
    let user = request.guard::<User>().await.succeeded();
    page(&user, "Not Found — Mido's House", html! {
        h1 : "Error 404: Not Found";
        div(class = "banner") {
            img(src = "https://cdn.discordapp.com/attachments/512048482677424138/905673263005433866/unknown.png");
        }
    })
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
        favicon::favicon_ico,
        favicon::favicon_png,
        auth::login,
        auth::racetime_login,
        auth::racetime_callback,
        auth::register_racetime,
        auth::logout,
    ])
    .mount("/static", FileServer::new("assets/static", rocket::fs::Options::None))
    .register("/", rocket::catchers![not_found])
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
    .launch().await?;
    Ok(())
}
