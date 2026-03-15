use {
    kuchiki::traits::TendrilSink as _,
    rand::distr::{
        Alphanumeric,
        SampleString as _,
    },
    crate::prelude::*,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromFormField, UriDisplayQuery, Sequence)]
pub(crate) enum Format {
    League,
    Sgl,
    Saws,
    Bingo,
    Ice,
    Mixed,
    Franco,
    Triforce,
}

impl Format {
    pub(crate) fn slug(&self) -> &'static str {
        match self {
            Self::League => "league",
            Self::Sgl => "sgl",
            Self::Saws => "saws",
            Self::Bingo => "bingo",
            Self::Ice => "ice",
            Self::Mixed => "mixed",
            Self::Franco => "franco",
            Self::Triforce => "triforce",
        }
    }

    pub(crate) fn display_name(&self) -> &'static str {
        match self {
            Self::League => "League S9",
            Self::Sgl => "SGL 2025",
            Self::Saws => "SAWS Beginner",
            Self::Bingo => "Bingo SDG Settings",
            Self::Ice => "Ice%",
            Self::Mixed => "Mixed Pools 2025",
            Self::Franco => "Franco",
            Self::Triforce => "Triforce Hunt",
        }
    }

    pub(crate) fn for_race(race: &Race) -> Option<Self> {
        if let Series::SlugOpen = race.series {
            race.draft.as_ref().and_then(|draft| draft.settings.get("sco_format")).map(|s| s.parse().expect("unexpected SlugCentral Open format"))
        } else {
            None
        }
    }

    pub(crate) fn draft_kind(&self) -> Option<draft::Kind> {
        match self {
            Self::Franco => Some(draft::Kind::TournoiFrancoS5),
            Self::League | Self::Sgl | Self::Saws | Self::Bingo | Self::Ice | Self::Mixed | Self::Triforce => None,
        }
    }

    pub(crate) fn default_race_duration(&self) -> TimeDelta {
        match self {
            Self::Ice => TimeDelta::minutes(30),
            Self::Sgl | Self::Bingo /*TODO verify */ | Self::Mixed => TimeDelta::hours(3),
            Self::League | Self::Saws | Self::Franco | Self::Triforce /*TODO verify */ => TimeDelta::hours(3) + TimeDelta::minutes(30),
        }
    }

    pub(crate) async fn single_settings(&self, global: &GlobalState, bingo_room_name: Option<&str>) -> Result<Option<(VersionedBranch, seed::Settings, Option<String>)>, SingleSettingsError> {
        let preset = match self {
            Self::League => "League S9",
            Self::Sgl => "SGL 2025 Tournament",
            Self::Saws => "Standard Anti-Weekly Settings (Beginner)",
            Self::Bingo => "SDG Bingo Tournament 3",
            Self::Ice => "Ice%",
            Self::Mixed => "4th Mixed Pools Tournament",
            Self::Franco => return Ok(None), // settings draft
            Self::Triforce => return Ok(Some((VersionedBranch::Latest { branch: ootr_utils::Branch::DevFenhl }, collect![
                format!("password_lock") => json!(true),
                format!("triforce_hunt") => json!(true),
            ], None))), //TODO add this preset once settings are decided
        };
        ootr_utils::Branch::DevFenhl.clone_repo(true).await?;
        let mut presets = fs::read_json::<HashMap<String, seed::Settings>>(ootr_utils::Branch::DevFenhl.dir(true)?.join("data").join("presets_default.json")).await?;
        let mut settings = presets.remove(preset).ok_or(SingleSettingsError::MissingPreset(*self, preset))?;
        settings.insert(format!("password_lock"), json!(true));
        let bingo_passphrase = if let Some(room_name) = bingo_room_name && let Self::Bingo = self {
            #[derive(Serialize)]
            struct BingoForm<'a> {
                csrfmiddlewaretoken: String,
                room_name: &'a str,
                passphrase: String,
                nickname: &'static str,
                game_type: u8,
                variant_type: u8,
                lockout_mode: u8,
                is_spectator: bool,
                hide_card: bool,
            }

            let index = global.http_client.get("https://bingosync.com/")
                .send().await?
                .detailed_error_for_status().await?
                .text().await?;
            let csrfmiddlewaretoken = kuchiki::parse_html().one(index)
                .select_first("input[name=csrfmiddlewaretoken]").map_err(|()| SingleSettingsError::BingoIndex)?
                .attributes
                .borrow_mut()
                .remove("value")
                .ok_or(SingleSettingsError::BingoIndex)?
                .value;
            let passphrase = Alphanumeric.sample_string(&mut rng(), 8);
            let response = global.http_client.post("https://bingosync.com/")
                .form(&BingoForm {
                    passphrase: passphrase.clone(),
                    nickname: "Mido",
                    game_type: 1, // OoT
                    variant_type: 90, // Item Randomizer Blackout
                    lockout_mode: 1, // Non-Lockout
                    is_spectator: true,
                    hide_card: true,
                    csrfmiddlewaretoken, room_name,
                })
                .send().await?
                .detailed_error_for_status().await?;
            settings.insert(format!("bingosync_url"), json!(response.url()));
            Some(passphrase)
        } else {
            None
        };
        Ok(Some((VersionedBranch::Latest { branch: ootr_utils::Branch::DevFenhl }, settings, bingo_passphrase)))
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        all::<Self>().find(|format| format.slug() == s).ok_or_else(|| s.to_owned())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SingleSettingsError {
    #[error(transparent)] Clone(#[from] ootr_utils::CloneError),
    #[error(transparent)] Dir(#[from] ootr_utils::DirError),
    #[error(transparent)] Http(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("failed to parse Bingosync index page")]
    BingoIndex,
    #[error("the settings preset {1:?} for SlugCentral Open format {0:?} is not available on the dev-fenhl branch of the randomizer")]
    MissingPreset(Format, &'static str),
}

impl IsNetworkError for SingleSettingsError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Clone(_) => false, //TODO implement IsNetworkError for ootr_utils::CloneError
            Self::Dir(_) => false,
            Self::Http(e) => e.is_network_error(),
            Self::Wheel(e) => e.is_network_error(),
            Self::BingoIndex => false,
            Self::MissingPreset(_, _) => false,
        }
    }
}
