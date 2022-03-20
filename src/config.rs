use {
    anyhow::{
        Result,
        bail,
    },
    serde::Deserialize,
    tokio::fs,
    xdg::BaseDirectories,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Config {
    pub(crate) racetime: ConfigRaceTime,
    pub(crate) discord: ConfigDiscord,
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
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    #[serde(rename = "devClientID")]
    pub(crate) dev_client_id: String,
    pub(crate) dev_client_secret: String,
}
