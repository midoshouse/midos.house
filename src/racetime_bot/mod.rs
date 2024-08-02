use {
    std::{
        io::prelude::*,
        process::Stdio,
    },
    git2::{
        BranchType,
        Repository,
        ResetType,
    },
    kuchiki::{
        NodeRef,
        traits::TendrilSink as _,
    },
    ootr_utils as rando,
    racetime::{
        Error,
        ResultExt as _,
        handler::{
            RaceContext,
            RaceHandler,
        },
        model::*,
    },
    reqwest::{
        IntoUrl,
        StatusCode,
    },
    semver::Version,
    serde_json::Value as Json,
    serde_plain::derive_deserialize_from_fromstr,
    serde_with::{
        DisplayFromStr,
        json::JsonString,
    },
    serenity::all::{
        CreateAllowedMentions,
        CreateMessage,
        UserId,
    },
    tokio::{
        io::{
            AsyncBufReadExt as _,
            AsyncWriteExt as _,
            BufReader,
        },
        sync::{
            Notify,
            Semaphore,
            TryAcquireError,
            mpsc,
        },
        time::timeout,
    },
    wheel::traits::AsyncCommandOutputExt as _,
    crate::{
        cal::Entrant,
        config::ConfigRaceTime,
        prelude::*,
    },
};
#[cfg(unix)] use async_proto::Protocol;
#[cfg(windows)] use directories::UserDirs;

mod report;

#[cfg(unix)] const PYTHON: &str = "python3";
#[cfg(windows)] const PYTHON: &str = "py";

pub(crate) const CATEGORY: &str = "ootr";

/// Randomizer versions that are known to exist on the ootrandomizer.com API despite not being listed by the version endpoint since supplementary versions weren't tracked at the time.
const KNOWN_GOOD_WEB_VERSIONS: [rando::Version; 4] = [
    rando::Version::from_branch(rando::Branch::DevR, 6, 2, 238, 1),
    rando::Version::from_branch(rando::Branch::DevR, 7, 1, 83, 1), // commit 578a64f4c78a831cde4215e0ac31565d3bf9bc46
    rando::Version::from_branch(rando::Branch::DevR, 7, 1, 143, 1), // commit 06390ece7e38fce1dd02ca60a28a7b1ff9fceb10
    rando::Version::from_branch(rando::Branch::DevFenhl, 6, 9, 14, 2),
];

const MULTIWORLD_RATE_LIMIT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ParseUserError {
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("this seems to be neither a URL, nor a racetime.gg user ID, nor a Mido's House user ID")]
    Format,
    #[error("there is no racetime.gg user with this ID (error 404)")]
    IdNotFound,
    #[error("this URL is not a racetime.gg user profile URL")]
    InvalidUrl,
    #[error("there is no Mido's House user with this ID")]
    MidosHouseId,
    #[error("There is no racetime.gg account associated with this Mido's House account. Ask the user to go to their profile and select “Connect a racetime.gg account”. You can also link to their racetime.gg profile directly.")]
    MidosHouseUserNoRacetime,
    #[error("there is no racetime.gg user with this URL (error 404)")]
    UrlNotFound,
}

pub(crate) async fn parse_user(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, host: &str, id_or_url: &str) -> Result<String, ParseUserError> {
    if let Ok(id) = id_or_url.parse() {
        return if let Some(user) = User::from_id(&mut **transaction, id).await? {
            if let Some(racetime) = user.racetime {
                Ok(racetime.id)
            } else {
                Err(ParseUserError::MidosHouseUserNoRacetime)
            }
        } else {
            Err(ParseUserError::MidosHouseId)
        }
    }
    if regex_is_match!("^[0-9A-Za-z]+$", id_or_url) {
        return match http_client.get(format!("https://{host}/user/{id_or_url}/data"))
            .send().await?
            .detailed_error_for_status().await
        {
            Ok(_) => Ok(id_or_url.to_owned()),
            Err(wheel::Error::ResponseStatus { inner, .. }) if inner.status() == Some(StatusCode::NOT_FOUND) => Err(ParseUserError::IdNotFound),
            Err(e) => Err(e.into()),
        }
    }
    if let Ok(url) = Url::parse(id_or_url) {
        return if_chain! {
            if let Some("racetime.gg" | "www.racetime.gg") = url.host_str();
            if let Some(mut path_segments) = url.path_segments();
            if path_segments.next() == Some("user");
            if let Some(url_part) = path_segments.next();
            if path_segments.next().is_none();
            then {
                match http_client.get(format!("https://{host}/user/{url_part}/data"))
                    .send().await?
                    .detailed_error_for_status().await
                {
                    Ok(response) => Ok(response.json_with_text_in_error::<UserData>().await?.id),
                    Err(wheel::Error::ResponseStatus { inner, .. }) if inner.status() == Some(StatusCode::NOT_FOUND) => Err(ParseUserError::UrlNotFound),
                    Err(e) => Err(e.into()),
                }
            } else {
                Err(ParseUserError::InvalidUrl)
            }
        }
    }
    Err(ParseUserError::Format)
}

#[derive(Clone)]
pub(crate) enum VersionedBranch {
    Pinned(rando::Version),
    Latest(rando::Branch),
    Custom {
        github_username: &'static str,
        branch: &'static str,
    },
}

impl VersionedBranch {
    fn branch(&self) -> Option<rando::Branch> {
        match self {
            Self::Pinned(version) => Some(version.branch()),
            Self::Latest(branch) => Some(*branch),
            Self::Custom { .. } => None,
        }
    }
}

#[derive(Default)]
pub(crate) enum RslDevFenhlPreset {
    #[default]
    Fenhl,
    Pictionary,
}

impl RslDevFenhlPreset {
    fn name(&self) -> &'static str {
        match self {
            Self::Fenhl => "fenhl",
            Self::Pictionary => "pictionary",
        }
    }
}

impl FromStr for RslDevFenhlPreset {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match &*s.to_ascii_lowercase() {
            "fenhl" => Self::Fenhl,
            "pic" | "pictionary" => Self::Pictionary,
            _ => return Err(()),
        })
    }
}

pub(crate) enum VersionedRslPreset {
    Xopar {
        version: Option<Version>,
        preset: rsl::Preset,
    },
    Fenhl {
        version: Option<(Version, u8)>,
        preset: RslDevFenhlPreset,
    },
}

impl VersionedRslPreset {
    #[cfg(unix)] pub(crate) fn new_unversioned(branch: &str, preset: Option<&str>) -> Result<Self, ()> {
        Ok(match branch {
            "xopar" => Self::Xopar { version: None, preset: preset.map(rsl::Preset::from_str).transpose()?.unwrap_or_default() },
            "fenhl" => Self::Fenhl { version: None, preset: preset.map(RslDevFenhlPreset::from_str).transpose()?.unwrap_or_default() },
            _ => return Err(()),
        })
    }

    #[cfg(unix)] pub(crate) fn new_versioned(version: rando::Version, preset: Option<&str>) -> Result<Self, ()> {
        Ok(match version.branch() {
            rando::Branch::DevR | rando::Branch::DevRob => Self::Xopar { version: Some(version.base().clone()), preset: preset.map(rsl::Preset::from_str).transpose()?.unwrap_or_default() },
            rando::Branch::DevFenhl => Self::Fenhl { version: Some((version.base().clone(), version.supplementary().unwrap())), preset: preset.map(RslDevFenhlPreset::from_str).transpose()?.unwrap_or_default() },
            _ => return Err(()),
        })
    }

    fn base_version(&self) -> Option<&Version> {
        match self {
            Self::Xopar { version, .. } => version.as_ref(),
            Self::Fenhl { version, .. } => version.as_ref().map(|(base, _)| base),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Xopar { preset, .. } => preset.name(),
            Self::Fenhl { preset, .. } => preset.name(),
        }
    }

    fn is_version_locked(&self) -> bool {
        match self {
            Self::Xopar { version, .. } => version.is_some(),
            Self::Fenhl { version, .. } => version.is_some(),
        }
    }

    fn script_path(&self) -> Result<Cow<'static, Path>, RollError> {
        Ok({
            #[cfg(unix)] {
                match self {
                    Self::Fenhl { version: None, .. } => Cow::Borrowed(Path::new("/opt/git/github.com/fenhl/plando-random-settings/main")),
                    Self::Fenhl { version: Some((base, supplementary)), .. } => Cow::Owned(BaseDirectories::new()?.find_data_file(Path::new("midos-house").join(format!("rsl-dev-fenhl-{base}-{supplementary}"))).ok_or(RollError::RslPath)?),
                    Self::Xopar { version: None, .. } => Cow::Owned(BaseDirectories::new()?.find_data_file("fenhl/rslbot/plando-random-settings").ok_or(RollError::RslPath)?),
                    Self::Xopar { version: Some(version), .. } => Cow::Owned(BaseDirectories::new()?.find_data_file(Path::new("midos-house").join(format!("rsl-{version}"))).ok_or(RollError::RslPath)?),
                }
            }
            #[cfg(windows)] {
                match self {
                    Self::Fenhl { .. } => Cow::Borrowed(Path::new("C:/Users/fenhl/git/github.com/fenhl/plando-random-settings/main")), //TODO respect script version field
                    Self::Xopar { .. } => Cow::Borrowed(Path::new("C:/Users/fenhl/git/github.com/matthewkirby/plando-random-settings/main")), //TODO respect script version field
                }
            }
        })
    }
}

/// Determines how early the bot may start generating the seed for an official race.
///
/// There are two factors to consider here:
///
/// 1. If we start rolling the seed too late, players may have to wait for the seed to become available, which may delay the start of the race.
/// 2. If we start rolling the seed too early, players may be able to cheat by finding the seed's sequential ID on ootrandomizer.com
///    or by finding the seed in the list of recently rolled seeds on triforceblitz.com.
///    This is not an issue for seeds rolled locally, so the local generator will always be started immediately after the room is opened.
///
/// How early we should start rolling seeds therefore depends on how long seed generation is expected to take, which depends on the settings.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PrerollMode {
    /// Do not preroll seeds.
    None,
    /// Preroll seeds within the 5 minutes before the deadline.
    Short,
    /// Start prerolling seeds between the time the room is opened and 15 minutes before the deadline.
    Medium,
    /// Always keep one seed in reserve until the end of the event. Fetch that seed or start rolling a new one immediately as the room is opened.
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) enum UnlockSpoilerLog {
    Now,
    Progression,
    After,
    Never,
}

#[derive(Clone, Copy, Sequence)]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) enum Goal {
    Cc7,
    CoOpS3,
    CopaDoBrasil,
    MixedPoolsS2,
    MixedPoolsS3,
    MultiworldS3,
    MultiworldS4,
    NineDaysOfSaws,
    Pic7,
    PicRs2,
    Rsl,
    Sgl2023,
    Sgl2024,
    SongsOfHope,
    StandardRuleset,
    TournoiFrancoS3,
    TournoiFrancoS4,
    TriforceBlitz,
    TriforceBlitzProgressionSpoiler,
    WeTryToBeBetter,
}

#[derive(Debug, thiserror::Error)]
#[error("this racetime.gg goal is not handled by Mido")]
pub(crate) struct GoalFromStrError;

impl Goal {
    pub(crate) fn for_event(series: Series, event: &str) -> Option<Self> {
        all::<Self>().find(|goal| goal.matches_event(series, event))
    }

    fn from_race_data(race_data: &RaceData) -> Option<Self> {
        let Ok(bot_goal) = race_data.goal.name.parse::<Self>() else { return None };
        if race_data.goal.custom != bot_goal.is_custom() { return None }
        if let (Goal::StandardRuleset, Some(_)) = (bot_goal, &race_data.opened_by) { return None }
        Some(bot_goal)
    }

    fn matches_event(&self, series: Series, event: &str) -> bool {
        match self {
            Self::Cc7 => series == Series::Standard && event == "7cc",
            Self::CoOpS3 => series == Series::CoOp && event == "3",
            Self::CopaDoBrasil => series == Series::CopaDoBrasil && event == "1",
            Self::MixedPoolsS2 => series == Series::MixedPools && event == "2",
            Self::MixedPoolsS3 => series == Series::MixedPools && event == "3",
            Self::MultiworldS3 => series == Series::Multiworld && event == "3",
            Self::MultiworldS4 => series == Series::Multiworld && event == "4",
            Self::NineDaysOfSaws => series == Series::NineDaysOfSaws,
            Self::Pic7 => series == Series::Pictionary && event == "7",
            Self::PicRs2 => series == Series::Pictionary && event == "rs2",
            Self::Rsl => series == Series::Rsl,
            Self::Sgl2023 => series == Series::SpeedGaming && event.starts_with("2023"),
            Self::Sgl2024 => series == Series::SpeedGaming && event.starts_with("2024"),
            Self::SongsOfHope => series == Series::SongsOfHope && event == "1",
            Self::StandardRuleset => series == Series::Standard && event == "w",
            Self::TournoiFrancoS3 => series == Series::TournoiFrancophone && event == "3",
            Self::TournoiFrancoS4 => series == Series::TournoiFrancophone && event == "4",
            Self::TriforceBlitz => series == Series::TriforceBlitz,
            Self::TriforceBlitzProgressionSpoiler => false, // possible future tournament but no concrete plans
            Self::WeTryToBeBetter => series == Series::WeTryToBeBetter && event == "1",
        }
    }

    pub(crate) fn is_custom(&self) -> bool {
        match self {
            | Self::Rsl
            | Self::StandardRuleset
            | Self::TriforceBlitz
                => false,
            | Self::Cc7
            | Self::CoOpS3
            | Self::CopaDoBrasil
            | Self::MixedPoolsS2
            | Self::MixedPoolsS3
            | Self::MultiworldS3
            | Self::MultiworldS4
            | Self::NineDaysOfSaws
            | Self::Pic7
            | Self::PicRs2
            | Self::Sgl2023
            | Self::Sgl2024
            | Self::SongsOfHope
            | Self::TournoiFrancoS3
            | Self::TournoiFrancoS4
            | Self::TriforceBlitzProgressionSpoiler
            | Self::WeTryToBeBetter
                => true,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Cc7 => "Standard Tournament Season 7 Challenge Cup",
            Self::CoOpS3 => "Co-op Tournament Season 3",
            Self::CopaDoBrasil => "Copa do Brasil",
            Self::MixedPoolsS2 => "2nd Mixed Pools Tournament",
            Self::MixedPoolsS3 => "3rd Mixed Pools Tournament",
            Self::MultiworldS3 => "3rd Multiworld Tournament",
            Self::MultiworldS4 => "4th Multiworld Tournament",
            Self::NineDaysOfSaws => "9 Days of SAWS",
            Self::Pic7 => "7th Pictionary Spoiler Log Race",
            Self::PicRs2 => "2nd Random Settings Pictionary Spoiler Log Race",
            Self::Rsl => "Random settings league",
            Self::Sgl2023 => "SGL 2023",
            Self::Sgl2024 => "SGL 2024",
            Self::SongsOfHope => "Songs of Hope",
            Self::StandardRuleset => "Standard Ruleset",
            Self::TournoiFrancoS3 => "Tournoi Francophone Saison 3",
            Self::TournoiFrancoS4 => "Tournoi Francophone Saison 4",
            Self::TriforceBlitz => "Triforce Blitz",
            Self::TriforceBlitzProgressionSpoiler => "Triforce Blitz Progression Spoiler",
            Self::WeTryToBeBetter => "WeTryToBeBetter",
        }
    }

    fn language(&self) -> Language {
        match self {
            | Self::Cc7
            | Self::CoOpS3
            | Self::MixedPoolsS2
            | Self::MixedPoolsS3
            | Self::MultiworldS3
            | Self::MultiworldS4
            | Self::NineDaysOfSaws
            | Self::Pic7
            | Self::PicRs2
            | Self::Rsl
            | Self::Sgl2023
            | Self::Sgl2024
            | Self::SongsOfHope
            | Self::StandardRuleset
            | Self::TournoiFrancoS4 //TODO change to bilingual English/French
            | Self::TriforceBlitz
            | Self::TriforceBlitzProgressionSpoiler
                => English,
            | Self::TournoiFrancoS3
            | Self::WeTryToBeBetter
                => French,
            | Self::CopaDoBrasil
                => Portuguese,
        }
    }

    fn draft_kind(&self) -> Option<draft::Kind> {
        match self {
            Self::Cc7 => Some(draft::Kind::S7),
            Self::MultiworldS3 => Some(draft::Kind::MultiworldS3),
            Self::MultiworldS4 => Some(draft::Kind::MultiworldS4),
            Self::TournoiFrancoS3 => Some(draft::Kind::TournoiFrancoS3),
            Self::TournoiFrancoS4 => Some(draft::Kind::TournoiFrancoS4),
            | Self::CoOpS3
            | Self::CopaDoBrasil
            | Self::MixedPoolsS2
            | Self::MixedPoolsS3
            | Self::NineDaysOfSaws
            | Self::Pic7
            | Self::PicRs2
            | Self::Rsl
            | Self::Sgl2023
            | Self::Sgl2024
            | Self::SongsOfHope
            | Self::StandardRuleset
            | Self::TriforceBlitz
            | Self::TriforceBlitzProgressionSpoiler
            | Self::WeTryToBeBetter
                => None,
        }
    }

    /// See the [`PrerollMode`] docs.
    pub(crate) fn preroll_seeds(&self) -> PrerollMode {
        match self {
            | Self::Sgl2023
            | Self::Sgl2024
            | Self::TriforceBlitz
                => PrerollMode::None,
            | Self::Cc7
            | Self::CoOpS3
            | Self::StandardRuleset //TODO allow organizers to configure this
                => PrerollMode::Short,
            | Self::CopaDoBrasil
            | Self::MultiworldS3
            | Self::MultiworldS4
            | Self::NineDaysOfSaws
            | Self::Pic7
            | Self::PicRs2
            | Self::Rsl
            | Self::SongsOfHope
            | Self::TournoiFrancoS3
            | Self::TournoiFrancoS4
            | Self::TriforceBlitzProgressionSpoiler
            | Self::WeTryToBeBetter
                => PrerollMode::Medium,
            | Self::MixedPoolsS2
            | Self::MixedPoolsS3
                => PrerollMode::Long,
        }
    }

    pub(crate) fn unlock_spoiler_log(&self, official_race: bool, spoiler_seed: bool) -> UnlockSpoilerLog {
        if spoiler_seed {
            UnlockSpoilerLog::Now
        } else {
            match self {
                | Self::Pic7
                | Self::PicRs2
                    => UnlockSpoilerLog::Now,
                | Self::TriforceBlitzProgressionSpoiler
                    => UnlockSpoilerLog::Progression,
                | Self::CopaDoBrasil
                | Self::MixedPoolsS2
                | Self::MixedPoolsS3
                | Self::MultiworldS3
                | Self::MultiworldS4
                | Self::NineDaysOfSaws
                | Self::Rsl
                | Self::Sgl2023
                | Self::Sgl2024
                | Self::SongsOfHope
                | Self::TournoiFrancoS3
                | Self::TournoiFrancoS4
                | Self::TriforceBlitz
                | Self::WeTryToBeBetter
                    => UnlockSpoilerLog::After,
                | Self::Cc7
                | Self::CoOpS3
                | Self::StandardRuleset
                    => if official_race { UnlockSpoilerLog::Never } else { UnlockSpoilerLog::After },
            }
        }
    }

    pub(crate) fn rando_version(&self) -> VersionedBranch {
        match self {
            Self::Cc7 => VersionedBranch::Pinned(rando::Version::from_dev(8, 1, 0)),
            Self::CoOpS3 => VersionedBranch::Pinned(rando::Version::from_dev(8, 1, 0)),
            Self::CopaDoBrasil => VersionedBranch::Pinned(rando::Version::from_dev(7, 1, 143)),
            Self::MixedPoolsS2 => VersionedBranch::Pinned(rando::Version::from_branch(rando::Branch::DevFenhl, 7, 1, 117, 17)),
            Self::MixedPoolsS3 => VersionedBranch::Latest(rando::Branch::DevFenhl),
            Self::MultiworldS3 => VersionedBranch::Pinned(rando::Version::from_dev(6, 2, 205)),
            Self::MultiworldS4 => VersionedBranch::Pinned(rando::Version::from_dev(7, 1, 199)),
            Self::NineDaysOfSaws => VersionedBranch::Pinned(rando::Version::from_branch(rando::Branch::DevFenhl, 6, 9, 14, 2)),
            Self::Pic7 => VersionedBranch::Custom { github_username: "fenhl", branch: "frogs2-melody" },
            Self::Sgl2023 => VersionedBranch::Latest(rando::Branch::Sgl2023),
            Self::Sgl2024 => VersionedBranch::Latest(rando::Branch::Sgl2024),
            Self::SongsOfHope => VersionedBranch::Pinned(rando::Version::from_dev(8, 1, 0)),
            Self::StandardRuleset => VersionedBranch::Latest(rando::Branch::Dev), //TODO allow organizers to configure this
            Self::TournoiFrancoS3 => VersionedBranch::Pinned(rando::Version::from_branch(rando::Branch::DevR, 7, 1, 143, 1)),
            Self::TournoiFrancoS4 => VersionedBranch::Pinned(rando::Version::from_branch(rando::Branch::DevRob, 8, 1, 45, 105)),
            Self::TriforceBlitz => VersionedBranch::Latest(rando::Branch::DevBlitz),
            Self::TriforceBlitzProgressionSpoiler => VersionedBranch::Latest(rando::Branch::DevBlitz),
            Self::WeTryToBeBetter => VersionedBranch::Latest(rando::Branch::Dev),
            Self::PicRs2 | Self::Rsl => panic!("randomizer version for this goal must be parsed from RSL script"),
        }
    }

    /// Only returns a value for goals that only have one possible set of settings.
    fn single_settings(&self) -> Option<serde_json::Map<String, Json>> {
        match self {
            Self::Cc7 => None, // settings draft
            Self::CoOpS3 => Some(coop::s3_settings()),
            Self::CopaDoBrasil => Some(br::s1_settings()),
            Self::MixedPoolsS2 => Some(mp::s2_settings()),
            Self::MixedPoolsS3 => Some(mp::s3_settings()),
            Self::MultiworldS3 => None, // settings draft
            Self::MultiworldS4 => None, // settings draft
            Self::NineDaysOfSaws => None, // per-event settings
            Self::Pic7 => Some(pic::race7_settings()),
            Self::PicRs2 => None, // random settings
            Self::Rsl => None, // random settings
            Self::Sgl2023 => Some(sgl::settings_2023()),
            Self::Sgl2024 => Some(sgl::settings_2024()),
            Self::SongsOfHope => Some(soh::settings()),
            Self::StandardRuleset => Some(s::weekly_settings_2024w31()), //TODO allow organizers to configure this
            Self::TournoiFrancoS3 => None, // settings draft
            Self::TournoiFrancoS4 => None, // settings draft
            Self::TriforceBlitz => None, // per-event settings
            Self::TriforceBlitzProgressionSpoiler => Some(tfb::progression_spoiler_settings()),
            Self::WeTryToBeBetter => Some(wttbb::settings()),
        }
    }

    pub(crate) fn should_create_rooms(&self) -> bool {
        match self {
            | Self::MixedPoolsS2
            | Self::MixedPoolsS3
            | Self::NineDaysOfSaws
            | Self::Rsl
                => false,
            | Self::Cc7
            | Self::CoOpS3
            | Self::CopaDoBrasil
            | Self::MultiworldS3
            | Self::MultiworldS4
            | Self::Pic7
            | Self::PicRs2
            | Self::Sgl2023
            | Self::Sgl2024
            | Self::SongsOfHope
            | Self::StandardRuleset
            | Self::TournoiFrancoS3
            | Self::TournoiFrancoS4
            | Self::TriforceBlitz
            | Self::TriforceBlitzProgressionSpoiler
            | Self::WeTryToBeBetter
                => true,
        }
    }

