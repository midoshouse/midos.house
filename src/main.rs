#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    futures::future::FutureExt as _,
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

fn parse_view_as(arg: &str) -> Result<(Id, Id), anyhow::Error> {
    let (from, to) = arg.split_once(':').ok_or(anyhow::anyhow!("missing colon in view-as option"))?;
    Ok((from.parse()?, to.parse()?))
}

#[derive(clap::Parser)]
struct Args {
    #[clap(long = "dev")]
    is_dev: bool,
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
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Task(#[from] tokio::task::JoinError),
}

#[wheel::main(rocket, debug)]
async fn main(Args { is_dev, view_as }: Args) -> Result<(), Error> {
    let config = Config::load().await?;
    let pool = PgPool::connect_with(PgConnectOptions::default().username("mido").database("midos_house").application_name("midos-house")).await?;
    let rocket = http::rocket(pool, &config, is_dev, view_as.into_iter().collect()).await?;
    let racetime_task = tokio::spawn(racetime_bot::main(config.racetime_bot.clone(), rocket.shutdown())).map(|res| match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let rocket_task = tokio::spawn(rocket.launch()).map(|res| match res {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let ((), ()) = tokio::try_join!(racetime_task, rocket_task)?;
    Ok(())
}
