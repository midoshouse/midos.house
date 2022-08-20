#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::time::Duration,
    futures::future::FutureExt as _,
    rocket::Rocket,
    serenity::{
        model::prelude::*,
        prelude::*,
    },
    serenity_utils::{
        builder::ErrorNotifier,
        handler::HandlerMethods as _,
    },
    sqlx::{
        PgPool,
        postgres::PgConnectOptions,
    },
    tokio::sync::broadcast,
    crate::{
        config::Config,
        util::Id,
    },
};

mod auth;
mod cal;
mod config;
mod event;
mod favicon;
mod http;
mod notification;
mod racetime_bot;
mod seed;
mod user;
mod util;

const FENHL: UserId = UserId(86841168427495424);

enum RoleReceiver {}

impl TypeMapKey for RoleReceiver {
    type Value = broadcast::Sender<Role>;
}

fn parse_view_as(arg: &str) -> Result<(Id, Id), anyhow::Error> {
    let (from, to) = arg.split_once(':').ok_or(anyhow::anyhow!("missing colon in view-as option"))?;
    Ok((from.parse()?, to.parse()?))
}

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
}

#[derive(clap::Parser)]
struct Args {
    #[clap(long, value_enum, default_value_t)]
    env: Environment,
    #[clap(long, parse(try_from_str = parse_view_as))]
    view_as: Vec<(Id, Id)>,
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
async fn main(Args { env, view_as }: Args) -> Result<(), Error> {
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
    let pool = PgPool::connect_with(PgConnectOptions::default().username("mido").database("midos_house").application_name("midos-house")).await?;
    let rocket = http::rocket(pool, discord_builder.ctx_fut.clone(), http_client.clone(), &config, env, view_as.into_iter().collect()).await?;
    let shutdown = rocket.shutdown();
    let discord_builder = discord_builder
        .data::<RoleReceiver>(broadcast::channel(1024).0)
        .error_notifier(ErrorNotifier::User(FENHL))
        .on_guild_role_create(|ctx, role| Box::pin(async move {
            let _ = ctx.data.read().await.get::<RoleReceiver>().expect("missing role create sender").send(role.clone());
            Ok(())
        }))
        .task(|ctx_fut, _| async move {
            shutdown.await;
            serenity_utils::shut_down(&*ctx_fut.read().await).await;
        });
    let racetime_task = tokio::spawn(racetime_bot::main(
        http_client,
        config.ootr_api_key.clone(),
        if env.is_dev() { "racetime.midos.house" } else { "racetime.gg" },
        if env.is_dev() { config.racetime_bot_dev.clone() } else { config.racetime_bot_production.clone() },
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