    async fn send_presets(&self, ctx: &RaceContext<GlobalState>) -> Result<(), Error> {
        match self {
            | Self::Pic7
                => ctx.say("!seed: The settings used for the race").await?,
            | Self::PicRs2
                => ctx.say("!seed: The weights used for the race").await?,
            | Self::CoOpS3
            | Self::CopaDoBrasil
            | Self::MixedPoolsS2
            | Self::MixedPoolsS3
            | Self::Sgl2023
            | Self::Sgl2024
            | Self::SongsOfHope
                => ctx.say("!seed: The settings used for the tournament").await?,
            | Self::WeTryToBeBetter
                => ctx.say("!seed : Les settings utilisés pour le tournoi").await?,
            Self::Cc7 => {
                ctx.say("!seed base: The tournament's base settings.").await?;
                ctx.say("!seed random: Simulate a settings draft with both players picking randomly. The settings are posted along with the seed.").await?;
                ctx.say("!seed draft: Pick the settings here in the chat.").await?;
                ctx.say("!seed <setting> <value> <setting> <value>... (e.g. !seed deku open camc off): Pick a set of draftable settings without doing a full draft. Use “!settings” for a list of available settings.").await?;
            }
            Self::MultiworldS3 => {
                ctx.say("!seed base: The settings used for the qualifier and tiebreaker asyncs.").await?;
                ctx.say("!seed random: Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.").await?;
                ctx.say("!seed draft: Pick the settings here in the chat.").await?;
                ctx.say("!seed <setting> <value> <setting> <value>... (e.g. !seed trials 2 wincon scrubs): Pick a set of draftable settings without doing a full draft. Use “!settings” for a list of available settings.").await?;
            }
            Self::MultiworldS4 => {
                ctx.say("!seed base: The settings used for the qualifier and tiebreaker asyncs.").await?;
                ctx.say("!seed random: Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.").await?;
                ctx.say("!seed draft: Pick the settings here in the chat.").await?;
                ctx.say("!seed <setting> <value> <setting> <value>... (e.g. !seed trials 2 gbk stones): Pick a set of draftable settings without doing a full draft. Use “!settings” for a list of available settings.").await?;
            }
            Self::NineDaysOfSaws => {
                ctx.say("!seed day1: S6").await?;
                ctx.say("!seed day2: Beginner").await?;
                ctx.say("!seed day3: Advanced").await?;
                ctx.say("!seed day4: S5 + one bonk KO").await?;
                ctx.say("!seed day5: Beginner + mixed pools").await?;
                ctx.say("!seed day6: Beginner 3-player multiworld").await?;
                ctx.say("!seed day7: Beginner").await?;
                ctx.say("!seed day8: S6 + dungeon ER").await?;
                ctx.say("!seed day9: S6").await?;
            }
            Self::Rsl => for preset in all::<rsl::Preset>() {
                ctx.say(format!("!seed{}: {}", match preset {
                    rsl::Preset::League => String::default(),
                    rsl::Preset::Multiworld => format!(" {} <worldcount>", preset.name()),
                    _ => format!(" {}", preset.name()),
                }, match preset {
                    rsl::Preset::League => "official Random Settings League weights",
                    rsl::Preset::Beginner => "random settings for beginners, see https://ootr.fenhl.net/static/rsl-beginner-weights.html for details",
                    rsl::Preset::Intermediate => "a step between Beginner and League",
                    rsl::Preset::Ddr => "League but always normal damage and with cutscenes useful for tricks in the DDR ruleset",
                    rsl::Preset::CoOp => "weights tuned for co-op play",
                    rsl::Preset::Multiworld => "weights tuned for multiworld",
                })).await?;
            },
            Self::StandardRuleset => ctx.say("!seed: The current weekly settings").await?,
            Self::TournoiFrancoS3 => {
                ctx.say("!seed base : Settings de base.").await?;
                ctx.say("!seed random : Simule en draft en sélectionnant des settings au hasard pour les deux joueurs. Les settings seront affichés avec la seed.").await?;
                ctx.say("!seed draft : Vous fait effectuer un draft dans le chat.").await?;
                ctx.say("!seed <setting> <configuration> <setting> <configuration>... ex : !seed trials random bridge ad : Créé une seed avec les settings que vous définissez. Tapez “!settings” pour obtenir la liste des settings.").await?;
                ctx.say("Utilisez “!seed random advanced” ou “!seed draft advanced” pour autoriser les settings difficiles.").await?;
                ctx.say("Activez les donjons Master Quest en utilisant par exemple : “!seed base 6mq” ou “!seed draft advanced 12mq”").await?;
            }
            Self::TournoiFrancoS4 => {
                ctx.say("!seed base: The tournament's base settings / Settings de base.").await?;
                ctx.say("!seed random: Simulate a settings draft with both players picking randomly. The settings are posted along with the seed. / Simule en draft en sélectionnant des settings au hasard pour les deux joueurs. Les settings seront affichés avec la seed.").await?;
                ctx.say("!seed draft: Pick the settings here in the chat. / Vous fait effectuer un draft dans le chat.").await?;
                ctx.say("!seed <setting> <value> <setting> <value>... (e.g. !seed trials random bridge ad): Pick a set of draftable settings without doing a full draft. Use “!settings” for a list of available settings. / Créé une seed avec les settings que vous définissez. Tapez “!settings” pour obtenir la liste des settings.").await?;
                ctx.say("Use “!seed random advanced” or “!seed draft advanced” to allow advanced settings. / Utilisez “!seed random advanced” ou “!seed draft advanced” pour autoriser les settings difficiles.").await?;
                ctx.say("Enable Master Quest using e.g. “!seed base 6mq” or “!seed draft advanced 12mq” / Activez les donjons Master Quest en utilisant par exemple : “!seed base 6mq” ou “!seed draft advanced 12mq”").await?;
            }
            Self::TriforceBlitz => {
                ctx.say("!seed s3: Triforce Blitz season 3 settings").await?;
                ctx.say("!seed jr: Jabu's Revenge").await?;
                ctx.say("!seed s2: Triforce Blitz season 2 settings").await?;
                ctx.say("!seed daily: Triforce Blitz Seed of the Day").await?;
            }
            Self::TriforceBlitzProgressionSpoiler => ctx.say("!seed: The current settings for the mode").await?,
        }
        Ok(())
    }

    pub(crate) async fn parse_seed_command(&self, transaction: &mut Transaction<'_, Postgres>, global_state: &GlobalState, is_official: bool, spoiler_seed: bool, args: &[String]) -> Result<SeedCommandParseResult, Error> {
        let unlock_spoiler_log = self.unlock_spoiler_log(is_official, spoiler_seed);
        Ok(match self {
            | Self::CoOpS3
            | Self::CopaDoBrasil
            | Self::MixedPoolsS2
            | Self::MixedPoolsS3
            | Self::Pic7
            | Self::Sgl2023
            | Self::Sgl2024
            | Self::SongsOfHope
            | Self::StandardRuleset
            | Self::TriforceBlitzProgressionSpoiler
            | Self::WeTryToBeBetter
                => {
                    let (article, description) = match self.language() {
                        French => ("une", format!("seed")),
                        _ => ("a", format!("seed")),
                    };
                    if let Some(row) = sqlx::query!(r#"DELETE FROM prerolled_seeds WHERE ctid IN (SELECT ctid FROM prerolled_seeds WHERE goal_name = $1 LIMIT 1) RETURNING
                        goal_name,
                        file_stem,
                        locked_spoiler_log_path,
                        hash1 AS "hash1: HashIcon",
                        hash2 AS "hash2: HashIcon",
                        hash3 AS "hash3: HashIcon",
                        hash4 AS "hash4: HashIcon",
                        hash5 AS "hash5: HashIcon"
                    "#, self.as_str()).fetch_optional(&mut **transaction).await.to_racetime()? {
                        let _ = global_state.seed_cache_tx.send(());
                        SeedCommandParseResult::QueueExisting {
                            data: seed::Data::from_db(
                                None,
                                None,
                                None,
                                None,
                                row.file_stem,
                                row.locked_spoiler_log_path,
                                None,
                                None,
                                None,
                                row.hash1,
                                row.hash2,
                                row.hash3,
                                row.hash4,
                                row.hash5,
                            ),
                            language: self.language(),
                            article, description,
                        }
                    } else {
                        SeedCommandParseResult::Regular { settings: self.single_settings().expect("goal has no single settings"), unlock_spoiler_log, language: self.language(), article, description }
                    }
                }
            Self::Cc7 => {
                let settings = match args {
                    [] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "the preset is required" }),
                    [arg] if arg == "base" => HashMap::default(),
                    [arg] if arg == "random" => Draft {
                        high_seed: Id::dummy(), // Draft::complete_randomly doesn't check for active team
                        went_first: None,
                        skipped_bans: 0,
                        settings: HashMap::default(),
                    }.complete_randomly(draft::Kind::S7).await.to_racetime()?,
                    [arg] if arg == "draft" => return Ok(SeedCommandParseResult::StartDraft {
                        new_state: Draft {
                            high_seed: Id::dummy(), // racetime.gg bot doesn't check for active team
                            went_first: None,
                            skipped_bans: 0,
                            settings: HashMap::default(),
                        },
                        unlock_spoiler_log,
                    }),
                    [arg] if s::S7_SETTINGS.into_iter().any(|s::Setting { name, .. }| name == arg) => {
                        return Ok(SeedCommandParseResult::SendSettings { language: English, msg: "you need to pair each setting with a value.".into() })
                    }
                    [_] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "I don't recognize that preset" }),
                    args => {
                        let args = args.iter().map(|arg| arg.to_owned()).collect_vec();
                        let mut settings = HashMap::default();
                        let mut tuples = args.into_iter().tuples();
                        for (setting, value) in &mut tuples {
                            if let Some(s::Setting { other, .. }) = s::S7_SETTINGS.into_iter().find(|s::Setting { name, .. }| **name == setting) {
                                if value == "default" || other.iter().any(|(other, _, _)| value == **other) {
                                    settings.insert(Cow::Owned(setting), Cow::Owned(value));
                                } else {
                                    return Ok(SeedCommandParseResult::Error { language: English, msg: format!("I don't recognize that value for the {setting} setting. Use {}", iter::once("default").chain(other.iter().map(|&(other, _, _)| other)).join(" or ")).into() })
                                }
                            } else {
                                return Ok(SeedCommandParseResult::Error { language: English, msg: format!(
                                    "I don't recognize {}. Use one of the following:",
                                    if setting.chars().all(|c| c.is_ascii_alphanumeric()) { Cow::Owned(format!("the setting “{setting}”")) } else { Cow::Borrowed("one of those settings") },
                                ).into() })
                            }
                        }
                        if tuples.into_buffer().next().is_some() {
                            return Ok(SeedCommandParseResult::SendSettings { language: English, msg: "you need to pair each setting with a value.".into() })
                        } else {
                            settings
                        }
                    }
                };
                SeedCommandParseResult::Regular { settings: s::resolve_s7_draft_settings(&settings), unlock_spoiler_log, language: English, article: "a", description: format!("seed with {}", s::display_s7_draft_picks(&settings)) }
            }
            Self::MultiworldS3 => {
                let settings = match args {
                    [] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "the preset is required" }),
                    [arg] if arg == "base" => HashMap::default(),
                    [arg] if arg == "random" => Draft {
                        high_seed: Id::dummy(), // Draft::complete_randomly doesn't check for active team
                        went_first: None,
                        skipped_bans: 0,
                        settings: HashMap::default(),
                    }.complete_randomly(draft::Kind::MultiworldS3).await.to_racetime()?,
                    [arg] if arg == "draft" => return Ok(SeedCommandParseResult::StartDraft {
                        new_state: Draft {
                            high_seed: Id::dummy(), // racetime.gg bot doesn't check for active team
                            went_first: None,
                            skipped_bans: 0,
                            settings: HashMap::default(),
                        },
                        unlock_spoiler_log,
                    }),
                    [arg] if mw::S3_SETTINGS.into_iter().any(|mw::Setting { name, .. }| name == arg) => {
                        return Ok(SeedCommandParseResult::SendSettings { language: English, msg: "you need to pair each setting with a value.".into() })
                    }
                    [_] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "I don't recognize that preset" }),
                    args => {
                        let args = args.iter().map(|arg| arg.to_owned()).collect_vec();
                        let mut settings = HashMap::default();
                        let mut tuples = args.into_iter().tuples();
                        for (setting, value) in &mut tuples {
                            if let Some(mw::Setting { default, other, .. }) = mw::S3_SETTINGS.into_iter().find(|mw::Setting { name, .. }| **name == setting) {
                                if value == default || other.iter().any(|(other, _)| value == **other) {
                                    settings.insert(Cow::Owned(setting), Cow::Owned(value));
                                } else {
                                    return Ok(SeedCommandParseResult::Error { language: English, msg: format!("I don't recognize that value for the {setting} setting. Use {}", iter::once(default).chain(other.iter().map(|&(other, _)| other)).join(" or ")).into() })
                                }
                            } else {
                                return Ok(SeedCommandParseResult::Error { language: English, msg: format!(
                                    "I don't recognize {}. Use one of the following:",
                                    if setting.chars().all(|c| c.is_ascii_alphanumeric()) { Cow::Owned(format!("the setting “{setting}”")) } else { Cow::Borrowed("one of those settings") },
                                ).into() })
                            }
                        }
                        if tuples.into_buffer().next().is_some() {
                            return Ok(SeedCommandParseResult::SendSettings { language: English, msg: "you need to pair each setting with a value.".into() })
                        } else {
                            settings
                        }
                    }
                };
                SeedCommandParseResult::Regular { settings: mw::resolve_s3_draft_settings(&settings), unlock_spoiler_log, language: English, article: "a", description: format!("seed with {}", mw::display_s3_draft_picks(&settings)) }
            }
            Self::MultiworldS4 => {
                let settings = match args {
                    [] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "the preset is required" }),
                    [arg] if arg == "base" => HashMap::default(),
                    [arg] if arg == "random" => Draft {
                        high_seed: Id::dummy(), // Draft::complete_randomly doesn't check for active team
                        went_first: None,
                        skipped_bans: 0,
                        settings: HashMap::default(),
                    }.complete_randomly(draft::Kind::MultiworldS4).await.to_racetime()?,
                    [arg] if arg == "draft" => return Ok(SeedCommandParseResult::StartDraft {
                        new_state: Draft {
                            high_seed: Id::dummy(), // racetime.gg bot doesn't check for active team
                            went_first: None,
                            skipped_bans: 0,
                            settings: HashMap::default(),
                        },
                        unlock_spoiler_log,
                    }),
                    [arg] if mw::S4_SETTINGS.into_iter().any(|mw::Setting { name, .. }| name == arg) => {
                        return Ok(SeedCommandParseResult::SendSettings { language: English, msg: "you need to pair each setting with a value.".into() })
                    }
                    [_] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "I don't recognize that preset" }),
                    args => {
                        let args = args.iter().map(|arg| arg.to_owned()).collect_vec();
                        let mut settings = HashMap::default();
                        let mut tuples = args.into_iter().tuples();
                        for (setting, value) in &mut tuples {
                            if let Some(mw::Setting { default, other, .. }) = mw::S4_SETTINGS.into_iter().find(|mw::Setting { name, .. }| **name == setting) {
                                if value == default || other.iter().any(|(other, _)| value == **other) {
                                    settings.insert(Cow::Owned(setting), Cow::Owned(value));
                                } else {
                                    return Ok(SeedCommandParseResult::Error { language: English, msg: format!("I don't recognize that value for the {setting} setting. Use {}", iter::once(default).chain(other.iter().map(|&(other, _)| other)).join(" or ")).into() })
                                }
                            } else {
                                return Ok(SeedCommandParseResult::SendSettings { language: English, msg: format!(
                                    "I don't recognize {}. Use one of the following:",
                                    if setting.chars().all(|c| c.is_ascii_alphanumeric()) { Cow::Owned(format!("the setting “{setting}”")) } else { Cow::Borrowed("one of those settings") },
                                ).into() })
                            }
                        }
                        if tuples.into_buffer().next().is_some() {
                            return Ok(SeedCommandParseResult::SendSettings { language: English, msg: "you need to pair each setting with a value.".into() })
                        } else {
                            settings
                        }
                    }
                };
                SeedCommandParseResult::Regular { settings: mw::resolve_s4_draft_settings(&settings), unlock_spoiler_log, language: English, article: "a", description: format!("seed with {}", mw::display_s4_draft_picks(&settings)) }
            }
            Self::NineDaysOfSaws => match args {
                [] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "the preset is required" }),
                [arg] => if let Some((description, mut settings)) = match &**arg {
                    "day1" | "day9" => Some(("SAWS (S6)", ndos::s6_preset())),
                    "day2" | "day7" => Some(("SAWS (Beginner)", ndos::beginner_preset())),
                    "day3" => Some(("SAWS (Advanced)", ndos::advanced_preset())),
                    "day4" => Some(("SAWS (S5) + one bonk KO", {
                        let mut settings = ndos::s6_preset();
                        settings.insert(format!("dungeon_shortcuts_choice"), json!("off"));
                        settings.insert(format!("shuffle_child_spawn"), json!("balanced"));
                        settings.insert(format!("fix_broken_drops"), json!(false));
                        settings.insert(format!("item_pool_value"), json!("minimal"));
                        settings.insert(format!("blue_fire_arrows"), json!(false));
                        settings.insert(format!("junk_ice_traps"), json!("off"));
                        settings.insert(format!("deadly_bonks"), json!("ohko"));
                        settings.insert(format!("hint_dist_user"), json!({
                            "name":                  "tournament",
                            "gui_name":              "Tournament",
                            "description":           "Hint Distribution for the S5 Tournament. 5 Goal Hints, 3 Barren Hints, 5 Sometimes hints, 7 Always hints (including skull mask).",
                            "add_locations":         [
                                { "location": "Deku Theater Skull Mask", "types": ["always"] },
                            ],
                            "remove_locations":      [
                                {"location": "Ganons Castle Shadow Trial Golden Gauntlets Chest", "types": ["sometimes"] },
                            ],
                            "add_items":             [],
                            "remove_items":          [
                                { "item": "Zeldas Lullaby", "types": ["goal"] },
                            ],
                            "dungeons_woth_limit":   2,
                            "dungeons_barren_limit": 1,
                            "named_items_required":  true,
                            "vague_named_items":     false,
                            "use_default_goals":     true,
                            "distribution":          {
                                "trial":           {"order": 1, "weight": 0.0, "fixed":   0, "copies": 2},
                                "entrance_always": {"order": 2, "weight": 0.0, "fixed":   0, "copies": 2},
                                "always":          {"order": 3, "weight": 0.0, "fixed":   0, "copies": 2},
                                "goal":            {"order": 4, "weight": 0.0, "fixed":   5, "copies": 2},
                                "barren":          {"order": 5, "weight": 0.0, "fixed":   3, "copies": 2},
                                "entrance":        {"order": 6, "weight": 0.0, "fixed":   4, "copies": 2},
                                "sometimes":       {"order": 7, "weight": 0.0, "fixed": 100, "copies": 2},
                                "random":          {"order": 8, "weight": 9.0, "fixed":   0, "copies": 2},
                                "named-item":      {"order": 9, "weight": 0.0, "fixed":   0, "copies": 2},
                                "item":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                "song":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                "overworld":       {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                "dungeon":         {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                "junk":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                "woth":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                "dual_always":     {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                                "dual":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                            },
                        }));
                        settings
                    })),
                    "day5" => Some(("SAWS (Beginner) + mixed pools", {
                        let mut settings = ndos::beginner_preset();
                        settings.insert(format!("shuffle_interior_entrances"), json!("all"));
                        settings.insert(format!("shuffle_grotto_entrances"), json!(true));
                        settings.insert(format!("shuffle_dungeon_entrances"), json!("all"));
                        settings.insert(format!("shuffle_overworld_entrances"), json!(true));
                        settings.insert(format!("mix_entrance_pools"), json!([
                            "Interior",
                            "GrottoGrave",
                            "Dungeon",
                            "Overworld",
                        ]));
                        settings.insert(format!("shuffle_child_spawn"), json!("full"));
                        settings.insert(format!("shuffle_adult_spawn"), json!("full"));
                        settings.insert(format!("shuffle_gerudo_valley_river_exit"), json!("full"));
                        settings.insert(format!("owl_drops"), json!("full"));
                        settings.insert(format!("warp_songs"), json!("full"));
                        settings.insert(format!("blue_warps"), json!("dungeon"));
                        settings
                    })),
                    "day6" => Some(("SAWS (Beginner) 3-player multiworld", {
                        let mut settings = ndos::beginner_preset();
                        settings.insert(format!("world_count"), json!(3));
                        settings
                    })),
                    "day8" => Some(("SAWS (S6) + dungeon ER", {
                        let mut settings = ndos::s6_preset();
                        settings.insert(format!("shuffle_dungeon_entrances"), json!("simple"));
                        settings.insert(format!("blue_warps"), json!("dungeon"));
                        settings
                    })),
                    _ => None,
                } {
                    settings.insert(format!("user_message"), json!(format!("9 Days of SAWS: day {}", &arg[3..])));
                    SeedCommandParseResult::Regular { settings, unlock_spoiler_log, language: English, article: "a", description: format!("{description} seed") }
                } else {
                    SeedCommandParseResult::SendPresets { language: English, msg: "I don't recognize that preset" }
                },
                [..] => SeedCommandParseResult::SendPresets { language: English, msg: "I didn't quite understand that" },
            }
            Self::PicRs2 => SeedCommandParseResult::Rsl { preset: VersionedRslPreset::Fenhl {
                version: Some((Version::new(2, 3, 8), 10)),
                preset: RslDevFenhlPreset::Pictionary,
            }, world_count: 1, unlock_spoiler_log, language: English, article: "a", description: format!("seed") },
            Self::Rsl => {
                let (preset, world_count) = match args {
                    [] => (rsl::Preset::League, 1),
                    [preset] => if let Ok(preset) = preset.parse() {
                        if let rsl::Preset::Multiworld = preset {
                            return Ok(SeedCommandParseResult::Error { language: English, msg: "Missing world count (e.g. “!seed multiworld 2” for 2 worlds)".into() })
                        } else {
                            (preset, 1)
                        }
                    } else {
                        return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "I don't recognize that preset" })
                    },
                    [preset, world_count] => if let Ok(preset) = preset.parse() {
                        if let rsl::Preset::Multiworld = preset {
                            if let Ok(world_count) = world_count.parse() {
                                if world_count < 2 {
                                    return Ok(SeedCommandParseResult::Error { language: English, msg: "the world count must be a number between 2 and 15.".into() })
                                } else if world_count > 15 {
                                    return Ok(SeedCommandParseResult::Error { language: English, msg: "I can currently only roll seeds with up to 15 worlds. Please download the RSL script from https://github.com/matthewkirby/plando-random-settings to roll seeds for more than 15 players.".into() })
                                } else {
                                    (preset, world_count)
                                }
                            } else {
                                return Ok(SeedCommandParseResult::Error { language: English, msg: "the world count must be a number between 2 and 255.".into() })
                            }
                        } else {
                            return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "I didn't quite understand that" })
                        }
                    } else {
                        return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "I don't recognize that preset" })
                    },
                    [..] => return Ok(SeedCommandParseResult::SendPresets { language: English, msg: "I didn't quite understand that" }),
                };
                let (article, description) = match preset {
                    rsl::Preset::League => ("a", format!("Random Settings League seed")),
                    rsl::Preset::Beginner => ("a", format!("random settings Beginner seed")),
                    rsl::Preset::Intermediate => ("a", format!("random settings Intermediate seed")),
                    rsl::Preset::Ddr => ("a", format!("random settings DDR seed")),
                    rsl::Preset::CoOp => ("a", format!("random settings co-op seed")),
                    rsl::Preset::Multiworld => ("a", format!("random settings multiworld seed for {world_count} players")),
                };
                SeedCommandParseResult::Rsl { preset: VersionedRslPreset::Xopar { version: None, preset }, world_count, unlock_spoiler_log, language: English, article, description }
            }
            Self::TournoiFrancoS3 | Self::TournoiFrancoS4 => {
                let all_settings = match self {
                    Self::TournoiFrancoS3 => &fr::S3_SETTINGS[..],
                    Self::TournoiFrancoS4 => &fr::S4_SETTINGS[..],
                    _ => unreachable!(),
                };
                let mut args = args.to_owned();
                let mut mq_dungeons_count = None::<u8>;
                let mut hard_settings_ok = false;
                args.retain(|arg| if arg == "advanced" && !hard_settings_ok {
                    hard_settings_ok = true;
                    false
                } else if let (None, Some(mq)) = (mq_dungeons_count, regex_captures!("^([0-9]+)mq$"i, arg).and_then(|(_, mq)| mq.parse().ok())) {
                    mq_dungeons_count = Some(mq);
                    false
                } else {
                    true
                });
                let settings = match &*args {
                    [] => return Ok(SeedCommandParseResult::SendPresets { language: French, msg: "un preset doit être défini" }),
                    [arg] if arg == "base" => HashMap::default(),
                    [arg] if arg == "random" => Draft {
                        high_seed: Id::dummy(), // Draft::complete_randomly doesn't check for active team
                        went_first: None,
                        skipped_bans: 0,
                        settings: collect![as HashMap<_, _>:
                            Cow::Borrowed("hard_settings_ok") => Cow::Borrowed(if hard_settings_ok { "ok" } else { "no" }),
                            Cow::Borrowed("mq_ok") => Cow::Borrowed(if mq_dungeons_count.is_some() { "ok" } else { "no" }),
                            Cow::Borrowed("mq_dungeons_count") => Cow::Owned(mq_dungeons_count.unwrap_or_default().to_string()),
                        ],
                    }.complete_randomly(self.draft_kind().unwrap()).await.to_racetime()?,
                    [arg] if arg == "draft" => return Ok(SeedCommandParseResult::StartDraft {
                        new_state: Draft {
                            high_seed: Id::dummy(), // racetime.gg bot doesn't check for active team
                            went_first: None,
                            skipped_bans: 0,
                            settings: collect![as HashMap<_, _>:
                                Cow::Borrowed("hard_settings_ok") => Cow::Borrowed(if hard_settings_ok { "ok" } else { "no" }),
                                Cow::Borrowed("mq_ok") => Cow::Borrowed(if mq_dungeons_count.is_some() { "ok" } else { "no" }),
                                Cow::Borrowed("mq_dungeons_count") => Cow::Owned(mq_dungeons_count.unwrap_or_default().to_string()),
                            ],
                        },
                        unlock_spoiler_log,
                    }),
                    [arg] if all_settings.iter().any(|&fr::Setting { name, .. }| name == arg) => return Ok(SeedCommandParseResult::SendSettings { language: French, msg: "vous devez associer un setting avec une configuration.".into() }),
                    [_] => return Ok(SeedCommandParseResult::SendPresets { language: French, msg: "je ne reconnais pas ce preset" }),
                    args => {
                        let args = args.iter().map(|arg| arg.to_owned()).collect_vec();
                        let mut settings = HashMap::default();
                        let mut tuples = args.into_iter().tuples();
                        for (setting, value) in &mut tuples {
                            if let Some(&fr::Setting { default, other, .. }) = all_settings.iter().find(|&fr::Setting { name, .. }| **name == setting) {
                                if setting == "dungeon-er" && value == "mixed" {
                                    settings.insert(Cow::Borrowed("dungeon-er"), Cow::Borrowed("on"));
                                    settings.insert(Cow::Borrowed("mixed-dungeons"), Cow::Borrowed("mixed"));
                                } else if value == default || other.iter().any(|(other, _, _)| value == **other) {
                                    settings.insert(Cow::Owned(setting), Cow::Owned(value));
                                } else {
                                    return Ok(SeedCommandParseResult::Error { language: French, msg: format!("je ne reconnais pas cette configuration pour {setting}. Utilisez {}", iter::once(default).chain(other.iter().map(|&(other, _, _)| other)).join(" ou ")).into() })
                                }
                            } else {
                                return Ok(SeedCommandParseResult::SendSettings { language: French, msg: format!("je ne reconnais pas {}. Utilisez cette liste :",
                                    if setting.chars().all(|c| c.is_ascii_alphanumeric()) { Cow::Owned(format!("le setting « {setting} »")) } else { Cow::Borrowed("un des settings") },
                                ).into() })
                            }
                        }
                        if tuples.into_buffer().next().is_some() {
                            return Ok(SeedCommandParseResult::SendSettings { language: French, msg: "vous devez associer un setting avec une configuration.".into() })
                        } else {
                            settings.insert(Cow::Borrowed("mq_dungeons_count"), Cow::Owned(mq_dungeons_count.unwrap_or_default().to_string()));
                            settings
                        }
                    }
                };
                SeedCommandParseResult::Regular {
                    settings: match self {
                        Self::TournoiFrancoS3 => fr::resolve_s3_draft_settings(&settings),
                        Self::TournoiFrancoS4 => fr::resolve_s4_draft_settings(&settings),
                        _ => unreachable!(),
                    },
                    unlock_spoiler_log,
                    language: self.language(),
                    article: if let French = self.language() { "une" } else { "a" },
                    description: format!("seed {} {}", if let French = self.language() { "avec" } else { "with" }, fr::display_draft_picks(self.language(), all_settings, &settings)),
                }
            }
            Self::TriforceBlitz => match args {
                [] => SeedCommandParseResult::SendPresets { language: English, msg: "the preset is required" },
                [arg] if arg == "daily" => {
                    let (date, ordinal, file_hash) = {
                        let response = global_state.http_client
                            .get("https://www.triforceblitz.com/seed/daily/all")
                            .send().await?
                            .detailed_error_for_status().await.to_racetime()?;
                        let response_body = response.text().await?;
                        let latest = kuchiki::parse_html().one(response_body)
                            .select_first("main > section > div > div").map_err(|()| RollError::TfbHtml).to_racetime()?;
                        let latest = latest.as_node();
                        let a = latest.select_first("a").map_err(|()| RollError::TfbHtml).to_racetime()?;
                        let a_attrs = a.attributes.borrow();
                        let href = a_attrs.get("href").ok_or(RollError::TfbHtml).to_racetime()?;
                        let (_, ordinal) = regex_captures!("^/seed/daily/([0-9]+)$", href).ok_or(RollError::TfbHtml).to_racetime()?;
                        let ordinal = ordinal.parse().to_racetime()?;
                        let date = NaiveDate::parse_from_str(&a.text_contents(), "%B %-d, %Y").to_racetime()?;
                        let file_hash = latest.select_first(".hash-icons").map_err(|()| RollError::TfbHtml).to_racetime()?
                            .as_node()
                            .children()
                            .filter_map(NodeRef::into_element_ref)
                            .filter_map(|elt| elt.attributes.borrow().get("title").and_then(|title| title.parse().ok()))
                            .collect_vec()
                            .try_into().map_err(|_| RollError::TfbHtml).to_racetime()?;
                        (date, ordinal, file_hash)
                    };
                    SeedCommandParseResult::QueueExisting { data: seed::Data {
                        file_hash: Some(file_hash),
                        files: Some(seed::Files::TfbSotd { date, ordinal }),
                    }, language: English, article: "the", description: format!("Triforce Blitz seed of the day") }
                }
                [arg] if arg == "jr" => SeedCommandParseResult::Tfb { version: "v7.1.143-blitz-0.43", unlock_spoiler_log, language: English, article: "a", description: format!("Triforce Blitz: Jabu's Revenge seed") },
                [arg] if arg == "s2" => SeedCommandParseResult::Tfb { version: "v7.1.3-blitz-0.42", unlock_spoiler_log, language: English, article: "a", description: format!("Triforce Blitz S2 seed") },
                [arg] if arg == "s3" => SeedCommandParseResult::Tfb { version: "LATEST", unlock_spoiler_log, language: English, article: "a", description: format!("Triforce Blitz S3 seed") },
                [..] => SeedCommandParseResult::SendPresets { language: English, msg: "I didn't quite understand that" },
            },
        })
    }
}

