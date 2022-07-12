#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    futures::future::FutureExt as _,
    rocket::Rocket,
    serenity::model::prelude::*,
    serenity_utils::builder::ErrorNotifier,
    sqlx::{
        PgPool,
        postgres::PgConnectOptions,
    },
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
    let discord_config = if env.is_dev() { &config.discord_dev } else { &config.discord_production };
    let discord_builder = serenity_utils::builder(discord_config.client_id, discord_config.bot_token.clone()).await?;
    let pool = PgPool::connect_with(PgConnectOptions::default().username("mido").database("midos_house").application_name("midos-house")).await?;
    let rocket = http::rocket(pool, discord_builder.ctx_fut.clone(), &config, env, view_as.into_iter().collect()).await?;
    let shutdown = rocket.shutdown();
    let discord_builder = discord_builder
        .error_notifier(ErrorNotifier::User(FENHL))
        .task(|ctx_fut, _| async move {
            shutdown.await;
            serenity_utils::shut_down(&*ctx_fut.read().await).await;
        });
    let racetime_task = tokio::spawn(racetime_bot::main(config.racetime_bot.clone(), rocket.shutdown())).map(|res| match res {
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
