use {
    anyhow::Result,
    serde::Deserialize,
    serenity::model::prelude::*,
};
#[cfg(unix)] use {
    anyhow::bail,
    xdg::BaseDirectories,
    tokio::fs,
};
#[cfg(windows)] use tokio::process::Command;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Config {
    pub(crate) discord_production: ConfigDiscord,
    pub(crate) discord_dev: ConfigDiscord,
    pub(crate) ootr_api_key: String,
    pub(crate) racetime_bot_production: ConfigRaceTime,
    pub(crate) racetime_bot_dev: ConfigRaceTime,
    #[serde(rename = "racetimeOAuthProduction", alias = "racetimeOAuth")]
    pub(crate) racetime_oauth_production: ConfigRaceTime,
    #[serde(rename = "racetimeOAuthDev")]
    pub(crate) racetime_oauth_dev: ConfigRaceTime,
    #[allow(unused)] //TODO
    startgg_production: String,
    #[allow(unused)] //TODO
    startgg_dev: String,
    pub(crate) secret_key: String,
}

impl Config {
    pub(crate) async fn load() -> Result<Self> {
        #[cfg(unix)] {
            if let Some(config_path) = BaseDirectories::new()?.find_config_file("midos-house.json") {
                let buf = fs::read(config_path).await?;
                Ok(serde_json::from_slice(&buf)?)
            } else {
                bail!("missing config file")
            }
        }
        #[cfg(windows)] { // allow testing without having rust-analyzer slow down mercredi
            Ok(serde_json::from_slice(&Command::new("ssh").arg("mercredi").arg("cat").arg("/etc/xdg/midos-house.json").output().await?.stdout)?)
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigRaceTime {
    #[serde(rename = "clientID")]
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigDiscord {
    #[serde(rename = "clientID")]
    pub(crate) client_id: ApplicationId,
    pub(crate) client_secret: String,
    pub(crate) bot_token: String,
}