pub(crate) enum SeedCommandParseResult {
    Regular {
        settings: serde_json::Map<String, Json>,
        unlock_spoiler_log: UnlockSpoilerLog,
        language: Language,
        article: &'static str,
        description: String,
    },
    Rsl {
        preset: VersionedRslPreset,
        world_count: u8,
        unlock_spoiler_log: UnlockSpoilerLog,
        language: Language,
        article: &'static str,
        description: String,
    },
    Tfb {
        version: &'static str,
        unlock_spoiler_log: UnlockSpoilerLog,
        language: Language,
        article: &'static str,
        description: String,
    },
    QueueExisting {
        data: seed::Data,
        language: Language,
        article: &'static str,
        description: String,
    },
    SendPresets {
        language: Language,
        msg: &'static str,
    },
    SendSettings {
        language: Language,
        msg: Cow<'static, str>,
    },
    StartDraft {
        new_state: Draft,
        unlock_spoiler_log: UnlockSpoilerLog,
    },
    Error {
        language: Language,
        msg: Cow<'static, str>,
    },
}

impl FromStr for Goal {
    type Err = GoalFromStrError;

    fn from_str(s: &str) -> Result<Self, GoalFromStrError> {
        all::<Self>().find(|goal| goal.as_str() == s).ok_or(GoalFromStrError)
    }
}

#[derive(Default)]
pub(crate) struct CleanShutdown {
    pub(crate) requested: bool,
    pub(crate) block_new: bool,
    pub(crate) open_rooms: HashSet<String>,
    pub(crate) notifier: Arc<Notify>,
}

impl CleanShutdown {
    fn should_handle_new(&self) -> bool {
        !self.requested || !self.block_new && !self.open_rooms.is_empty()
    }
}

pub(crate) struct GlobalState {
    /// Locked while event rooms are being created. Wait with handling new rooms while it's held.
    new_room_lock: Arc<Mutex<()>>,
    pub(crate) env: Environment,
    host_info: racetime::HostInfo,
    racetime_config: ConfigRaceTime,
    extra_room_tx: Arc<RwLock<mpsc::Sender<String>>>,
    pub(crate) db_pool: PgPool,
    pub(crate) http_client: reqwest::Client,
    #[allow(unused)] //TODO use for set reporting
    startgg_token: String,
    ootr_api_client: OotrApiClient,
    pub(crate) discord_ctx: RwFuture<DiscordCtx>,
    clean_shutdown: Arc<Mutex<CleanShutdown>>,
    seed_cache_tx: watch::Sender<()>,
}

impl GlobalState {
    pub(crate) async fn new(
        new_room_lock: Arc<Mutex<()>>,
        racetime_config: ConfigRaceTime,
        extra_room_tx: Arc<RwLock<mpsc::Sender<String>>>,
        db_pool: PgPool,
        http_client: reqwest::Client,
        ootr_api_key: String,
        ootr_api_key_encryption: String,
        startgg_token: String,
        env: Environment,
        discord_ctx: RwFuture<DiscordCtx>,
        clean_shutdown: Arc<Mutex<CleanShutdown>>,
        seed_cache_tx: watch::Sender<()>,
    ) -> Self {
        Self {
            host_info: racetime::HostInfo {
                hostname: Cow::Borrowed(env.racetime_host()),
                ..racetime::HostInfo::default()
            },
            ootr_api_client: OotrApiClient::new(http_client.clone(), ootr_api_key, ootr_api_key_encryption),
            new_room_lock, env, racetime_config, extra_room_tx, db_pool, http_client, startgg_token, discord_ctx, clean_shutdown, seed_cache_tx,
        }
    }

    pub(crate) fn roll_seed(self: Arc<Self>, preroll: PrerollMode, allow_web: bool, delay_until: Option<DateTime<Utc>>, version: VersionedBranch, settings: serde_json::Map<String, Json>, unlock_spoiler_log: UnlockSpoilerLog) -> mpsc::Receiver<SeedRollUpdate> {
        let world_count = settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64").try_into().expect("too many worlds"));
        let (update_tx, update_rx) = mpsc::channel(128);
        tokio::spawn(async move {
            if_chain! {
                if allow_web;
                if let Some(web_version) = self.ootr_api_client.can_roll_on_web(None, &version, world_count, unlock_spoiler_log).await;
                then {
                    // ootrandomizer.com seed IDs are sequential, making it easy to find a seed if you know when it was rolled.
                    // This is especially true for open races, whose rooms are opened an entire hour before start.
                    // To make this a bit more difficult, we delay the start of seed rolling depending on the goal.
                    match preroll {
                        // The type of seed being rolled is unlikely to require a long time or multiple attempts to generate,
                        // so we avoid the issue with sequential IDs by simply not rolling ahead of time.
                        PrerollMode::None => if let Some(sleep_duration) = delay_until.and_then(|delay_until| (delay_until - Utc::now()).to_std().ok()) {
                            sleep(sleep_duration).await;
                        },
                        // Middle-ground option. Start rolling the seed at a random point between 20 and 15 minutes before start.
                        PrerollMode::Short => if let Some(max_sleep_duration) = delay_until.and_then(|delay_until| (delay_until - Utc::now()).to_std().ok()) {
                            let min_sleep_duration = max_sleep_duration.saturating_sub(Duration::from_secs(5 * 60));
                            let sleep_duration = thread_rng().gen_range(min_sleep_duration..max_sleep_duration);
                            sleep(sleep_duration).await;
                        },
                        // The type of seed being rolled is fairly likely to require a long time and/or multiple attempts to generate.
                        // Start rolling the seed at a random point between the room being opened and 30 minutes before start.
                        PrerollMode::Medium => if let Some(max_sleep_duration) = delay_until.and_then(|delay_until| (delay_until - TimeDelta::minutes(15) - Utc::now()).to_std().ok()) {
                            let sleep_duration = thread_rng().gen_range(Duration::default()..max_sleep_duration);
                            sleep(sleep_duration).await;
                        },
                        // The type of seed being rolled is extremely likely to require a very long time and/or a large number of attempts to generate.
                        // Start rolling the seed immediately upon the room being opened.
                        PrerollMode::Long => {}
                    }
                    match self.ootr_api_client.roll_seed_web(update_tx.clone(), delay_until, web_version, false, unlock_spoiler_log, settings).await {
                        Ok((id, gen_time, file_hash, file_stem)) => update_tx.send(SeedRollUpdate::Done {
                            seed: seed::Data {
                                file_hash: Some(file_hash),
                                files: Some(seed::Files::OotrWeb {
                                    file_stem: Cow::Owned(file_stem),
                                    id, gen_time,
                                }),
                            },
                            rsl_preset: None,
                            unlock_spoiler_log,
                        }).await?,
                        Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?, //TODO fall back to rolling locally for network errors
                    }
                } else {
                    update_tx.send(SeedRollUpdate::Started).await?;
                    match roll_seed_locally(delay_until, version, unlock_spoiler_log, settings).await {
                        Ok((patch_filename, spoiler_log_path)) => update_tx.send(match spoiler_log_path.map(|spoiler_log_path| spoiler_log_path.into_os_string().into_string()).transpose() {
                            Ok(locked_spoiler_log_path) => match regex_captures!(r"^(.+)\.zpfz?$", &patch_filename) {
                                Some((_, file_stem)) => SeedRollUpdate::Done {
                                    seed: seed::Data {
                                        file_hash: None,
                                        files: Some(seed::Files::MidosHouse {
                                            file_stem: Cow::Owned(file_stem.to_owned()),
                                            locked_spoiler_log_path,
                                        }),
                                    },
                                    rsl_preset: None,
                                    unlock_spoiler_log,
                                },
                                None => SeedRollUpdate::Error(RollError::PatchPath),
                            },
                            Err(e) => SeedRollUpdate::Error(e.into())
                        }).await?,
                        Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?,
                    }
                }
            }
            Ok::<_, mpsc::error::SendError<_>>(())
        });
        update_rx
    }

    pub(crate) fn roll_rsl_seed(self: Arc<Self>, delay_until: Option<DateTime<Utc>>, preset: VersionedRslPreset, world_count: u8, unlock_spoiler_log: UnlockSpoilerLog) -> mpsc::Receiver<SeedRollUpdate> {
        let (update_tx, update_rx) = mpsc::channel(128);
        let update_tx2 = update_tx.clone();
        tokio::spawn(async move {
            let rsl_script_path = preset.script_path()?; //TODO automatically clone if not present and ensure base rom is in place (need to create data directory)
            // update the RSL script
            if !preset.is_version_locked() {
                let repo = Repository::open(&rsl_script_path)?;
                let mut origin = repo.find_remote("origin")?;
                let branch_name = match preset {
                    VersionedRslPreset::Xopar { .. } => "release",
                    VersionedRslPreset::Fenhl { .. } => "dev-fenhl",
                };
                origin.fetch(&[branch_name], None, None)?;
                repo.reset(&repo.find_branch(&format!("origin/{branch_name}"), BranchType::Remote)?.into_reference().peel_to_commit()?.into_object(), ResetType::Hard, None)?;
            }
            // check required randomizer version
            let local_version_path = rsl_script_path.join("rslversion.py");
            let local_version_file = BufReader::new(File::open(&local_version_path).await?);
            let mut lines = local_version_file.lines();
            let version = loop {
                let line = lines.next_line().await.at(&local_version_path)?.ok_or(RollError::RslVersion)?;
                if let Some((_, local_version)) = regex_captures!("^randomizer_version = '(.+)'$", &line) {
                    break local_version.parse::<rando::Version>()?
                }
            };
            let web_version = self.ootr_api_client.can_roll_on_web(Some(&preset), &VersionedBranch::Pinned(version.clone()), world_count, unlock_spoiler_log).await;
            // run the RSL script
            let _ = update_tx.send(SeedRollUpdate::Started).await;
            let outer_tries = if web_version.is_some() { 5 } else { 1 }; // when generating locally, retries are already handled by the RSL script
            let mut last_error = None;
            for attempt in 0.. {
                if attempt >= outer_tries && delay_until.map_or(true, |delay_until| Utc::now() >= delay_until) {
                    return Err(RollError::Retries {
                        num_retries: 3 * attempt,
                        last_error,
                    })
                }
                let mut rsl_cmd = Command::new(PYTHON);
                rsl_cmd.arg("RandomSettingsGenerator.py");
                rsl_cmd.arg("--no_log_errors");
                if !matches!(preset, VersionedRslPreset::Xopar { preset: rsl::Preset::League, .. }) {
                    rsl_cmd.arg(format!(
                        "--override={}{}_override.json",
                        if preset.base_version().map_or(true, |version| *version >= Version::new(2, 3, 9)) { "weights/" } else { "" },
                        preset.name(),
                    ));
                }
                if world_count > 1 {
                    rsl_cmd.arg(format!("--worldcount={world_count}"));
                }
                if web_version.is_some() {
                    rsl_cmd.arg("--no_seed");
                }
                let output = rsl_cmd.current_dir(&rsl_script_path).output().await.at_command("RandomSettingsGenerator.py")?;
                match output.status.code() {
                    Some(0) => {}
                    Some(2) => {
                        last_error = Some(String::from_utf8_lossy(&output.stderr).into_owned());
                        continue
                    }
                    _ => return Err(RollError::Wheel(wheel::Error::CommandExit { name: Cow::Borrowed("RandomSettingsGenerator.py"), output })),
                }
                if let Some(web_version) = web_version.clone() {
                    #[derive(Deserialize)]
                    struct Plando {
                        settings: serde_json::Map<String, Json>,
                    }

                    let plando_filename = BufRead::lines(&*output.stdout)
                        .filter_map_ok(|line| Some(regex_captures!("^Plando File: (.+)$", &line)?.1.to_owned()))
                        .next().ok_or(RollError::RslScriptOutput)?.at_command("RandomSettingsGenerator.py")?;
                    let plando_path = rsl_script_path.join("data").join(plando_filename);
                    let plando_file = fs::read_to_string(&plando_path).await?;
                    let settings = serde_json::from_str::<Plando>(&plando_file)?.settings;
                    fs::remove_file(plando_path).await?;
                    if let Some(max_sleep_duration) = delay_until.and_then(|delay_until| (delay_until - TimeDelta::minutes(15) - Utc::now()).to_std().ok()) {
                        // ootrandomizer.com seed IDs are sequential, making it easy to find a seed if you know when it was rolled.
                        // This is especially true for open races, whose rooms are opened an entire hour before start.
                        // To make this a bit more difficult, we start rolling the seed at a random point between the room being opened and 30 minutes before start.
                        let sleep_duration = thread_rng().gen_range(Duration::default()..max_sleep_duration);
                        sleep(sleep_duration).await;
                    }
                    let (seed_id, gen_time, file_hash, file_stem) = match self.ootr_api_client.roll_seed_web(update_tx.clone(), None /* always limit to 3 tries per settings */, web_version, true, unlock_spoiler_log, settings).await {
                        Ok(data) => data,
                        Err(RollError::Retries { .. }) => continue,
                        Err(e) => return Err(e), //TODO fall back to rolling locally for network errors
                    };
                    let _ = update_tx.send(SeedRollUpdate::Done {
                        seed: seed::Data {
                            file_hash: Some(file_hash),
                            files: Some(seed::Files::OotrWeb {
                                id: seed_id,
                                file_stem: Cow::Owned(file_stem),
                                gen_time,
                            }),
                        },
                        rsl_preset: if let VersionedRslPreset::Xopar { preset, .. } = preset { Some(preset) } else { None },
                        unlock_spoiler_log,
                    }).await;
                    return Ok(())
                } else {
                    let patch_filename = BufRead::lines(&*output.stdout)
                        .filter_map_ok(|line| Some(regex_captures!("^Creating Patch File: (.+)$", &line)?.1.to_owned()))
                        .next().ok_or(RollError::RslScriptOutput)?.at_command("RandomSettingsGenerator.py")?;
                    let patch_path = rsl_script_path.join("patches").join(&patch_filename);
                    let spoiler_log_filename = BufRead::lines(&*output.stdout)
                        .filter_map_ok(|line| Some(regex_captures!("^Created spoiler log at: (.+)$", &line)?.1.to_owned()))
                        .next().ok_or(RollError::RslScriptOutput)?.at_command("RandomSettingsGenerator.py")?;
                    let spoiler_log_path = rsl_script_path.join("patches").join(spoiler_log_filename);
                    let (_, file_stem) = regex_captures!(r"^(.+)\.zpfz?$", &patch_filename).ok_or(RollError::RslScriptOutput)?;
                    for extra_output_filename in [format!("{file_stem}_Cosmetics.json"), format!("{file_stem}_Distribution.json")] {
                        fs::remove_file(rsl_script_path.join("patches").join(extra_output_filename)).await.missing_ok()?;
                    }
                    fs::rename(patch_path, Path::new(seed::DIR).join(&patch_filename)).await?;
                    let _ = update_tx.send(match regex_captures!(r"^(.+)\.zpfz?$", &patch_filename) {
                        Some((_, file_stem)) => SeedRollUpdate::Done {
                            seed: seed::Data {
                                file_hash: None,
                                files: Some(seed::Files::MidosHouse {
                                    file_stem: Cow::Owned(file_stem.to_owned()),
                                    locked_spoiler_log_path: Some(spoiler_log_path.into_os_string().into_string()?),
                                }),
                            },
                            rsl_preset: if let VersionedRslPreset::Xopar { preset, .. } = preset { Some(preset) } else { None },
                            unlock_spoiler_log,
                        },
                        None => SeedRollUpdate::Error(RollError::PatchPath),
                    }).await;
                    return Ok(())
                }
            }
            Ok(())
        }.then(|res| async move {
            match res {
                Ok(()) => {}
                Err(e) => { let _ = update_tx2.send(SeedRollUpdate::Error(e)).await; }
            }
        }));
        update_rx
    }

    pub(crate) fn roll_tfb_seed(self: Arc<Self>, delay_until: Option<DateTime<Utc>>, version: &'static str, room: Option<String>, unlock_spoiler_log: UnlockSpoilerLog) -> mpsc::Receiver<SeedRollUpdate> {
        let (update_tx, update_rx) = mpsc::channel(128);
        let update_tx2 = update_tx.clone();
        tokio::spawn(async move {
            if let Some(max_sleep_duration) = delay_until.and_then(|delay_until| (delay_until - TimeDelta::minutes(15) - Utc::now()).to_std().ok()) {
                // triforceblitz.com has a list of recently rolled seeds, making it easy to find a seed if you know when it was rolled.
                // This is especially true for open races, whose rooms are opened an entire hour before start.
                // To make this a bit more difficult, we start rolling the seed at a random point between the room being opened and 30 minutes before start.
                let sleep_duration = thread_rng().gen_range(Duration::default()..max_sleep_duration);
                sleep(sleep_duration).await;
            }
            let _ = update_tx.send(SeedRollUpdate::Started).await;
            let form_data = match unlock_spoiler_log {
                UnlockSpoilerLog::Now => vec![
                    ("unlockSetting", "ALWAYS"),
                    ("version", version),
                ],
                UnlockSpoilerLog::Progression => panic!("progression spoiler mode not supported by triforceblitz.com"),
                UnlockSpoilerLog::After => if let Some(ref room) = room {
                    vec![
                        ("unlockSetting", "RACETIME"),
                        ("racetimeRoom", room),
                        ("version", version),
                    ]
                } else {
                    panic!("cannot set a Triforce Blitz seed to unlock after the race without a race room")
                },
                UnlockSpoilerLog::Never => vec![
                    ("unlockSetting", "NEVER"),
                    ("version", version),
                ],
            };
            let response = self.http_client
                .post("https://www.triforceblitz.com/generator")
                .form(&form_data)
                .timeout(Duration::from_secs(5 * 60))
                .send().await?
                .detailed_error_for_status().await?;
            let uuid = tfb::parse_seed_url(response.url()).ok_or(RollError::TfbUrl)?;
            let response_body = response.text().await?;
            let file_hash = kuchiki::parse_html().one(response_body)
                .select_first(".hash-icons").map_err(|()| RollError::TfbHtml)?
                .as_node()
                .children()
                .filter_map(NodeRef::into_element_ref)
                .filter_map(|elt| elt.attributes.borrow().get("title").and_then(|title| title.parse().ok()))
                .collect_vec();
            let _ = update_tx.send(SeedRollUpdate::Done {
                seed: seed::Data {
                    file_hash: Some(file_hash.try_into().map_err(|_| RollError::TfbHash)?),
                    files: Some(seed::Files::TriforceBlitz { uuid }),
                },
                rsl_preset: None,
                unlock_spoiler_log,
            }).await;
            Ok(())
        }.then(|res| async move {
            match res {
                Ok(()) => {}
                Err(e) => { let _ = update_tx2.send(SeedRollUpdate::Error(e)).await; }
            }
        }));
        update_rx
    }
}

async fn roll_seed_locally(delay_until: Option<DateTime<Utc>>, version: VersionedBranch, unlock_spoiler_log: UnlockSpoilerLog, mut settings: serde_json::Map<String, Json>) -> Result<(String, Option<PathBuf>), RollError> {
    let rando_path = match version {
        VersionedBranch::Pinned(version) => {
            version.clone_repo().await?;
            version.dir()?
        }
        VersionedBranch::Latest(branch) => {
            branch.clone_repo(true).await?;
            branch.dir(true)?
        }
        VersionedBranch::Custom { github_username, branch } => {
            let parent = {
                #[cfg(unix)] { Path::new("/opt/git/github.com").join(github_username).join("OoT-Randomizer").join("branch") }
                #[cfg(windows)] { UserDirs::new().ok_or(RollError::UserDirs)?.home_dir().join("git").join("github.com").join(github_username).join("OoT-Randomizer").join("branch") }
            };
            let dir = parent.join(branch);
            if dir.exists() {
                //TODO hard reset to remote instead?
                //TODO use git2 or gix instead?
                Command::new("git").arg("pull").current_dir(&dir).check("git").await?;
            } else {
                fs::create_dir_all(&parent).await?;
                let mut command = Command::new("git"); //TODO use git2 or gix instead? (git2 doesn't support shallow clones, gix is very low level)
                command.arg("clone");
                command.arg(format!("https://github.com/{github_username}/OoT-Randomizer.git"));
                command.arg(format!("--branch={branch}"));
                command.arg(branch);
                command.current_dir(parent);
                command.check("git").await?;
            }
            dir
        }
    };
    #[cfg(unix)] {
        settings.insert(format!("rom"), json!(BaseDirectories::new()?.find_data_file(Path::new("midos-house").join("oot-ntscu-1.0.z64")).ok_or(RollError::RomPath)?));
        if settings.get("language").and_then(|language| language.as_str()).map_or(false, |language| matches!(language, "french" | "german")) {
            settings.insert(format!("pal_rom"), json!(BaseDirectories::new()?.find_data_file(Path::new("midos-house").join("oot-pal-1.0.z64")).ok_or(RollError::RomPath)?));
        }
    }
    settings.insert(format!("create_patch_file"), json!(true));
    settings.insert(format!("create_compressed_rom"), json!(false));
    if settings.insert(format!("create_spoiler"), json!(match unlock_spoiler_log {
        UnlockSpoilerLog::Now | UnlockSpoilerLog::Progression | UnlockSpoilerLog::After => true,
        UnlockSpoilerLog::Never => false,
    })).is_some() {
        eprintln!("warning: overriding create_spoiler setting");
        wheel::night_report("/net/midoshouse/error", Some("warning: overriding create_spoiler setting")).await?;
    };
    let mut last_error = None;
    for attempt in 0.. {
        if attempt >= 3 && delay_until.map_or(true, |delay_until| Utc::now() >= delay_until) {
            return Err(RollError::Retries {
                num_retries: attempt,
                last_error,
            })
        }
        let mut rando_process = Command::new(PYTHON).arg("OoTRandomizer.py").arg("--no_log").arg("--settings=-").current_dir(&rando_path).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().at_command(PYTHON)?;
        rando_process.stdin.as_mut().expect("piped stdin missing").write_all(&serde_json::to_vec(&settings)?).await.at_command(PYTHON)?;
        let output = rando_process.wait_with_output().await.at_command(PYTHON)?;
        let stderr = if output.status.success() { BufRead::lines(&*output.stderr).try_collect::<_, Vec<_>, _>().at_command(PYTHON)? } else {
            last_error = Some(String::from_utf8_lossy(&output.stderr).into_owned());
            continue
        };
        let world_count = settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64").try_into().expect("too many worlds"));
        let patch_path_prefix = if world_count > 1 { "Created patch file archive at: " } else { "Creating Patch File: " };
        let patch_path = rando_path.join("Output").join(stderr.iter().rev().find_map(|line| line.strip_prefix(patch_path_prefix)).ok_or(RollError::PatchPath)?);
        let spoiler_log_path = match unlock_spoiler_log {
            UnlockSpoilerLog::Now | UnlockSpoilerLog::Progression | UnlockSpoilerLog::After => Some(rando_path.join("Output").join(stderr.iter().rev().find_map(|line| line.strip_prefix("Created spoiler log at: ")).ok_or_else(|| RollError::SpoilerLogPath {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })?).to_owned()),
            UnlockSpoilerLog::Never => None,
        };
        let patch_filename = patch_path.file_name().expect("patch file path with no file name");
        fs::rename(&patch_path, Path::new(seed::DIR).join(patch_filename)).await?;
        return Ok((
            patch_filename.to_str().expect("non-UTF-8 patch filename").to_owned(),
            spoiler_log_path,
        ))
    }
    unreachable!()
}

