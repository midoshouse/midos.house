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

enum HeaderPage {
    Index,
    Login,
    Other,
}

struct HeaderStyle {
    page: HeaderPage,
    chests: ChestAppearances,
}

impl Default for HeaderStyle {
    fn default() -> Self {
        Self {
            page: HeaderPage::Other,
            chests: ChestAppearances::random(),
        }
    }
}

fn page(user: &Option<User>, header: HeaderStyle, title: &str, content: impl RenderOnce) -> HtmlResult {
    html! {
        : doctype::HTML;
        html {
            head {
                meta(charset = "utf-8");
                meta(name = "viewport", content = "width=device-width, initial-scale=1, shrink-to-fit=no");
                title : title;
                link(rel = "icon", sizes = "512x512", type = "image/png", href = uri!(favicon::favicon_png(header.chests.textures(), Suffix(512, "png"))).to_string());
                link(rel = "icon", sizes = "1024x1024", type = "image/png", href = uri!(favicon::favicon_png(header.chests.textures(), Suffix(1024, "png"))).to_string());
                link(rel = "stylesheet", href = "/static/common.css");
            }
            body {
                div {
                    nav(class? = matches!(header.page, HeaderPage::Index).then(|| "index")) {
                        a(href? = (!matches!(header.page, HeaderPage::Index)).then(|| uri!(index).to_string())) {
                            //TODO get 128px images, use those (with 256 as a 2x srcset)
                            div(class = "logo") {
                                @for chest in header.chests.0 {
                                    img(class = "chest", src = format!("/static/chest/{}256.png", char::from(chest.texture)));
                                }
                            }
                            h1 : "Mido's House";
                        }
                        @if !matches!(header.page, HeaderPage::Login) {
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
                footer {
                    : "hosted by ";
                    a(href = "https://fenhl.net/") : "Fenhl";
                    : " • ";
                    a(href = "https://fenhl.net/disc") : "disclaimer";
                    : " • ";
                    a(href = "https://github.com/midoshouse/midos.house") : "source code";
                }
            }
        }
    }.write_to_html()
}

#[rocket::get("/")]
fn index(user: Option<User>) -> HtmlResult {
    page(&user, HeaderStyle { page: HeaderPage::Index, ..HeaderStyle::default() }, "Mido's House", html! {
        h1 : "Events";
        ul {
            li {
                a(href = uri!(pictionary_random_settings).to_string()) : "1st Random Settings Pictionary Spoiler Log Race";
            }
        }
    })
}

#[rocket::get("/event/pic/rs1")]
fn pictionary_random_settings(user: Option<User>) -> HtmlResult {
    page(&user, HeaderStyle { chests: ChestAppearances::VANILLA, ..HeaderStyle::default() }, "1st Random Settings Pictionary Spoiler Log Race", html! {
        main {
            h1 : "1st Random Settings Pictionary Spoiler Log Race";
            h2 : "Saturday, May 14, 2021 • 20:00 CEST • 18:00 UTC • 2PM EDT";
            div(class = "button-row") {
                span(class = "button selected") : "Info";
                //a(class = "button") : "Enter"; //TODO
                //TODO if participating, replace “Enter” with “My Status” and “Resign”
                //a(class = "button") : "Find Teammates"; //TODO
                //a(class = "button") : "Volunteer"; //TODO
                //a(class = "button") : "Watch"; //TODO
            }
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
                    : " The Pilot is now allowed to look at the spoiler log and can start figuring out the route.";
                }
                p {
                    strong : "At the +30 minute mark:";
                    : " The Pilot is allowed to start drawing and the runner is allowed to start the file.";
                }
                h2 : "Rules";
                p {
                    : "The race uses the ";
                    a(href = "https://rsl-leaderboard.web.app/rules") : "Random Settings League";
                    : " ruleset.";
                }
                p : "The pilot is allowed to communicate to their partner only via drawing and may watch and hear the stream of the runner. Runners may talk to their drawer. We would prefer if the drawer did not directly respond to questions, as figuring things out is supposed to be part of the challenge, but in the end it's up to the individual teams.";
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
                        : "The drawer draws 3 spiders and a bow. The runner then asks if there is a bow on 30 skulls. The pilot then draws a smiley or a checkmark for confirmation or a sad face for „no“ — that is ";
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
                        : "The runner says „if I need to do the toilet check, draw a heart“ — that is ";
                        strong : "not allowed";
                        : ".";
                    }
                    li {
                        : "The runner says: „since you didn't draw anything in the lost woods, I'm gonna skip all the checks there and go immediately to the Sacred Forest Meadow“ — that is ";
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
                    : "The race is organized by TJ, ksinjah, Fenhl, melqwii, and TeaGrenadier. We will answer questions and inform about recent events on The Silver Gauntlets Discord in the #pictionary-spoiler-log channel (";
                    a(href = "https://discord.gg/m8z8ZqtN8H") : "invite link";
                    : " • ";
                    a(href = "https://discord.com/channels/663207960432082944/865206020015128586") : "direct channel link";
                    : "). If you have any questions, feel free to ask there!";
                }
                p : "Special thanks to winniedemon who will be helping us keep important posts from getting lost in the Discord!";
            }
        }
    })
}

#[rocket::catch(404)]
async fn not_found(request: &Request<'_>) -> HtmlResult {
    let user = request.guard::<User>().await.succeeded();
    page(&user, HeaderStyle::default(), "Not Found — Mido's House", html! {
        h1 : "Error 404: Not Found";
        div(class = "banner") {
            img(class = "nearest-neighbor", src = "https://cdn.discordapp.com/attachments/512048482677424138/905673263005433866/unknown.png");
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
        pictionary_random_settings,
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
