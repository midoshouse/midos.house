use {
    anyhow::{
        Result,
        bail,
    },
    serde::Deserialize,
    serenity::model::prelude::*,
    tokio::fs,
    xdg::BaseDirectories,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Config {
    #[allow(unused)] //TODO
    racetime_bot: ConfigRaceTime,
    #[serde(rename = "racetimeOAuth")]
    pub(crate) racetime_oauth: ConfigRaceTime,
    pub(crate) discord_production: ConfigDiscord,
    pub(crate) discord_dev: ConfigDiscord,
    #[allow(unused)] //TODO
    startgg_production: String,
    #[allow(unused)] //TODO
    startgg_dev: String,
    pub(crate) secret_key: String,
}

impl Config {
    pub(crate) async fn load() -> Result<Self> {
        if let Some(config_path) = BaseDirectories::new()?.find_config_file("midos-house.json") {
            let buf = fs::read(config_path).await?;
            Ok(serde_json::from_slice(&buf)?)
        } else {
            bail!("missing config file")
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ConfigRaceTime {
    #[serde(rename = "clientID")]
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ConfigDiscord {
    #[serde(rename = "clientID")]
    pub(crate) client_id: ApplicationId,
    pub(crate) client_secret: String,
    #[allow(unused)] //TODO
    pub(crate) bot_token: String,
}
