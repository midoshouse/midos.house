use crate::prelude::*;

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

    pub(crate) async fn single_settings(&self) -> Result<Option<(VersionedBranch, seed::Settings)>, SingleSettingsError> {
        let preset = match self {
            Self::League => "League S9",
            Self::Sgl => "SGL 2025 Tournament",
            Self::Saws => "Standard Anti-Weekly Settings (Beginner)",
            Self::Bingo => "SDG Bingo Tournament 3",
            Self::Ice => "Ice%",
            Self::Mixed => "4th Mixed Pools Tournament",
            Self::Franco => return Ok(None), // settings draft
            Self::Triforce => "SlugCentral Open Triforce Hunt", //TODO add this preset once settings are decided
        };
        let mut presets = fs::read_json::<HashMap<String, seed::Settings>>(ootr_utils::Branch::DevFenhl.dir(true)?.join("data").join("presets_default.json")).await?;
        let mut settings = presets.remove(preset).ok_or(SingleSettingsError::MissingPreset)?;
        settings.insert(format!("password_lock"), json!(true));
        Ok(Some((VersionedBranch::Latest { branch: ootr_utils::Branch::DevFenhl }, settings)))
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
    #[error(transparent)] Dir(#[from] ootr_utils::DirError),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("the settings preset for this SlugCentral Open format is not available on the dev-fenhl branch of the randomizer")]
    MissingPreset,
}

impl IsNetworkError for SingleSettingsError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Dir(_) => false,
            Self::Wheel(e) => e.is_network_error(),
            Self::MissingPreset => false,
        }
    }
}
