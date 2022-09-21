#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::time::Duration,
    futures::future::FutureExt as _,
    rocket::Rocket,
    sqlx::{
        PgPool,
        postgres::PgConnectOptions,
    },
    crate::config::Config,
};

mod auth;
mod cal;
mod config;
mod discord_bot;
mod event;
mod favicon;
mod http;
mod notification;
mod racetime_bot;
mod seed;
mod startgg;
mod user;
mod util;

include!(concat!(env!("OUT_DIR"), "/version.rs"));

#[derive(Default, Clone, Copy, clap::ValueEnum)]
enum Environment {
    #[cfg_attr(not(debug_assertions), default)]
    Production,
    #[cfg_attr(debug_assertions, default)]
    Dev,
    Local,
}

impl Environment {
    fn is_dev(&self) -> bool {
        match self {
            Self::Production => false,
            Self::Dev => true,
            Self::Local => true,
        }
    }

    fn racetime_host(&self) -> &'static str {
        if self.is_dev() { "racetime.midos.house" } else { "racetime.gg" }
    }
}

#[derive(clap::Parser)]
struct Args {
    #[clap(long, value_enum, default_value_t)]
    env: Environment,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)] Any(#[from] anyhow::Error),
    #[error(transparent)] Base64(#[from] base64::DecodeError),
    #[error(transparent)] Racetime(#[from] racetime::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Rocket(#[from] rocket::Error),
    #[error(transparent)] Serenity(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Task(#[from] tokio::task::JoinError),
}

#[wheel::main(rocket, debug)]
async fn main(Args { env }: Args) -> Result<(), Error> {
    let config = Config::load().await?;
    let http_client = reqwest::Client::builder()
        .user_agent(concat!("MidosHouse/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .use_rustls_tls()
        .trust_dns(true)
        .https_only(true)
        .build()?;
    let discord_config = if env.is_dev() { &config.discord_dev } else { &config.discord_production };
    let discord_builder = serenity_utils::builder(discord_config.client_id, discord_config.bot_token.clone()).await?;
    let db_pool = PgPool::connect_with(PgConnectOptions::default().username("mido").database(if env.is_dev() { "fados_house" } else { "midos_house" }).application_name("midos-house")).await?;
    let rocket = http::rocket(db_pool.clone(), discord_builder.ctx_fut.clone(), http_client.clone(), config.clone(), env).await?;
    let discord_builder = discord_bot::configure_builder(discord_builder, db_pool.clone(), http_client.clone(), config.clone(), env, rocket.shutdown());
    let racetime_task = tokio::spawn(racetime_bot::main(
        db_pool,
        http_client,
        discord_builder.ctx_fut.clone(),
        config.ootr_api_key.clone(),
        env,
        config.clone(),
        rocket.shutdown(),
    )).map(|res| match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let rocket_task = tokio::spawn(rocket.launch()).map(|res| match res {
        Ok(Ok(Rocket { .. })) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let discord_task = tokio::spawn(discord_builder.run()).map(|res| match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let ((), (), ()) = tokio::try_join!(discord_task, racetime_task, rocket_task)?;
    Ok(())
}