#[derive(Debug, thiserror::Error)]
#[cfg_attr(unix, derive(Protocol))]
#[cfg_attr(unix, async_proto(via = (String, String)))]
pub(crate) enum RollError {
    #[error(transparent)] Clone(#[from] rando::CloneError),
    #[error(transparent)] Dir(#[from] rando::DirError),
    #[error(transparent)] Git(#[from] git2::Error),
    #[error(transparent)] Header(#[from] reqwest::header::ToStrError),
    #[error(transparent)] Json(#[from] serde_json::Error),
    #[error(transparent)] RaceTime(#[from] Error),
    #[error(transparent)] RandoVersion(#[from] rando::VersionParseError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[cfg(unix)] #[error(transparent)] Xdg(#[from] xdg::BaseDirectoriesError),
    #[error("{display}")]
    Cloned {
        debug: String,
        display: String,
    },
    #[error("there is nothing waiting for this seed anymore")]
    ChannelClosed,
    #[cfg(unix)]
    #[error("randomizer settings must be a JSON object")]
    NonObjectSettings,
    #[error("non-UTF-8 filename")]
    OsString(std::ffi::OsString),
    #[error("randomizer did not report patch location")]
    PatchPath,
    #[error("attempted to roll a random settings seed on web, but this branch isn't available with hidden settings on web")]
    RandomSettingsWeb,
    #[cfg(unix)]
    #[error("base rom not found")]
    RomPath,
    #[cfg(unix)]
    #[error("RSL script not found")]
    RslPath,
    #[error("max retries exceeded")]
    Retries {
        num_retries: u8,
        last_error: Option<String>,
    },
    #[error("failed to parse random settings script output")]
    RslScriptOutput,
    #[error("failed to parse randomizer version from RSL script")]
    RslVersion,
    #[error("randomizer did not report spoiler log location")]
    SpoilerLogPath {
        stdout: String,
        stderr: String,
    },
    #[error("didn't find 5 hash icons on Triforce Blitz seed page")]
    TfbHash,
    #[error("failed to parse Triforce Blitz seed page")]
    TfbHtml,
    #[error("Triforce Blitz website returned unexpected URL")]
    TfbUrl,
    #[error("seed status API endpoint returned unknown value {0}")]
    UnespectedSeedStatus(u8),
    #[cfg(windows)]
    #[error("failed to access user directories")]
    UserDirs,
}

impl From<mpsc::error::SendError<SeedRollUpdate>> for RollError {
    fn from(_: mpsc::error::SendError<SeedRollUpdate>) -> Self {
        Self::ChannelClosed
    }
}

impl From<std::ffi::OsString> for RollError {
    fn from(value: std::ffi::OsString) -> Self {
        Self::OsString(value)
    }
}

impl From<(String, String)> for RollError {
    fn from((debug, display): (String, String)) -> Self {
        Self::Cloned { debug, display }
    }
}

impl<'a> From<&'a RollError> for (String, String) {
    fn from(e: &RollError) -> Self {
        (e.to_string(), format!("{e:?}"))
    }
}

#[derive(Debug)]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) enum SeedRollUpdate {
    /// The seed rollers are busy and the seed has been queued.
    Queued(u64),
    /// A seed in front of us is done and we've moved to a new position in the queue.
    MovedForward(u64),
    /// We've cleared the queue and are now being rolled.
    Started,
    /// The seed has been rolled successfully.
    Done {
        seed: seed::Data,
        rsl_preset: Option<rsl::Preset>,
        unlock_spoiler_log: UnlockSpoilerLog,
    },
    /// Seed rolling failed.
    Error(RollError),
    #[cfg(unix)]
    /// A custom message.
    Message(String),
}

impl SeedRollUpdate {
    async fn handle(self, db_pool: &PgPool, ctx: &RaceContext<GlobalState>, state: &ArcRwLock<RaceState>, official_data: Option<&OfficialRaceData>, language: Language, article: &'static str, description: &str) -> Result<(), Error> {
        match self {
            Self::Queued(0) => ctx.say("I'm already rolling other multiworld seeds so your seed has been queued. It is at the front of the queue so it will be rolled next.").await?,
            Self::Queued(1) => ctx.say("I'm already rolling other multiworld seeds so your seed has been queued. There is 1 seed in front of it in the queue.").await?,
            Self::Queued(pos) => ctx.say(format!("I'm already rolling other multiworld seeds so your seed has been queued. There are {pos} seeds in front of it in the queue.")).await?,
            Self::MovedForward(0) => ctx.say("The queue has moved and your seed is now at the front so it will be rolled next.").await?,
            Self::MovedForward(1) => ctx.say("The queue has moved and there is only 1 more seed in front of yours.").await?,
            Self::MovedForward(pos) => ctx.say(format!("The queue has moved and there are now {pos} seeds in front of yours.")).await?,
            Self::Started => ctx.say(if let French = language {
                format!("Génération d'{article} {description}…")
            } else {
                format!("Rolling {article} {description}…")
            }).await?,
            Self::Done { mut seed, rsl_preset, unlock_spoiler_log } => {
                if let Some(seed::Files::MidosHouse { ref file_stem, ref mut locked_spoiler_log_path }) = seed.files {
                    if unlock_spoiler_log == UnlockSpoilerLog::Now && locked_spoiler_log_path.is_some() {
                        fs::rename(locked_spoiler_log_path.as_ref().unwrap(), Path::new(seed::DIR).join(format!("{file_stem}_Spoiler.json"))).await.to_racetime()?;
                        *locked_spoiler_log_path = None;
                    }
                }
                let extra = seed.extra(Utc::now()).await.to_racetime()?;
                if let Some(OfficialRaceData { cal_event, .. }) = official_data {
                    match seed.files.as_ref().expect("received seed with no files") {
                        seed::Files::MidosHouse { file_stem, .. } => {
                            sqlx::query!(
                                "UPDATE races SET file_stem = $1 WHERE id = $2",
                                file_stem, cal_event.race.id as _,
                            ).execute(db_pool).await.to_racetime()?;
                        }
                        seed::Files::OotrWeb { id, gen_time, file_stem } => {
                            sqlx::query!(
                                "UPDATE races SET web_id = $1, web_gen_time = $2, file_stem = $3 WHERE id = $4",
                                *id as i64, gen_time, file_stem, cal_event.race.id as _,
                            ).execute(db_pool).await.to_racetime()?;
                        }
                        seed::Files::TriforceBlitz { uuid } => {
                            sqlx::query!(
                                "UPDATE races SET tfb_uuid = $1 WHERE id = $2",
                                uuid, cal_event.race.id as _,
                            ).execute(db_pool).await.to_racetime()?;
                        }
                        seed::Files::TfbSotd { .. } => unimplemented!("Triforce Blitz seed of the day not supported for official races"),
                    }
                    if let Some([hash1, hash2, hash3, hash4, hash5]) = extra.file_hash {
                        sqlx::query!(
                            "UPDATE races SET hash1 = $1, hash2 = $2, hash3 = $3, hash4 = $4, hash5 = $5 WHERE id = $6",
                            hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _, cal_event.race.id as _,
                        ).execute(db_pool).await.to_racetime()?;
                        if let Some(preset) = rsl_preset {
                            match seed.files.as_ref().expect("received seed with no files") {
                                seed::Files::MidosHouse { file_stem, .. } => {
                                    sqlx::query!(
                                        "INSERT INTO rsl_seeds (room, file_stem, preset, hash1, hash2, hash3, hash4, hash5) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                                        format!("https://{}{}", ctx.global_state.env.racetime_host(), ctx.data().await.url), &file_stem, preset as _, hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _,
                                    ).execute(db_pool).await.to_racetime()?;
                                }
                                seed::Files::OotrWeb { id, gen_time, file_stem } => {
                                    sqlx::query!(
                                        "INSERT INTO rsl_seeds (room, file_stem, preset, web_id, web_gen_time, hash1, hash2, hash3, hash4, hash5) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                                        format!("https://{}{}", ctx.global_state.env.racetime_host(), ctx.data().await.url), &file_stem, preset as _, *id as i64, gen_time, hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _,
                                    ).execute(db_pool).await.to_racetime()?;
                                }
                                seed::Files::TriforceBlitz { .. } | seed::Files::TfbSotd { .. } => unreachable!(), // no such thing as random settings Triforce Blitz
                            }
                        }
                    }
                }
                let seed_url = match seed.files.as_ref().expect("received seed with no files") {
                    seed::Files::MidosHouse { file_stem, .. } => format!("https://midos.house/seed/{file_stem}"),
                    seed::Files::OotrWeb { id, .. } => format!("https://ootrandomizer.com/seed/get?id={id}"),
                    seed::Files::TriforceBlitz { uuid } => format!("https://www.triforceblitz.com/seed/{uuid}"),
                    seed::Files::TfbSotd { ordinal, .. } => format!("https://www.triforceblitz.com/seed/daily/{ordinal}"),
                };
                ctx.say(if let French = language {
                    format!("@entrants Voici votre seed : {seed_url}")
                } else {
                    format!("@entrants Here is your seed: {seed_url}")
                }).await?;
                if let Some(file_hash) = extra.file_hash {
                    ctx.say(format_hash(file_hash)).await?;
                }
                match unlock_spoiler_log {
                    UnlockSpoilerLog::Now => ctx.say("The spoiler log is also available on the seed page.").await?,
                    UnlockSpoilerLog::Progression => ctx.say("The progression spoiler is also available on the seed page. The full spoiler will be available there after the race.").await?,
                    UnlockSpoilerLog::After => if let Some(seed::Files::TfbSotd { date, .. }) = seed.files {
                        if let Some(unlock_date) = date.succ_opt().and_then(|next| next.succ_opt()) {
                            let unlock_time = Utc.from_utc_datetime(&unlock_date.and_hms_opt(20, 0, 0).expect("failed to construct naive datetime at 20:00:00"));
                            let unlock_time = (unlock_time - Utc::now()).to_std().expect("unlock time for current daily seed in the past");
                            ctx.say(format!("The spoiler log will be available on the seed page in {}.", English.format_duration(unlock_time, true))).await?;
                        } else {
                            unimplemented!("distant future Triforce Blitz SotD")
                        }
                    } else {
                        ctx.say(if let French = language {
                            "Le spoiler log sera disponible sur le lien de la seed après la seed."
                        } else {
                            "The spoiler log will be available on the seed page after the race."
                        }).await?;
                    },
                    UnlockSpoilerLog::Never => {}
                }
                ctx.set_bot_raceinfo(&format!(
                    "{}{}{seed_url}",
                    if let Some(preset) = rsl_preset { format!("{}\n", preset.race_info()) } else { String::default() },
                    extra.file_hash.map(|file_hash| format!("{}\n", format_hash(file_hash))).unwrap_or_default(),
                )).await?;
                if let Some(OfficialRaceData { cal_event, event, .. }) = official_data {
                    // send multiworld rooms
                    let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
                    let mut mw_rooms_created = 0;
                    for team in cal_event.active_teams() {
                        if let Some(mw::Impl::MidosHouse) = team.mw_impl {
                            let members = team.members_roles(&mut transaction).await.to_racetime()?;
                            let mut reply_to = String::default();
                            for (member, role) in &members {
                                if event.team_config.role_is_racing(*role) {
                                    if let Some(ref racetime) = member.racetime {
                                        if !reply_to.is_empty() {
                                            reply_to.push_str(", ");
                                        }
                                        reply_to.push_str(&racetime.display_name);
                                    } else {
                                        reply_to = team.name.clone().unwrap_or_else(|| format!("(unnamed team)"));
                                        break
                                    }
                                }
                            }
                            let mut mw_room_name = if let Ok(other_team) = cal_event.race.teams().filter(|iter_team| iter_team.id != team.id).exactly_one() {
                                format!(
                                    "{}vs. {}",
                                    if let Some(game) = cal_event.race.game { format!("game {game} ") } else { String::default() },
                                    other_team.name.as_deref().unwrap_or("unnamed team"),
                                )
                            } else {
                                let mut mw_room_name = match (&cal_event.race.phase, &cal_event.race.round) {
                                    (Some(phase), Some(round)) => format!("{phase} {round}"),
                                    (Some(phase), None) => phase.clone(),
                                    (None, Some(round)) => round.clone(),
                                    (None, None) => event.display_name.clone(),
                                };
                                if let Some(game) = cal_event.race.game {
                                    mw_room_name.push_str(&format!(", game {game}"));
                                }
                                mw_room_name
                            };
                            if mw_room_name.len() > 64 {
                                // maximum room name length in database is 64
                                let ellipsis = "[…]";
                                let split_at = (0..=64 - ellipsis.len()).rev().find(|&idx| mw_room_name.is_char_boundary(idx)).unwrap_or(0);
                                mw_room_name.truncate(split_at);
                                mw_room_name.push_str(ellipsis);
                            }
                            if let Some([hash1, hash2, hash3, hash4, hash5]) = extra.file_hash {
                                let mut cmd = Command::new("/usr/local/share/midos-house/bin/ootrmwd");
                                cmd.arg("create-tournament-room");
                                cmd.arg(&mw_room_name);
                                cmd.arg(hash1.to_string());
                                cmd.arg(hash2.to_string());
                                cmd.arg(hash3.to_string());
                                cmd.arg(hash4.to_string());
                                cmd.arg(hash5.to_string());
                                for (member, role) in members {
                                    if event.team_config.role_is_racing(role) {
                                        cmd.arg(member.id.to_string());
                                    }
                                }
                                cmd.check("ootrmwd create-tournament-room").await.to_racetime()?;
                                ctx.say(format!("{reply_to}, your Mido's House Multiworld room named “{mw_room_name}” is now open.")).await?;
                                mw_rooms_created += 1;
                            } else {
                                ctx.say(format!("Sorry {reply_to}, there was an error creating your Mido's House Multiworld room. Please create one manually.")).await?;
                            }
                        }
                    }
                    if mw_rooms_created > 0 {
                        ctx.say(format!("You can find your room{} at the top of the room list after signing in with racetime.gg or Discord from the multiworld app's settings screen.", if mw_rooms_created > 1 { "s" } else { "" })).await?;
                    }
                    transaction.commit().await.to_racetime()?;
                }
                lock!(@write state = state; *state = RaceState::Rolled(seed));
            }
            Self::Error(RollError::Retries { num_retries, last_error }) => {
                if let Some(last_error) = last_error {
                    eprintln!("seed rolling failed {num_retries} times, sample error:\n{last_error}");
                } else {
                    eprintln!("seed rolling failed {num_retries} times, no sample error recorded");
                }
                ctx.say(if let French = language {
                    format!("Désolé @entrants, le randomizer a rapporté une erreur {num_retries} fois de suite donc je vais laisser tomber. Veuillez réessayer et, si l'erreur persiste, essayer de roll une seed de votre côté et contacter Fenhl.")
                } else {
                    format!("Sorry @entrants, the randomizer reported an error {num_retries} times, so I'm giving up on rolling the seed. Please try again. If this error persists, please report it to Fenhl.")
                }).await?; //TODO for official races, explain that retrying is done using !seed
                lock!(@write state = state; *state = RaceState::Init);
            }
            Self::Error(e) => {
                eprintln!("seed roll error: {e} ({e:?})");
                if let Environment::Production = ctx.global_state.env {
                    wheel::night_report("/net/midoshouse/error", Some(&format!("seed roll error: {e} ({e:?})"))).await.to_racetime()?;
                }
                ctx.say("Sorry @entrants, something went wrong while rolling the seed. Please report this error to Fenhl and if necessary roll the seed manually.").await?;
            }
            #[cfg(unix)] Self::Message(msg) => ctx.say(msg).await?,
        }
        Ok(())
    }
}

struct OotrApiClient {
    http_client: reqwest::Client,
    api_key: String,
    api_key_encryption: String,
    next_request: Mutex<Instant>,
    mw_seed_rollers: Semaphore,
    waiting: Mutex<Vec<mpsc::UnboundedSender<()>>>,
}

struct VersionsResponse {
    currently_active_version: Option<rando::Version>,
    available_versions: Vec<rando::Version>,
}

impl OotrApiClient {
    pub fn new(http_client: reqwest::Client, api_key: String, api_key_encryption: String) -> Self {
        Self {
            next_request: Mutex::new(Instant::now() + MULTIWORLD_RATE_LIMIT),
            mw_seed_rollers: Semaphore::new(2), // we're allowed to roll a maximum of 2 multiworld seeds at the same time
            waiting: Mutex::default(),
            http_client, api_key, api_key_encryption,
        }
    }

    async fn get(&self, uri: impl IntoUrl + Clone, query: Option<&(impl Serialize + ?Sized)>) -> reqwest::Result<reqwest::Response> {
        lock!(next_request = self.next_request; {
            sleep_until(*next_request).await;
            let mut builder = self.http_client.get(uri.clone());
            if let Some(query) = query {
                builder = builder.query(query);
            }
            let res = builder.send().await;
            *next_request = Instant::now() + Duration::from_millis(500);
            res
        })
    }

    async fn post(&self, uri: impl IntoUrl + Clone, query: Option<&(impl Serialize + ?Sized)>, json: Option<&(impl Serialize + ?Sized)>, rate_limit: Option<Duration>) -> reqwest::Result<reqwest::Response> {
        lock!(next_request = self.next_request; {
            sleep_until(*next_request).await;
            let mut builder = self.http_client.post(uri.clone());
            if let Some(query) = query {
                builder = builder.query(query);
            }
            if let Some(json) = json {
                builder = builder.json(json);
            }
            let res = builder.send().await;
            *next_request = Instant::now() + rate_limit.unwrap_or_else(|| Duration::from_millis(500));
            res
        })
    }

    async fn get_versions(&self, branch: Option<rando::Branch>, random_settings: bool) -> Result<VersionsResponse, RollError> {
        struct VersionsResponseVersion {
            major: u8,
            minor: u8,
            patch: u8,
            supplementary: Option<u8>,
        }

        #[derive(Debug, thiserror::Error)]
        enum VersionsResponseVersionParseError {
            #[error(transparent)] ParseInt(#[from] std::num::ParseIntError),
            #[error("ootrandomizer.com API returned randomizer version in unexpected format")]
            Format,
        }

        impl FromStr for VersionsResponseVersion {
            type Err = VersionsResponseVersionParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if let Some((_, major, minor, patch, supplementary)) = regex_captures!("^([0-9]+)\\.([0-9]+)\\.([0-9]+)-([0-9]+)$", s) {
                    Ok(Self { major: major.parse()?, minor: minor.parse()?, patch: patch.parse()?, supplementary: Some(supplementary.parse()?) })
                } else if let Some((_, major, minor, patch)) = regex_captures!("^([0-9]+)\\.([0-9]+)\\.([0-9]+)$", s) {
                    Ok(Self { major: major.parse()?, minor: minor.parse()?, patch: patch.parse()?, supplementary: None })
                } else {
                    Err(VersionsResponseVersionParseError::Format)
                }
            }
        }

        derive_deserialize_from_fromstr!(VersionsResponseVersion, "randomizer version in ootrandomizer.com API format");

        impl VersionsResponseVersion {
            fn normalize(self, branch: Option<rando::Branch>) -> Option<rando::Version> {
                if let Some(supplementary) = self.supplementary.filter(|&supplementary| supplementary != 0) {
                    Some(rando::Version::from_branch(branch?, self.major, self.minor, self.patch, supplementary))
                } else if branch.map_or(true, |branch| branch == rando::Branch::Dev) {
                    Some(rando::Version::from_dev(self.major, self.minor, self.patch))
                } else {
                    None
                }
            }
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawVersionsResponse {
            currently_active_version: VersionsResponseVersion,
            available_versions: Vec<VersionsResponseVersion>,
        }

        let web_branch = if let Some(branch) = branch {
            branch.latest_web_name(random_settings).ok_or(RollError::RandomSettingsWeb)?
        } else {
            // API lists releases under the “master” branch
            "master"
        };
        let RawVersionsResponse { currently_active_version, available_versions } = self.get("https://ootrandomizer.com/api/version", Some(&[("key", &*self.api_key), ("branch", web_branch)])).await?
            .detailed_error_for_status().await?
            .json_with_text_in_error().await?;
        Ok(VersionsResponse {
            currently_active_version: currently_active_version.normalize(branch),
            available_versions: available_versions.into_iter().filter_map(|ver| ver.normalize(branch)).collect(),
        })
    }

    /// Checks if the given randomizer branch/version is available on web, and if so, which version to use.
    async fn can_roll_on_web(&self, rsl_preset: Option<&VersionedRslPreset>, version: &VersionedBranch, world_count: u8, unlock_spoiler_log: UnlockSpoilerLog) -> Option<rando::Version> {
        if world_count > 3 { return None }
        if let UnlockSpoilerLog::Progression = unlock_spoiler_log { return None }
        if rsl_preset.is_some() && version.branch().map_or(true, |branch| branch.latest_web_name_random_settings().is_none()) { return None }
        match version {
            VersionedBranch::Pinned(version) => {
                if matches!(rsl_preset, Some(VersionedRslPreset::Xopar { .. })) && *version == rando::Version::from_branch(rando::Branch::DevR, 7, 1, 181, 1) || *version == rando::Version::from_branch(rando::Branch::DevR, 8, 0, 1, 1) {
                    return Some(rando::Version::from_branch(
                        version.branch(),
                        version.base().major.try_into().expect("taken from existing rando::Version"),
                        version.base().minor.try_into().expect("taken from existing rando::Version"),
                        version.base().patch.try_into().expect("taken from existing rando::Version"),
                        0, // legacy devR/devRSL version which was not yet tagged with its supplementary version number but is only available in random settings mode (devRSL), not regularly (devR)
                    ))
                }
                let is_available = KNOWN_GOOD_WEB_VERSIONS.contains(version)
                    || self.get_versions((!version.is_release()).then(|| version.branch()), rsl_preset.is_some()).await
                        // the version API endpoint sometimes returns HTML instead of the expected JSON, fallback to generating locally when that happens
                        .is_ok_and(|VersionsResponse { available_versions, .. }| available_versions.contains(version));
                is_available.then(|| version.clone())
            }
            VersionedBranch::Latest(branch) => self.get_versions(Some(*branch), rsl_preset.is_some()).await.ok().and_then(|response| response.currently_active_version),
            VersionedBranch::Custom { .. } => None,
        }
    }

    async fn roll_seed_web(&self, update_tx: mpsc::Sender<SeedRollUpdate>, delay_until: Option<DateTime<Utc>>, version: rando::Version, random_settings: bool, unlock_spoiler_log: UnlockSpoilerLog, settings: serde_json::Map<String, Json>) -> Result<(i64, DateTime<Utc>, [HashIcon; 5], String), RollError> {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct CreateSeedResponse {
            #[serde_as(as = "DisplayFromStr")]
            id: i64,
        }

        #[derive(Deserialize)]
        struct SeedStatusResponse {
            status: u8,
        }

        #[derive(Deserialize)]
        struct SettingsLog {
            file_hash: [HashIcon; 5],
        }

        #[serde_as]
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SeedDetailsResponse {
            creation_timestamp: DateTime<Utc>,
            #[serde_as(as = "JsonString")]
            settings_log: SettingsLog,
        }

        let encrypt = version.is_release() && unlock_spoiler_log == UnlockSpoilerLog::Never;
        let api_key = if encrypt { &*self.api_key_encryption } else { &*self.api_key };
        let is_mw = settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64")) > 1;
        let mw_permit = if is_mw {
            Some(match self.mw_seed_rollers.try_acquire() {
                Ok(permit) => permit,
                Err(TryAcquireError::Closed) => unreachable!(),
                Err(TryAcquireError::NoPermits) => {
                    let (mut pos, mut pos_rx) = lock!(waiting = self.waiting; {
                        let pos = waiting.len();
                        let (pos_tx, pos_rx) = mpsc::unbounded_channel();
                        waiting.push(pos_tx);
                        (pos, pos_rx)
                    });
                    update_tx.send(SeedRollUpdate::Queued(pos.try_into().unwrap())).await?;
                    while pos > 0 {
                        let () = pos_rx.recv().await.expect("queue position notifier closed");
                        pos -= 1;
                        update_tx.send(SeedRollUpdate::MovedForward(pos.try_into().unwrap())).await?;
                    }
                    lock!(waiting = self.waiting; {
                        let permit = self.mw_seed_rollers.acquire().await.expect("seed queue semaphore closed");
                        waiting.remove(0);
                        for tx in &*waiting {
                            let _ = tx.send(());
                        }
                        permit
                    })
                }
            })
        } else {
            None
        };
        let mut last_id = None;
        for attempt in 0.. {
            if attempt >= 3 && delay_until.map_or(true, |delay_until| Utc::now() >= delay_until) {
                drop(mw_permit);
                return Err(RollError::Retries {
                    num_retries: attempt,
                    last_error: last_id.map(|id| format!("https://ootrandomizer.com/seed/get?id={id}")),
                })
            }
            if attempt == 0 && !random_settings {
                update_tx.send(SeedRollUpdate::Started).await?;
            }
            let CreateSeedResponse { id } = self.post("https://ootrandomizer.com/api/v2/seed/create", Some(&[
                ("key", api_key),
                ("version", &*version.to_string_web(random_settings).ok_or(RollError::RandomSettingsWeb)?),
                if encrypt {
                    ("encrypt", "1")
                } else {
                    ("locked", if let UnlockSpoilerLog::Now = unlock_spoiler_log { "0" } else { "1" })
                },
            ]), Some(&settings), is_mw.then_some(MULTIWORLD_RATE_LIMIT)).await?
                .detailed_error_for_status().await?
                .json_with_text_in_error().await?;
            last_id = Some(id);
            loop {
                sleep(Duration::from_secs(1)).await;
                let resp = self.get(
                    "https://ootrandomizer.com/api/v2/seed/status",
                    Some(&[("key", api_key), ("id", &*id.to_string())]),
                ).await?;
                if resp.status() == StatusCode::NO_CONTENT { continue }
                resp.error_for_status_ref()?;
                match resp.json_with_text_in_error::<SeedStatusResponse>().await?.status {
                    0 => continue, // still generating
                    1 => { // generated success
                        drop(mw_permit);
                        let SeedDetailsResponse { creation_timestamp, settings_log } = self.get("https://ootrandomizer.com/api/v2/seed/details", Some(&[("key", api_key), ("id", &*id.to_string())])).await?
                            .detailed_error_for_status().await?
                            .json_with_text_in_error().await?;
                        let patch_response = self.get("https://ootrandomizer.com/api/v2/seed/patch", Some(&[("key", api_key), ("id", &*id.to_string())])).await?
                            .detailed_error_for_status().await?;
                        let (_, patch_file_name) = regex_captures!("^attachment; filename=(.+)$", patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION).ok_or(RollError::PatchPath)?.to_str()?).ok_or(RollError::PatchPath)?;
                        let patch_file_name = patch_file_name.to_owned();
                        let (_, patch_file_stem) = regex_captures!(r"^(.+)\.zpfz?$", &patch_file_name).ok_or(RollError::PatchPath)?;
                        let patch_path = Path::new(seed::DIR).join(&patch_file_name);
                        io::copy_buf(&mut StreamReader::new(patch_response.bytes_stream().map_err(io_error_from_reqwest)), &mut File::create(&patch_path).await?).await.at(patch_path)?;
                        return Ok((id, creation_timestamp, settings_log.file_hash, patch_file_stem.to_owned()))
                    }
                    2 => unreachable!(), // generated with link (not possible from API)
                    3 => break, // failed to generate
                    n => {
                        drop(mw_permit);
                        return Err(RollError::UnespectedSeedStatus(n))
                    }
                }
            }
        }
        unreachable!()
    }
}

