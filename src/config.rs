use crate::prelude::*;
#[cfg(windows)] use directories::ProjectDirs;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[cfg(windows)] #[error(transparent)] Json(#[from] serde_json::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[cfg(unix)]
    #[error("missing config file")]
    Missing,
    #[cfg(windows)]
    #[error("failed to find project folder")]
    ProjectDirs,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Config {
    pub(crate) challonge: ConfigOAuth,
    pub(crate) challonge_api_key: String,
    pub(crate) discord: ConfigDiscord,
    pub(crate) league_api_key: String,
    pub(crate) ootr_api_key: String,
    pub(crate) ootr_api_key_encryption: String,
    pub(crate) racetime_bot: ConfigRaceTime,
    #[serde(rename = "racetimeOAuth")]
    pub(crate) racetime_oauth: ConfigRaceTime,
    pub(crate) secret_key: String,
    pub(crate) startgg: String,
    #[serde(rename = "startggOAuth")]
    pub(crate) startgg_oauth: ConfigOAuth,
    pub(crate) tfb_api_key: String,
}

impl Config {
    pub(crate) async fn load() -> Result<Self, Error> {
        #[cfg(unix)] {
            if let Some(config_path) = BaseDirectories::new().find_config_file(if Environment::default().is_dev() { "midos-house-dev.json" } else { "midos-house.json" }) {
                Ok(fs::read_json(config_path).await?)
            } else {
                Err(Error::Missing)
            }
        }
        #[cfg(windows)] {
            Ok(match Environment::default() {
                Environment::Local => fs::read_json(ProjectDirs::from("net", "Fenhl", "Midos House").ok_or(Error::ProjectDirs)?.config_dir().join("dev.json")).await?,
                // allow testing without having rust-analyzer slow down the server
                Environment::Production => serde_json::from_slice(&Command::new("ssh").arg("midos.house").arg("cat").arg("/etc/xdg/midos-house.json").check("ssh").await?.stdout)?,
                Environment::Dev => serde_json::from_slice(&Command::new("ssh").arg("midos.house").arg("cat").arg("/etc/xdg/midos-house-dev.json").check("ssh").await?.stdout)?,
            })
        }
    }

    #[cfg(test)]
    pub(crate) fn dummy() -> Self {
        Self {
            challonge: ConfigOAuth {
                client_id: String::default(),
                client_secret: String::default(),
            },
            challonge_api_key: String::default(),
            discord: ConfigDiscord {
                client_id: ApplicationId::new(1),
                client_secret: String::default(),
                bot_token: String::default(),
            },
            league_api_key: String::default(),
            ootr_api_key: String::default(),
            ootr_api_key_encryption: String::default(),
            racetime_bot: ConfigRaceTime {
                client_id: String::default(),
                client_secret: String::default(),
            },
            racetime_oauth: ConfigRaceTime {
                client_id: String::default(),
                client_secret: String::default(),
            },
            secret_key: format!("SY6LI8modMlaLp6dq6Bm/aWkr4OZ+Y73NzGN2/EoKp21gR3Cphlyl8sdGltKPEPDfvIeT35a3FHfm7wLboU17A=="),
            startgg: String::default(),
            startgg_oauth: ConfigOAuth {
                client_id: String::default(),
                client_secret: String::default(),
            },
            tfb_api_key: String::default(),
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

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigDiscord {
    #[serde(rename = "clientID")]
    pub(crate) client_id: ApplicationId,
    pub(crate) client_secret: String,
    pub(crate) bot_token: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigOAuth {
    #[serde(rename = "clientID")]
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
}
