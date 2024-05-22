#![recursion_limit = "512"]

use {
    std::env,
    rocket::Rocket,
    sqlx::{
        ConnectOptions as _,
        postgres::PgConnectOptions,
    },
    crate::prelude::*,
};
#[cfg(unix)] use {
    openssl as _, // `vendored` feature required to fix release build
    tokio::net::UnixStream,
    crate::{
        racetime_bot::SeedRollUpdate,
        unix_socket::ClientMessage as Subcommand,
    },
};

mod api;
mod auth;
mod cal;
mod config;
mod discord_bot;
mod draft;
mod event;
mod favicon;
mod form;
#[macro_use] mod http;
mod id;
mod lang;
#[macro_use] mod macros;
mod notification;
mod prelude;
mod racetime_bot;
mod seed;
mod series;
mod startgg;
mod team;
mod time;
#[cfg(unix)] mod unix_socket;
mod user;

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

fn parse_port(arg: &str) -> Result<u16, std::num::ParseIntError> {
    match arg {
        "production" => Ok(24812),
        "dev" => Ok(24814),
        _ => arg.parse(),
    }
}

#[cfg(not(unix))]
#[derive(clap::Subcommand)]
enum Subcommand {}

#[derive(clap::Parser)]
struct Args {
    #[clap(long, value_enum, default_value_t)]
    env: Environment,
    #[clap(long, value_parser = parse_port)]
    port: Option<u16>,
    #[clap(subcommand)]
    subcommand: Option<Subcommand>,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)] Any(#[from] anyhow::Error),
    #[error(transparent)] Base64(#[from] base64::DecodeError),
    #[error(transparent)] Error(#[from] event::Error),
    #[cfg(unix)] #[error(transparent)] Io(#[from] io::Error),
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

#[wheel::main(rocket)]
async fn main(Args { env, port, subcommand }: Args) -> Result<(), Error> {
    if let Some(subcommand) = subcommand {
        #[cfg(unix)] let mut sock = UnixStream::connect(unix_socket::PATH).await?;
        #[cfg(unix)] subcommand.write(&mut sock).await?;
        match subcommand {
            #[cfg(unix)] Subcommand::CleanupRoles { .. } => {
                u8::read(&mut sock).await?;
            }
            #[cfg(unix)] Subcommand::PrepareStop { .. } => {
                println!("preparing to stop Mido's House: waiting for reply");
                u8::read(&mut sock).await?;
                println!("preparing to stop Mido's House: done");
            }
            #[cfg(unix)] Subcommand::Roll { .. } | Subcommand::RollRsl { .. } | Subcommand::Seed { .. } => while let Some(update) = Option::<SeedRollUpdate>::read(&mut sock).await? {
                println!("{update:#?}");
            },
            #[cfg(unix)] Subcommand::UpdateRegionalVc { .. } => {
                println!("Mido's House: updating regional voice chat");
                u8::read(&mut sock).await?;
                println!("Mido's House: done updating regional voice chat");
            }
        }
    } else {
        let default_panic_hook = std::panic::take_hook();
        if let Environment::Production = env {
            std::panic::set_hook(Box::new(move |info| {
                let _ = wheel::night_report_sync("/net/midoshouse/error", Some("thread panic"));
                default_panic_hook(info)
            }));
        }
        let config = Config::load().await?;
        let http_client = reqwest::Client::builder()
            .user_agent(concat!("MidosHouse/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .use_rustls_tls()
            .hickory_dns(true)
            .https_only(true)
            .build()?;
        let discord_config = if env.is_dev() { &config.discord_dev } else { &config.discord_production };
        let discord_builder = serenity_utils::builder(discord_config.bot_token.clone()).await?;
        let db_pool = PgPool::connect_with(PgConnectOptions::default()
            .username("mido")
            .database(if env.is_dev() { "fados_house" } else { "midos_house" })
            .application_name("midos-house")
            .log_slow_statements(log::LevelFilter::Warn, Duration::from_secs(10))
        ).await?;
        let rocket = http::rocket(db_pool.clone(), discord_builder.ctx_fut.clone(), http_client.clone(), config.clone(), env, port.unwrap_or_else(|| if env.is_dev() { 24814 } else { 24812 })).await?;
        let new_room_lock = Arc::default();
        let extra_room_tx = Arc::new(RwLock::new(mpsc::channel(1).0));
        let discord_builder = discord_bot::configure_builder(discord_builder, db_pool.clone(), http_client.clone(), config.clone(), env, Arc::clone(&new_room_lock), Arc::clone(&extra_room_tx), rocket.shutdown());
        let clean_shutdown = Arc::default();
        let racetime_config = if env.is_dev() { &config.racetime_bot_dev } else { &config.racetime_bot_production }.clone();
        let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
        let (seed_cache_tx, seed_cache_rx) = watch::channel(());
        let global_state = Arc::new(racetime_bot::GlobalState::new(
            new_room_lock,
            racetime_config,
            extra_room_tx,
            db_pool.clone(),
            http_client.clone(),
            config.ootr_api_key.clone(),
            config.ootr_api_key_encryption.clone(),
            startgg_token.clone(),
            env,
            discord_builder.ctx_fut.clone(),
            Arc::clone(&clean_shutdown),
            seed_cache_tx,
        ).await);
        #[cfg(unix)] let unix_listener = unix_socket::listen(rocket.shutdown(), clean_shutdown, Arc::clone(&global_state));
        let racetime_task = tokio::spawn(racetime_bot::main(env, config.clone(), rocket.shutdown(), global_state, seed_cache_rx)).map(|res| match res {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(Error::from(e)),
            Err(e) => Err(Error::from(e)),
        });
        let import_task = tokio::spawn(cal::auto_import_races(db_pool, http_client, env, config, rocket.shutdown(), discord_builder.ctx_fut.clone())).map(|res| match res {
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
        let ((), (), (), (), ()) = tokio::try_join!(discord_task, import_task, racetime_task, rocket_task, unix_socket_task)?;
    }
    Ok(())
}