fn format_hash(file_hash: [HashIcon; 5]) -> impl fmt::Display {
    file_hash.into_iter().map(|icon| icon.to_racetime_emoji()).format(" ")
}

#[derive(Clone, Copy)]
struct Breaks {
    duration: Duration,
    interval: Duration,
}

impl Breaks {
    fn format(&self, language: Language) -> String {
        if let French = language {
            format!("{} toutes les {}", French.format_duration(self.duration, true), French.format_duration(self.interval, true))
        } else {
            format!("{} every {}", English.format_duration(self.duration, true), English.format_duration(self.interval, true))
        }
    }
}

impl FromStr for Breaks {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (_, duration, interval) = regex_captures!("^(.+?) ?e(?:very)? ?(.+?)$", s).ok_or(())?;
        Ok(Self {
            duration: parse_duration(duration, DurationUnit::Minutes).ok_or(())?,
            interval: parse_duration(interval, DurationUnit::Hours).ok_or(())?,
        })
    }
}

#[derive(Default)]
enum RaceState {
    #[default]
    Init,
    Draft {
        state: Draft,
        unlock_spoiler_log: UnlockSpoilerLog,
    },
    Rolling,
    Rolled(seed::Data),
    SpoilerSent,
}

#[derive(Clone)]
struct OfficialRaceData {
    cal_event: cal::Event,
    event: event::Data<'static>,
    restreams: HashMap<Url, RestreamState>,
    entrants: Vec<String>,
    fpa_invoked: bool,
    scores: HashMap<String, Option<tfb::Score>>,
}

#[derive(Default, Clone)]
struct RestreamState {
    language: Option<Language>,
    restreamer_racetime_id: Option<String>,
    ready: bool,
}

struct Handler {
    official_data: Option<OfficialRaceData>,
    high_seed_name: String,
    low_seed_name: String,
    breaks: Option<Breaks>,
    break_notifications: Option<tokio::task::JoinHandle<()>>,
    goal_notifications: Option<tokio::task::JoinHandle<()>>,
    start_saved: bool,
    fpa_enabled: bool,
    locked: bool,
    race_state: ArcRwLock<RaceState>,
}

impl Handler {
    /// For `existing_state`, `Some(None)` means this is an existing race room with unknown state, while `None` means this is a new race room.
    async fn should_handle_inner(race_data: &RaceData, global_state: Arc<GlobalState>, existing_state: Option<Option<&Self>>) -> bool {
        if Goal::from_race_data(race_data).is_none() { return false }
        if let Some(existing_state) = existing_state {
            if let Some(existing_state) = existing_state {
                if let Some(ref official_data) = existing_state.official_data {
                    if race_data.entrants.iter().any(|entrant| entrant.status.value == EntrantStatusValue::Done && official_data.scores.get(&entrant.user.id).is_some_and(|score| score.is_none())) {
                        return true
                    }
                }
            }
        } else {
            lock!(clean_shutdown = global_state.clean_shutdown; {
                if !clean_shutdown.should_handle_new() {
                    unlock!();
                    return false
                }
                assert!(clean_shutdown.open_rooms.insert(race_data.url.clone()));
            });
        }
        if let RaceStatusValue::Finished | RaceStatusValue::Cancelled = race_data.status.value { return false }
        true
    }

    fn is_official(&self) -> bool { self.official_data.is_some() }

    async fn goal(&self, ctx: &RaceContext<GlobalState>) -> Result<Goal, GoalFromStrError> {
        ctx.data().await.goal.name.parse()
    }

