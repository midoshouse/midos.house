#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

#![recursion_limit = "512"]

use {
    std::{
        env,
        sync::Arc,
        time::Duration,
    },
    futures::future::FutureExt as _,
    rocket::Rocket,
    sqlx::{
        PgPool,
        postgres::PgConnectOptions,
    },
    crate::config::Config,
};
#[cfg(unix)] use {
    async_proto::Protocol as _,
    tokio::net::UnixStream,
};
#[cfg(not(unix))] use futures::future;

mod api;
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
mod team;
#[cfg(unix)] mod unix_socket;
mod user;
mod util;

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
    #[clap(subcommand)]
    subcommand: Option<Subcommand>,
}

#[derive(clap::Subcommand)]
enum Subcommand {
    #[cfg(unix)] PrepareStop,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)] Any(#[from] anyhow::Error),
    #[error(transparent)] Base64(#[from] base64::DecodeError),
    #[cfg(unix)] #[error(transparent)] Io(#[from] tokio::io::Error),
    #[error(transparent)] Racetime(#[from] racetime::Error),
    #[cfg(unix)] #[error(transparent)] Read(#[from] async_proto::ReadError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Rocket(#[from] rocket::Error),
    #[error(transparent)] Serenity(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Task(#[from] tokio::task::JoinError),
    #[cfg(unix)] #[error(transparent)] Wheel(#[from] wheel::Error),
    #[cfg(unix)] #[error(transparent)] Write(#[from] async_proto::WriteError),
}

#[wheel::main(rocket, debug)]
async fn main(Args { env, subcommand }: Args) -> Result<(), Error> {
    if let Some(subcommand) = subcommand {
        match subcommand {
            #[cfg(unix)] Subcommand::PrepareStop => {
                println!("preparing to stop Mido's House: connecting UNIX socket");
                let mut sock = UnixStream::connect(unix_socket::PATH).await?;
                println!("preparing to stop Mido's House: sending command");
                unix_socket::ClientMessage::PrepareStop.write(&mut sock).await?;
                println!("preparing to stop Mido's House: waiting for reply");
                u8::read(&mut sock).await?;
                println!("preparing to stop Mido's House: done");
            }
        }
    } else {
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
        let clean_shutdown = Arc::default();
        #[cfg(unix)] let unix_listener = unix_socket::listen(rocket.shutdown(), Arc::clone(&clean_shutdown));
        let racetime_task = tokio::spawn(racetime_bot::main(
            db_pool,
            http_client,
            discord_builder.ctx_fut.clone(),
            config.ootr_api_key.clone(),
            env,
            config.clone(),
            rocket.shutdown(),
            clean_shutdown,
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
        #[cfg(unix)] let unix_socket_task = tokio::spawn(unix_listener).map(|res| match res {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(Error::from(e)),
            Err(e) => Err(Error::from(e)),
        });
        #[cfg(not(unix))] let unix_socket_task = future::ok(());
        let ((), (), (), ()) = tokio::try_join!(discord_task, racetime_task, rocket_task, unix_socket_task)?;
    }
    Ok(())
}