    async fn can_monitor(&self, ctx: &RaceContext<GlobalState>, is_monitor: bool, msg: &ChatMessage) -> sqlx::Result<bool> {
        if is_monitor { return Ok(true) }
        if let Some(OfficialRaceData { ref event, .. }) = self.official_data {
            if let Some(UserData { ref id, .. }) = msg.user {
                if let Some(user) = User::from_racetime(&ctx.global_state.db_pool, id).await? {
                    return sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM organizers WHERE series = $1 AND event = $2 AND organizer = $3) AS "exists!""#, event.series as _, &event.event, user.id as _).fetch_one(&ctx.global_state.db_pool).await
                }
            }
        }
        Ok(false)
    }

    async fn send_settings(&self, ctx: &RaceContext<GlobalState>, preface: &str, reply_to: &str) -> Result<(), Error> {
        let goal = self.goal(ctx).await.to_racetime()?;
        if let Some(draft_kind) = goal.draft_kind() {
            let available_settings = lock!(@read state = self.race_state; if let RaceState::Draft { state: ref draft, .. } = *state {
                match draft.next_step(draft_kind, self.official_data.as_ref().and_then(|OfficialRaceData { cal_event, .. }| cal_event.race.game), &mut draft::MessageContext::RaceTime { high_seed_name: &self.high_seed_name, low_seed_name: &self.low_seed_name, reply_to }).await.to_racetime()?.kind {
                    draft::StepKind::GoFirst => None,
                    draft::StepKind::Ban { available_settings, .. } => Some(available_settings.all().map(|setting| setting.description).collect()),
                    draft::StepKind::Pick { available_choices, .. } => Some(available_choices.all().map(|setting| setting.description).collect()),
                    draft::StepKind::BooleanChoice { .. } | draft::StepKind::Done(_) => Some(Vec::default()),
                }
            } else {
                None
            });
            let available_settings = available_settings.unwrap_or_else(|| match draft_kind {
                draft::Kind::S7 => s::S7_SETTINGS.into_iter().map(|setting| Cow::Owned(setting.description())).collect(),
                draft::Kind::MultiworldS3 => mw::S3_SETTINGS.into_iter().map(|mw::Setting { description, .. }| Cow::Borrowed(description)).collect(),
                draft::Kind::MultiworldS4 => mw::S4_SETTINGS.into_iter().map(|mw::Setting { description, .. }| Cow::Borrowed(description)).collect(),
                draft::Kind::TournoiFrancoS3 => fr::S3_SETTINGS.into_iter().map(|fr::Setting { description, .. }| Cow::Borrowed(description)).collect(),
                draft::Kind::TournoiFrancoS4 => fr::S4_SETTINGS.into_iter().map(|fr::Setting { description, .. }| Cow::Borrowed(description)).collect(),
            });
            if available_settings.is_empty() {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, aucun setting n'est demandé pour le moment.")
                } else {
                    format!("Sorry {reply_to}, no settings are currently available.")
                }).await?;
            } else {
                ctx.say(preface).await?;
                for setting in available_settings {
                    ctx.say(setting).await?;
                }
            }
        } else {
            ctx.say(format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?;
        }
        Ok(())
    }

    async fn advance_draft(&self, ctx: &RaceContext<GlobalState>, state: &RaceState) -> Result<(), Error> {
        let goal = self.goal(ctx).await.to_racetime()?;
        let Some(draft_kind) = goal.draft_kind() else { unreachable!() };
        let RaceState::Draft { state: ref draft, unlock_spoiler_log } = *state else { unreachable!() };
        let step = draft.next_step(draft_kind, self.official_data.as_ref().and_then(|OfficialRaceData { cal_event, .. }| cal_event.race.game), &mut draft::MessageContext::RaceTime { high_seed_name: &self.high_seed_name, low_seed_name: &self.low_seed_name, reply_to: "friend" }).await.to_racetime()?;
        if let draft::StepKind::Done(settings) = step.kind {
            let (article, description) = if let French = goal.language() {
                ("une", format!("seed avec {}", step.message))
            } else {
                ("a", format!("seed with {}", step.message))
            };
            self.roll_seed(ctx, goal.preroll_seeds(), goal.rando_version(), settings, unlock_spoiler_log, goal.language(), article, description).await;
            return Ok(())
        } else {
            ctx.say(step.message).await?;
        }
        Ok(())
    }

    async fn draft_action(&self, ctx: &RaceContext<GlobalState>, sender: Option<&UserData>, action: draft::Action) -> Result<(), Error> {
        let goal = self.goal(ctx).await.to_racetime()?;
        let reply_to = sender.map_or("friend", |user| &user.name);
        if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
            lock!(@write state = self.race_state; if let Some(draft_kind) = goal.draft_kind() {
                match *state {
                    RaceState::Init => match draft_kind {
                        draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 => ctx.say(format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                        draft::Kind::TournoiFrancoS3 => ctx.say(format!("Désolé {reply_to}, le draft n'a pas débuté. Utilisez “!seed draft” pour en commencer un. Pour plus d'infos, utilisez !presets")).await?,
                        draft::Kind::TournoiFrancoS4 => ctx.say(format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one. For more info about these options, use !presets / le draft n'a pas débuté. Utilisez “!seed draft” pour en commencer un. Pour plus d'infos, utilisez !presets")).await?,
                    },
                    RaceState::Draft { state: ref mut draft, .. } => {
                        let is_active_team = if let Some(OfficialRaceData { ref cal_event, ref event, .. }) = self.official_data {
                            let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
                            let is_active_team = if_chain! {
                                if let Some(sender) = sender;
                                if let Some(user) = User::from_racetime(&mut *transaction, &sender.id).await.to_racetime()?;
                                if let Some(team) = Team::from_event_and_member(&mut transaction, event.series, &event.event, user.id).await.to_racetime()?;
                                then {
                                    draft.is_active_team(draft_kind, cal_event.race.game, team.id).await.to_racetime()?
                                } else {
                                    false
                                }
                            };
                            transaction.commit().await.to_racetime()?;
                            is_active_team
                        } else {
                            true
                        };
                        if is_active_team {
                            match draft.apply(draft_kind, self.official_data.as_ref().and_then(|OfficialRaceData { cal_event, .. }| cal_event.race.game), &mut draft::MessageContext::RaceTime { high_seed_name: &self.high_seed_name, low_seed_name: &self.low_seed_name, reply_to }, action).await.to_racetime()? {
                                Ok(_) => self.advance_draft(ctx, &state).await?,
                                Err(error_msg) => {
                                    unlock!();
                                    ctx.say(error_msg).await?;
                                    return Ok(())
                                }
                            }
                        } else {
                            match draft_kind {
                                draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 => ctx.say(format!("Sorry {reply_to}, it's not your turn in the settings draft.")).await?,
                                draft::Kind::TournoiFrancoS3 => ctx.say(format!("Désolé {reply_to}, mais ce n'est pas votre tour.")).await?,
                                draft::Kind::TournoiFrancoS4 => ctx.say(format!("Sorry {reply_to}, it's not your turn in the settings draft. / mais ce n'est pas votre tour.")).await?,
                            }
                        }
                    }
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => match goal.language() {
                        French => ctx.say(format!("Désolé {reply_to}, mais il n'y a pas de draft, ou la phase de pick&ban est terminée.")).await?,
                        _ => ctx.say(format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                    },
                }
            } else {
                ctx.say(format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?;
            });
        } else {
            match goal.language() {
                French => ctx.say(format!("Désolé {reply_to}, mais la race a débuté.")).await?,
                _ => ctx.say(format!("Sorry {reply_to}, but the race has already started.")).await?,
            }
        }
        Ok(())
    }

    async fn roll_seed_inner(&self, ctx: &RaceContext<GlobalState>, delay_until: Option<DateTime<Utc>>, mut updates: mpsc::Receiver<SeedRollUpdate>, language: Language, article: &'static str, description: String) {
        let db_pool = ctx.global_state.db_pool.clone();
        let ctx = ctx.clone();
        let state = self.race_state.clone();
        let official_data = self.official_data.clone();
        tokio::spawn(async move {
            lock!(@write state = state; *state = RaceState::Rolling); //TODO ensure only one seed is rolled at a time
            let mut seed_state = None::<SeedRollUpdate>;
            if let Some(delay) = delay_until.and_then(|delay_until| (delay_until - Utc::now()).to_std().ok()) {
                // don't want to give an unnecessarily exact estimate if the room was opened automatically 30 or 60 minutes ahead of start
                let display_delay = if delay > Duration::from_secs(14 * 60) && delay < Duration::from_secs(16 * 60) {
                    Duration::from_secs(15 * 60)
                } else if delay > Duration::from_secs(44 * 60) && delay < Duration::from_secs(46 * 60) {
                    Duration::from_secs(45 * 60)
                } else {
                    delay
                };
                ctx.say(if let French = language {
                    format!("Votre {description} sera postée dans {}.", French.format_duration(display_delay, true))
                } else {
                    format!("Your {description} will be posted in {}.", English.format_duration(display_delay, true))
                }).await?;
                let mut sleep = pin!(sleep_until(Instant::now() + delay));
                loop {
                    select! {
                        () = &mut sleep => {
                            if let Some(update) = seed_state.take() {
                                update.handle(&db_pool, &ctx, &state, official_data.as_ref(), language, article, &description).await?;
                            }
                            while let Some(update) = updates.recv().await {
                                update.handle(&db_pool, &ctx, &state, official_data.as_ref(), language, article, &description).await?;
                            }
                            break
                        }
                        Some(update) = updates.recv() => seed_state = Some(update),
                    }
                }
            } else {
                while let Some(update) = updates.recv().await {
                    update.handle(&db_pool, &ctx, &state, official_data.as_ref(), language, article, &description).await?;
                }
            }
            Ok::<_, Error>(())
        });
    }

    async fn roll_seed(&self, ctx: &RaceContext<GlobalState>, preroll: PrerollMode, version: VersionedBranch, settings: serde_json::Map<String, Json>, unlock_spoiler_log: UnlockSpoilerLog, language: Language, article: &'static str, description: String) {
        let official_start = self.official_data.as_ref().map(|official_data| official_data.cal_event.start().expect("handling room for official race without start time"));
        let delay_until = official_start.map(|start| start - TimeDelta::minutes(15));
        self.roll_seed_inner(ctx, delay_until, Arc::clone(&ctx.global_state).roll_seed(preroll, true, delay_until, version, settings, unlock_spoiler_log), language, article, description).await;
    }

    async fn roll_rsl_seed(&self, ctx: &RaceContext<GlobalState>, preset: VersionedRslPreset, world_count: u8, unlock_spoiler_log: UnlockSpoilerLog, language: Language, article: &'static str, description: String) {
        let official_start = self.official_data.as_ref().map(|official_data| official_data.cal_event.start().expect("handling room for official race without start time"));
        let delay_until = official_start.map(|start| start - TimeDelta::minutes(15));
        self.roll_seed_inner(ctx, delay_until, Arc::clone(&ctx.global_state).roll_rsl_seed(delay_until, preset, world_count, unlock_spoiler_log), language, article, description).await;
    }

    async fn roll_tfb_seed(&self, ctx: &RaceContext<GlobalState>, version: &'static str, unlock_spoiler_log: UnlockSpoilerLog, language: Language, article: &'static str, description: String) {
        let official_start = self.official_data.as_ref().map(|official_data| official_data.cal_event.start().expect("handling room for official race without start time"));
        let delay_until = official_start.map(|start| start - TimeDelta::minutes(15));
        self.roll_seed_inner(ctx, delay_until, Arc::clone(&ctx.global_state).roll_tfb_seed(delay_until, version, Some(format!("https://{}{}", ctx.global_state.env.racetime_host(), ctx.data().await.url)), unlock_spoiler_log), language, article, description).await;
    }

    async fn queue_existing_seed(&self, ctx: &RaceContext<GlobalState>, seed: seed::Data, language: Language, article: &'static str, description: String) {
        let official_start = self.official_data.as_ref().map(|official_data| official_data.cal_event.start().expect("handling room for official race without start time"));
        let delay_until = official_start.map(|start| start - TimeDelta::minutes(15));
        let (tx, rx) = mpsc::channel(1);
        tx.send(SeedRollUpdate::Done { rsl_preset: None, unlock_spoiler_log: UnlockSpoilerLog::After, seed }).await.unwrap();
        self.roll_seed_inner(ctx, delay_until, rx, language, article, description).await;
    }

    /// Returns `false` if this race was already finished/cancelled.
    async fn unlock_spoiler_log(&self, ctx: &RaceContext<GlobalState>, goal: Goal) -> Result<bool, Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SeedDetailsResponse {
            spoiler_log: String,
        }

        lock!(@write state = self.race_state; {
            match *state {
                RaceState::Rolled(seed::Data { files: Some(ref files), .. }) => if self.official_data.as_ref().map_or(true, |official_data| !official_data.cal_event.is_private_async_part()) {
                    if let UnlockSpoilerLog::Progression | UnlockSpoilerLog::After = goal.unlock_spoiler_log(self.is_official(), false /* we may try to unlock a log that's already unlocked, but other than that, this assumption doesn't break anything */) {
                        match files {
                            seed::Files::MidosHouse { file_stem, locked_spoiler_log_path } => if let Some(locked_spoiler_log_path) = locked_spoiler_log_path {
                                fs::rename(locked_spoiler_log_path, Path::new(seed::DIR).join(format!("{file_stem}_Spoiler.json"))).await.to_racetime()?;
                            },
                            seed::Files::OotrWeb { id, file_stem, .. } => {
                                ctx.global_state.ootr_api_client.post("https://ootrandomizer.com/api/v2/seed/unlock", Some(&[("key", &ctx.global_state.ootr_api_client.api_key), ("id", &id.to_string())]), None::<&()>, None).await?
                                    .detailed_error_for_status().await.to_racetime()?;
                                let spoiler_log = ctx.global_state.ootr_api_client.get("https://ootrandomizer.com/api/v2/seed/details", Some(&[("key", &ctx.global_state.ootr_api_client.api_key), ("id", &id.to_string())])).await?
                                    .detailed_error_for_status().await.to_racetime()?
                                    .json_with_text_in_error::<SeedDetailsResponse>().await.to_racetime()?
                                    .spoiler_log;
                                fs::write(Path::new(seed::DIR).join(format!("{file_stem}_Spoiler.json")), &spoiler_log).await.to_racetime()?;
                            }
                            seed::Files::TriforceBlitz { .. } | seed::Files::TfbSotd { .. } => {} // automatically unlocked by triforceblitz.com
                        }
                    }
                },
                RaceState::SpoilerSent => {
                    unlock!();
                    return Ok(false)
                }
                _ => {}
            }
            *state = RaceState::SpoilerSent;
        });
        Ok(true)
    }
}

#[async_trait]
impl RaceHandler<GlobalState> for Handler {
    async fn should_handle(race_data: &RaceData, global_state: Arc<GlobalState>) -> Result<bool, Error> {
        Ok(Self::should_handle_inner(race_data, global_state, None).await)
    }

    async fn should_stop(&mut self, ctx: &RaceContext<GlobalState>) -> Result<bool, Error> {
        Ok(!Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), Some(Some(self))).await)
    }

    async fn task(global_state: Arc<GlobalState>, race_data: Arc<tokio::sync::RwLock<RaceData>>, join_handle: tokio::task::JoinHandle<()>) -> Result<(), Error> {
        let race_data = ArcRwLock::from(race_data);
        tokio::spawn(async move {
            lock!(@read data = race_data; println!("race handler for https://{}{} started", global_state.env.racetime_host(), data.url));
            let res = join_handle.await;
            lock!(@read data = race_data; {
                lock!(clean_shutdown = global_state.clean_shutdown; {
                    assert!(clean_shutdown.open_rooms.remove(&data.url));
                    if clean_shutdown.requested && clean_shutdown.open_rooms.is_empty() {
                        clean_shutdown.notifier.notify_waiters();
                    }
                });
                if let Ok(()) = res {
                    println!("race handler for https://{}{} stopped", global_state.env.racetime_host(), data.url);
                } else {
                    eprintln!("race handler for https://{}{} panicked", global_state.env.racetime_host(), data.url);
                    if let Environment::Production = global_state.env {
                        let _ = wheel::night_report("/net/midoshouse/error", Some(&format!("race handler for https://{}{} panicked", global_state.env.racetime_host(), data.url))).await;
                    }
                }
            });
        });
        Ok(())
    }

    async fn new(ctx: &RaceContext<GlobalState>) -> Result<Self, Error> {
        let data = ctx.data().await;
        let goal = data.goal.name.parse::<Goal>().to_racetime()?;
        let (existing_seed, official_data, race_state, high_seed_name, low_seed_name, fpa_enabled) = lock!(new_room_lock = ctx.global_state.new_room_lock; { // make sure a new room isn't handled before it's added to the database
            let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
            let new_data = if let Some(cal_event) = cal::Event::from_room(&mut transaction, &ctx.global_state.http_client, format!("https://{}{}", ctx.global_state.env.racetime_host(), ctx.data().await.url).parse()?).await.to_racetime()? {
                let event = cal_event.race.event(&mut transaction).await.to_racetime()?;
                let mut entrants = Vec::default();
                for team in cal_event.active_teams() {
                    for (member, role) in team.members_roles(&mut transaction).await.to_racetime()? {
                        if event.team_config.role_is_racing(role) {
                            if let Some(member) = member.racetime {
                                if let Some(entrant) = data.entrants.iter().find(|entrant| entrant.user.id == member.id) {
                                    match entrant.status.value {
                                        EntrantStatusValue::Requested => ctx.accept_request(&member.id).await?,
                                        EntrantStatusValue::Invited |
                                        EntrantStatusValue::Declined |
                                        EntrantStatusValue::Ready |
                                        EntrantStatusValue::NotReady |
                                        EntrantStatusValue::InProgress |
                                        EntrantStatusValue::Done |
                                        EntrantStatusValue::Dnf |
                                        EntrantStatusValue::Dq => {}
                                    }
                                } else {
                                    ctx.invite_user(&member.id).await?;
                                }
                                entrants.push(member.id);
                            } else {
                                ctx.say(format!(
                                    "Warning: {name} could not be invited because {subj} {has_not} linked {poss} racetime.gg account to {poss} Mido's House account. Please contact an organizer to invite {obj} manually for now.",
                                    name = member,
                                    subj = member.subjective_pronoun(),
                                    has_not = if member.subjective_pronoun_uses_plural_form() { "haven't" } else { "hasn't" },
                                    poss = member.possessive_determiner(),
                                    obj = member.objective_pronoun(),
                                )).await?;
                            }
                        }
                    }
                }
                ctx.send_message(&if_chain! {
                    if let French = goal.language();
                    if !event.is_single_race();
                    if let (Some(phase), Some(round)) = (cal_event.race.phase.as_ref(), cal_event.race.round.as_ref());
                    if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut *transaction).await.to_racetime()?;
                    then {
                        format!(
                            "Bienvenue pour cette race de {phase_round} ! Pour plus d'informations : https://midos.house/event/{}/{}",
                            event.series,
                            event.event,
                        )
                    } else {
                        format!(
                            "Welcome to {}! Learn more about the event at https://midos.house/event/{}/{}",
                            if event.is_single_race() {
                                format!("the {}", event.display_name) //TODO remove “the” depending on event name
                            } else {
                                match (cal_event.race.phase.as_deref(), cal_event.race.round.as_deref()) {
                                    (Some("Qualifier"), Some(round)) => format!("qualifier {round}"),
                                    (Some("Live Qualifier"), Some(round)) => format!("live qualifier {round}"),
                                    (None, Some("Friday Weekly")) => format!("the Friday weekly"),
                                    (None, Some("Saturday Weekly")) => format!("the Saturday weekly"),
                                    (None, Some("Sunday Weekly")) => format!("the Sunday weekly"),
                                    (Some(phase), Some(round)) => format!("this {phase} {round} race"),
                                    (Some(phase), None) => format!("this {phase} race"),
                                    (None, Some(round)) => format!("this {round} race"),
                                    (None, None) => format!("this {} race", event.display_name),
                                }
                            },
                            event.series,
                            event.event,
                        )
                    }
                }, true, Vec::default()).await?;
                let (race_state, high_seed_name, low_seed_name) = if let Some(draft_kind) = event.draft_kind() {
                    let state = cal_event.race.draft.clone().expect("missing draft state");
                    let [high_seed_name, low_seed_name] = if let draft::StepKind::Done(_) = state.next_step(draft_kind, cal_event.race.game, &mut draft::MessageContext::None).await.to_racetime()?.kind {
                        // we just need to roll the seed so player/team names are no longer required
                        [format!("Team A"), format!("Team B")]
                    } else {
                        match cal_event.race.entrants {
                            Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => [format!("Team A"), format!("Team B")],
                            Entrants::Two([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2)]) => {
                                let name1 = if_chain! {
                                    if let Ok(member) = team1.members(&mut transaction).await.to_racetime()?.into_iter().exactly_one();
                                    if let Some(ref racetime) = member.racetime;
                                    then {
                                        racetime.display_name.clone()
                                    } else {
                                        team1.name(&mut transaction).await.to_racetime()?.map_or_else(|| format!("Team A"), Cow::into_owned)
                                    }
                                };
                                let name2 = if_chain! {
                                    if let Ok(member) = team2.members(&mut transaction).await.to_racetime()?.into_iter().exactly_one();
                                    if let Some(ref racetime) = member.racetime;
                                    then {
                                        racetime.display_name.clone()
                                    } else {
                                        team2.name(&mut transaction).await.to_racetime()?.map_or_else(|| format!("Team B"), Cow::into_owned)
                                    }
                                };
                                if team1.id == state.high_seed {
                                    [name1, name2]
                                } else {
                                    [name2, name1]
                                }
                            }
                            Entrants::Two([_, _]) => unimplemented!("draft with non-MH teams"),
                            Entrants::Three([_, _, _]) => unimplemented!("draft with 3 teams"),
                        }
                    };
                    (RaceState::Draft {
                        unlock_spoiler_log: goal.unlock_spoiler_log(true, false),
                        state,
                    }, high_seed_name, low_seed_name)
                } else {
                    (RaceState::Init, format!("Team A"), format!("Team B"))
                };
                let restreams = cal_event.race.video_urls.iter().map(|(&language, video_url)| (video_url.clone(), RestreamState {
                    language: Some(language),
                    restreamer_racetime_id: cal_event.race.restreamers.get(&language).cloned(),
                    ready: false,
                })).collect();
                if let Series::SpeedGaming = event.series {
                    let delay_until = cal_event.start().expect("handling room for official race without start time") - TimeDelta::minutes(20);
                    if let Ok(delay) = (delay_until - Utc::now()).to_std() {
                        let ctx = ctx.clone();
                        let requires_emote_only = cal_event.race.phase.as_ref().map_or(false, |phase| phase == "Bracket");
                        tokio::spawn(async move {
                            sleep_until(Instant::now() + delay).await;
                            if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), Some(None)).await { return }
                            ctx.say(if requires_emote_only {
                                "@entrants Remember to go live with a 15 minute (900 second) delay and set your chat to emote only!"
                            } else {
                                "@entrants Remember to go live with a 15 minute (900 second) delay!"
                            }).await.expect("failed to send stream delay notice");
                            sleep(Duration::from_secs(15 * 60)).await;
                            let data = ctx.data().await;
                            if !Self::should_handle_inner(&*data, ctx.global_state.clone(), Some(None)).await { return }
                            if let RaceStatusValue::Open = data.status.value {
                                ctx.set_invitational().await.expect("failed to make the room invitational");
                            }
                        });
                    }
                }
                let fpa_enabled = match data.status.value {
                    RaceStatusValue::Invitational => {
                        ctx.say(if let French = goal.language() {
                            "Le FPA est activé pour cette race. Les joueurs pourront utiliser !fpa pendant la race pour signaler d'un problème technique de leur côté. Les race monitors doivent activer les notifications en cliquant sur l'icône de cloche 🔔 sous le chat."
                        } else {
                            "Fair play agreement is active for this official race. Entrants may use the !fpa command during the race to notify of a crash. Race monitors (if any) should enable notifications using the bell 🔔 icon below chat."
                        }).await?; //TODO different message for monitorless FPA?
                        true
                    }
                    RaceStatusValue::Open => false,
                    _ => data.entrants.len() < 10, // guess based on entrant count, assuming an open race for 10 or more
                };
                (
                    cal_event.race.seed.clone(),
                    Some(OfficialRaceData {
                        fpa_invoked: false,
                        scores: HashMap::default(),
                        cal_event, event, restreams, entrants,
                    }),
                    race_state,
                    high_seed_name,
                    low_seed_name,
                    fpa_enabled,
                )
            } else {
                let mut race_state = RaceState::Init;
                if let Some(ref info_bot) = data.info_bot {
                    for section in info_bot.split(" | ") {
                        if let Some((_, file_stem)) = regex_captures!(r"^Seed: https://midos\.house/seed/(.+)(?:\.zpfz?)?$", section) {
                            race_state = RaceState::Rolled(seed::Data {
                                file_hash: None,
                                files: Some(seed::Files::MidosHouse {
                                    file_stem: Cow::Owned(file_stem.to_owned()),
                                    locked_spoiler_log_path: None,
                                }),
                            });
                            break
                        } else if let Some((_, seed_id)) = regex_captures!(r"^Seed: https://ootrandomizer\.com/seed/get?id=([0-9]+)$", section) {
                            let patch_response = ctx.global_state.ootr_api_client.get("https://ootrandomizer.com/api/v2/seed/patch", Some(&[("key", &*ctx.global_state.ootr_api_client.api_key), ("id", seed_id)])).await?
                                .detailed_error_for_status().await.to_racetime()?;
                            let (_, file_stem) = regex_captures!(r"^attachment; filename=(.+)\.zpfz?$", patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION).ok_or(RollError::PatchPath).to_racetime()?.to_str()?).ok_or(RollError::PatchPath).to_racetime()?;
                            race_state = RaceState::Rolled(seed::Data {
                                file_hash: None,
                                files: Some(seed::Files::OotrWeb {
                                    id: seed_id.parse().to_racetime()?,
                                    gen_time: Utc::now(),
                                    file_stem: Cow::Owned(file_stem.to_owned()),
                                }),
                            });
                            break
                        }
                    }
                }
                if let RaceStatusValue::Pending | RaceStatusValue::InProgress = data.status.value { //TODO also check this in official races
                    //TODO get chatlog and recover breaks config instead of sending this
                    ctx.say("@entrants I just restarted and it looks like the race is already in progress. If the !breaks command was used, break notifications may be broken now. Sorry about that.").await?;
                } else {
                    match race_state {
                        RaceState::Init => match goal {
                            Goal::Cc7 => ctx.send_message(
                                "Welcome! This is a practice room for the S7 Challenge Cup. Learn more about the tournament at https://midos.house/event/s/7cc",
                                true,
                                vec![
                                    ("Roll seed (base settings)", ActionButton::Message {
                                        message: format!("!seed base"),
                                        help_text: Some(format!("Create a seed with the tournament's base settings.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Roll seed (random settings)", ActionButton::Message {
                                        message: format!("!seed random"),
                                        help_text: Some(format!("Simulate a settings draft with both players picking randomly. The settings are posted along with the seed.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Roll seed (custom settings)", ActionButton::Message {
                                        message: format!("!seed {}", s::S7_SETTINGS.into_iter().map(|setting| format!("{0} ${{{0}}}", setting.name)).format(" ")),
                                        help_text: Some(format!("Pick a set of draftable settings without doing a full draft.")),
                                        survey: Some(s::S7_SETTINGS.into_iter().map(|setting| SurveyQuestion {
                                            name: setting.name.to_owned(),
                                            label: setting.display.to_owned(),
                                            default: Some(format!("default")),
                                            help_text: None,
                                            kind: SurveyQuestionKind::Radio,
                                            placeholder: None,
                                            options: iter::once((format!("default"), setting.default_display.to_owned()))
                                                .chain(setting.other.iter().map(|(name, display, _)| (name.to_string(), display.to_string())))
                                                .collect(),
                                        }).collect()),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (settings draft)", ActionButton::Message {
                                        message: format!("!seed draft"),
                                        help_text: Some(format!("Pick the settings here in the chat.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::CoOpS3 => ctx.send_message(
                                "Welcome! This is a practice room for the 3rd co-op tournament. Learn more about the tournament at https://midos.house/event/coop/3",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the tournament.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::CopaDoBrasil => ctx.send_message(
                                "Welcome! This is a practice room for the Copa do Brasil. Learn more about the tournament at https://midos.house/event/br/1",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the tournament.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::MixedPoolsS2 => ctx.send_message(
                                "Welcome! This is a practice room for the 2nd Mixed Pools Tournament. Learn more about the tournament at https://midos.house/event/mp/2",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the tournament.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::MixedPoolsS3 => ctx.send_message(
                                "Welcome! This is a practice room for the 3rd Mixed Pools Tournament. Learn more about the tournament at https://midos.house/event/mp/3",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the tournament.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::MultiworldS3 => ctx.send_message(
                                "Welcome! This is a practice room for the 3rd Multiworld Tournament. Learn more about the tournament at https://midos.house/event/mw/3",
                                true,
                                vec![
                                    ("Roll seed (base settings)", ActionButton::Message {
                                        message: format!("!seed base"),
                                        help_text: Some(format!("Create a seed with the settings used for the qualifier and tiebreaker asyncs.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Roll seed (random settings)", ActionButton::Message {
                                        message: format!("!seed random"),
                                        help_text: Some(format!("Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Roll seed (custom settings)", ActionButton::Message {
                                        message: format!("!seed {}", mw::S3_SETTINGS.into_iter().map(|setting| format!("{0} ${{{0}}}", setting.name)).format(" ")),
                                        help_text: Some(format!("Pick a set of draftable settings without doing a full draft.")),
                                        survey: Some(mw::S3_SETTINGS.into_iter().map(|setting| SurveyQuestion {
                                            name: setting.name.to_owned(),
                                            label: setting.display.to_owned(),
                                            default: Some(setting.default.to_owned()),
                                            help_text: None,
                                            kind: SurveyQuestionKind::Radio,
                                            placeholder: None,
                                            options: iter::once((setting.default.to_owned(), setting.default_display.to_owned()))
                                                .chain(setting.other.iter().map(|(name, display)| (name.to_string(), display.to_string())))
                                                .collect(),
                                        }).collect()),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (settings draft)", ActionButton::Message {
                                        message: format!("!seed draft"),
                                        help_text: Some(format!("Pick the settings here in the chat.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::MultiworldS4 => ctx.send_message(
                                "Welcome! This is a practice room for the 4th Multiworld Tournament. Learn more about the tournament at https://midos.house/event/mw/4",
                                true,
                                vec![
                                    ("Roll seed (base settings)", ActionButton::Message {
                                        message: format!("!seed base"),
                                        help_text: Some(format!("Create a seed with the settings used for the qualifier and tiebreaker asyncs.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Roll seed (random settings)", ActionButton::Message {
                                        message: format!("!seed random"),
                                        help_text: Some(format!("Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Roll seed (custom settings)", ActionButton::Message {
                                        message: format!("!seed {}", mw::S4_SETTINGS.into_iter().map(|setting| format!("{0} ${{{0}}}", setting.name)).format(" ")),
                                        help_text: Some(format!("Pick a set of draftable settings without doing a full draft.")),
                                        survey: Some(mw::S4_SETTINGS.into_iter().map(|setting| SurveyQuestion {
                                            name: setting.name.to_owned(),
                                            label: setting.display.to_owned(),
                                            default: Some(setting.default.to_owned()),
                                            help_text: None,
                                            kind: SurveyQuestionKind::Radio,
                                            placeholder: None,
                                            options: iter::once((setting.default.to_owned(), setting.default_display.to_owned()))
                                                .chain(setting.other.iter().map(|(name, display)| (name.to_string(), display.to_string())))
                                                .collect(),
                                        }).collect()),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (settings draft)", ActionButton::Message {
                                        message: format!("!seed draft"),
                                        help_text: Some(format!("Pick the settings here in the chat.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::NineDaysOfSaws => ctx.send_message(
                                "Welcome! This is a practice room for 9 Days of SAWS. Learn more about the event at https://docs.google.com/document/d/1xELThZtIctwN-vYtYhUqtd88JigNzabk8OZHANa0gqY/edit",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed ${{preset}}"),
                                        help_text: Some(format!("Select a preset and create a seed.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("preset"),
                                                label: format!("Preset"),
                                                default: None,
                                                help_text: Some(format!("Days 7 and 9 are identical to days 2 and 1, respectively. They are listed for the sake of convenience.")),
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: vec![
                                                    (format!("day1"), format!("Day 1: S6")),
                                                    (format!("day2"), format!("Day 2: Beginner")),
                                                    (format!("day3"), format!("Day 3: Advanced")),
                                                    (format!("day4"), format!("Day 4: S5 + one bonk KO")),
                                                    (format!("day5"), format!("Day 5: Beginner + mixed pools")),
                                                    (format!("day6"), format!("Day 6: Beginner 3-player multiworld")),
                                                    (format!("day7"), format!("Day 7: Beginner")),
                                                    (format!("day8"), format!("Day 8: S6 + dungeon ER")),
                                                    (format!("day9"), format!("Day 9: S6")),
                                                ],
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                ],
                            ).await?,
                            Goal::Pic7 => ctx.send_message(
                                "Welcome! This is a practice room for the 7th Pictionary Spoiler Log Race. Learn more about the race at https://midos.house/event/pic/7",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the race.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::PicRs2 => ctx.send_message(
                                "Welcome! This is a practice room for the 2nd Random Settings Pictionary Spoiler Log Race. Learn more about the race at https://midos.house/event/pic/rs2",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the weights used for the race.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::Rsl => ctx.send_message(
                                "Welcome to the OoTR Random Settings League! Learn more at https://rsl.one/",
                                true,
                                vec![
                                    ("Roll RSL seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with official Random Settings League weights.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Roll multiworld seed", ActionButton::Message {
                                        message: format!("!seed mw ${{worldcount}}"),
                                        help_text: Some(format!("Create a random settings multiworld seed. Supports up to 15 players.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("worldcount"),
                                                label: format!("World count"),
                                                default: None,
                                                help_text: Some(format!("Please download the RSL script from https://github.com/matthewkirby/plando-random-settings if you want to roll seeds for more than 15 players.")),
                                                kind: SurveyQuestionKind::Radio,
                                                placeholder: None,
                                                options: (2..=15).map(|world_count| (world_count.to_string(), world_count.to_string())).collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("More presets", ActionButton::Message {
                                        message: format!("!seed ${{preset}}"),
                                        help_text: Some(format!("Select a preset and create a seed.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("preset"),
                                                label: format!("Preset"),
                                                default: None,
                                                help_text: Some(format!("Use !presets for more info.")),
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: all()
                                                    .filter(|preset| !matches!(preset, rsl::Preset::League | rsl::Preset::Multiworld))
                                                    .map(|preset| (preset.name().to_owned(), preset.race_info().to_owned()))
                                                    .collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                ],
                            ).await?,
                            Goal::Sgl2023 => ctx.send_message(
                                "Welcome! This is a practice room for SpeedGaming Live 2023. Learn more about the tournaments at https://docs.google.com/document/d/1EACqBl8ZOreD6xT5jQ2HrdLOnpBpKyjS3FUYK8XFeqg/edit",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the tournaments.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::Sgl2024 => ctx.send_message(
                                "Welcome! This is a practice room for SpeedGaming Live 2024. Learn more about the tournaments at https://docs.google.com/document/d/1I0IcnGMqKr3QaCgg923SR_SxVu0iytIA_lOhN2ybj9w/edit",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the tournaments.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::SongsOfHope => ctx.send_message(
                                "Welcome! This is a practice room for Songs of Hope, a charity tournament for the Autism of Society of America. Learn more about the tournament at https://midos.house/event/soh/1",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the settings used for the tournament.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::StandardRuleset => unreachable!("attempted to handle a user-opened Standard Ruleset room"),
                            Goal::TournoiFrancoS3 => ctx.send_message(
                                "Bienvenue ! Ceci est une practice room pour le tournoi francophone saison 3. Vous pouvez obtenir des renseignements supplémentaires ici : https://midos.house/event/fr/3",
                                true,
                                vec![
                                    ("Roll seed (settings de base)", ActionButton::Message {
                                        message: format!("!seed base ${{mq}}mq"),
                                        help_text: Some(format!("Roll une seed avec les settings de base, sans setting additionnel.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("mq"),
                                                label: format!("Donjons Master Quest"),
                                                default: Some(format!("0")),
                                                help_text: None,
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (settings aléatoires)", ActionButton::Message {
                                        message: format!("!seed random ${{advanced}} ${{mq}}mq"),
                                        help_text: Some(format!("Simule en draft en sélectionnant des settings au hasard.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("advanced"),
                                                label: format!("Active les settings difficiles"),
                                                default: None,
                                                help_text: None,
                                                kind: SurveyQuestionKind::Bool,
                                                placeholder: None,
                                                options: Vec::default(),
                                            },
                                            SurveyQuestion {
                                                name: format!("mq"),
                                                label: format!("Donjons Master Quest"),
                                                default: Some(format!("0")),
                                                help_text: None,
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (settings à choisir)", ActionButton::Message {
                                        message: format!("!seed {} ${{mq}}mq", fr::S3_SETTINGS.into_iter().map(|setting| format!("{0} ${{{0}}}", setting.name)).format(" ")),
                                        help_text: Some(format!("Vous laisse sélectionner les settings que vous voulez dans votre seed.")),
                                        survey: Some(fr::S3_SETTINGS.into_iter().map(|setting| SurveyQuestion {
                                            name: setting.name.to_owned(),
                                            label: setting.display.to_owned(),
                                            default: Some(setting.default.to_owned()),
                                            help_text: None,
                                            kind: SurveyQuestionKind::Radio,
                                            placeholder: None,
                                            options: iter::once((setting.default.to_owned(), setting.default_display.to_owned()))
                                                .chain(setting.other.iter().map(|(name, _, display)| (name.to_string(), display.to_string())))
                                                .chain((setting.name == "dungeon-er").then(|| (format!("mixed"), format!("dungeon ER (mixés)"))))
                                                .collect(),
                                        }).chain(iter::once(SurveyQuestion {
                                            name: format!("mq"),
                                            label: format!("Donjons Master Quest"),
                                            default: Some(format!("0")),
                                            help_text: None,
                                            kind: SurveyQuestionKind::Select,
                                            placeholder: None,
                                            options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                        })).collect()),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (avec draft)", ActionButton::Message {
                                        message: format!("!seed draft ${{advanced}} ${{mq}}mq"),
                                        help_text: Some(format!("Vous fait effectuer un draft dans le chat racetime.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("advanced"),
                                                label: format!("Active les settings difficiles"),
                                                default: None,
                                                help_text: None,
                                                kind: SurveyQuestionKind::Bool,
                                                placeholder: None,
                                                options: Vec::default(),
                                            },
                                            SurveyQuestion {
                                                name: format!("mq"),
                                                label: format!("Donjons Master Quest"),
                                                default: Some(format!("0")),
                                                help_text: None,
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                ],
                            ).await?,
                            Goal::TournoiFrancoS4 => ctx.send_message( //TODO post welcome message in both English and French
                                "Welcome! This is a practice room for the Tournoi Francophone Saison 4. Learn more about the tournament at https://midos.house/event/fr/4",
                                true,
                                vec![
                                    ("Roll seed (base settings)", ActionButton::Message {
                                        message: format!("!seed base ${{mq}}mq"),
                                        help_text: Some(format!("Create a seed with the base settings.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("mq"),
                                                label: format!("Master Quest Dungeons"),
                                                default: Some(format!("0")),
                                                help_text: None,
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (random settings)", ActionButton::Message {
                                        message: format!("!seed random ${{advanced}} ${{mq}}mq"),
                                        help_text: Some(format!("Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("advanced"),
                                                label: format!("Allow advanced settings"),
                                                default: None,
                                                help_text: None,
                                                kind: SurveyQuestionKind::Bool,
                                                placeholder: None,
                                                options: Vec::default(),
                                            },
                                            SurveyQuestion {
                                                name: format!("mq"),
                                                label: format!("Master Quest Dungeons"),
                                                default: Some(format!("0")),
                                                help_text: None,
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (custom settings)", ActionButton::Message {
                                        message: format!("!seed {} ${{mq}}mq", fr::S4_SETTINGS.into_iter().map(|setting| format!("{0} ${{{0}}}", setting.name)).format(" ")),
                                        help_text: Some(format!("Pick a set of draftable settings without doing a full draft.")),
                                        survey: Some(fr::S4_SETTINGS.into_iter().map(|setting| SurveyQuestion {
                                            name: setting.name.to_owned(),
                                            label: setting.display.to_owned(),
                                            default: Some(setting.default.to_owned()),
                                            help_text: None,
                                            kind: SurveyQuestionKind::Radio,
                                            placeholder: None,
                                            options: iter::once((setting.default.to_owned(), setting.default_display.to_owned()))
                                                .chain(setting.other.iter().map(|(name, _, display)| (name.to_string(), display.to_string())))
                                                .chain((setting.name == "dungeon-er").then(|| (format!("mixed"), format!("dungeon ER (mixed)"))))
                                                .collect(),
                                        }).chain(iter::once(SurveyQuestion {
                                            name: format!("mq"),
                                            label: format!("Master Quest Dungeons"),
                                            default: Some(format!("0")),
                                            help_text: None,
                                            kind: SurveyQuestionKind::Select,
                                            placeholder: None,
                                            options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                        })).collect()),
                                        submit: Some(format!("Roll")),
                                    }),
                                    ("Roll seed (settings draft)", ActionButton::Message {
                                        message: format!("!seed draft ${{advanced}} ${{mq}}mq"),
                                        help_text: Some(format!("Pick the settings here in the chat.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("advanced"),
                                                label: format!("Allow advanced settings"),
                                                default: None,
                                                help_text: None,
                                                kind: SurveyQuestionKind::Bool,
                                                placeholder: None,
                                                options: Vec::default(),
                                            },
                                            SurveyQuestion {
                                                name: format!("mq"),
                                                label: format!("Master Quest Dungeons"),
                                                default: Some(format!("0")),
                                                help_text: None,
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: (0..=12).map(|mq| (mq.to_string(), mq.to_string())).collect(),
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                ],
                            ).await?,
                            Goal::TriforceBlitz => ctx.send_message(
                                "Welcome to Triforce Blitz! Learn more at https://triforceblitz.com/",
                                true,
                                vec![
                                    ("Roll S3 seed", ActionButton::Message {
                                        message: format!("!seed s3"),
                                        help_text: Some(format!("Create a Triforce Blitz season 3 seed.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("Seed of the day", ActionButton::Message {
                                        message: format!("!seed daily"),
                                        help_text: Some(format!("Link the current seed of the day.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                    ("More presets", ActionButton::Message {
                                        message: format!("!seed ${{preset}}"),
                                        help_text: Some(format!("Select a preset and create a seed.")),
                                        survey: Some(vec![
                                            SurveyQuestion {
                                                name: format!("preset"),
                                                label: format!("Preset"),
                                                default: None,
                                                help_text: None,
                                                kind: SurveyQuestionKind::Select,
                                                placeholder: None,
                                                options: vec![
                                                    (format!("jr"), format!("Jabu's Revenge")),
                                                    (format!("s2"), format!("S2")),
                                                ],
                                            },
                                        ]),
                                        submit: Some(format!("Roll")),
                                    }),
                                ],
                            ).await?,
                            Goal::TriforceBlitzProgressionSpoiler => ctx.send_message(
                                "Welcome to Triforce Blitz Progression Spoiler!",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Create a seed with the current settings for the mode.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                            Goal::WeTryToBeBetter => ctx.send_message(
                                "Bienvenue ! Ceci est une practice room pour le tournoi WeTryToBeBetter. Vous pouvez obtenir des renseignements supplémentaires ici : https://midos.house/event/wttbb/1",
                                true,
                                vec![
                                    ("Roll seed", ActionButton::Message {
                                        message: format!("!seed"),
                                        help_text: Some(format!("Roll une seed avec les settings utilisés pour le tournoi.")),
                                        survey: None,
                                        submit: None,
                                    }),
                                ],
                            ).await?,
                        },
                        RaceState::Rolled(_) => ctx.say("@entrants I just restarted. You may have to reconfigure !breaks and !fpa. Sorry about that.").await?,
                        RaceState::Draft { .. } | RaceState::Rolling | RaceState::SpoilerSent => unreachable!(),
                    }
                }
                (
                    seed::Data::default(),
                    None,
                    RaceState::default(),
                    format!("Team A"),
                    format!("Team B"),
                    false,
                )
            };
            transaction.commit().await.to_racetime()?;
            new_data
        });
        let this = Self {
            breaks: None, //TODO default breaks for restreamed matches?
            break_notifications: None,
            goal_notifications: None,
            start_saved: false,
            locked: false,
            race_state: ArcRwLock::new(race_state),
            official_data, high_seed_name, low_seed_name, fpa_enabled,
        };
        if let Some(OfficialRaceData { ref restreams, .. }) = this.official_data {
            if let Some(restreams_text) = English.join_str(restreams.iter().map(|(video_url, state)| format!("in {} at {video_url}", state.language.expect("preset restreams should have languages assigned")))) {
                for restreamer in restreams.values().flat_map(|RestreamState { restreamer_racetime_id, .. }| restreamer_racetime_id) {
                    let data = ctx.data().await;
                    if data.monitors.iter().find(|monitor| monitor.id == *restreamer).is_some() { continue }
                    if let Some(entrant) = data.entrants.iter().find(|entrant| entrant.user.id == *restreamer) { //TODO keep track of pending changes to the entrant list made in this method and match accordingly, e.g. players who are also monitoring should not be uninvited
                        match entrant.status.value {
                            EntrantStatusValue::Requested => {
                                ctx.accept_request(restreamer).await?;
                                ctx.add_monitor(restreamer).await?;
                                ctx.remove_entrant(restreamer).await?;
                            }
                            EntrantStatusValue::Invited |
                            EntrantStatusValue::Declined |
                            EntrantStatusValue::Ready |
                            EntrantStatusValue::NotReady |
                            EntrantStatusValue::InProgress |
                            EntrantStatusValue::Done |
                            EntrantStatusValue::Dnf |
                            EntrantStatusValue::Dq => {
                                ctx.add_monitor(restreamer).await?;
                            }
                        }
                    } else {
                        ctx.invite_user(restreamer).await?;
                        ctx.add_monitor(restreamer).await?;
                        ctx.remove_entrant(restreamer).await?;
                    }
                }
                let text = if restreams.values().any(|state| state.restreamer_racetime_id.is_none()) {
                    if_chain! {
                        if let French = goal.language();
                        if let Ok((video_url, state)) = restreams.iter().exactly_one();
                        if let Some(French) = state.language;
                        then {
                            format!("Cette race est restreamée en français chez {video_url} — l'auto-start est désactivé. Les organisateurs du tournoi peuvent utiliser “!monitor” pour devenir race monitor, puis pour inviter les restreamers en tant que race monitor et leur autoriser le force start.")
                        } else {
                            format!("This race is being restreamed {restreams_text} — auto-start is disabled. Tournament organizers can use “!monitor” to become race monitors, then invite the restreamer{0} as race monitor{0} to allow them to force-start.", if restreams.len() == 1 { "" } else { "s" })
                        }
                    }
                } else if let Ok((video_url, state)) = restreams.iter().exactly_one() {
                    if_chain! {
                        if let French = goal.language();
                        if let Some(French) = state.language;
                        then {
                            format!("Cette race est restreamée en français chez {video_url} — l'auto start est désactivé. Le restreamer peut utiliser “!ready” pour débloquer l'auto-start.")
                        } else {
                            format!("This race is being restreamed {restreams_text} — auto-start is disabled. The restreamer can use “!ready” to unlock auto-start.")
                        }
                    }
                } else {
                    format!("This race is being restreamed {restreams_text} — auto-start is disabled. Restreamers can use “!ready” once the restream is ready. Auto-start will be unlocked once all restreams are ready.")
                };
                ctx.send_message(&text, true, Vec::default()).await?;
            }
            lock!(@read state = this.race_state; {
                if existing_seed.files.is_some() {
                    this.queue_existing_seed(ctx, existing_seed, English, "a", format!("seed")).await;
                } else {
                    match *state {
                        RaceState::Init => match goal {
                            | Goal::CoOpS3
                            | Goal::CopaDoBrasil
                            | Goal::MixedPoolsS2
                            | Goal::MixedPoolsS3
                            | Goal::Pic7
                            | Goal::Sgl2023
                            | Goal::Sgl2024
                            | Goal::SongsOfHope
                            | Goal::StandardRuleset
                            | Goal::TriforceBlitzProgressionSpoiler
                               => this.roll_seed(ctx, goal.preroll_seeds(), goal.rando_version(), goal.single_settings().expect("goal has no single settings"), goal.unlock_spoiler_log(true, false), English, "a", format!("seed")).await,
                            | Goal::WeTryToBeBetter
                                => this.roll_seed(ctx, goal.preroll_seeds(), goal.rando_version(), goal.single_settings().expect("goal has no single settings"), goal.unlock_spoiler_log(true, false), French, "une", format!("seed")).await,
                            Goal::Rsl => unreachable!("no official race rooms"),
                            Goal::Cc7 | Goal::MultiworldS3 | Goal::MultiworldS4 | Goal::TournoiFrancoS3 | Goal::TournoiFrancoS4 => unreachable!("should have draft state set"),
                            Goal::NineDaysOfSaws => unreachable!("9dos series has concluded"),
                            Goal::PicRs2 => this.roll_rsl_seed(ctx, VersionedRslPreset::Fenhl {
                                version: Some((Version::new(2, 3, 8), 10)),
                                preset: RslDevFenhlPreset::Pictionary,
                            }, 1, goal.unlock_spoiler_log(true, false), English, "a", format!("seed")).await,
                            Goal::TriforceBlitz => this.roll_tfb_seed(ctx, "LATEST", goal.unlock_spoiler_log(true, false), English, "a", format!("Triforce Blitz S3 seed")).await,
                        },
                        RaceState::Draft { .. } => this.advance_draft(ctx, &state).await?,
                        RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => {}
                    }
                }
            });
        }
        Ok(this)
    }

    async fn command(&mut self, ctx: &RaceContext<GlobalState>, cmd_name: String, args: Vec<String>, _is_moderator: bool, is_monitor: bool, msg: &ChatMessage) -> Result<(), Error> {
        let goal = self.goal(ctx).await.to_racetime()?;
        let reply_to = msg.user.as_ref().map_or("friend", |user| &user.name);
        match &*cmd_name.to_ascii_lowercase() {
            "ban" => match args[..] {
                [] => self.send_settings(ctx, &if let French = goal.language() {
                    format!("Désolé {reply_to}, un setting doit être choisi. Utilisez un des suivants :")
                } else {
                    format!("Sorry {reply_to}, the setting is required. Use one of the following:")
                }, reply_to).await?,
                [ref setting] => self.draft_action(ctx, msg.user.as_ref(), draft::Action::Ban { setting: setting.clone() }).await?,
                [..] => ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, seul un setting peut être ban à la fois. Veuillez seulement utiliser “!ban <setting>”")
                } else {
                    format!("Sorry {reply_to}, only one setting can be banned at a time. Use “!ban <setting>”")
                }).await?,
            },
            "breaks" | "break" => match args[..] {
                [] => if let Some(breaks) = self.breaks {
                    ctx.say(if let French = goal.language() {
                        format!("Vous aurez une pause de {}. Vous pouvez les désactiver avec !breaks off.", breaks.format(French))
                    } else {
                        format!("Breaks are currently set to {}. Disable with !breaks off", breaks.format(English))
                    }).await?;
                } else {
                    ctx.say(if let French = goal.language() {
                        "Les pauses sont actuellement désactivées. Exemple pour les activer : !breaks 5m every 2h30."
                    } else {
                        "Breaks are currently disabled. Example command to enable: !breaks 5m every 2h30"
                    }).await?;
                },
                [ref arg] if arg == "off" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                    self.breaks = None;
                    ctx.say(if let French = goal.language() {
                        "Les pauses sont désormais désactivées."
                    } else {
                        "Breaks are now disabled."
                    }).await?;
                } else {
                    ctx.say(if let French = goal.language() {
                        format!("Désolé {reply_to}, mais la race a débuté.")
                    } else {
                        format!("Sorry {reply_to}, but the race has already started.")
                    }).await?;
                },
                _ => if let Ok(breaks) = args.join(" ").parse::<Breaks>() {
                    if breaks.duration < Duration::from_secs(60) {
                        ctx.say(if let French = goal.language() {
                            format!("Désolé {reply_to}, le temps minimum pour une pause (si active) est de 1 minute. Vous pouvez désactiver les pauses avec !breaks off")
                        } else {
                            format!("Sorry {reply_to}, minimum break time (if enabled at all) is 1 minute. You can disable breaks entirely with !breaks off")
                        }).await?;
                    } else if breaks.interval < breaks.duration + Duration::from_secs(5 * 60) {
                        ctx.say(if let French = goal.language() {
                            format!("Désolé {reply_to}, il doit y avoir un minimum de 5 minutes entre les pauses.")
                        } else {
                            format!("Sorry {reply_to}, there must be a minimum of 5 minutes between breaks since I notify runners 5 minutes in advance.")
                        }).await?;
                    } else if breaks.duration + breaks.interval >= Duration::from_secs(24 * 60 * 60) {
                        ctx.say(if let French = goal.language() {
                            format!("Désolé {reply_to}, vous ne pouvez pas faire de pauses si tard dans la race, vu que les race rooms se ferment au bout de 24 heures.")
                        } else {
                            format!("Sorry {reply_to}, race rooms are automatically closed after 24 hours so these breaks wouldn't work.")
                        }).await?;
                    } else {
                        self.breaks = Some(breaks);
                        ctx.say(if let French = goal.language() {
                            format!("Vous aurez une pause de {}.", breaks.format(French))
                        } else {
                            format!("Breaks set to {}.", breaks.format(English))
                        }).await?;
                    }
                } else {
                    ctx.say(if let French = goal.language() {
                        format!("Désolé {reply_to}, je ne reconnais pas ce format pour les pauses. Exemple pour les activer : !breaks 5m every 2h30.")
                    } else {
                        format!("Sorry {reply_to}, I don't recognize that format for breaks. Example commands: !breaks 5m every 2h30, !breaks off")
                    }).await?;
                },
            },
            "draft" | "pick" => match args[..] {
                [] => self.send_settings(ctx, &if let French = goal.language() {
                    format!("Désolé {reply_to}, un setting doit être choisi. Utilisez un des suivants :")
                } else {
                    format!("Sorry {reply_to}, the setting is required. Use one of the following:")
                }, reply_to).await?,
                [_] => ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, une configuration est requise.")
                } else {
                    format!("Sorry {reply_to}, the value is required.")
                }).await?, //TODO list available values
                [ref setting, ref value] => self.draft_action(ctx, msg.user.as_ref(), draft::Action::Pick { setting: setting.clone(), value: value.clone() }).await?,
                [..] => ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, vous ne pouvez pick qu'un setting à la fois. Veuillez seulement utiliser “!draft <setting> <configuration>”")
                } else {
                    format!("Sorry {reply_to}, only one setting can be drafted at a time. Use “!draft <setting> <value>”")
                }).await?,
            },
            "first" => self.draft_action(ctx, msg.user.as_ref(), draft::Action::GoFirst(true)).await?,
            "fpa" => match args[..] {
                [] => if self.fpa_enabled {
                    if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                        ctx.say(if let French = goal.language() {
                            "Le FPA ne peut pas être appelé avant que la race ne commence."
                        } else {
                            "FPA cannot be invoked before the race starts."
                        }).await?;
                    } else {
                        if let Some(OfficialRaceData { ref cal_event, ref restreams, ref mut fpa_invoked, ref event, .. }) = self.official_data {
                            *fpa_invoked = true;
                            if restreams.is_empty() {
                                ctx.say(if_chain! {
                                    if let French = goal.language();
                                    if let TeamConfig::Solo = event.team_config;
                                    then {
                                        format!(
                                            "@everyone Le FPA a été appelé par {reply_to}.{} La race sera re-timée après le fin de celle-ci.",
                                            if let RaceSchedule::Async { .. } = cal_event.race.schedule { "" } else { " Le joueur qui ne l'a pas demandé peut continuer à jouer." },
                                        )
                                    } else {
                                        format!(
                                            "@everyone FPA has been invoked by {reply_to}. T{}he race will be retimed once completed.",
                                            if let RaceSchedule::Async { .. } = cal_event.race.schedule {
                                                String::default()
                                            } else {
                                                format!(
                                                    "he {player_team} that did not call FPA can continue playing; t",
                                                    player_team = if let TeamConfig::Solo = event.team_config { "player" } else { "team" },
                                                )
                                            },
                                        )
                                    }
                                }).await?;
                            } else {
                                ctx.say(if let French = goal.language() {
                                    format!("@everyone Le FPA a été appelé par {reply_to}. Merci d'arrêter de jouer, la race étant restreamée.")
                                } else {
                                    format!("@everyone FPA has been invoked by {reply_to}. Please pause since this race is being restreamed.")
                                }).await?;
                            }
                        } else {
                            ctx.say(if let French = goal.language() {
                                format!("@everyone Le FPA a été appelé par {reply_to}.")
                            } else {
                                format!("@everyone FPA has been invoked by {reply_to}.")
                            }).await?;
                        }
                    }
                } else {
                    ctx.say(if let French = goal.language() {
                        "Le FPA n'est pas activé. Les Race Monitors peuvent l'activer avec !fpa on."
                    } else {
                        "Fair play agreement is not active. Race monitors may enable FPA for this race with !fpa on"
                    }).await?;
                },
                [ref arg] => match &*arg.to_ascii_lowercase() {
                    "on" => if self.is_official() {
                        ctx.say(if let French = goal.language() {
                            "Le FPA est toujours activé dans les races officielles."
                        } else {
                            "Fair play agreement is always active in official races."
                        }).await?;
                    } else if !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.say(if let French = goal.language() {
                            format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                        } else {
                            format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                        }).await?;
                    } else if self.fpa_enabled {
                        ctx.say(if let French = goal.language() {
                            "Le FPA est déjà activé."
                        } else {
                            "Fair play agreement is already activated."
                        }).await?;
                    } else {
                        self.fpa_enabled = true;
                        ctx.say(if let French = goal.language() {
                            "Le FPA est désormais activé. Les joueurs pourront utiliser !fpa pendant la race pour signaler d'un problème technique de leur côté. Les race monitors doivent activer les notifications en cliquant sur l'icône de cloche 🔔 sous le chat."
                        } else {
                            "Fair play agreement is now active. @entrants may use the !fpa command during the race to notify of a crash. Race monitors should enable notifications using the bell 🔔 icon below chat."
                        }).await?;
                    },
                    "off" => if self.is_official() {
                        ctx.say(if let French = goal.language() {
                            format!("Désolé {reply_to}, mais le FPA ne peut pas être désactivé pour les races officielles.")
                        } else {
                            format!("Sorry {reply_to}, but FPA can't be deactivated for official races.")
                        }).await?;
                    } else if !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.say(if let French = goal.language() {
                            format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                        } else {
                            format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                        }).await?;
                    } else if self.fpa_enabled {
                        self.fpa_enabled = false;
                        ctx.say(if let French = goal.language() {
                            "Le FPA est désormais désactivé."
                        } else {
                            "Fair play agreement is now deactivated."
                        }).await?;
                    } else {
                        ctx.say(if let French = goal.language() {
                            "Le FPA est déjà désactivé."
                        } else {
                            "Fair play agreement is not active."
                        }).await?;
                    },
                    _ => ctx.say(if let French = goal.language() {
                        format!("Désolé {reply_to}, les seules commandes sont “!fpa on”, “!fpa off” ou “!fpa”.")
                    } else {
                        format!("Sorry {reply_to}, I don't recognize that subcommand. Use “!fpa on” or “!fpa off”, or just “!fpa” to invoke FPA.")
                    }).await?,
                },
                [..] => ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, les seules commandes sont “!fpa on”, “!fpa off” ou “!fpa”.")
                } else {
                    format!("Sorry {reply_to}, I didn't quite understand that. Use “!fpa on” or “!fpa off”, or just “!fpa” to invoke FPA.")
                }).await?,
            },
            "lock" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                self.locked = true;
                ctx.say(if_chain! {
                    if let French = goal.language();
                    if !self.is_official();
                    then {
                        format!("Race verrouillée. Je ne génèrerai une seed que pour les race monitors.")
                    } else {
                        format!("Lock initiated. I will now only roll seeds for {}.", if self.is_official() { "race monitors or tournament organizers" } else { "race monitors" })
                    }
                }).await?;
            } else {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                } else {
                    format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                }).await?;
            },
            "monitor" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                let monitor = &msg.user.as_ref().expect("received !monitor command from bot").id;
                if let Some(entrant) = ctx.data().await.entrants.iter().find(|entrant| entrant.user.id == *monitor) {
                    match entrant.status.value {
                        EntrantStatusValue::Requested => {
                            ctx.accept_request(monitor).await?;
                            ctx.add_monitor(monitor).await?;
                            ctx.remove_entrant(monitor).await?;
                        }
                        EntrantStatusValue::Invited |
                        EntrantStatusValue::Declined |
                        EntrantStatusValue::Ready |
                        EntrantStatusValue::NotReady |
                        EntrantStatusValue::InProgress |
                        EntrantStatusValue::Done |
                        EntrantStatusValue::Dnf |
                        EntrantStatusValue::Dq => {
                            ctx.add_monitor(monitor).await?;
                        }
                    }
                } else {
                    ctx.invite_user(monitor).await?;
                    ctx.add_monitor(monitor).await?;
                    ctx.remove_entrant(monitor).await?;
                }
            } else if self.is_official() {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, seuls les organisateurs du tournoi peuvent faire cela.")
                } else {
                    format!("Sorry {reply_to}, only tournament organizers can do that.")
                }).await?;
            } else {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, cette commande n'est disponible que pour les races officielles.")
                } else {
                    format!("Sorry {reply_to}, this command is only available for official races.")
                }).await?;
            },
            "no" => self.draft_action(ctx, msg.user.as_ref(), draft::Action::BooleanChoice(false)).await?,
            "presets" => goal.send_presets(ctx).await?,
            "ready" => if let Some(OfficialRaceData { ref mut restreams, ref cal_event, ref event, .. }) = self.official_data {
                if let Some(state) = restreams.values_mut().find(|state| state.restreamer_racetime_id.as_ref() == Some(&msg.user.as_ref().expect("received !ready command from bot").id)) {
                    state.ready = true;
                } else {
                    ctx.say(if let French = goal.language() {
                        format!("Désolé {reply_to}, seuls les restreamers peuvent faire cela.")
                    } else {
                        format!("Sorry {reply_to}, only restreamers can do that.")
                    }).await?;
                    return Ok(())
                }
                if restreams.values().all(|state| state.ready) {
                    ctx.say(if_chain! {
                        if let French = goal.language();
                        if let Ok((_, state)) = restreams.iter().exactly_one();
                        if let Some(French) = state.language;
                        then {
                            "Restream prêt. Déverrouillage de l'auto-start."
                        } else {
                            "All restreams ready, unlocking auto-start…"
                        }
                    }).await?;
                    let (access_token, _) = racetime::authorize_with_host(&ctx.global_state.host_info, &ctx.global_state.racetime_config.client_id, &ctx.global_state.racetime_config.client_secret, &ctx.global_state.http_client).await?;
                    racetime::StartRace {
                        goal: goal.as_str().to_owned(),
                        goal_is_custom: goal.is_custom(),
                        team_race: event.team_config.is_racetime_team_format(),
                        invitational: !matches!(cal_event.race.entrants, Entrants::Open),
                        unlisted: cal_event.is_private_async_part(),
                        info_user: ctx.data().await.info_user.clone().unwrap_or_default(),
                        info_bot: ctx.data().await.info_bot.clone().unwrap_or_default(),
                        require_even_teams: true,
                        start_delay: 15,
                        time_limit: 24,
                        time_limit_auto_complete: false,
                        streaming_required: !cal_event.is_private_async_part(),
                        auto_start: true,
                        allow_comments: true,
                        hide_comments: true,
                        allow_prerace_chat: true,
                        allow_midrace_chat: true,
                        allow_non_entrant_chat: false,
                        chat_message_delay: 0,
                    }.edit_with_host(&ctx.global_state.host_info, &access_token, &ctx.global_state.http_client, CATEGORY, &ctx.data().await.slug).await?;
                } else {
                    ctx.say(format!("Restream ready, still waiting for other restreams.")).await?;
                }
            } else {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, cette commande n'est disponible que pour les races officielles.")
                } else {
                    format!("Sorry {reply_to}, this command is only available for official races.")
                }).await?;
            },
            "restreamer" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                if let Some(OfficialRaceData { ref mut restreams, ref cal_event, ref event, .. }) = self.official_data {
                    if let [restream_url, restreamer] = &args[..] {
                        let restream_url = if restream_url.contains('/') {
                            Url::parse(restream_url)
                        } else {
                            Url::parse(&format!("https://twitch.tv/{restream_url}"))
                        };
                        if let Ok(restream_url) = restream_url {
                            let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
                            match parse_user(&mut transaction, &ctx.global_state.http_client, ctx.global_state.env.racetime_host(), restreamer).await {
                                Ok(restreamer_racetime_id) => {
                                    if restreams.is_empty() {
                                        let (access_token, _) = racetime::authorize_with_host(&ctx.global_state.host_info, &ctx.global_state.racetime_config.client_id, &ctx.global_state.racetime_config.client_secret, &ctx.global_state.http_client).await?;
                                        racetime::StartRace {
                                            goal: goal.as_str().to_owned(),
                                            goal_is_custom: goal.is_custom(),
                                            team_race: event.team_config.is_racetime_team_format(),
                                            invitational: !matches!(cal_event.race.entrants, Entrants::Open),
                                            unlisted: cal_event.is_private_async_part(),
                                            info_user: ctx.data().await.info_user.clone().unwrap_or_default(),
                                            info_bot: ctx.data().await.info_bot.clone().unwrap_or_default(),
                                            require_even_teams: true,
                                            start_delay: 15,
                                            time_limit: 24,
                                            time_limit_auto_complete: false,
                                            streaming_required: !cal_event.is_private_async_part(),
                                            auto_start: false,
                                            allow_comments: true,
                                            hide_comments: true,
                                            allow_prerace_chat: true,
                                            allow_midrace_chat: true,
                                            allow_non_entrant_chat: false,
                                            chat_message_delay: 0,
                                        }.edit_with_host(&ctx.global_state.host_info, &access_token, &ctx.global_state.http_client, CATEGORY, &ctx.data().await.slug).await?;
                                    }
                                    restreams.entry(restream_url).or_default().restreamer_racetime_id = Some(restreamer_racetime_id.clone());
                                    ctx.say("Restreamer assigned. Use “!ready” once the restream is ready. Auto-start will be unlocked once all restreams are ready.").await?; //TODO mention restreamer
                                }
                                Err(e) => ctx.say(format!("Sorry {reply_to}, I couldn't parse the restreamer: {e}")).await?,
                            }
                            transaction.commit().await.to_racetime()?;
                        } else {
                            ctx.say(format!("Sorry {reply_to}, that doesn't seem to be a valid URL or Twitch channel.")).await?;
                        }
                    } else {
                        ctx.say(format!("Sorry {reply_to}, I don't recognize that format for adding a restreamer.")).await?; //TODO better help message
                    }
                } else {
                    ctx.say(if let French = goal.language() {
                        format!("Désolé {reply_to}, cette commande n'est disponible que pour les races officielles.")
                    } else {
                        format!("Sorry {reply_to}, this command is only available for official races.")
                    }).await?;
                }
            } else {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                } else {
                    format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                }).await?;
            },
            "score" => if_chain! {
                if let Goal::TriforceBlitz | Goal::TriforceBlitzProgressionSpoiler = goal;
                if let Some(OfficialRaceData { ref mut scores, .. }) = self.official_data;
                then {
                    if let Some(UserData { ref id, .. }) = msg.user {
                        if let Some(score) = scores.get_mut(id) {
                            let old_score = *score;
                            if_chain! {
                                if let Some((pieces, duration)) = args.split_first();
                                if let Ok(pieces) = pieces.parse();
                                if pieces <= 3;
                                then {
                                    let new_score = tfb::Score {
                                        last_collection_time: if pieces == 0 {
                                            Duration::default()
                                        } else {
                                            let Some(last_collection_time) = parse_duration(&duration.join(" "), DurationUnit::Hours) else {
                                                ctx.say(format!("Sorry {reply_to}, I don't recognize that time format. Example format: 1h23m45s")).await?;
                                                return Ok(())
                                            };
                                            last_collection_time
                                        },
                                        pieces,
                                    };
                                    *score = Some(new_score);
                                    ctx.say(if let Some(old_score) = old_score {
                                        format!("Score edited: {new_score} (was: {old_score})")
                                    } else {
                                        format!("Score reported: {new_score}")
                                    }).await?;
                                    self.check_tfb_finish(ctx).await?;
                                } else {
                                    ctx.send_message(
                                        &format!("Sorry {reply_to}, I didn't quite understand that. Please use this button to try again:"),
                                        false,
                                        vec![tfb::report_score_button(None)],
                                    ).await?;
                                }
                            }
                        } else {
                            ctx.say(format!("Sorry {reply_to}, only entrants who have already finished can do that.")).await?;
                        }
                    } else {
                        ctx.say(format!("Sorry {reply_to}, I was unable to read your user ID.")).await?;
                    }
                } else {
                    ctx.say(format!("Sorry {reply_to}, this command is only available for official Triforce Blitz races.")).await?;
                }
            },
            "second" => self.draft_action(ctx, msg.user.as_ref(), draft::Action::GoFirst(false)).await?,
            "seed" | "spoilerseed" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                lock!(@write state = self.race_state; match *state {
                    RaceState::Init => if self.locked && !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.say(if let French = goal.language() {
                            format!("Désolé {reply_to}, la race est verrouillée. Seuls {} peuvent générer une seed pour cette race.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                        } else {
                            format!("Sorry {reply_to}, seed rolling is locked. Only {} may roll a seed for this race.", if self.is_official() { "race monitors or tournament organizers" } else { "race monitors" })
                        }).await?;
                    } else {
                        let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
                        match goal.parse_seed_command(&mut transaction, &ctx.global_state, self.is_official(), cmd_name.to_ascii_lowercase() == "spoilerseed", &args).await.to_racetime()? {
                            SeedCommandParseResult::Regular { settings, unlock_spoiler_log, language, article, description } => self.roll_seed(ctx, goal.preroll_seeds(), goal.rando_version(), settings, unlock_spoiler_log, language, article, description).await,
                            SeedCommandParseResult::Rsl { preset, world_count, unlock_spoiler_log, language, article, description } => self.roll_rsl_seed(ctx, preset, world_count, unlock_spoiler_log, language, article, description).await,
                            SeedCommandParseResult::Tfb { version, unlock_spoiler_log, language, article, description } => self.roll_tfb_seed(ctx, version, unlock_spoiler_log, language, article, description).await,
                            SeedCommandParseResult::QueueExisting { data, language, article, description } => self.queue_existing_seed(ctx, data, language, article, description).await,
                            SeedCommandParseResult::SendPresets { language, msg } => {
                                ctx.say(if let French = language {
                                    format!("Désolé {reply_to}, {msg}. Veuillez utiliser un des suivants :")
                                } else {
                                    format!("Sorry {reply_to}, {msg}. Use one of the following:")
                                }).await?;
                                goal.send_presets(ctx).await?;
                            }
                            SeedCommandParseResult::SendSettings { language, msg } => {
                                unlock!();
                                self.send_settings(ctx, &if let French = language {
                                    format!("Désolé {reply_to}, {msg}")
                                } else {
                                    format!("Sorry {reply_to}, {msg}")
                                }, reply_to).await?;
                                return Ok(())
                            }
                            SeedCommandParseResult::StartDraft { new_state, unlock_spoiler_log } => {
                                *state = RaceState::Draft {
                                    state: new_state,
                                    unlock_spoiler_log,
                                };
                                self.advance_draft(ctx, &state).await?;
                            }
                            SeedCommandParseResult::Error { language, msg } => ctx.say(if let French = language {
                                format!("Désolé {reply_to}, {msg}")
                            } else {
                                format!("Sorry {reply_to}, {msg}")
                            }).await?,
                        }
                        transaction.commit().await.to_racetime()?;
                    },
                    RaceState::Draft { .. } => ctx.say(format!("Sorry {reply_to}, settings are already being drafted.")).await?,
                    RaceState::Rolling => ctx.say(format!("Sorry {reply_to}, but I'm already rolling a seed for this room. Please wait.")).await?,
                    RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.say(format!("Sorry {reply_to}, but I already rolled a seed. Check the race info!")).await?,
                });
            } else {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, mais la race a débuté.")
                } else {
                    format!("Sorry {reply_to}, but the race has already started.")
                }).await?;
            },
            "settings" => lock!(@read state = self.race_state; self.send_settings(ctx, if let RaceState::Draft { .. } = *state {
                if let French = goal.language() {
                    "Settings pouvant être actuellement choisis :"
                } else {
                    "Currently draftable settings:"
                }
            } else {
                if let French = goal.language() {
                    "Settings pouvant être choisis :"
                } else {
                    "Draftable settings:"
                }
            }, reply_to).await?),
            "skip" => self.draft_action(ctx, msg.user.as_ref(), draft::Action::Skip).await?,
            "unlock" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                self.locked = false;
                ctx.say(if let French = goal.language() {
                    "Race déverrouillée. N'importe qui peut désormais générer une seed."
                } else {
                    "Lock released. Anyone may now roll a seed."
                }).await?;
            } else {
                ctx.say(if let French = goal.language() {
                    format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                } else {
                    format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                }).await?;
            },
            "yes" => self.draft_action(ctx, msg.user.as_ref(), draft::Action::BooleanChoice(true)).await?,
            _ => ctx.say(if let French = goal.language() {
                format!("Désolé {reply_to}, je ne reconnais pas cette commande.")
            } else {
                format!("Sorry {reply_to}, I don't recognize that command.")
            }).await?, //TODO “did you mean”? list of available commands with !help?
        }
        Ok(())
    }

    async fn race_data(&mut self, ctx: &RaceContext<GlobalState>, _old_race_data: RaceData) -> Result<(), Error> {
        let data = ctx.data().await;
        let goal = self.goal(ctx).await.to_racetime()?;
        if let Some(OfficialRaceData { ref entrants, ref mut scores, .. }) = self.official_data {
            for entrant in &data.entrants {
                match entrant.status.value {
                    EntrantStatusValue::Requested => if entrants.contains(&entrant.user.id) {
                        ctx.accept_request(&entrant.user.id).await?;
                    },
                    EntrantStatusValue::Done => if let Goal::TriforceBlitz | Goal::TriforceBlitzProgressionSpoiler = goal {
                        if let hash_map::Entry::Vacant(entry) = scores.entry(entrant.user.id.clone()) {
                            let reply_to = &entrant.user.name;
                            ctx.send_message(
                                &format!("{reply_to}, please report your score:"),
                                false,
                                vec![tfb::report_score_button(entrant.finish_time)],
                            ).await?;
                            entry.insert(None);
                        }
                    },
                    _ => {}
                }
            }
        }
        if !self.start_saved {
            if let (Goal::Rsl, Some(start)) = (goal, data.started_at) {
                sqlx::query!("UPDATE rsl_seeds SET start = $1 WHERE room = $2", start, format!("https://{}{}", ctx.global_state.env.racetime_host(), ctx.data().await.url)).execute(&ctx.global_state.db_pool).await.to_racetime()?;
                self.start_saved = true;
            }
        }
        match data.status.value {
            RaceStatusValue::InProgress => {
                if let Some(breaks) = self.breaks {
                    self.break_notifications.get_or_insert_with(|| {
                        let ctx = ctx.clone();
                        tokio::spawn(async move {
                            sleep(breaks.interval - Duration::from_secs(5 * 60)).await;
                            while Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), Some(None)).await {
                                let (_, ()) = tokio::join!(
                                    ctx.say(if let French = goal.language() {
                                        "@entrants Rappel : pause dans 5 minutes."
                                    } else {
                                        "@entrants Reminder: Next break in 5 minutes."
                                    }),
                                    sleep(Duration::from_secs(5 * 60)),
                                );
                                if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), Some(None)).await { break }
                                let msg = if let French = goal.language() {
                                    format!("@entrants C'est l'heure de la pause ! Elle durera {}.", French.format_duration(breaks.duration, true))
                                } else {
                                    format!("@entrants Break time! Please pause for {}.", English.format_duration(breaks.duration, true))
                                };
                                let (_, ()) = tokio::join!(
                                    ctx.say(msg),
                                    sleep(breaks.duration),
                                );
                                if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), Some(None)).await { break }
                                let (_, ()) = tokio::join!(
                                    ctx.say(if let French = goal.language() {
                                        "@entrants Fin de la pause. Vous pouvez recommencer à jouer."
                                    } else {
                                        "@entrants Break ended. You may resume playing."
                                    }),
                                    sleep(breaks.interval - breaks.duration - Duration::from_secs(5 * 60)),
                                );
                            }
                        })
                    });
                }
                match goal {
                    Goal::Pic7 | Goal::PicRs2 => {
                        self.goal_notifications.get_or_insert_with(|| {
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                let initial_wait = ctx.data().await.started_at.expect("in-progress race with no start time") + TimeDelta::minutes(match goal {
                                    Goal::Pic7 => 10,
                                    Goal::PicRs2 => 25,
                                    _ => unreachable!(),
                                }) - Utc::now();
                                if let Ok(initial_wait) = initial_wait.to_std() {
                                    sleep(initial_wait).await;
                                    if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), Some(None)).await { return }
                                    let (_, ()) = tokio::join!(
                                        ctx.say("@entrants Reminder: 5 minutes until you can start drawing/playing."),
                                        sleep(Duration::from_secs(5 * 60)),
                                    );
                                    let _ = ctx.say("@entrants You may now start drawing/playing.").await;
                                }
                            })
                        });
                    }
                    Goal::TriforceBlitz => if ctx.data().await.time_limit > Duration::from_secs(2 * 60 * 60) {
                        self.goal_notifications.get_or_insert_with(|| {
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                let initial_wait = ctx.data().await.started_at.expect("in-progress race with no start time") + TimeDelta::hours(2) - Utc::now();
                                if let Ok(initial_wait) = initial_wait.to_std() {
                                    sleep(initial_wait).await;
                                    let is_1v1 = {
                                        let data = ctx.data().await;
                                        if !Self::should_handle_inner(&*data, ctx.global_state.clone(), Some(None)).await { return }
                                        data.entrants_count == 2
                                    };
                                    let _ = ctx.say(if is_1v1 {
                                        "@entrants Time limit reached. If anyone has found at least 1 Triforce piece, please .done. If neither player has any pieces, please continue and .done when one is found."
                                    } else {
                                        "@entrants Time limit reached. If you've found at least 1 Triforce piece, please mark yourself as done. If you haven't, you may continue playing until you find one."
                                    }).await;
                                }
                            })
                        });
                    },
                    Goal::TriforceBlitzProgressionSpoiler => {
                        self.goal_notifications.get_or_insert_with(|| {
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                let initial_wait = ctx.data().await.started_at.expect("in-progress race with no start time") + TimeDelta::minutes(10) - Utc::now();
                                if let Ok(initial_wait) = initial_wait.to_std() {
                                    sleep(initial_wait).await;
                                    if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), Some(None)).await { return }
                                    let (_, ()) = tokio::join!(
                                        ctx.say("@entrants Reminder: 5 minutes until you can start playing."),
                                        sleep(Duration::from_secs(5 * 60)),
                                    );
                                    let (_, ()) = tokio::join!(
                                        ctx.say("@entrants You may now start playing."),
                                        sleep(Duration::from_secs((60 + 45) * 60)),
                                    );
                                    let is_1v1 = {
                                        let data = ctx.data().await;
                                        if !Self::should_handle_inner(&*data, ctx.global_state.clone(), Some(None)).await { return }
                                        data.entrants_count == 2
                                    };
                                    let _ = ctx.say(if is_1v1 {
                                        "@entrants Time limit reached. If anyone has found at least 1 Triforce piece, please .done. If neither player has any pieces, please continue and .done when one is found."
                                    } else {
                                        "@entrants Time limit reached. If you've found at least 1 Triforce piece, please mark yourself as done. If you haven't, you may continue playing until you find one."
                                    }).await;
                                }
                            })
                        });
                    }
                    | Goal::Cc7
                    | Goal::CoOpS3
                    | Goal::CopaDoBrasil
                    | Goal::MixedPoolsS2
                    | Goal::MixedPoolsS3
                    | Goal::MultiworldS3
                    | Goal::MultiworldS4
                    | Goal::NineDaysOfSaws
                    | Goal::Rsl
                    | Goal::Sgl2023
                    | Goal::Sgl2024
                    | Goal::SongsOfHope
                    | Goal::StandardRuleset
                    | Goal::TournoiFrancoS3
                    | Goal::TournoiFrancoS4
                    | Goal::WeTryToBeBetter
                        => {}
                }
            }
            RaceStatusValue::Finished => if self.unlock_spoiler_log(ctx, goal).await? {
                if let Goal::TriforceBlitz | Goal::TriforceBlitzProgressionSpoiler = goal {
                    self.check_tfb_finish(ctx).await?;
                } else {
                    if let Some(OfficialRaceData { ref cal_event, ref event, fpa_invoked, .. }) = self.official_data {
                        self.official_race_finished(ctx, data, cal_event, event, fpa_invoked, None).await?;
                    }
                }
            },
            RaceStatusValue::Cancelled => {
                if let Some(OfficialRaceData { ref event, .. }) = self.official_data {
                    if let Some(organizer_channel) = event.discord_organizer_channel {
                        organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, MessageBuilder::default()
                            //TODO mention organizer role
                            .push("race cancelled: <https://")
                            .push(ctx.global_state.env.racetime_host())
                            .push(&ctx.data().await.url)
                            .push('>')
                            .build()
                        ).await.to_racetime()?;
                    }
                }
                self.unlock_spoiler_log(ctx, goal).await?;
                if let Goal::Rsl = goal {
                    sqlx::query!("DELETE FROM rsl_seeds WHERE room = $1", format!("https://{}{}", ctx.global_state.env.racetime_host(), ctx.data().await.url)).execute(&ctx.global_state.db_pool).await.to_racetime()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn error(&mut self, _: &RaceContext<GlobalState>, mut errors: Vec<String>) -> Result<(), Error> {
        errors.retain(|error|
            !error.ends_with(" is not allowed to join this race.") // failing to invite a user should not crash the race handler
            && !error.ends_with(" is already an entrant.") // failing to invite a user should not crash the race handler
            && error != "This user has not requested to join this race. Refresh to continue." // a join request may be accepted multiple times if multiple race data changes happen in quick succession
        );
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Server(errors))
        }
    }
}

pub(crate) async fn create_room(transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, host_info: &racetime::HostInfo, client_id: &str, client_secret: &str, extra_room_tx: &RwLock<mpsc::Sender<String>>, http_client: &reqwest::Client, cal_event: &cal::Event, event: &event::Data<'_>) -> Result<Option<String>, Error> {
    let Some(goal) = Goal::for_event(cal_event.race.series, &cal_event.race.event) else { return Ok(None) };
    match racetime::authorize_with_host(host_info, client_id, client_secret, http_client).await {
        Ok((access_token, _)) => {
            let info_user = if_chain! {
                if let French = event.language;
                if let (Some(phase), Some(round)) = (cal_event.race.phase.as_ref(), cal_event.race.round.as_ref());
                if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut **transaction).await.to_racetime()?;
                if cal_event.race.game.is_none();
                if let Some(entrants) = match cal_event.race.entrants {
                    Entrants::Open | Entrants::Count { .. } => Some(None), // no text
                    Entrants::Named(ref entrants) => Some(Some(entrants.clone())),
                    Entrants::Two([ref team1, ref team2]) => match cal_event.kind {
                        cal::EventKind::Normal => if let (Some(team1), Some(team2)) = (team1.name(&mut *transaction, discord_ctx).await.to_racetime()?, team2.name(&mut *transaction, discord_ctx).await.to_racetime()?) {
                            Some(Some(format!("{team1} vs {team2}")))
                        } else {
                            None // no French translation available
                        },
                        cal::EventKind::Async1 | cal::EventKind::Async2 | cal::EventKind::Async3 => None,
                    },
                    Entrants::Three([ref team1, ref team2, ref team3]) => match cal_event.kind {
                        cal::EventKind::Normal => if let (Some(team1), Some(team2), Some(team3)) = (team1.name(&mut *transaction, discord_ctx).await.to_racetime()?, team2.name(&mut *transaction, discord_ctx).await.to_racetime()?, team3.name(&mut *transaction, discord_ctx).await.to_racetime()?) {
                            Some(Some(format!("{team1} vs {team2} vs {team3}")))
                        } else {
                            None // no French translation available
                        },
                        cal::EventKind::Async1 | cal::EventKind::Async2 | cal::EventKind::Async3 => None,
                    },
                };
                then {
                    if let Some(entrants) = entrants {
                        format!("{phase_round} : {entrants}")
                    } else {
                        phase_round
                    }
                } else {
                    let info_prefix = match (&cal_event.race.phase, &cal_event.race.round) {
                        (Some(phase), Some(round)) => Some(format!("{phase} {round}")),
                        (Some(phase), None) => Some(phase.clone()),
                        (None, Some(round)) => Some(round.clone()),
                        (None, None) => None,
                    };
                    let mut info_user = match cal_event.race.entrants {
                        Entrants::Open | Entrants::Count { .. } => info_prefix.clone().unwrap_or_default(),
                        Entrants::Named(ref entrants) => format!("{}{entrants}", info_prefix.as_ref().map(|prefix| format!("{prefix}: ")).unwrap_or_default()),
                        Entrants::Two([ref team1, ref team2]) => match cal_event.kind {
                            cal::EventKind::Normal => format!(
                                "{}{} vs {}",
                                info_prefix.as_ref().map(|prefix| format!("{prefix}: ")).unwrap_or_default(),
                                team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            ),
                            cal::EventKind::Async1 => format!(
                                "{} (async): {} vs {}",
                                info_prefix.clone().unwrap_or_default(),
                                team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            ),
                            cal::EventKind::Async2 => format!(
                                "{} (async): {} vs {}",
                                info_prefix.clone().unwrap_or_default(),
                                team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            ),
                            cal::EventKind::Async3 => unreachable!(),
                        },
                        Entrants::Three([ref team1, ref team2, ref team3]) => match cal_event.kind {
                            cal::EventKind::Normal => format!(
                                "{}{} vs {} vs {}",
                                info_prefix.as_ref().map(|prefix| format!("{prefix}: ")).unwrap_or_default(),
                                team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team3.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            ),
                            cal::EventKind::Async1 => format!(
                                "{} (async): {} vs {} vs {}",
                                info_prefix.clone().unwrap_or_default(),
                                team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team3.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            ),
                            cal::EventKind::Async2 => format!(
                                "{} (async): {} vs {} vs {}",
                                info_prefix.clone().unwrap_or_default(),
                                team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team3.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            ),
                            cal::EventKind::Async3 => format!(
                                "{} (async): {} vs {} vs {}",
                                info_prefix.clone().unwrap_or_default(),
                                team3.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                                team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            ),
                        },
                    };
                    if let Some(game) = cal_event.race.game {
                        info_user.push_str(", game ");
                        info_user.push_str(&game.to_string());
                    }
                    info_user
                }
            };
            let race_slug = racetime::StartRace {
                goal: goal.as_str().to_owned(),
                goal_is_custom: goal.is_custom(),
                team_race: event.team_config.is_racetime_team_format() && matches!(cal_event.kind, cal::EventKind::Normal),
                invitational: !matches!(cal_event.race.entrants, Entrants::Open),
                unlisted: cal_event.is_private_async_part(),
                info_bot: String::default(),
                require_even_teams: true,
                start_delay: 15,
                time_limit: 24,
                time_limit_auto_complete: false,
                streaming_required: !cal_event.is_private_async_part(),
                auto_start: cal_event.is_private_async_part() || cal_event.race.video_urls.is_empty(),
                allow_comments: true,
                hide_comments: true,
                allow_prerace_chat: true,
                allow_midrace_chat: true,
                allow_non_entrant_chat: false, // only affects the race while it's ongoing, so !monitor still works
                chat_message_delay: 0,
                info_user,
            }.start_with_host(host_info, &access_token, &http_client, CATEGORY).await?;
            let room_url = Url::parse(&format!("https://{}/{CATEGORY}/{race_slug}", host_info.hostname))?;
            match cal_event.kind {
                cal::EventKind::Normal => { sqlx::query!("UPDATE races SET room = $1 WHERE id = $2", room_url.to_string(), cal_event.race.id as _).execute(&mut **transaction).await.to_racetime()?; }
                cal::EventKind::Async1 => { sqlx::query!("UPDATE races SET async_room1 = $1 WHERE id = $2", room_url.to_string(), cal_event.race.id as _).execute(&mut **transaction).await.to_racetime()?; }
                cal::EventKind::Async2 => { sqlx::query!("UPDATE races SET async_room2 = $1 WHERE id = $2", room_url.to_string(), cal_event.race.id as _).execute(&mut **transaction).await.to_racetime()?; }
                cal::EventKind::Async3 => { sqlx::query!("UPDATE races SET async_room3 = $1 WHERE id = $2", room_url.to_string(), cal_event.race.id as _).execute(&mut **transaction).await.to_racetime()?; }
            }
            let msg = if_chain! {
                if let French = event.language;
                if let (Some(phase), Some(round)) = (cal_event.race.phase.as_ref(), cal_event.race.round.as_ref());
                if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut **transaction).await.to_racetime()?;
                if cal_event.race.game.is_none();
                then {
                    let mut msg = MessageBuilder::default();
                    msg.push("La race commence ");
                    msg.push_timestamp(cal_event.start().expect("opening room for official race without start time"), serenity_utils::message::TimestampStyle::Relative);
                    msg.push(" : ");
                    match cal_event.race.entrants {
                        Entrants::Open | Entrants::Count { .. } => {
                            msg.push_safe(phase_round);
                        },
                        Entrants::Named(ref entrants) => {
                            msg.push_safe(phase_round);
                            msg.push(" : ");
                            msg.push_safe(entrants);
                        }
                        Entrants::Two([ref team1, ref team2]) => {
                            msg.push_safe(phase_round);
                            //TODO adjust for asyncs
                            msg.push(" : ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team1).await.to_racetime()?;
                            msg.push(" vs ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team2).await.to_racetime()?;
                        }
                        Entrants::Three([ref team1, ref team2, ref team3]) => {
                            msg.push_safe(phase_round);
                            //TODO adjust for asyncs
                            msg.push(" : ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team1).await.to_racetime()?;
                            msg.push(" vs ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team2).await.to_racetime()?;
                            msg.push(" vs ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team3).await.to_racetime()?;
                        }
                    }
                    msg.push(" <");
                    msg.push(room_url);
                    msg.push('>');
                    msg.build()
                } else {
                    let is_weekly = event.series == Series::Standard && event.event == "w";
                    let info_prefix = match (&cal_event.race.phase, &cal_event.race.round) {
                        (Some(phase), Some(round)) => Some(format!("{phase} {round}")),
                        (Some(phase), None) => Some(phase.clone()),
                        (None, Some(round)) => Some(round.clone()),
                        (None, None) => None,
                    };
                    let mut msg = MessageBuilder::default();
                    if is_weekly {
                        msg.mention(&RoleId::new(640750480246571014)); // @Standard
                        msg.push(' ');
                    }
                    msg.push("race starting ");
                    msg.push_timestamp(cal_event.start().expect("opening room for official race without start time"), serenity_utils::message::TimestampStyle::Relative);
                    msg.push(": ");
                    match cal_event.race.entrants {
                        Entrants::Open | Entrants::Count { .. } => if let Some(prefix) = info_prefix {
                            msg.push_safe(prefix);
                        },
                        Entrants::Named(ref entrants) => {
                            if let Some(prefix) = info_prefix {
                                msg.push_safe(prefix);
                                msg.push(": ");
                            }
                            msg.push_safe(entrants);
                        }
                        Entrants::Two([ref team1, ref team2]) => {
                            if let Some(prefix) = info_prefix {
                                msg.push_safe(prefix);
                                //TODO adjust for asyncs
                                msg.push(": ");
                            }
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team1).await.to_racetime()?;
                            msg.push(" vs ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team2).await.to_racetime()?;
                        }
                        Entrants::Three([ref team1, ref team2, ref team3]) => {
                            if let Some(prefix) = info_prefix {
                                msg.push_safe(prefix);
                                //TODO adjust for asyncs
                                msg.push(": ");
                            }
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team1).await.to_racetime()?;
                            msg.push(" vs ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team2).await.to_racetime()?;
                            msg.push(" vs ");
                            msg.mention_entrant(&mut *transaction, event.discord_guild, team3).await.to_racetime()?;
                        }
                    }
                    if let Some(game) = cal_event.race.game {
                        msg.push(", game ");
                        msg.push(game.to_string());
                    }
                    msg.push(' ');
                    if !is_weekly {
                        msg.push('<');
                    }
                    msg.push(room_url);
                    if !is_weekly {
                        msg.push('>');
                    }
                    msg.build()
                }
            };
            lock!(@read extra_room_tx = extra_room_tx; { let _ = extra_room_tx.send(race_slug).await; });
            Ok(Some(msg))
        }
        Err(Error::Reqwest(e)) if e.status().map_or(false, |status| status.is_server_error()) => {
            // racetime.gg's auth endpoint has been known to return server errors intermittently.
            // In that case, we simply try again in the next iteration of the sleep loop.
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

async fn prepare_seeds(global_state: Arc<GlobalState>, mut seed_cache_rx: watch::Receiver<()>, mut shutdown: rocket::Shutdown) -> Result<(), Error> {
    'outer: loop {
        let event_rows = sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE end_time IS NULL OR end_time > NOW()"#).fetch_all(&global_state.db_pool).await.to_racetime()?;
        for goal in all::<Goal>() {
            if let Some(settings) = goal.single_settings() {
                if goal.preroll_seeds() == PrerollMode::Long && event_rows.iter().any(|row| goal.matches_event(row.series, &row.event)) {
                    if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM prerolled_seeds WHERE goal_name = $1) AS "exists!""#, goal.as_str()).fetch_one(&global_state.db_pool).await.to_racetime()? {
                        'seed: loop {
                            let mut seed_rx = global_state.clone().roll_seed(
                                PrerollMode::Long,
                                false,
                                None,
                                goal.rando_version(),
                                settings.clone(),
                                UnlockSpoilerLog::After,
                            );
                            loop {
                                select! {
                                    () = &mut shutdown => break 'outer,
                                    Some(update) = seed_rx.recv() => match update {
                                        SeedRollUpdate::Queued(_) |
                                        SeedRollUpdate::MovedForward(_) |
                                        SeedRollUpdate::Started => {}
                                        SeedRollUpdate::Done { seed, rsl_preset: _, unlock_spoiler_log: _ } => {
                                            let extra = seed.extra(Utc::now()).await.to_racetime()?;
                                            let [hash1, hash2, hash3, hash4, hash5] = match extra.file_hash {
                                                Some(hash) => hash.map(Some),
                                                None => [None; 5],
                                            };
                                            match seed.files {
                                                Some(seed::Files::MidosHouse { file_stem, locked_spoiler_log_path }) => {
                                                    sqlx::query!("INSERT INTO prerolled_seeds
                                                        (goal_name, file_stem, locked_spoiler_log_path, hash1, hash2, hash3, hash4, hash5)
                                                    VALUES
                                                        ($1, $2, $3, $4, $5, $6, $7, $8)
                                                    ", goal.as_str(), &file_stem, locked_spoiler_log_path, hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _).execute(&global_state.db_pool).await.to_racetime()?;
                                                }
                                                _ => unimplemented!("unexpected seed files in prerolled seed"),
                                            }
                                            break 'seed
                                        }
                                        SeedRollUpdate::Error(RollError::Retries { num_retries, last_error }) => {
                                            if let Some(last_error) = last_error {
                                                eprintln!("seed rolling failed {num_retries} times, sample error:\n{last_error}");
                                            } else {
                                                eprintln!("seed rolling failed {num_retries} times, no sample error recorded");
                                            }
                                            continue 'seed
                                        }
                                        SeedRollUpdate::Error(e) => return Err(e).to_racetime(),
                                        #[cfg(unix)] SeedRollUpdate::Message(_) => {}
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
        select! {
            () = &mut shutdown => break,
            res = timeout(Duration::from_secs(24 * 60 * 60), seed_cache_rx.changed().then(|res| if let Ok(()) = res { Either::Left(future::ready(())) } else { Either::Right(future::pending()) })) => {
                let (Ok(()) | Err(_)) = res;
            }
        }
    }
    Ok(())
}

async fn create_rooms(global_state: Arc<GlobalState>, mut shutdown: rocket::Shutdown) -> Result<(), Error> {
    loop {
        select! {
            () = &mut shutdown => break,
            _ = sleep(Duration::from_secs(30)) => { //TODO exact timing (coordinate with everything that can change the schedule)
                lock!(new_room_lock = global_state.new_room_lock; { // make sure a new room isn't handled before it's added to the database
                    let mut transaction = global_state.db_pool.begin().await.to_racetime()?;
                    let rooms_to_open = cal::Event::rooms_to_open(&mut transaction, &global_state.http_client).await.to_racetime()?;
                    for cal_event in rooms_to_open {
                        let event = cal_event.race.event(&mut transaction).await.to_racetime()?;
                        if !cal_event.should_create_room(&mut transaction, &event).await.to_racetime()? { continue }
                        if let Some(msg) = create_room(&mut transaction, &*global_state.discord_ctx.read().await, &global_state.host_info, &global_state.racetime_config.client_id, &global_state.racetime_config.client_secret, &global_state.extra_room_tx, &global_state.http_client, &cal_event, &event).await? {
                            let ctx = global_state.discord_ctx.read().await;
                            if cal_event.is_private_async_part() {
                                let msg = match cal_event.race.entrants {
                                    Entrants::Two(_) => format!("unlisted room for first async half: {msg}"),
                                    Entrants::Three(_) => format!("unlisted room for first/second async part: {msg}"),
                                    _ => format!("unlisted room for async part: {msg}"),
                                };
                                if let Some(channel) = event.discord_organizer_channel {
                                    channel.say(&*ctx, &msg).await.to_racetime()?;
                                } else {
                                    // DM Fenhl
                                    UserId::new(86841168427495424).create_dm_channel(&*ctx).await.to_racetime()?.say(&*ctx, &msg).await.to_racetime()?;
                                }
                                for team in cal_event.active_teams() {
                                    for member in team.members(&mut transaction).await.to_racetime()? {
                                        if let Some(discord) = member.discord {
                                            discord.id.create_dm_channel(&*ctx).await.to_racetime()?.say(&*ctx, &msg).await.to_racetime()?;
                                        }
                                    }
                                }
                            } else {
                                if let Some(channel) = event.discord_race_room_channel {
                                    if let Some(thread) = cal_event.race.scheduling_thread {
                                        thread.say(&*ctx, &msg).await.to_racetime()?;
                                        channel.send_message(&*ctx, CreateMessage::default().content(msg).allowed_mentions(CreateAllowedMentions::default())).await.to_racetime()?;
                                    } else {
                                        channel.say(&*ctx, msg).await.to_racetime()?;
                                    }
                                } else if let Some(thread) = cal_event.race.scheduling_thread {
                                    thread.say(&*ctx, msg).await.to_racetime()?;
                                } else if let Some(channel) = event.discord_organizer_channel {
                                    channel.say(&*ctx, msg).await.to_racetime()?;
                                } else {
                                    // DM Fenhl
                                    UserId::new(86841168427495424).create_dm_channel(&*ctx).await.to_racetime()?.say(&*ctx, msg).await.to_racetime()?;
                                }
                            }
                        }
                    }
                    transaction.commit().await.to_racetime()?;
                });
            }
        }
    }
    Ok(())
}

async fn handle_rooms(global_state: Arc<GlobalState>, racetime_config: &ConfigRaceTime, shutdown: rocket::Shutdown) -> Result<(), Error> {
    let mut last_crash = Instant::now();
    let mut wait_time = Duration::from_secs(1);
    loop {
        match racetime::Bot::new_with_host(global_state.host_info.clone(), CATEGORY, &racetime_config.client_id, &racetime_config.client_secret, global_state.clone()).await {
            Ok(bot) => {
                lock!(@write extra_room_tx = global_state.extra_room_tx; *extra_room_tx = bot.extra_room_sender());
                let () = bot.run_until::<Handler, _, _>(shutdown).await?;
                break Ok(())
            }
            Err(e) if e.is_network_error() => {
                if last_crash.elapsed() >= Duration::from_secs(60 * 60 * 24) {
                    wait_time = Duration::from_secs(1); // reset wait time after no crash for a day
                } else {
                    wait_time *= 2; // exponential backoff
                }
                eprintln!("failed to connect to racetime.gg (retrying in {}): {e} ({e:?})", English.format_duration(wait_time, true));
                if wait_time >= Duration::from_secs(16) {
                    wheel::night_report("/net/midoshouse/error", Some(&format!("failed to connect to racetime.gg (retrying in {}): {e} ({e:?})", English.format_duration(wait_time, true)))).await.to_racetime()?;
                }
                sleep(wait_time).await;
                last_crash = Instant::now();
            }
            Err(e) => {
                wheel::night_report("/net/midoshouse/error", Some(&format!("error handling racetime.gg rooms: {e} ({e:?})"))).await.to_racetime()?;
                break Err(e)
            }
        }
    }
}

pub(crate) async fn main(env: Environment, config: Config, shutdown: rocket::Shutdown, global_state: Arc<GlobalState>, seed_cache_rx: watch::Receiver<()>) -> Result<(), Error> {
    let ((), (), ()) = tokio::try_join!(
        prepare_seeds(global_state.clone(), seed_cache_rx, shutdown.clone()),
        create_rooms(global_state.clone(), shutdown.clone()),
        handle_rooms(global_state, if env.is_dev() { &config.racetime_bot_dev } else { &config.racetime_bot_production }, shutdown),
    )?;
    Ok(())
}
