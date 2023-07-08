use {
    std::{
        borrow::Cow,
        collections::{
            HashMap,
            HashSet,
        },
        fmt,
        io::prelude::*,
        iter,
        path::{
            Path,
            PathBuf,
        },
        pin::pin,
        process::Stdio,
        str::FromStr,
        sync::Arc,
        time::Duration,
    },
    async_trait::async_trait,
    chrono::prelude::*,
    collect_mac::collect,
    enum_iterator::{
        Sequence,
        all,
    },
    futures::{
        future::FutureExt as _,
        stream::TryStreamExt as _,
    },
    git2::{
        BranchType,
        Repository,
        ResetType,
    },
    if_chain::if_chain,
    itertools::Itertools as _,
    kuchiki::{
        NodeRef,
        traits::TendrilSink as _,
    },
    lazy_regex::{
        regex_captures,
        regex_is_match,
    },
    ootr_utils::{
        self as rando,
        spoiler::HashIcon,
    },
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
    serde::{
        Deserialize,
        Serialize,
    },
    serde_json::{
        Value as Json,
        json,
    },
    serde_with::{
        DisplayFromStr,
        json::JsonString,
        serde_as,
    },
    serenity::all::{
        Context as DiscordCtx,
        MessageBuilder,
        UserId,
    },
    serenity_utils::{
        RwFuture,
        message::MessageBuilderExt as _,
    },
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
    tokio::{
        io::{
            self,
            AsyncBufReadExt as _,
            AsyncWriteExt as _,
            BufReader,
        },
        process::Command,
        select,
        sync::{
            Notify,
            Semaphore,
            TryAcquireError,
            mpsc,
        },
        time::{
            Instant,
            sleep,
            sleep_until,
        },
    },
    tokio_util::io::StreamReader,
    url::Url,
    wheel::{
        fs::{
            self,
            File,
        },
        traits::{
            IoResultExt as _,
            ReqwestResponseExt as _,
        },
    },
    crate::{
        Environment,
        cal::{
            self,
            Entrant,
            Entrants,
        },
        config::{
            Config,
            ConfigRaceTime,
        },
        draft::{
            self,
            Draft,
        },
        event::{
            self,
            Series,
            TeamConfig,
        },
        lang::Language::{
            self,
            *,
        },
        seed,
        series::*,
        team::Team,
        user::User,
        util::{
            DurationUnit,
            Id,
            MessageBuilderExt as _,
            io_error_from_reqwest,
            parse_duration,
            sync::{
                ArcRwLock,
                Mutex,
                OwnedRwLockWriteGuard,
                RwLock,
                lock,
            },
        },
    },
};
#[cfg(unix)] use {
    async_proto::Protocol,
    xdg::BaseDirectories,
};

#[cfg(unix)] const PYTHON: &str = "python3";
#[cfg(windows)] const PYTHON: &str = "py";

pub(crate) const CATEGORY: &str = "ootr";

/// Randomizer versions that are known to exist on the ootrandomizer.com API. Hardcoded because the API doesn't have a “does version x exist?” endpoint.
const KNOWN_GOOD_WEB_VERSIONS: [rando::Version; 6] = [
    rando::Version::from_dev(6, 2, 181),
    rando::Version::from_dev(6, 2, 205),
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
            Err(wheel::Error::ResponseStatus { inner, .. }) if inner.status() == Some(reqwest::StatusCode::NOT_FOUND) => Err(ParseUserError::IdNotFound),
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
                    Err(wheel::Error::ResponseStatus { inner, .. }) if inner.status() == Some(reqwest::StatusCode::NOT_FOUND) => Err(ParseUserError::UrlNotFound),
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
}

impl VersionedBranch {
    fn branch(&self) -> rando::Branch {
        match self {
            Self::Pinned(version) => version.branch(),
            Self::Latest(branch) => *branch,
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
            rando::Branch::DevR => Self::Xopar { version: Some(version.base().clone()), preset: preset.map(rsl::Preset::from_str).transpose()?.unwrap_or_default() },
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
                    Self::Fenhl { version: None, .. } => Cow::Borrowed(Path::new("/opt/git/github.com/fenhl/plando-random-settings/master")),
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

#[derive(Clone, Copy, Sequence)]
pub(crate) enum Goal {
    CopaDoBrasil,
    MixedPoolsS2,
    MultiworldS3,
    NineDaysOfSaws,
    PicRs2,
    Rsl,
    Sgl2023,
    TournoiFrancoS3,
    TriforceBlitz,
}

#[derive(Debug, thiserror::Error)]
#[error("this racetime.gg goal is not handled by Mido")]
pub(crate) struct GoalFromStrError;

impl Goal {
    pub(crate) fn for_event(series: Series, event: &str) -> Option<Self> {
        all::<Self>().find(|goal| goal.matches_event(series, event))
    }

    fn matches_event(&self, series: Series, event: &str) -> bool {
        match self {
            Self::CopaDoBrasil => series == Series::CopaDoBrasil && event == "1",
            Self::MixedPoolsS2 => series == Series::MixedPools && event == "2",
            Self::MultiworldS3 => series == Series::Multiworld && event == "3",
            Self::NineDaysOfSaws => series == Series::NineDaysOfSaws,
            Self::PicRs2 => series == Series::Pictionary && event == "rs2",
            Self::Rsl => series == Series::Rsl,
            Self::Sgl2023 => series == Series::SpeedGaming && matches!(event, "2023onl" | "2023live"),
            Self::TournoiFrancoS3 => series == Series::TournoiFrancophone && event == "3",
            Self::TriforceBlitz => series == Series::TriforceBlitz,
        }
    }

    pub(crate) fn is_custom(&self) -> bool {
        match self {
            Self::Rsl | Self::TriforceBlitz => false,
            Self::CopaDoBrasil | Self::MixedPoolsS2 | Self::MultiworldS3 | Self::NineDaysOfSaws | Self::PicRs2 | Self::Sgl2023 | Self::TournoiFrancoS3 => true,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::CopaDoBrasil => "Copa do Brasil",
            Self::MixedPoolsS2 => "2nd Mixed Pools Tournament",
            Self::MultiworldS3 => "3rd Multiworld Tournament",
            Self::NineDaysOfSaws => "9 Days of SAWS",
            Self::PicRs2 => "2nd Random Settings Pictionary Spoiler Log Race",
            Self::Rsl => "Random settings league",
            Self::Sgl2023 => "SGL 2023",
            Self::TournoiFrancoS3 => "Tournoi Francophone Saison 3",
            Self::TriforceBlitz => "Triforce Blitz",
        }
    }

    fn language(&self) -> Language {
        match self {
            Self::CopaDoBrasil => Portuguese,
            Self::TournoiFrancoS3 => French,
            _ => English,
        }
    }

    fn draft_kind(&self) -> Option<draft::Kind> {
        match self {
            Self::MultiworldS3 => Some(draft::Kind::MultiworldS3),
            Self::TournoiFrancoS3 => Some(draft::Kind::TournoiFrancoS3),
            Self::CopaDoBrasil | Self::MixedPoolsS2 | Self::NineDaysOfSaws | Self::PicRs2 | Self::Rsl | Self::Sgl2023 | Self::TriforceBlitz => None,
        }
    }

    fn rando_version(&self) -> VersionedBranch {
        match self {
            Self::CopaDoBrasil => VersionedBranch::Pinned(rando::Version::from_dev(7, 1, 143)),
            Self::MixedPoolsS2 => VersionedBranch::Pinned(rando::Version::from_branch(rando::Branch::DevFenhl, 7, 1, 117, 17)),
            Self::MultiworldS3 => VersionedBranch::Pinned(rando::Version::from_dev(6, 2, 205)),
            Self::NineDaysOfSaws => VersionedBranch::Pinned(rando::Version::from_branch(rando::Branch::DevFenhl, 6, 9, 14, 2)),
            Self::Sgl2023 => VersionedBranch::Latest(rando::Branch::Sgl),
            Self::TournoiFrancoS3 => VersionedBranch::Pinned(rando::Version::from_branch(rando::Branch::DevR, 7, 1, 143, 1)),
            Self::TriforceBlitz => VersionedBranch::Latest(rando::Branch::DevBlitz),
            Self::PicRs2 | Self::Rsl => panic!("randomizer version for this goal must be parsed from RSL script"),
        }
    }

    fn should_create_rooms(&self) -> bool {
        match self {
            Self::MixedPoolsS2 | Self::NineDaysOfSaws | Self::Rsl => false,
            Self::CopaDoBrasil | Self::MultiworldS3 | Self::PicRs2 | Self::Sgl2023 | Self::TournoiFrancoS3 | Self::TriforceBlitz => true,
        }
    }

    async fn send_presets(&self, ctx: &RaceContext<GlobalState>) -> Result<(), Error> {
        match self {
            Self::PicRs2 => ctx.send_message("!seed: The settings used for the race").await?,
            Self::CopaDoBrasil | Self::MixedPoolsS2 | Self::Sgl2023 => ctx.send_message("!seed: The settings used for the tournament").await?,
            Self::MultiworldS3 => {
                ctx.send_message("!seed base: The settings used for the qualifier and tiebreaker asyncs.").await?;
                ctx.send_message("!seed random: Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.").await?;
                ctx.send_message("!seed draft: Pick the settings here in the chat.").await?;
                ctx.send_message("!seed <setting> <value> <setting> <value>... (e.g. !seed trials 2 wincon scrubs): Pick a set of draftable settings without doing a full draft. Use “!settings” for a list of available settings.").await?;
            }
            Self::NineDaysOfSaws => {
                ctx.send_message("!seed day1: S6").await?;
                ctx.send_message("!seed day2: Beginner").await?;
                ctx.send_message("!seed day3: Advanced").await?;
                ctx.send_message("!seed day4: S5 + one bonk KO").await?;
                ctx.send_message("!seed day5: Beginner + mixed pools").await?;
                ctx.send_message("!seed day6: Beginner 3-player multiworld").await?;
                ctx.send_message("!seed day7: Beginner").await?;
                ctx.send_message("!seed day8: S6 + dungeon ER").await?;
                ctx.send_message("!seed day9: S6").await?;
            }
            Self::Rsl => for preset in all::<rsl::Preset>() {
                ctx.send_message(&format!("!seed{}: {}", match preset {
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
                    rsl::Preset::S6Test => "test settings for RSL season 6",
                })).await?;
            },
            Self::TournoiFrancoS3 => {
                ctx.send_message("!seed base : Settings de base.").await?;
                ctx.send_message("!seed random : Simule en draft en sélectionnant des settings au hasard pour les deux joueurs. Les settings seront affichés avec la seed.").await?;
                ctx.send_message("!seed draft : Vous fait effectuer un draft dans le chat.").await?;
                ctx.send_message("!seed <setting> <configuration> <setting> <configuration>... ex : !seed trials random bridge ad : Créé une seed avec les settings que vous définissez. Tapez “!settings” pour obtenir la liste des settings.").await?;
                ctx.send_message("Utilisez “!seed random advanced” ou “!seed draft advanced” pour autoriser les settings difficiles.").await?;
                ctx.send_message("Activez les donjons Master Quest en utilisant par exemple : “!seed base 6mq” ou “!seed draft advanced 12mq”").await?;
            }
            Self::TriforceBlitz => {
                ctx.send_message("!seed jr: Jabu's Revenge").await?;
                ctx.send_message("!seed s2: Triforce Blitz season 2 settings").await?;
                ctx.send_message("!seed daily: Triforce Blitz Seed of the Day").await?;
            }
        }
        Ok(())
    }
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
    pub(crate) open_rooms: HashSet<String>,
    pub(crate) notifier: Arc<Notify>,
}

pub(crate) struct GlobalState {
    /// Locked while event rooms are being created. Wait with handling new rooms while it's held.
    new_room_lock: Arc<Mutex<()>>,
    host_info: racetime::HostInfo,
    pub(crate) host: &'static str,
    racetime_config: ConfigRaceTime,
    extra_room_tx: Arc<RwLock<mpsc::Sender<String>>>,
    db_pool: PgPool,
    http_client: reqwest::Client,
    startgg_token: String,
    ootr_api_client: OotrApiClient,
    discord_ctx: RwFuture<DiscordCtx>,
    clean_shutdown: Arc<Mutex<CleanShutdown>>,
    cached_mixed_pools_seed: Mutex<Option<seed::Data>>,
    seed_cache_tx: mpsc::Sender<()>,
}

impl GlobalState {
    pub(crate) async fn new(new_room_lock: Arc<Mutex<()>>, racetime_config: ConfigRaceTime, extra_room_tx: Arc<RwLock<mpsc::Sender<String>>>, db_pool: PgPool, http_client: reqwest::Client, ootr_api_key: String, startgg_token: String, host: &'static str, discord_ctx: RwFuture<DiscordCtx>, clean_shutdown: Arc<Mutex<CleanShutdown>>, seed_cache_tx: mpsc::Sender<()>) -> Self {
        let _ = seed_cache_tx.send(()).await;
        Self {
            host_info: racetime::HostInfo {
                hostname: Cow::Borrowed(host),
                ..racetime::HostInfo::default()
            },
            ootr_api_client: OotrApiClient::new(http_client.clone(), ootr_api_key),
            cached_mixed_pools_seed: Mutex::default(),
            new_room_lock, host, racetime_config, extra_room_tx, db_pool, http_client, startgg_token, discord_ctx, clean_shutdown, seed_cache_tx,
        }
    }

    pub(crate) fn roll_seed(self: Arc<Self>, version: VersionedBranch, settings: serde_json::Map<String, Json>, spoiler_log: bool) -> mpsc::Receiver<SeedRollUpdate> {
        let world_count = settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64").try_into().expect("too many worlds"));
        let (update_tx, update_rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let web_version = match self.ootr_api_client.can_roll_on_web(None, &version, world_count).await {
                Ok(web_version) => web_version,
                Err(e) => {
                    update_tx.send(SeedRollUpdate::Error(e)).await?;
                    return Ok(())
                }
            };
            let mw_permit = if web_version.is_some() && world_count > 1 {
                Some(match self.ootr_api_client.mw_seed_rollers.try_acquire() {
                    Ok(permit) => permit,
                    Err(TryAcquireError::Closed) => unreachable!(),
                    Err(TryAcquireError::NoPermits) => {
                        let (mut pos, mut pos_rx) = {
                            let mut waiting = lock!(self.ootr_api_client.waiting);
                            let pos = waiting.len();
                            let (pos_tx, pos_rx) = mpsc::unbounded_channel();
                            waiting.push(pos_tx);
                            (pos, pos_rx)
                        };
                        update_tx.send(SeedRollUpdate::Queued(pos.try_into().unwrap())).await?;
                        while pos > 0 {
                            let () = pos_rx.recv().await.expect("queue position notifier closed");
                            pos -= 1;
                            update_tx.send(SeedRollUpdate::MovedForward(pos.try_into().unwrap())).await?;
                        }
                        let mut waiting = lock!(self.ootr_api_client.waiting);
                        let permit = self.ootr_api_client.mw_seed_rollers.acquire().await.expect("seed queue semaphore closed");
                        waiting.remove(0);
                        for tx in &*waiting {
                            let _ = tx.send(());
                        }
                        permit
                    }
                })
            } else {
                None
            };
            if let Some(web_version) = web_version {
                match self.ootr_api_client.roll_seed_web(update_tx.clone(), version.branch(), web_version, false, spoiler_log, settings).await {
                    Ok((id, gen_time, file_hash, file_stem)) => update_tx.send(SeedRollUpdate::Done {
                        seed: seed::Data {
                            file_hash: Some(file_hash),
                            files: seed::Files::OotrWeb {
                                file_stem: Cow::Owned(file_stem),
                                id, gen_time,
                            },
                        },
                        rsl_preset: None,
                        send_spoiler_log: spoiler_log,
                    }).await?,
                    Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?,
                }
                drop(mw_permit);
            } else if let VersionedBranch::Pinned(version) = version {
                update_tx.send(SeedRollUpdate::Started).await?;
                match roll_seed_locally(version, settings).await {
                    Ok((patch_filename, spoiler_log_path)) => update_tx.send(match spoiler_log_path.into_os_string().into_string() {
                        Ok(spoiler_log_path) => match regex_captures!(r"^(.+)\.zpfz?$", &patch_filename) {
                            Some((_, file_stem)) => SeedRollUpdate::Done {
                                seed: seed::Data {
                                    file_hash: None,
                                    files: seed::Files::MidosHouse {
                                        file_stem: Cow::Owned(file_stem.to_owned()),
                                        locked_spoiler_log_path: Some(spoiler_log_path),
                                    },
                                },
                                rsl_preset: None,
                                send_spoiler_log: spoiler_log,
                            },
                            None => SeedRollUpdate::Error(RollError::PatchPath),
                        },
                        Err(e) => SeedRollUpdate::Error(e.into())
                    }).await?,
                    Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?,
                }
            } else {
                update_tx.send(SeedRollUpdate::Error(RollError::LatestLocal)).await?; //TODO resolve latest version of this branch, roll locally
            }
            Ok::<_, mpsc::error::SendError<_>>(())
        });
        update_rx
    }

    pub(crate) fn roll_rsl_seed(self: Arc<Self>, preset: VersionedRslPreset, world_count: u8, spoiler_log: bool) -> mpsc::Receiver<SeedRollUpdate> {
        let (update_tx, update_rx) = mpsc::channel(128);
        let update_tx2 = update_tx.clone();
        tokio::spawn(async move {
            let rsl_script_path = preset.script_path()?; //TODO automatically clone if not present and ensure base rom is in place (need to create data directory)
            // update the RSL script
            if !preset.is_version_locked() {
                let repo = Repository::open(&rsl_script_path)?;
                let mut origin = repo.find_remote("origin")?;
                origin.fetch(&["master"], None, None)?;
                repo.reset(&repo.find_branch("origin/master", BranchType::Remote)?.into_reference().peel_to_commit()?.into_object(), ResetType::Hard, None)?;
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
            let web_version = self.ootr_api_client.can_roll_on_web(Some(&preset), &VersionedBranch::Pinned(version.clone()), world_count).await?;
            // run the RSL script
            let _ = update_tx.send(SeedRollUpdate::Started).await;
            let outer_tries = if web_version.is_some() { 5 } else { 1 }; // when generating locally, retries are already handled by the RSL script
            for _ in 0..outer_tries {
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
                    Some(2) => return Err(RollError::Retries {
                        num_retries: 15,
                        last_error: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
                    }),
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
                    let mw_permit = if world_count > 1 {
                        Some(match self.ootr_api_client.mw_seed_rollers.try_acquire() {
                            Ok(permit) => permit,
                            Err(TryAcquireError::Closed) => unreachable!(),
                            Err(TryAcquireError::NoPermits) => {
                                let (mut pos, mut pos_rx) = {
                                    let mut waiting = lock!(self.ootr_api_client.waiting);
                                    let pos = waiting.len();
                                    let (pos_tx, pos_rx) = mpsc::unbounded_channel();
                                    waiting.push(pos_tx);
                                    (pos, pos_rx)
                                };
                                let _ = update_tx.send(SeedRollUpdate::Queued(pos.try_into().unwrap())).await;
                                while pos > 0 {
                                    let () = pos_rx.recv().await.expect("queue position notifier closed");
                                    pos -= 1;
                                    let _ = update_tx.send(SeedRollUpdate::MovedForward(pos.try_into().unwrap())).await;
                                }
                                let mut waiting = lock!(self.ootr_api_client.waiting);
                                let permit = self.ootr_api_client.mw_seed_rollers.acquire().await.expect("seed queue semaphore closed");
                                waiting.remove(0);
                                for tx in &*waiting {
                                    let _ = tx.send(());
                                }
                                permit
                            }
                        })
                    } else {
                        None
                    };
                    let (seed_id, gen_time, file_hash, file_stem) = match self.ootr_api_client.roll_seed_web(update_tx.clone(), version.branch(), web_version, true, spoiler_log, settings).await {
                        Ok(data) => data,
                        Err(RollError::Retries { .. }) => continue,
                        Err(e) => return Err(e),
                    };
                    drop(mw_permit);
                    let _ = update_tx.send(SeedRollUpdate::Done {
                        seed: seed::Data {
                            file_hash: Some(file_hash),
                            files: seed::Files::OotrWeb {
                                id: seed_id,
                                file_stem: Cow::Owned(file_stem),
                                gen_time,
                            },
                        },
                        rsl_preset: if let VersionedRslPreset::Xopar { preset, .. } = preset { Some(preset) } else { None },
                        send_spoiler_log: spoiler_log,
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
                                files: seed::Files::MidosHouse {
                                    file_stem: Cow::Owned(file_stem.to_owned()),
                                    locked_spoiler_log_path: Some(spoiler_log_path.into_os_string().into_string()?),
                                },
                            },
                            rsl_preset: if let VersionedRslPreset::Xopar { preset, .. } = preset { Some(preset) } else { None },
                            send_spoiler_log: spoiler_log,
                        },
                        None => SeedRollUpdate::Error(RollError::PatchPath),
                    }).await;
                    return Ok(())
                }
            }
            let _ = update_tx.send(SeedRollUpdate::Error(RollError::Retries {
                num_retries: 15,
                last_error: None,
            })).await;
            Ok(())
        }.then(|res| async move {
            match res {
                Ok(()) => {}
                Err(e) => { let _ = update_tx2.send(SeedRollUpdate::Error(e)).await; }
            }
        }));
        update_rx
    }

    pub(crate) fn roll_tfb_seed(self: Arc<Self>, room: String, spoiler_log: bool) -> mpsc::Receiver<SeedRollUpdate> {
        let (update_tx, update_rx) = mpsc::channel(128);
        let update_tx2 = update_tx.clone();
        tokio::spawn(async move {
            let _ = update_tx.send(SeedRollUpdate::Started).await;
            let form_data = if spoiler_log {
                vec![
                    ("unlockSetting", "ALWAYS"),
                    ("version", "LATEST"),
                ]
            } else {
                vec![
                    ("unlockSetting", "RACETIME"),
                    ("racetimeRoom", &room),
                    ("version", "LATEST"),
                ]
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
                    files: seed::Files::TriforceBlitz { uuid },
                },
                rsl_preset: None,
                send_spoiler_log: spoiler_log,
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

async fn roll_seed_locally(version: rando::Version, mut settings: serde_json::Map<String, Json>) -> Result<(String, PathBuf), RollError> {
    version.clone_repo().await?;
    #[cfg(unix)] {
        settings.insert(format!("rom"), json!(BaseDirectories::new()?.find_data_file(Path::new("midos-house").join("oot-ntscu-1.0.z64")).ok_or(RollError::RomPath)?));
        if settings.get("language").and_then(|language| language.as_str()).map_or(false, |language| matches!(language, "french" | "german")) {
            settings.insert(format!("pal_rom"), json!(BaseDirectories::new()?.find_data_file(Path::new("midos-house").join("oot-pal-1.0.z64")).ok_or(RollError::RomPath)?));
        }
    }
    settings.insert(format!("create_patch_file"), json!(true));
    settings.insert(format!("create_compressed_rom"), json!(false));
    let mut last_error = None;
    for _ in 0..3 {
        let rando_path = version.dir()?;
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
        let spoiler_log_path = rando_path.join("Output").join(stderr.iter().rev().find_map(|line| line.strip_prefix("Created spoiler log at: ")).ok_or(RollError::SpoilerLogPath)?);
        let patch_filename = patch_path.file_name().expect("patch file path with no file name");
        fs::rename(&patch_path, Path::new(seed::DIR).join(patch_filename)).await?;
        return Ok((
            patch_filename.to_str().expect("non-UTF-8 patch filename").to_owned(),
            spoiler_log_path.to_owned(),
        ))
    }
    Err(RollError::Retries {
        num_retries: 3,
        last_error,
    })
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
    #[error(transparent)] RandoVersion(#[from] rando::VersionParseError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[cfg(unix)] #[error(transparent)] Xdg(#[from] xdg::BaseDirectoriesError),
    #[error("{display}")]
    Cloned {
        debug: String,
        display: String,
    },
    #[error("there is nothing waiting for this seed anymore")]
    ChannelClosed,
    #[error("attempted to roll with the latest version, but web claimed it was unsupported")]
    LatestLocal,
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
    SpoilerLogPath,
    #[error("didn't find 5 hash icons on Triforce Blitz seed page")]
    TfbHash,
    #[error("failed to parse Triforce Blitz seed page")]
    TfbHtml,
    #[error("Triforce Blitz website returned unexpected URL")]
    TfbUrl,
    #[error("seed status API endpoint returned unknown value {0}")]
    UnespectedSeedStatus(u8),
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
    /// We've cleared the queue but have to wait for the rate limit to expire.
    WaitRateLimit(Duration),
    /// We've cleared the queue and are now being rolled.
    Started,
    /// The seed has been rolled successfully.
    Done {
        seed: seed::Data,
        rsl_preset: Option<rsl::Preset>,
        send_spoiler_log: bool,
    },
    /// Seed rolling failed.
    Error(RollError),
}

impl SeedRollUpdate {
    async fn handle(self, db_pool: &PgPool, ctx: &RaceContext<GlobalState>, state: &ArcRwLock<RaceState>, race_id: Option<Id>, language: Language, article: &'static str, description: &str) -> Result<(), Error> {
        match self {
            Self::Queued(0) => ctx.send_message("I'm already rolling other multiworld seeds so your seed has been queued. It is at the front of the queue so it will be rolled next.").await?,
            Self::Queued(1) => ctx.send_message("I'm already rolling other multiworld seeds so your seed has been queued. There is 1 seed in front of it in the queue.").await?,
            Self::Queued(pos) => ctx.send_message(&format!("I'm already rolling other multiworld seeds so your seed has been queued. There are {pos} seeds in front of it in the queue.")).await?,
            Self::MovedForward(0) => ctx.send_message("The queue has moved and your seed is now at the front so it will be rolled next.").await?,
            Self::MovedForward(1) => ctx.send_message("The queue has moved and there is only 1 more seed in front of yours.").await?,
            Self::MovedForward(pos) => ctx.send_message(&format!("The queue has moved and there are now {pos} seeds in front of yours.")).await?,
            Self::WaitRateLimit(duration) => ctx.send_message(&format!("Your seed will be rolled in {}.", English.format_duration(duration, true))).await?,
            Self::Started => ctx.send_message(&if let French = language {
                format!("Génération d'{article} {description}…")
            } else {
                format!("Rolling {article} {description}…")
            }).await?,
            Self::Done { mut seed, rsl_preset, send_spoiler_log } => {
                if let seed::Files::MidosHouse { ref file_stem, ref mut locked_spoiler_log_path } = seed.files {
                    if send_spoiler_log && locked_spoiler_log_path.is_some() {
                        fs::rename(locked_spoiler_log_path.as_ref().unwrap(), Path::new(seed::DIR).join(format!("{file_stem}_Spoiler.json"))).await.to_racetime()?;
                        *locked_spoiler_log_path = None;
                    }
                }
                let extra = seed.extra(Utc::now()).await.to_racetime()?;
                if let Some(race_id) = race_id {
                    match seed.files {
                        seed::Files::MidosHouse { ref file_stem, .. } => {
                            sqlx::query!(
                                "UPDATE races SET file_stem = $1 WHERE id = $2",
                                file_stem, race_id as _,
                            ).execute(db_pool).await.to_racetime()?;
                        }
                        seed::Files::OotrWeb { id, gen_time, ref file_stem } => {
                            sqlx::query!(
                                "UPDATE races SET web_id = $1, web_gen_time = $2, file_stem = $3 WHERE id = $4",
                                id as i64, gen_time, file_stem, race_id as _,
                            ).execute(db_pool).await.to_racetime()?;
                        }
                        seed::Files::TriforceBlitz { uuid } => {
                            sqlx::query!(
                                "UPDATE races SET tfb_uuid = $1 WHERE id = $2",
                                uuid, race_id as _,
                            ).execute(db_pool).await.to_racetime()?;
                        }
                        seed::Files::TfbSotd { .. } => unimplemented!("Triforce Blitz seed of the day not supported for official races"),
                    }
                    if let Some([hash1, hash2, hash3, hash4, hash5]) = extra.file_hash {
                        sqlx::query!(
                            "UPDATE races SET hash1 = $1, hash2 = $2, hash3 = $3, hash4 = $4, hash5 = $5 WHERE id = $6",
                            hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _, race_id as _,
                        ).execute(db_pool).await.to_racetime()?;
                        if let Some(preset) = rsl_preset {
                            match seed.files {
                                seed::Files::MidosHouse { ref file_stem, .. } => {
                                    sqlx::query!(
                                        "INSERT INTO rsl_seeds (room, file_stem, preset, hash1, hash2, hash3, hash4, hash5) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                                        format!("https://{}{}", ctx.global_state.host, ctx.data().await.url), &file_stem, preset as _, hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _,
                                    ).execute(db_pool).await.to_racetime()?;
                                }
                                seed::Files::OotrWeb { id, gen_time, ref file_stem } => {
                                    sqlx::query!(
                                        "INSERT INTO rsl_seeds (room, file_stem, preset, web_id, web_gen_time, hash1, hash2, hash3, hash4, hash5) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                                        format!("https://{}{}", ctx.global_state.host, ctx.data().await.url), &file_stem, preset as _, id as i64, gen_time, hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _,
                                    ).execute(db_pool).await.to_racetime()?;
                                }
                                seed::Files::TriforceBlitz { .. } | seed::Files::TfbSotd { .. } => unreachable!(), // no such thing as random settings Triforce Blitz
                            }
                        }
                    }
                }
                let seed_url = match seed.files {
                    seed::Files::MidosHouse { ref file_stem, .. } => format!("https://midos.house/seed/{file_stem}"),
                    seed::Files::OotrWeb { id, .. } => format!("https://ootrandomizer.com/seed/get?id={id}"),
                    seed::Files::TriforceBlitz { uuid } => format!("https://www.triforceblitz.com/seed/{uuid}"),
                    seed::Files::TfbSotd { ordinal, .. } => format!("https://www.triforceblitz.com/seed/daily/{ordinal}"),
                };
                ctx.send_message(&if let French = language {
                    format!("@entrants Voici votre seed : {seed_url}")
                } else {
                    format!("@entrants Here is your seed: {seed_url}")
                }).await?;
                if send_spoiler_log {
                    ctx.send_message("The spoiler log is also available on the seed page.").await?;
                } else if let seed::Files::TfbSotd { date, .. } = seed.files {
                    if let Some(unlock_date) = date.succ_opt().and_then(|next| next.succ_opt()) {
                        let unlock_time = Utc.from_utc_datetime(&unlock_date.and_hms_opt(20, 0, 0).expect("failed to construct naive datetime at 20:00:00"));
                        let unlock_time = (unlock_time - Utc::now()).to_std().expect("unlock time for current daily seed in the past");
                        ctx.send_message(&format!("The spoiler log will be available on the seed page in {}.", English.format_duration(unlock_time, true))).await?;
                    } else {
                        unimplemented!("distant future Triforce Blitz SotD")
                    }
                } else {
                    ctx.send_message(if let French = language {
                        "Le spoiler log sera disponible sur le lien de la seed après la seed."
                    } else {
                        "The spoiler log will be available on the seed page after the race."
                    }).await?;
                }
                ctx.set_bot_raceinfo(&format!(
                    "{}{}{seed_url}",
                    if let Some(preset) = rsl_preset { format!("{}\n", preset.race_info()) } else { String::default() },
                    extra.file_hash.map(|file_hash| format!("{}\n", format_hash(file_hash))).unwrap_or_default(),
                )).await?;
                *lock!(@write state) = RaceState::Rolled(seed);
            }
            Self::Error(RollError::Retries { num_retries, last_error }) => {
                if let Some(last_error) = last_error {
                    eprintln!("seed rolling failed {num_retries} times, sample error:\n{last_error}");
                } else {
                    eprintln!("seed rolling failed {num_retries} times, no sample error recorded");
                }
                ctx.send_message(&if let French = language {
                    format!("Désolé @entrants, le randomizer a rapporté une erreur {num_retries} fois de suite donc je vais laisser tomber. Veuillez réessayer et, si l'erreur persiste, essayer de roll une seed de votre côté et contacter Fenhl.")
                } else {
                    format!("Sorry @entrants, the randomizer reported an error {num_retries} times, so I'm giving up on rolling the seed. Please try again. If this error persists, please report it to Fenhl.")
                }).await?; //TODO for official races, explain that retrying is done using !seed
                *lock!(@write state) = RaceState::Init;
            }
            Self::Error(msg) => {
                eprintln!("seed roll error: {msg:?}");
                let _ = Command::new("sudo").arg("-u").arg("fenhl").arg("/opt/night/bin/nightd").arg("report").arg("/net/midoshouse/error").spawn(); //TODO include error details in report
                ctx.send_message("Sorry @entrants, something went wrong while rolling the seed. Please report this error to Fenhl.").await?;
            }
        }
        Ok(())
    }
}

struct OotrApiClient {
    http_client: reqwest::Client,
    api_key: String,
    next_request: Mutex<Instant>,
    next_mw_seed: Mutex<Instant>,
    mw_seed_rollers: Semaphore,
    waiting: Mutex<Vec<mpsc::UnboundedSender<()>>>,
}

impl OotrApiClient {
    pub fn new(http_client: reqwest::Client, api_key: String) -> Self {
        Self {
            next_request: Mutex::new(Instant::now() + Duration::from_millis(500)),
            next_mw_seed: Mutex::new(Instant::now() + MULTIWORLD_RATE_LIMIT),
            mw_seed_rollers: Semaphore::new(2), // we're allowed to roll a maximum of 2 multiworld seeds at the same time
            waiting: Mutex::default(),
            http_client, api_key,
        }
    }

    async fn get(&self, uri: impl IntoUrl + Clone, query: Option<&(impl Serialize + ?Sized)>) -> reqwest::Result<reqwest::Response> {
        let mut next_request = lock!(self.next_request);
        sleep_until(*next_request).await;
        let mut builder = self.http_client.get(uri.clone());
        if let Some(query) = query {
            builder = builder.query(query);
        }
        println!("OotrApiClient: GET {}", uri.into_url()?);
        let res = builder.send().await;
        *next_request = Instant::now() + Duration::from_millis(500);
        res
    }

    async fn post(&self, uri: impl IntoUrl + Clone, query: Option<&(impl Serialize + ?Sized)>, json: Option<&(impl Serialize + ?Sized)>) -> reqwest::Result<reqwest::Response> {
        let mut next_request = lock!(self.next_request);
        sleep_until(*next_request).await;
        let mut builder = self.http_client.post(uri.clone());
        if let Some(query) = query {
            builder = builder.query(query);
        }
        if let Some(json) = json {
            builder = builder.json(json);
        }
        println!("OotrApiClient: POST {}", uri.into_url()?);
        let res = builder.send().await;
        *next_request = Instant::now() + Duration::from_millis(500);
        res
    }

    async fn get_version(&self, branch: rando::Branch, random_settings: bool) -> Result<Version, RollError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct VersionResponse {
            currently_active_version: Version,
        }

        Ok(self.get("https://ootrandomizer.com/api/version", Some(&[("key", &*self.api_key), ("branch", branch.web_name(random_settings).ok_or(RollError::RandomSettingsWeb)?)])).await?
            .detailed_error_for_status().await?
            .json_with_text_in_error::<VersionResponse>().await?
            .currently_active_version)
    }

    async fn can_roll_on_web(&self, rsl_preset: Option<&VersionedRslPreset>, version: &VersionedBranch, world_count: u8) -> Result<Option<Version>, RollError> {
        if world_count > 3 { return Ok(None) }
        if rsl_preset.is_some() && version.branch().web_name_random_settings().is_none() { return Ok(None) }
        // check if randomizer version is available on web
        match version {
            VersionedBranch::Pinned(version) => {
                if !KNOWN_GOOD_WEB_VERSIONS.contains(&version) {
                    if version.supplementary().is_some() && !matches!(rsl_preset, Some(VersionedRslPreset::Xopar { .. })) {
                        // The version API endpoint does not return the supplementary version number, so we can't be sure we have the right version unless it was manually checked and added to KNOWN_GOOD_WEB_VERSIONS.
                        // For the RSL script's main branch, we assume the supplementary version number is correct since we dynamically get the version from the RSL script.
                        // The dev-fenhl branch of the RSL script can point to versions not available on web, so we can't make this assumption there.
                        return Ok(None)
                    }
                    if let Ok(latest_web_version) = self.get_version(version.branch(), rsl_preset.is_some()).await {
                        if latest_web_version != *version.base() { // there is no endpoint for checking whether a given version is available on the website, so for now we assume that if the required version isn't the current one, it's not available
                            println!("web version mismatch on {} branch: we need {} but latest is {latest_web_version}", version.branch().web_name(rsl_preset.is_some()).expect("checked above"), version.base());
                            return Ok(None)
                        }
                    } else {
                        // the version API endpoint sometimes returns HTML instead of the expected JSON, fallback to generating locally when that happens
                        return Ok(None)
                    }
                }
                Ok(Some(version.base().clone()))
            }
            VersionedBranch::Latest(branch) => Ok(self.get_version(*branch, rsl_preset.is_some()).await.ok()),
        }
    }

    async fn roll_seed_web(&self, update_tx: mpsc::Sender<SeedRollUpdate>, branch: rando::Branch, version: Version, random_settings: bool, spoiler_log: bool, settings: serde_json::Map<String, Json>) -> Result<(u64, DateTime<Utc>, [HashIcon; 5], String), RollError> {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct CreateSeedResponse {
            #[serde_as(as = "DisplayFromStr")]
            id: u64,
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

        let is_mw = settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64")) > 1;
        for attempt in 0..3 {
            let next_seed = if is_mw {
                let next_seed = lock!(self.next_mw_seed);
                if let Some(duration) = next_seed.checked_duration_since(Instant::now()) {
                    update_tx.send(SeedRollUpdate::WaitRateLimit(duration)).await?;
                    sleep(duration).await;
                }
                Some(next_seed)
            } else {
                None
            };
            if attempt == 0 && !random_settings {
                update_tx.send(SeedRollUpdate::Started).await?;
            }
            let CreateSeedResponse { id } = self.post("https://ootrandomizer.com/api/v2/seed/create", Some(&[
                ("key", &*self.api_key),
                ("version", &*format!("{}_{}", branch.web_name(random_settings).ok_or(RollError::RandomSettingsWeb)?, version)),
                ("locked", if spoiler_log { "0" } else { "1" }),
            ]), Some(&settings)).await?
                .detailed_error_for_status().await?
                .json_with_text_in_error().await?;
            if let Some(mut next_seed) = next_seed {
                *next_seed = Instant::now() + MULTIWORLD_RATE_LIMIT;
            }
            if is_mw {
                sleep(MULTIWORLD_RATE_LIMIT).await; // extra rate limiting rule
            }
            loop {
                sleep(Duration::from_secs(1)).await;
                let resp = self.get(
                    "https://ootrandomizer.com/api/v2/seed/status",
                    Some(&[("key", &self.api_key), ("id", &id.to_string())]),
                ).await?;
                if resp.status() == StatusCode::NO_CONTENT { continue }
                resp.error_for_status_ref()?;
                match resp.json_with_text_in_error::<SeedStatusResponse>().await?.status {
                    0 => continue, // still generating
                    1 => { // generated success
                        let SeedDetailsResponse { creation_timestamp, settings_log } = self.get("https://ootrandomizer.com/api/v2/seed/details", Some(&[("key", &self.api_key), ("id", &id.to_string())])).await?
                            .detailed_error_for_status().await?
                            .json_with_text_in_error().await?;
                        let patch_response = self.get("https://ootrandomizer.com/api/v2/seed/patch", Some(&[("key", &self.api_key), ("id", &id.to_string())])).await?
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
                    n => return Err(RollError::UnespectedSeedStatus(n)),
                }
            }
        }
        Err(RollError::Retries {
            num_retries: 3,
            last_error: None,
        })
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
        let (_, duration, interval) = regex_captures!("^(.+) every (.+)$", s).ok_or(())?;
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
        spoiler_log: bool,
    },
    Rolling,
    Rolled(seed::Data),
    SpoilerSent,
}

struct OfficialRaceData {
    cal_event: cal::Event,
    event: event::Data<'static>,
    restreams: HashMap<Url, RestreamState>,
    entrants: Vec<String>,
    fpa_invoked: bool,
}

#[derive(Default)]
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
    async fn should_handle_inner(race_data: &RaceData, global_state: Arc<GlobalState>, increment_num_rooms: bool) -> bool {
        let mut clean_shutdown = lock!(global_state.clean_shutdown);
        let Ok(bot_goal) = race_data.goal.name.parse::<Goal>() else { return false };
        race_data.goal.custom == bot_goal.is_custom()
        && !matches!(race_data.status.value, RaceStatusValue::Finished | RaceStatusValue::Cancelled)
        && if !clean_shutdown.requested || !clean_shutdown.open_rooms.is_empty() {
            if increment_num_rooms { assert!(clean_shutdown.open_rooms.insert(race_data.url.clone())) }
            true
        } else {
            false
        }
    }

    fn is_official(&self) -> bool { self.official_data.is_some() }

    async fn goal(&self, ctx: &RaceContext<GlobalState>) -> Goal {
        ctx.data().await.goal.name.parse::<Goal>().expect("running race handler for unknown goal")
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
        let goal = self.goal(ctx).await;
        if let Some(draft_kind) = goal.draft_kind() {
            let available_settings = if let RaceState::Draft { state: ref draft, .. } = *lock!(@read self.race_state) {
                match draft.next_step(draft_kind, &mut draft::MessageContext::RaceTime { high_seed_name: &self.high_seed_name, low_seed_name: &self.low_seed_name, reply_to }).await.to_racetime()?.kind {
                    draft::StepKind::GoFirst => None,
                    draft::StepKind::Ban { available_settings, .. } => Some(available_settings.all().map(|setting| setting.description).collect()),
                    draft::StepKind::Pick { available_choices, .. } => Some(available_choices.all().map(|setting| setting.description).collect()),
                    draft::StepKind::BooleanChoice { .. } | draft::StepKind::Done(_) => Some(Vec::default()),
                }
            } else {
                None
            };
            let available_settings = available_settings.unwrap_or_else(|| match draft_kind {
                draft::Kind::MultiworldS3 => mw::S3_SETTINGS.into_iter().map(|mw::S3Setting { description, .. }| description).collect(),
                draft::Kind::TournoiFrancoS3 => fr::S3_SETTINGS.into_iter().map(|fr::S3Setting { description, .. }| description).collect(),
            });
            if available_settings.is_empty() {
                ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, aucun setting n'est demandé pour le moment.")
                } else {
                    format!("Sorry {reply_to}, no settings are currently available.")
                }).await?;
            } else {
                ctx.send_message(preface).await?;
                for setting in available_settings {
                    ctx.send_message(setting).await?;
                }
            }
        } else {
            ctx.send_message(&format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?;
        }
        Ok(())
    }

    async fn advance_draft(&self, ctx: &RaceContext<GlobalState>) -> Result<(), Error> {
        let goal = self.goal(ctx).await;
        let state = lock!(@write_owned self.race_state.clone());
        let Some(draft_kind) = goal.draft_kind() else { unreachable!() };
        let RaceState::Draft { state: ref draft, spoiler_log } = *state else { unreachable!() };
        let step = draft.next_step(draft_kind, &mut draft::MessageContext::RaceTime { high_seed_name: &self.high_seed_name, low_seed_name: &self.low_seed_name, reply_to: "friend" }).await.to_racetime()?;
        if let draft::StepKind::Done(settings) = step.kind {
            let (article, description) = if let French = goal.language() {
                ("une", format!("seed avec {}", step.message))
            } else {
                ("a", format!("seed with {}", step.message))
            };
            self.roll_seed(ctx, state, goal.rando_version(), settings, spoiler_log, goal.language(), article, description);
        } else {
            ctx.send_message(&step.message).await?;
        }
        Ok(())
    }

    async fn draft_action(&self, ctx: &RaceContext<GlobalState>, reply_to: &str, action: draft::Action) -> Result<(), Error> {
        let goal = self.goal(ctx).await;
        if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
            let mut state = lock!(@write self.race_state);
            if let Some(draft_kind) = goal.draft_kind() {
                match *state {
                    RaceState::Init => match draft_kind {
                        draft::Kind::MultiworldS3 => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                        draft::Kind::TournoiFrancoS3 => ctx.send_message(&format!("Désolé {reply_to}, le draft n'a pas débuté. Utilisez “!seed draft” pour en commencer un. Pour plus d'infos, utilisez !presets")).await?,
                    },
                    RaceState::Draft { state: ref mut draft, .. } => match draft.apply(draft_kind, &mut draft::MessageContext::RaceTime { high_seed_name: &self.high_seed_name, low_seed_name: &self.low_seed_name, reply_to }, action).await.to_racetime()? {
                        Ok(_) => {
                            drop(state);
                            self.advance_draft(ctx).await?;
                        }
                        Err(error_msg) => {
                            drop(state);
                            ctx.send_message(&error_msg).await?;
                        }
                    },
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => match goal.language() {
                        French => ctx.send_message(&format!("Désolé {reply_to}, mais il n'y a pas de draft, ou la phase de pick&ban est terminée.")).await?,
                        _ => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                    },
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?;
            }
        } else {
            match goal.language() {
                French => ctx.send_message(&format!("Désolé {reply_to}, mais la race a débuté.")).await?,
                _ => ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?,
            }
        }
        Ok(())
    }

    fn roll_seed_inner(&self, ctx: &RaceContext<GlobalState>, mut state: OwnedRwLockWriteGuard<RaceState>, mut updates: mpsc::Receiver<SeedRollUpdate>, language: Language, article: &'static str, description: String) {
        *state = RaceState::Rolling;
        drop(state);
        let db_pool = ctx.global_state.db_pool.clone();
        let ctx = ctx.clone();
        let state = self.race_state.clone();
        let id = self.official_data.as_ref().map(|official_data| official_data.cal_event.race.id);
        let official_start = self.official_data.as_ref().map(|official_data| official_data.cal_event.start().expect("handling room for official race without start time"));
        tokio::spawn(async move {
            let mut seed_state = None::<SeedRollUpdate>;
            if let Some(delay) = official_start.and_then(|start| (start - chrono::Duration::minutes(15) - Utc::now()).to_std().ok()) {
                // don't want to give an unnecessarily exact estimate if the room was opened automatically 30 or 60 minutes ahead of start
                let display_delay = if delay > Duration::from_secs(14 * 60) && delay < Duration::from_secs(16 * 60) {
                    Duration::from_secs(15 * 60)
                } else if delay > Duration::from_secs(44 * 60) && delay < Duration::from_secs(46 * 60) {
                    Duration::from_secs(45 * 60)
                } else {
                    delay
                };
                ctx.send_message(&if let French = language {
                    format!("Votre {description} sera postée dans {}.", French.format_duration(display_delay, true))
                } else {
                    format!("Your {description} will be posted in {}.", English.format_duration(display_delay, true))
                }).await?;
                let mut sleep = pin!(sleep_until(Instant::now() + delay));
                loop {
                    select! {
                        () = &mut sleep => {
                            if let Some(update) = seed_state.take() {
                                update.handle(&db_pool, &ctx, &state, id, language, article, &description).await?;
                            }
                            while let Some(update) = updates.recv().await {
                                update.handle(&db_pool, &ctx, &state, id, language, article, &description).await?;
                            }
                        }
                        Some(update) = updates.recv() => seed_state = Some(update), //TODO if update is RollError::Retries, restart seed rolling?
                    }
                }
            } else {
                while let Some(update) = updates.recv().await {
                    update.handle(&db_pool, &ctx, &state, id, language, article, &description).await?;
                }
            }
            Ok::<_, Error>(())
        });
    }

    fn roll_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, version: VersionedBranch, settings: serde_json::Map<String, Json>, spoiler_log: bool, language: Language, article: &'static str, description: String) {
        self.roll_seed_inner(ctx, state, Arc::clone(&ctx.global_state).roll_seed(version, settings, spoiler_log), language, article, description);
    }

    fn roll_rsl_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, preset: VersionedRslPreset, world_count: u8, spoiler_log: bool, language: Language, article: &'static str, description: String) {
        self.roll_seed_inner(ctx, state, Arc::clone(&ctx.global_state).roll_rsl_seed(preset, world_count, spoiler_log), language, article, description);
    }

    async fn roll_tfb_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, spoiler_log: bool, language: Language, article: &'static str, description: String) {
        self.roll_seed_inner(ctx, state, Arc::clone(&ctx.global_state).roll_tfb_seed(format!("https://{}{}", ctx.global_state.host, ctx.data().await.url), spoiler_log), language, article, description);
    }

    async fn queue_existing_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, seed: seed::Data, language: Language, article: &'static str, description: String) {
        let (tx, rx) = mpsc::channel(1);
        tx.send(SeedRollUpdate::Done { rsl_preset: None, send_spoiler_log: false, seed }).await.unwrap();
        self.roll_seed_inner(ctx, state, rx, language, article, description);
    }

    /// Returns `false` if this race was already finished/cancelled.
    async fn unlock_spoiler_log(&self, ctx: &RaceContext<GlobalState>) -> Result<bool, Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SeedDetailsResponse {
            spoiler_log: String,
        }

        let mut state = lock!(@write self.race_state);
        match *state {
            RaceState::Rolled(seed::Data { ref files, .. }) => if self.official_data.as_ref().map_or(true, |official_data| !official_data.cal_event.is_first_async_half()) {
                match files {
                    seed::Files::MidosHouse { file_stem, locked_spoiler_log_path } => if let Some(locked_spoiler_log_path) = locked_spoiler_log_path {
                        fs::rename(locked_spoiler_log_path, Path::new(seed::DIR).join(format!("{file_stem}_Spoiler.json"))).await.to_racetime()?;
                    },
                    seed::Files::OotrWeb { id, file_stem, .. } => {
                        ctx.global_state.ootr_api_client.post("https://ootrandomizer.com/api/v2/seed/unlock", Some(&[("key", &ctx.global_state.ootr_api_client.api_key), ("id", &id.to_string())]), None::<&()>).await?
                            .detailed_error_for_status().await.to_racetime()?;
                        let spoiler_log = ctx.global_state.ootr_api_client.get("https://ootrandomizer.com/api/v2/seed/details", Some(&[("key", &ctx.global_state.ootr_api_client.api_key), ("id", &id.to_string())])).await?
                            .detailed_error_for_status().await.to_racetime()?
                            .json_with_text_in_error::<SeedDetailsResponse>().await.to_racetime()?
                            .spoiler_log;
                        fs::write(Path::new(seed::DIR).join(format!("{file_stem}_Spoiler.json")), &spoiler_log).await.to_racetime()?;
                    }
                    seed::Files::TriforceBlitz { .. } | seed::Files::TfbSotd { .. } => {} // automatically unlocked by triforceblitz.com
                }
            },
            RaceState::SpoilerSent => return Ok(false),
            _ => {}
        }
        *state = RaceState::SpoilerSent;
        Ok(true)
    }
}

#[async_trait]
impl RaceHandler<GlobalState> for Handler {
    async fn should_handle(race_data: &RaceData, global_state: Arc<GlobalState>) -> Result<bool, Error> {
        Ok(Self::should_handle_inner(race_data, global_state, true).await)
    }

    async fn should_stop(&mut self, ctx: &RaceContext<GlobalState>) -> Result<bool, Error> {
        Ok(!Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), false).await)
    }

    async fn task(global_state: Arc<GlobalState>, race_data: Arc<tokio::sync::RwLock<RaceData>>, join_handle: tokio::task::JoinHandle<()>) -> Result<(), Error> {
        let race_data = ArcRwLock::from(race_data);
        tokio::spawn(async move {
            println!("race handler for https://{}{} started", global_state.host, lock!(@read race_data).url);
            let res = join_handle.await;
            let mut clean_shutdown = lock!(global_state.clean_shutdown);
            assert!(clean_shutdown.open_rooms.remove(&lock!(@read race_data).url));
            if clean_shutdown.requested && clean_shutdown.open_rooms.is_empty() {
                clean_shutdown.notifier.notify_waiters();
            }
            if let Ok(()) = res {
                println!("race handler for https://{}{} stopped", global_state.host, lock!(@read race_data).url);
            } else {
                println!("race handler for https://{}{} panicked", global_state.host, lock!(@read race_data).url);
            }
        });
        Ok(())
    }

    async fn new(ctx: &RaceContext<GlobalState>) -> Result<Self, Error> {
        let data = ctx.data().await;
        let goal = data.goal.name.parse::<Goal>().to_racetime()?;
        let new_room_lock = lock!(ctx.global_state.new_room_lock); // make sure a new room isn't handled before it's added to the database
        let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
        let (existing_seed, official_data, race_state, high_seed_name, low_seed_name, fpa_enabled) = if let Some(cal_event) = cal::Event::from_room(&mut transaction, &ctx.global_state.http_client, &ctx.global_state.startgg_token, format!("https://{}{}", ctx.global_state.host, ctx.data().await.url).parse()?).await.to_racetime()? {
            let event = cal_event.race.event(&mut transaction).await.to_racetime()?;
            let mut entrants = Vec::default();
            for team in cal_event.active_teams() {
                for (member, role) in team.members_roles(&mut transaction).await.to_racetime()? {
                    if event.team_config().role_is_racing(role) {
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
                            ctx.send_message(&format!(
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
                        } else if let (Some("Qualifier"), Some(round)) = (cal_event.race.phase.as_deref(), cal_event.race.round.as_ref()) {
                            format!("qualifier {round}")
                        } else {
                            format!("this {} race", match (cal_event.race.phase.as_ref(), cal_event.race.round.as_ref()) {
                                (Some(phase), Some(round)) => format!("{phase} {round}"),
                                (Some(phase), None) => phase.clone(),
                                (None, Some(round)) => round.clone(),
                                (None, None) => event.display_name.clone(),
                            })
                        },
                        event.series,
                        event.event,
                    )
                }
            }).await?;
            let (race_state, high_seed_name, low_seed_name) = if event.draft_kind().is_some() {
                let state = cal_event.race.draft.clone().expect("missing draft state");
                let [high_seed_name, low_seed_name] = match cal_event.race.entrants {
                    Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => [format!("Team A"), format!("Team B")],
                    Entrants::Two([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2)]) => if team1.id == state.high_seed {
                        [
                            team1.name(&mut transaction).await.to_racetime()?.map_or_else(|| format!("Team A"), Cow::into_owned),
                            team2.name(&mut transaction).await.to_racetime()?.map_or_else(|| format!("Team B"), Cow::into_owned),
                        ]
                    } else {
                        [
                            team2.name(&mut transaction).await.to_racetime()?.map_or_else(|| format!("Team A"), Cow::into_owned),
                            team1.name(&mut transaction).await.to_racetime()?.map_or_else(|| format!("Team B"), Cow::into_owned),
                        ]
                    },
                    Entrants::Two([_, _]) => unimplemented!("draft with non-MH teams"),
                    Entrants::Three([_, _, _]) => unimplemented!("draft with 3 teams"),
                };
                (RaceState::Draft {
                    spoiler_log: false,
                    //TODO restrict draft picks
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
            (
                cal_event.race.seed.clone(),
                Some(OfficialRaceData {
                    fpa_invoked: false,
                    cal_event, event, restreams, entrants,
                }),
                race_state,
                high_seed_name,
                low_seed_name,
                if let RaceStatusValue::Invitational = data.status.value {
                    ctx.send_message(if let French = goal.language() {
                        "Le FPA est activé pour cette race. Les joueurs pourront utiliser !fpa pendant la race pour signaler d'un problème technique de leur côté. Les race monitors doivent activer les notifications en cliquant sur l'icône de cloche 🔔 sous le chat."
                    } else {
                        "Fair play agreement is active for this official race. Entrants may use the !fpa command during the race to notify of a crash. Race monitors should enable notifications using the bell 🔔 icon below chat."
                    }).await?; //TODO different message for monitorless FPA?
                    true
                } else {
                    false
                },
            )
        } else {
            let mut race_state = RaceState::Init;
            if let Some(ref info_bot) = data.info_bot {
                for section in info_bot.split(" | ") {
                    if let Some((_, file_stem)) = regex_captures!(r"^Seed: https://midos\.house/seed/(.+)(?:\.zpfz?)?$", section) {
                        race_state = RaceState::Rolled(seed::Data {
                            file_hash: None,
                            files: seed::Files::MidosHouse {
                                file_stem: Cow::Owned(file_stem.to_owned()),
                                locked_spoiler_log_path: None,
                            },
                        });
                        break
                    } else if let Some((_, seed_id)) = regex_captures!(r"^Seed: https://ootrandomizer\.com/seed/get?id=([0-9]+)$", section) {
                        let patch_response = ctx.global_state.ootr_api_client.get("https://ootrandomizer.com/api/v2/seed/patch", Some(&[("key", &*ctx.global_state.ootr_api_client.api_key), ("id", seed_id)])).await?
                            .detailed_error_for_status().await.to_racetime()?;
                        let (_, file_stem) = regex_captures!(r"^attachment; filename=(.+)\.zpfz?$", patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION).ok_or(RollError::PatchPath).to_racetime()?.to_str()?).ok_or(RollError::PatchPath).to_racetime()?;
                        race_state = RaceState::Rolled(seed::Data {
                            file_hash: None,
                            files: seed::Files::OotrWeb {
                                id: seed_id.parse().to_racetime()?,
                                gen_time: Utc::now(),
                                file_stem: Cow::Owned(file_stem.to_owned()),
                            },
                        });
                        break
                    }
                }
            }
            if let RaceStatusValue::Pending | RaceStatusValue::InProgress = data.status.value { //TODO also check this in official races
                //TODO get chatlog and recover breaks config instead of sending this
                ctx.send_message("@entrants I just restarted and it looks like the race is already in progress. If the !breaks command was used, break notifications may be broken now. Sorry about that.").await?;
            } else {
                match race_state {
                    RaceState::Init => match goal {
                        Goal::CopaDoBrasil => {
                            ctx.send_message("Welcome! This is a practice room for the Copa do Brasil. Learn more about the tournament at https://midos.house/event/br/1").await?;
                            ctx.send_message("Create a seed with !seed").await?;
                        }
                        Goal::MixedPoolsS2 => {
                            ctx.send_message("Welcome! This is a practice room for the 2nd Mixed Pools Tournament. Learn more about the tournament at https://midos.house/event/mp/2").await?;
                            ctx.send_message("Create a seed with !seed").await?;
                        }
                        Goal::MultiworldS3 => {
                            ctx.send_message("Welcome! This is a practice room for the 3rd Multiworld Tournament. Learn more about the tournament at https://midos.house/event/mw/3").await?;
                            ctx.send_message("You can roll a seed using “!seed base”, “!seed random”, or “!seed draft”. You can also choose settings directly (e.g. !seed trials 2 wincon scrubs). For more info about these options, use !presets").await?;
                        }
                        Goal::NineDaysOfSaws => {
                            ctx.send_message("Welcome! This is a practice room for 9 Days of SAWS. Learn more about the event at https://docs.google.com/document/d/1xELThZtIctwN-vYtYhUqtd88JigNzabk8OZHANa0gqY/edit").await?;
                            ctx.send_message("You can roll a seed using “!seed day1”, “!seed day2”, etc. For more info about these options, use !presets").await?;
                        }
                        Goal::PicRs2 => {
                            ctx.send_message("Welcome! This is a practice room for the 2nd Random Settings Pictionary Spoiler Log Race. Learn more about the race at https://midos.house/event/pic/rs2").await?;
                            ctx.send_message("Create a seed with !seed").await?;
                        }
                        Goal::Rsl => {
                            ctx.send_message("Welcome to the OoTR Random Settings League! Create a seed with !seed <preset>").await?;
                            ctx.send_message("If no preset is selected, default RSL settings will be used. For a list of presets, use !presets").await?;
                        }
                        Goal::Sgl2023 => {
                            ctx.send_message("Welcome! This is a practice room for SpeedGaming Live 2023. Learn more about the tournaments at https://docs.google.com/document/d/1EACqBl8ZOreD6xT5jQ2HrdLOnpBpKyjS3FUYK8XFeqg/edit").await?;
                            ctx.send_message("Create a seed with !seed").await?;
                        }
                        Goal::TournoiFrancoS3 => {
                            ctx.send_message("Bienvenue ! Ceci est une practice room pour le tournoi francophone saison 3. Vous pouvez obtenir des renseignements supplémentaires ici : https://midos.house/event/fr/3").await?;
                            ctx.send_message("Vous pouvez roll une seed en utilisant “!seed base”, “!seed random” ou “!seed draft”. Vous pouvez également définir directement les settings (ex : !seed trials random bridge ad). Pour plus d'informations, tapez !presets").await?;
                        }
                        Goal::TriforceBlitz => {
                            ctx.send_message("Welcome to Triforce Blitz!").await?;
                            ctx.send_message("You can roll a seed using “!seed jr” for Jabu's Revenge or “!seed s2” for S2 settings, or link the seed of the day with “!seed daily”").await?;
                        }
                    },
                    RaceState::Rolled(_) => ctx.send_message("@entrants I just restarted. You may have to reconfigure !breaks and !fpa. Sorry about that.").await?,
                    RaceState::Draft { .. } | RaceState::Rolling | RaceState::SpoilerSent => unreachable!(),
                }
            }
            (
                None,
                None,
                RaceState::default(),
                format!("Team A"),
                format!("Team B"),
                false,
            )
        };
        transaction.commit().await.to_racetime()?;
        drop(new_room_lock);
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
                    if let Some(entrant) = data.entrants.iter().find(|entrant| entrant.user.id == *restreamer) {
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
                ctx.send_message(&text).await?;
            }
            let state = lock!(@write_owned this.race_state.clone());
            if let Some(seed) = existing_seed {
                this.queue_existing_seed(ctx, state, seed, English, "a", format!("seed")).await;
            } else {
                match *state {
                    RaceState::Init => match goal {
                        Goal::MixedPoolsS2 | Goal::Rsl => unreachable!("no official race rooms"),
                        Goal::MultiworldS3 | Goal::TournoiFrancoS3 => unreachable!("should have draft state set"),
                        Goal::NineDaysOfSaws => unreachable!("9dos series has concluded"),
                        Goal::CopaDoBrasil => this.roll_seed(ctx, state, goal.rando_version(), br::s1_settings(), false, English, "a", format!("seed")),
                        Goal::PicRs2 => this.roll_rsl_seed(ctx, state, VersionedRslPreset::Fenhl {
                            version: Some((Version::new(2, 3, 8), 10)),
                            preset: RslDevFenhlPreset::Pictionary,
                        }, 1, true, English, "a", format!("random settings Pictionary seed")),
                        Goal::Sgl2023 => this.roll_seed(ctx, state, goal.rando_version(), sgl::settings_2023(), false, English, "a", format!("seed")),
                        Goal::TriforceBlitz => this.roll_tfb_seed(ctx, state, false, English, "a", format!("Triforce Blitz S2 seed")).await,
                    },
                    RaceState::Draft { .. } => {
                        drop(state);
                        this.advance_draft(ctx).await?;
                    }
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => {}
                }
            }
        }
        Ok(this)
    }

    async fn command(&mut self, ctx: &RaceContext<GlobalState>, cmd_name: String, mut args: Vec<String>, _is_moderator: bool, is_monitor: bool, msg: &ChatMessage) -> Result<(), Error> {
        let goal = self.goal(ctx).await;
        let reply_to = msg.user.as_ref().map_or("friend", |user| &user.name);
        match &*cmd_name.to_ascii_lowercase() {
            "ban" => match args[..] {
                [] => self.send_settings(ctx, &if let French = goal.language() {
                    format!("Désolé {reply_to}, un setting doit être choisi. Utilisez un des suivants :")
                } else {
                    format!("Sorry {reply_to}, the setting is required. Use one of the following:")
                }, reply_to).await?,
                [ref setting] => self.draft_action(ctx, reply_to, draft::Action::Ban { setting: setting.clone() }).await?,
                [..] => ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, seul un setting peut être ban à la fois. Veuillez seulement utiliser “!ban <setting>”")
                } else {
                    format!("Sorry {reply_to}, only one setting can be banned at a time. Use “!ban <setting>”")
                }).await?,
            },
            "breaks" => match args[..] {
                [] => if let Some(breaks) = self.breaks {
                    ctx.send_message(&if let French = goal.language() {
                        format!("Vous aurez une pause de {}. Vous pouvez les désactiver avec !breaks off.", breaks.format(French))
                    } else {
                        format!("Breaks are currently set to {}. Disable with !breaks off", breaks.format(English))
                    }).await?;
                } else {
                    ctx.send_message(if let French = goal.language() {
                        "Les pauses sont actuellement désactivées. Exemple pour les activer : !breaks 5m every 2h30."
                    } else {
                        "Breaks are currently disabled. Example command to enable: !breaks 5m every 2h30"
                    }).await?;
                },
                [ref arg] if arg == "off" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                    self.breaks = None;
                    ctx.send_message(if let French = goal.language() {
                        "Les pauses sont désormais désactivées."
                    } else {
                        "Breaks are now disabled."
                    }).await?;
                } else {
                    ctx.send_message(&if let French = goal.language() {
                        format!("Désolé {reply_to}, mais la race a débuté.")
                    } else {
                        format!("Sorry {reply_to}, but the race has already started.")
                    }).await?;
                },
                _ => if let Ok(breaks) = args.join(" ").parse::<Breaks>() {
                    if breaks.duration < Duration::from_secs(60) {
                        ctx.send_message(&if let French = goal.language() {
                            format!("Désolé {reply_to}, le temps minimum pour une pause (si active) est de 1 minute. Vous pouvez désactiver les pauses avec !breaks off")
                        } else {
                            format!("Sorry {reply_to}, minimum break time (if enabled at all) is 1 minute. You can disable breaks entirely with !breaks off")
                        }).await?;
                    } else if breaks.interval < breaks.duration + Duration::from_secs(5 * 60) {
                        ctx.send_message(&if let French = goal.language() {
                            format!("Désolé {reply_to}, il doit y avoir un minimum de 5 minutes entre les pauses.")
                        } else {
                            format!("Sorry {reply_to}, there must be a minimum of 5 minutes between breaks since I notify runners 5 minutes in advance.")
                        }).await?;
                    } else if breaks.duration + breaks.interval >= Duration::from_secs(24 * 60 * 60) {
                        ctx.send_message(&if let French = goal.language() {
                            format!("Désolé {reply_to}, vous ne pouvez pas faire de pauses si tard dans la race, vu que les race rooms se ferment au bout de 24 heures.")
                        } else {
                            format!("Sorry {reply_to}, race rooms are automatically closed after 24 hours so these breaks wouldn't work.")
                        }).await?;
                    } else {
                        self.breaks = Some(breaks);
                        ctx.send_message(&if let French = goal.language() {
                            format!("Vous aurez une pause de {}.", breaks.format(French))
                        } else {
                            format!("Breaks set to {}.", breaks.format(English))
                        }).await?;
                    }
                } else {
                    ctx.send_message(&if let French = goal.language() {
                        format!("Désolé {reply_to}, je ne reconnais pas ce format pour les pauses. Exemple pour les activer : !breaks 5m every 2h30.")
                    } else {
                        format!("Sorry {reply_to}, I don't recognise that format for breaks. Example commands: !breaks 5m every 2h30, !breaks off")
                    }).await?;
                },
            },
            "draft" => match args[..] {
                [] => self.send_settings(ctx, &if let French = goal.language() {
                    format!("Désolé {reply_to}, un setting doit être choisi. Utilisez un des suivants :")
                } else {
                    format!("Sorry {reply_to}, the setting is required. Use one of the following:")
                }, reply_to).await?,
                [_] => ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, une configuration est requise.")
                } else {
                    format!("Sorry {reply_to}, the value is required.")
                }).await?, //TODO list available values
                [ref setting, ref value] => self.draft_action(ctx, reply_to, draft::Action::Pick { setting: setting.clone(), value: value.clone() }).await?,
                [..] => ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, vous ne pouvez pick qu'un setting à la fois. Veuillez seulement utiliser “!draft <setting> <configuration>”")
                } else {
                    format!("Sorry {reply_to}, only one setting can be drafted at a time. Use “!draft <setting> <value>”")
                }).await?,
            },
            "first" => self.draft_action(ctx, reply_to, draft::Action::GoFirst(true)).await?,
            "fpa" => match args[..] {
                [] => if self.fpa_enabled {
                    if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                        ctx.send_message(if let French = goal.language() {
                            "Le FPA ne peut pas être appelé avant que la race ne commence."
                        } else {
                            "FPA cannot be invoked before the race starts."
                        }).await?;
                    } else {
                        if let Some(OfficialRaceData { ref restreams, ref mut fpa_invoked, ref event, .. }) = self.official_data {
                            *fpa_invoked = true;
                            if restreams.is_empty() {
                                ctx.send_message(&if_chain! {
                                    if let French = goal.language();
                                    if let TeamConfig::Solo = event.team_config();
                                    then {
                                        format!("@everyone Le FPA a été appelé par {reply_to}. Le joueur qui ne l'a pas demandé peut continuer à jouer. La race sera re-timée après le fin de celle-ci.")
                                    } else {
                                        let player_team = if let TeamConfig::Solo = event.team_config() { "player" } else { "team" };
                                        format!("@everyone FPA has been invoked by {reply_to}. The {player_team} that did not call FPA can continue playing; the race will be retimed once completed.")
                                    }
                                }).await?;
                            } else {
                                ctx.send_message(&if let French = goal.language() {
                                    format!("@everyone Le FPA a été appelé par {reply_to}. Merci d'arrêter de jouer, la race étant restreamée.")
                                } else {
                                    format!("@everyone FPA has been invoked by {reply_to}. Please pause since this race is being restreamed.")
                                }).await?;
                            }
                        } else {
                            ctx.send_message(&if let French = goal.language() {
                                format!("@everyone Le FPA a été appelé par {reply_to}.")
                            } else {
                                format!("@everyone FPA has been invoked by {reply_to}.")
                            }).await?;
                        }
                    }
                } else {
                    ctx.send_message(if let French = goal.language() {
                        "Le FPA n'est pas activé. Les Race Monitors peuvent l'activer avec !fpa on."
                    } else {
                        "Fair play agreement is not active. Race monitors may enable FPA for this race with !fpa on"
                    }).await?;
                },
                [ref arg] => match &*arg.to_ascii_lowercase() {
                    "on" => if self.is_official() {
                        ctx.send_message(if let French = goal.language() {
                            "Le FPA est toujours activé dans les races officielles."
                        } else {
                            "Fair play agreement is always active in official races."
                        }).await?;
                    } else if !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.send_message(&if let French = goal.language() {
                            format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                        } else {
                            format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                        }).await?;
                    } else if self.fpa_enabled {
                        ctx.send_message(if let French = goal.language() {
                            "Le FPA est déjà activé."
                        } else {
                            "Fair play agreement is already activated."
                        }).await?;
                    } else {
                        self.fpa_enabled = true;
                        ctx.send_message(if let French = goal.language() {
                            "Le FPA est désormais activé. Les joueurs pourront utiliser !fpa pendant la race pour signaler d'un problème technique de leur côté. Les race monitors doivent activer les notifications en cliquant sur l'icône de cloche 🔔 sous le chat."
                        } else {
                            "Fair play agreement is now active. @entrants may use the !fpa command during the race to notify of a crash. Race monitors should enable notifications using the bell 🔔 icon below chat."
                        }).await?;
                    },
                    "off" => if self.is_official() {
                        ctx.send_message(&if let French = goal.language() {
                            format!("Désolé {reply_to}, mais le FPA ne peut pas être désactivé pour les races officielles.")
                        } else {
                            format!("Sorry {reply_to}, but FPA can't be deactivated for official races.")
                        }).await?;
                    } else if !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.send_message(&if let French = goal.language() {
                            format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                        } else {
                            format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                        }).await?;
                    } else if self.fpa_enabled {
                        self.fpa_enabled = false;
                        ctx.send_message(if let French = goal.language() {
                            "Le FPA est désormais désactivé."
                        } else {
                            "Fair play agreement is now deactivated."
                        }).await?;
                    } else {
                        ctx.send_message(if let French = goal.language() {
                            "Le FPA est déjà désactivé."
                        } else {
                            "Fair play agreement is not active."
                        }).await?;
                    },
                    _ => ctx.send_message(&if let French = goal.language() {
                        format!("Désolé {reply_to}, les seules commandes sont “!fpa on”, “!fpa off” ou “!fpa”.")
                    } else {
                        format!("Sorry {reply_to}, I don't recognize that subcommand. Use “!fpa on” or “!fpa off”, or just “!fpa” to invoke FPA.")
                    }).await?,
                },
                [..] => ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, les seules commandes sont “!fpa on”, “!fpa off” ou “!fpa”.")
                } else {
                    format!("Sorry {reply_to}, I didn't quite understand that. Use “!fpa on” or “!fpa off”, or just “!fpa” to invoke FPA.")
                }).await?,
            },
            "lock" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                self.locked = true;
                ctx.send_message(&if_chain! {
                    if let French = goal.language();
                    if !self.is_official();
                    then {
                        format!("Race verrouillée. Je ne génèrerai une seed que pour les race monitors.")
                    } else {
                        format!("Lock initiated. I will now only roll seeds for {}.", if self.is_official() { "race monitors or tournament organizers" } else { "race monitors" })
                    }
                }).await?;
            } else {
                ctx.send_message(&if let French = goal.language() {
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
                ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, seuls les organisateurs du tournoi peuvent faire cela.")
                } else {
                    format!("Sorry {reply_to}, only tournament organizers can do that.")
                }).await?;
            } else {
                ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, cette commande n'est disponible que pour les races officielles.")
                } else {
                    format!("Sorry {reply_to}, this command is only available for official races.")
                }).await?;
            },
            "presets" => goal.send_presets(ctx).await?,
            "ready" => if let Some(OfficialRaceData { ref mut restreams, ref cal_event, ref event, .. }) = self.official_data {
                if let Some(state) = restreams.values_mut().find(|state| state.restreamer_racetime_id.as_ref() == Some(&msg.user.as_ref().expect("received !ready command from bot").id)) {
                    state.ready = true;
                } else {
                    ctx.send_message(&if let French = goal.language() {
                        format!("Désolé {reply_to}, seuls les restreamers peuvent faire cela.")
                    } else {
                        format!("Sorry {reply_to}, only restreamers can do that.")
                    }).await?;
                    return Ok(())
                }
                if restreams.values().all(|state| state.ready) {
                    ctx.send_message(if_chain! {
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
                        team_race: event.team_config().is_racetime_team_format(),
                        invitational: !matches!(cal_event.race.entrants, Entrants::Open),
                        unlisted: cal_event.is_first_async_half(),
                        info_user: ctx.data().await.info_user.clone().unwrap_or_default(),
                        info_bot: ctx.data().await.info_bot.clone().unwrap_or_default(),
                        require_even_teams: true,
                        start_delay: 15,
                        time_limit: 24,
                        time_limit_auto_complete: false,
                        streaming_required: !cal_event.is_first_async_half(),
                        auto_start: true,
                        allow_comments: true,
                        hide_comments: true,
                        allow_prerace_chat: true,
                        allow_midrace_chat: true,
                        allow_non_entrant_chat: false,
                        chat_message_delay: 0,
                    }.edit_with_host(&ctx.global_state.host_info, &access_token, &ctx.global_state.http_client, CATEGORY, &ctx.data().await.slug).await?;
                } else {
                    ctx.send_message(&format!("Restream ready, still waiting for other restreams.")).await?;
                }
            } else {
                ctx.send_message(&if let French = goal.language() {
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
                            match parse_user(&mut transaction, &ctx.global_state.http_client, ctx.global_state.host, restreamer).await {
                                Ok(restreamer_racetime_id) => {
                                    if restreams.is_empty() {
                                        let (access_token, _) = racetime::authorize_with_host(&ctx.global_state.host_info, &ctx.global_state.racetime_config.client_id, &ctx.global_state.racetime_config.client_secret, &ctx.global_state.http_client).await?;
                                        racetime::StartRace {
                                            goal: goal.as_str().to_owned(),
                                            goal_is_custom: goal.is_custom(),
                                            team_race: event.team_config().is_racetime_team_format(),
                                            invitational: !matches!(cal_event.race.entrants, Entrants::Open),
                                            unlisted: cal_event.is_first_async_half(),
                                            info_user: ctx.data().await.info_user.clone().unwrap_or_default(),
                                            info_bot: ctx.data().await.info_bot.clone().unwrap_or_default(),
                                            require_even_teams: true,
                                            start_delay: 15,
                                            time_limit: 24,
                                            time_limit_auto_complete: false,
                                            streaming_required: !cal_event.is_first_async_half(),
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
                                    ctx.send_message("Restreamer assigned. Use “!ready” once the restream is ready. Auto-start will be unlocked once all restreams are ready.").await?; //TODO mention restreamer
                                }
                                Err(e) => ctx.send_message(&format!("Sorry {reply_to}, I couldn't parse the restreamer: {e}")).await?,
                            }
                            transaction.commit().await.to_racetime()?;
                        } else {
                            ctx.send_message(&format!("Sorry {reply_to}, that doesn't seem to be a valid URL or Twitch channel.")).await?;
                        }
                    } else {
                        ctx.send_message(&format!("Sorry {reply_to}, I don't recognise that format for adding a restreamer.")).await?; //TODO better help message
                    }
                } else {
                    ctx.send_message(&if let French = goal.language() {
                        format!("Désolé {reply_to}, cette commande n'est disponible que pour les races officielles.")
                    } else {
                        format!("Sorry {reply_to}, this command is only available for official races.")
                    }).await?;
                }
            } else {
                ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                } else {
                    format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                }).await?;
            },
            "second" => self.draft_action(ctx, reply_to, draft::Action::GoFirst(false)).await?,
            "seed" | "spoilerseed" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let spoiler_log = cmd_name.to_ascii_lowercase() == "spoilerseed";
                let mut state = lock!(@write_owned self.race_state.clone());
                match *state {
                    RaceState::Init => if self.locked && !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.send_message(&if let French = goal.language() {
                            format!("Désolé {reply_to}, la race est verrouillée. Seuls {} peuvent générer une seed pour cette race.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                        } else {
                            format!("Sorry {reply_to}, seed rolling is locked. Only {} may roll a seed for this race.", if self.is_official() { "race monitors or tournament organizers" } else { "race monitors" })
                        }).await?;
                    } else {
                        match goal {
                            Goal::CopaDoBrasil => self.roll_seed(ctx, state, goal.rando_version(), br::s1_settings(), false, English, "a", format!("seed")),
                            Goal::MixedPoolsS2 => if let Some(seed) = lock!(ctx.global_state.cached_mixed_pools_seed).take() {
                                let _ = ctx.global_state.seed_cache_tx.send(()).await;
                                self.queue_existing_seed(ctx, state, seed, English, "a", format!("mixed pools seed")).await;
                            } else {
                                self.roll_seed(ctx, state, goal.rando_version(), mp::s2_settings(), spoiler_log, English, "a", format!("mixed pools seed"));
                            },
                            Goal::MultiworldS3 => {
                                let settings = match args[..] {
                                    [] => {
                                        ctx.send_message(&format!("Sorry {reply_to}, the preset is required. Use one of the following:")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    }
                                    [ref arg] if arg == "base" => HashMap::default(),
                                    [ref arg] if arg == "random" => Draft {
                                        high_seed: Id(0), // Draft::complete_randomly doesn't check for active team
                                        went_first: None,
                                        skipped_bans: 0,
                                        settings: HashMap::default(),
                                    }.complete_randomly(draft::Kind::MultiworldS3).await.to_racetime()?,
                                    [ref arg] if arg == "draft" => {
                                        *state = RaceState::Draft {
                                            state: Draft {
                                                high_seed: Id(0), // racetime.gg bot doesn't check for active team
                                                went_first: None,
                                                skipped_bans: 0,
                                                settings: HashMap::default(),
                                            },
                                            spoiler_log,
                                        };
                                        drop(state);
                                        self.advance_draft(ctx).await?;
                                        return Ok(())
                                    }
                                    [ref arg] if mw::S3_SETTINGS.into_iter().any(|mw::S3Setting { name, .. }| name == arg) => {
                                        drop(state);
                                        self.send_settings(ctx, &format!("Sorry {reply_to}, you need to pair each setting with a value."), reply_to).await?;
                                        return Ok(())
                                    }
                                    [_] => {
                                        ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that preset. Use one of the following:")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    }
                                    ref args => {
                                        let args = args.iter().map(|arg| arg.to_owned()).collect_vec();
                                        let mut settings = HashMap::default();
                                        let mut tuples = args.into_iter().tuples();
                                        for (setting, value) in &mut tuples {
                                            if let Some(mw::S3Setting { default, other, .. }) = mw::S3_SETTINGS.into_iter().find(|mw::S3Setting { name, .. }| **name == setting) {
                                                if value == default || other.iter().any(|(other, _)| value == **other) {
                                                    settings.insert(Cow::Owned(setting), Cow::Owned(value));
                                                } else {
                                                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value for the {setting} setting. Use {}", iter::once(default).chain(other.iter().map(|&(other, _)| other)).join(" or "))).await?;
                                                    return Ok(())
                                                }
                                            } else {
                                                drop(state);
                                                self.send_settings(ctx, &format!("Sorry {reply_to}, I don't recognize one of those settings. Use one of the following:"), reply_to).await?;
                                                return Ok(())
                                            }
                                        }
                                        if tuples.into_buffer().next().is_some() {
                                            drop(state);
                                            self.send_settings(ctx, &format!("Sorry {reply_to}, you need to pair each setting with a value."), reply_to).await?;
                                            return Ok(())
                                        } else {
                                            settings
                                        }
                                    }
                                };
                                self.roll_seed(ctx, state, goal.rando_version(), mw::resolve_draft_settings(&settings), spoiler_log, English, "a", format!("seed with {}", mw::display_draft_picks(&settings)));
                            }
                            Goal::NineDaysOfSaws => match args[..] {
                                [] => {
                                    ctx.send_message(&format!("Sorry {reply_to}, the preset is required. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                }
                                [ref arg] => if let Some((description, mut settings)) = match &**arg {
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
                                    self.roll_seed(ctx, state, goal.rando_version(), settings, spoiler_log, goal.language(), "a", format!("{description} seed"));
                                } else {
                                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that preset. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                },
                                [..] => {
                                    ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                }
                            }
                            Goal::PicRs2 => self.roll_rsl_seed(ctx, state, VersionedRslPreset::Fenhl {
                                version: Some((Version::new(2, 3, 8), 10)),
                                preset: RslDevFenhlPreset::Pictionary,
                            }, 1, true, English, "a", format!("random settings Pictionary seed")),
                            Goal::Rsl => {
                                let (preset, world_count) = match args[..] {
                                    [] => (rsl::Preset::League, 1),
                                    [ref preset] => if let Ok(preset) = preset.parse() {
                                        if let rsl::Preset::Multiworld = preset {
                                            ctx.send_message("Missing world count (e.g. “!seed multiworld 2” for 2 worlds)").await?;
                                            return Ok(())
                                        } else {
                                            (preset, 1)
                                        }
                                    } else {
                                        ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that preset. Use one of the following:")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    },
                                    [ref preset, ref world_count] => if let Ok(preset) = preset.parse() {
                                        if let rsl::Preset::Multiworld = preset {
                                            if let Ok(world_count) = world_count.parse() {
                                                if world_count < 2 {
                                                    ctx.send_message(&format!("Sorry {reply_to}, the world count must be a number between 2 and 15.")).await?;
                                                    return Ok(())
                                                } else if world_count > 15 {
                                                    ctx.send_message(&format!("Sorry {reply_to}, I can currently only roll seeds with up to 15 worlds. Please download the RSL script from https://github.com/matthewkirby/plando-random-settings to roll seeds for more than 15 players.")).await?;
                                                    return Ok(())
                                                } else {
                                                    (preset, world_count)
                                                }
                                            } else {
                                                ctx.send_message(&format!("Sorry {reply_to}, the world count must be a number between 2 and 255.")).await?;
                                                return Ok(())
                                            }
                                        } else {
                                            ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use one of the following:")).await?;
                                            goal.send_presets(ctx).await?;
                                            return Ok(())
                                        }
                                    } else {
                                        ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that preset. Use one of the following:")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    },
                                    [..] => {
                                        ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use one of the following:")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    }
                                };
                                let (article, description) = match preset {
                                    rsl::Preset::League => ("a", format!("Random Settings League seed")),
                                    rsl::Preset::Beginner => ("a", format!("random settings Beginner seed")),
                                    rsl::Preset::Intermediate => ("a", format!("random settings Intermediate seed")),
                                    rsl::Preset::Ddr => ("a", format!("random settings DDR seed")),
                                    rsl::Preset::CoOp => ("a", format!("random settings co-op seed")),
                                    rsl::Preset::Multiworld => ("a", format!("random settings multiworld seed for {world_count} players")),
                                    rsl::Preset::S6Test => ("an", format!("RSL season 6 test seed")),
                                };
                                self.roll_rsl_seed(ctx, state, VersionedRslPreset::Xopar { version: None, preset }, world_count, spoiler_log, English, article, description);
                            }
                            Goal::Sgl2023 => self.roll_seed(ctx, state, goal.rando_version(), sgl::settings_2023(), false, English, "a", format!("seed")),
                            Goal::TournoiFrancoS3 => {
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
                                let settings = match args[..] {
                                    [] => {
                                        ctx.send_message(&format!("Désolé {reply_to}, un preset doit être défini. Veuillez utiliser un des suivants :")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    }
                                    [ref arg] if arg == "base" => HashMap::default(),
                                    [ref arg] if arg == "random" => Draft {
                                        high_seed: Id(0), // Draft::complete_randomly doesn't check for active team
                                        went_first: None,
                                        skipped_bans: 0,
                                        settings: collect![as HashMap<_, _>:
                                            Cow::Borrowed("hard_settings_ok") => Cow::Borrowed(if hard_settings_ok { "ok" } else { "no" }),
                                            Cow::Borrowed("mq_ok") => Cow::Borrowed(if mq_dungeons_count.is_some() { "ok" } else { "no" }),
                                            Cow::Borrowed("mq_dungeons_count") => Cow::Owned(mq_dungeons_count.unwrap_or_default().to_string()),
                                        ],
                                    }.complete_randomly(draft::Kind::TournoiFrancoS3).await.to_racetime()?,
                                    [ref arg] if arg == "draft" => {
                                        *state = RaceState::Draft {
                                            state: Draft {
                                                high_seed: Id(0), // racetime.gg bot doesn't check for active team
                                                went_first: None,
                                                skipped_bans: 0,
                                                settings: collect![as HashMap<_, _>:
                                                    Cow::Borrowed("hard_settings_ok") => Cow::Borrowed(if hard_settings_ok { "ok" } else { "no" }),
                                                    Cow::Borrowed("mq_ok") => Cow::Borrowed(if mq_dungeons_count.is_some() { "ok" } else { "no" }),
                                                    Cow::Borrowed("mq_dungeons_count") => Cow::Owned(mq_dungeons_count.unwrap_or_default().to_string()),
                                                ],
                                            },
                                            spoiler_log,
                                        };
                                        drop(state);
                                        self.advance_draft(ctx).await?;
                                        return Ok(())
                                    }
                                    [ref arg] if fr::S3_SETTINGS.into_iter().any(|fr::S3Setting { name, .. }| name == arg) => {
                                        drop(state);
                                        self.send_settings(ctx, &format!("Désolé {reply_to}, vous devez associer un setting avec une configuration."), reply_to).await?;
                                        return Ok(())
                                    }
                                    [_] => {
                                        ctx.send_message(&format!("Désolé {reply_to}, je ne reconnais pas ce preset. Veuillez utiliser un des suivants :")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    }
                                    ref args => {
                                        let args = args.iter().map(|arg| arg.to_owned()).collect_vec();
                                        let mut settings = HashMap::default();
                                        let mut tuples = args.into_iter().tuples();
                                        for (setting, value) in &mut tuples {
                                            if let Some(fr::S3Setting { default, other, .. }) = fr::S3_SETTINGS.into_iter().find(|fr::S3Setting { name, .. }| **name == setting) {
                                                if value == default || other.iter().any(|(other, _, _)| value == **other) {
                                                    settings.insert(Cow::Owned(setting), Cow::Owned(value));
                                                } else {
                                                    ctx.send_message(&format!("Désolé {reply_to}, je ne reconnais pas cette configuration pour {setting}. Utilisez {}", iter::once(default).chain(other.iter().map(|&(other, _, _)| other)).join(" or "))).await?;
                                                    return Ok(())
                                                }
                                            } else {
                                                drop(state);
                                                self.send_settings(ctx, &format!("Désolé {reply_to}, je ne reconnais pas un des settings. Utilisez cette liste :"), reply_to).await?;
                                                return Ok(())
                                            }
                                        }
                                        if tuples.into_buffer().next().is_some() {
                                            drop(state);
                                            self.send_settings(ctx, &format!("Désolé {reply_to}, vous devez associer un setting avec une configuration."), reply_to).await?;
                                            return Ok(())
                                        } else {
                                            settings
                                        }
                                    }
                                };
                                self.roll_seed(ctx, state, goal.rando_version(), fr::resolve_draft_settings(&settings), spoiler_log, French, "une", format!("seed avec {}", fr::display_draft_picks(&settings)));
                            }
                            Goal::TriforceBlitz => match args[..] {
                                [] => {
                                    ctx.send_message(&format!("Sorry {reply_to}, the preset is required. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                    return Ok(())
                                }
                                [ref arg] if arg == "daily" => {
                                    let (date, ordinal, file_hash) = {
                                        let response = ctx.global_state.http_client
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
                                        let date = NaiveDate::parse_from_str(&a.text_contents(), "%-d %B %Y").to_racetime()?;
                                        let file_hash = latest.select_first(".hash-icons").map_err(|()| RollError::TfbHtml).to_racetime()?
                                            .as_node()
                                            .children()
                                            .filter_map(NodeRef::into_element_ref)
                                            .filter_map(|elt| elt.attributes.borrow().get("title").and_then(|title| title.parse().ok()))
                                            .collect_vec()
                                            .try_into().map_err(|_| RollError::TfbHtml).to_racetime()?;
                                        (date, ordinal, file_hash)
                                    };
                                    self.queue_existing_seed(ctx, state, seed::Data {
                                        file_hash: Some(file_hash),
                                        files: seed::Files::TfbSotd { date, ordinal },
                                    }, English, "the", format!("Triforce Blitz seed of the day")).await;
                                }
                                [ref arg] if arg == "jr" => self.roll_seed(ctx, state, goal.rando_version(), tfb::jr_settings(), spoiler_log, English, "a", format!("Triforce Blitz: Jabu's Revenge seed")),
                                [ref arg] if arg == "s2" => self.roll_tfb_seed(ctx, state, spoiler_log, English, "a", format!("Triforce Blitz S2 seed")).await,
                                [..] => {
                                    ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                }
                            },
                        }
                    },
                    RaceState::Draft { .. } => ctx.send_message(&format!("Sorry {reply_to}, settings are already being drafted.")).await?,
                    RaceState::Rolling => ctx.send_message(&format!("Sorry {reply_to}, but I'm already rolling a seed for this room. Please wait.")).await?,
                    RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, but I already rolled a seed. Check the race info!")).await?,
                }
            } else {
                ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, mais la race a débuté.")
                } else {
                    format!("Sorry {reply_to}, but the race has already started.")
                }).await?;
            },
            "settings" => self.send_settings(ctx, if let RaceState::Draft { .. } = *lock!(@read self.race_state) {
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
            }, reply_to).await?,
            "skip" => self.draft_action(ctx, reply_to, draft::Action::Skip).await?,
            "unlock" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                self.locked = false;
                ctx.send_message(if let French = goal.language() {
                    "Race déverrouillée. N'importe qui peut désormais générer une seed."
                } else {
                    "Lock released. Anyone may now roll a seed."
                }).await?;
            } else {
                ctx.send_message(&if let French = goal.language() {
                    format!("Désolé {reply_to}, seuls {} peuvent faire cela.", if self.is_official() { "les race monitors et les organisateurs du tournoi" } else { "les race monitors" })
                } else {
                    format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })
                }).await?;
            },
            _ => ctx.send_message(&if let French = goal.language() {
                format!("Désolé {reply_to}, je ne reconnais pas cette commande.")
            } else {
                format!("Sorry {reply_to}, I don't recognize that command.")
            }).await?, //TODO “did you mean”? list of available commands with !help?
        }
        Ok(())
    }

    async fn race_data(&mut self, ctx: &RaceContext<GlobalState>, _old_race_data: RaceData) -> Result<(), Error> {
        let data = ctx.data().await;
        let goal = self.goal(ctx).await;
        if let Some(OfficialRaceData { ref entrants, .. }) = self.official_data {
            for entrant in &data.entrants {
                if entrant.status.value == EntrantStatusValue::Requested && entrants.contains(&entrant.user.id) {
                    ctx.accept_request(&entrant.user.id).await?;
                }
            }
        }
        if !self.start_saved {
            if let (Goal::Rsl, Some(start)) = (goal, data.started_at) {
                sqlx::query!("UPDATE rsl_seeds SET start = $1 WHERE room = $2", start, format!("https://{}{}", ctx.global_state.host, ctx.data().await.url)).execute(&ctx.global_state.db_pool).await.to_racetime()?;
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
                            while Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), false).await {
                                let (_, ()) = tokio::join!(
                                    ctx.send_message(if let French = goal.language() {
                                        "@entrants Rappel : pause dans 5 minutes."
                                    } else {
                                        "@entrants Reminder: Next break in 5 minutes."
                                    }),
                                    sleep(Duration::from_secs(5 * 60)),
                                );
                                if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), false).await { break }
                                let msg = if let French = goal.language() {
                                    format!("@entrants C'est l'heure de la pause ! Elle durera {}.", French.format_duration(breaks.duration, true))
                                } else {
                                    format!("@entrants Break time! Please pause for {}.", English.format_duration(breaks.duration, true))
                                };
                                let (_, ()) = tokio::join!(
                                    ctx.send_message(&msg),
                                    sleep(breaks.duration),
                                );
                                if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), false).await { break }
                                let (_, ()) = tokio::join!(
                                    ctx.send_message(if let French = goal.language() {
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
                    Goal::PicRs2 => {
                        self.goal_notifications.get_or_insert_with(|| {
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                let initial_wait = ctx.data().await.started_at.expect("in-progress race with no start time") + chrono::Duration::minutes(25) - Utc::now();
                                if let Ok(initial_wait) = initial_wait.to_std() {
                                    sleep(initial_wait).await;
                                    if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), false).await { return }
                                    let (_, ()) = tokio::join!(
                                        ctx.send_message("@entrants Reminder: 5 minutes until you can start drawing/playing."),
                                        sleep(Duration::from_secs(5 * 60)),
                                    );
                                    let _ = ctx.send_message("@entrants You may now start drawing/playing.").await;
                                }
                            })
                        });
                    }
                    Goal::TriforceBlitz => {
                        self.goal_notifications.get_or_insert_with(|| {
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                let initial_wait = ctx.data().await.started_at.expect("in-progress race with no start time") + chrono::Duration::hours(2) - Utc::now();
                                if let Ok(initial_wait) = initial_wait.to_std() {
                                    sleep(initial_wait).await;
                                    let is_1v1 = {
                                        let data = ctx.data().await;
                                        if !Self::should_handle_inner(&*data, ctx.global_state.clone(), false).await { return }
                                        data.entrants_count == 2
                                    };
                                    let _ = ctx.send_message(if is_1v1 {
                                        "@entrants Time limit reached. If anyone has found at least 1 Triforce piece, please .done. If neither player has any pieces, please continue and .done when one is found."
                                    } else {
                                        "@entrants Time limit reached. If you've found at least 1 Triforce piece, please mark yourself as done. If you haven't, you may continue playing until you find one."
                                    }).await;
                                }
                            })
                        });
                    }
                    Goal::CopaDoBrasil | Goal::MixedPoolsS2 | Goal::MultiworldS3 | Goal::NineDaysOfSaws | Goal::Rsl | Goal::Sgl2023 | Goal::TournoiFrancoS3 => {}
                }
            }
            RaceStatusValue::Finished => if self.unlock_spoiler_log(ctx).await? {
                if let Some(OfficialRaceData { ref cal_event, ref event, fpa_invoked, .. }) = self.official_data {
                    let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
                    if cal_event.is_first_async_half() {
                        if let Some(organizer_channel) = event.discord_organizer_channel {
                            organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, MessageBuilder::default()
                                //TODO mention organizer role
                                .push("first half of async finished: <https://")
                                .push(ctx.global_state.host)
                                .push(&ctx.data().await.url)
                                .push('>')
                                .build()
                            ).await.to_racetime()?;
                        }
                    } else if fpa_invoked {
                        if let Some(organizer_channel) = event.discord_organizer_channel {
                            organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, MessageBuilder::default()
                                //TODO mention organizer role
                                .push("race finished with FPA call: <https://")
                                .push(ctx.global_state.host)
                                .push(&ctx.data().await.url)
                                .push('>')
                                .build()
                            ).await.to_racetime()?;
                        }
                    } else {
                        if let Some(results_channel) = event.discord_race_results_channel.or(event.discord_organizer_channel) {
                            match event.team_config() {
                                TeamConfig::Solo => {
                                    let mut times = data.entrants.iter().map(|entrant| (entrant.user.id.clone(), entrant.finish_time.map(|time| time.to_std().expect("negative finish time")))).collect_vec();
                                    times.sort_by_key(|(_, time)| (time.is_none(), *time)); // sort DNF last
                                    if let [(ref winner, winning_time), (ref loser, losing_time)] = *times {
                                        if winning_time == losing_time {
                                            let entrant1 = User::from_racetime(&mut *transaction, winner).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let entrant2 = User::from_racetime(&mut *transaction, loser).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let msg = if_chain! {
                                                if let French = event.language;
                                                if cal_event.race.game.is_none();
                                                then {
                                                    let mut builder = MessageBuilder::default();
                                                    if let Some(finish_time) = winning_time {
                                                        builder.mention_user(&entrant1);
                                                        builder.push(" et ");
                                                        builder.mention_user(&entrant2);
                                                        builder.push(" ont fait égalité avec un temps de ");
                                                        builder.push(French.format_duration(finish_time, true));
                                                    } else {
                                                        builder.push("Ni ");
                                                        builder.mention_user(&entrant1);
                                                        builder.push(" ni ");
                                                        builder.mention_user(&entrant2);
                                                        builder.push(" n'ont fini");
                                                    }
                                                    builder
                                                        .push(" <https://")
                                                        .push(ctx.global_state.host)
                                                        .push(&ctx.data().await.url)
                                                        .push('>')
                                                        .build()
                                                } else {
                                                    let mut builder = MessageBuilder::default();
                                                    if let Some(game) = cal_event.race.game {
                                                        builder.push("game ");
                                                        builder.push(game.to_string());
                                                        builder.push(": ");
                                                    }
                                                    builder.mention_user(&entrant1);
                                                    builder.push(" and ");
                                                    builder.mention_user(&entrant2);
                                                    if let Some(finish_time) = winning_time {
                                                        builder.push(" tie their race with a time of ");
                                                        builder.push(English.format_duration(finish_time, true));
                                                    } else {
                                                        builder.push(" both did not finish");
                                                    }
                                                    builder
                                                        .push(" <https://")
                                                        .push(ctx.global_state.host)
                                                        .push(&ctx.data().await.url)
                                                        .push('>')
                                                        .build()
                                                }
                                            };
                                            results_channel.say(&*ctx.global_state.discord_ctx.read().await, msg).await.to_racetime()?;
                                        } else {
                                            let winner = User::from_racetime(&mut *transaction, winner).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let loser = User::from_racetime(&mut *transaction, loser).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let msg = if_chain! {
                                                if let French = event.language;
                                                if cal_event.race.game.is_none();
                                                then {
                                                    MessageBuilder::default()
                                                        .mention_user(&winner)
                                                        .push(" (")
                                                        .push(winning_time.map_or(Cow::Borrowed("forfait"), |time| Cow::Owned(French.format_duration(time, false))))
                                                        .push(") a battu ")
                                                        .mention_user(&loser)
                                                        .push(" (")
                                                        .push(losing_time.map_or(Cow::Borrowed("forfait"), |time| Cow::Owned(French.format_duration(time, false))))
                                                        .push(") <https://")
                                                        .push(ctx.global_state.host)
                                                        .push(&ctx.data().await.url)
                                                        .push('>')
                                                        .build()
                                                } else {
                                                    let mut builder = MessageBuilder::default();
                                                    if let Some(game) = cal_event.race.game {
                                                        builder.push("game ");
                                                        builder.push(game.to_string());
                                                        builder.push(": ");
                                                    }
                                                    builder
                                                        .mention_user(&winner)
                                                        .push(" (")
                                                        .push(winning_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(English.format_duration(time, false))))
                                                        .push(") defeats ")
                                                        .mention_user(&loser)
                                                        .push(" (")
                                                        .push(losing_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(English.format_duration(time, false))))
                                                        .push(") <https://")
                                                        .push(ctx.global_state.host)
                                                        .push(&ctx.data().await.url)
                                                        .push('>')
                                                        .build()
                                                }
                                            };
                                            results_channel.say(&*ctx.global_state.discord_ctx.read().await, msg).await.to_racetime()?;
                                        }
                                    } else {
                                        unimplemented!() //TODO handle races with more than 2 entrants
                                    }
                                }
                                TeamConfig::Pictionary => unimplemented!(), //TODO calculate like solo but report as teams
                                _ => {
                                    let mut team_times = HashMap::<_, Vec<_>>::default();
                                    for entrant in &data.entrants {
                                        if let Some(ref team) = entrant.team {
                                            team_times.entry(&team.slug).or_default().push(entrant.finish_time.map(|time| time.to_std().expect("negative finish time")));
                                        } else {
                                            unimplemented!("solo runner in team race")
                                        }
                                    }
                                    let mut team_averages = team_times.into_iter()
                                        .map(|(team_slug, times)| (team_slug, times.iter().try_fold(Duration::default(), |acc, &time| Some(acc + time?)).map(|total| total / u32::try_from(times.len()).expect("too many teams"))))
                                        .collect_vec();
                                    team_averages.sort_by_key(|(_, average)| (average.is_none(), *average)); // sort DNF last
                                    if let [(winner, winning_time), (loser, losing_time)] = *team_averages {
                                        let mut builder = MessageBuilder::default();
                                        if let Some(game) = cal_event.race.game {
                                            builder.push("game ");
                                            builder.push(game.to_string());
                                            builder.push(": ");
                                        }
                                        if winning_time == losing_time {
                                            let team1 = Team::from_racetime(&mut transaction, event.series, &event.event, winner).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let team2 = Team::from_racetime(&mut transaction, event.series, &event.event, loser).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            builder.mention_team(&mut transaction, event.discord_guild, &team1).await.to_racetime()?;
                                            builder.push(" and ");
                                            builder.mention_team(&mut transaction, event.discord_guild, &team2).await.to_racetime()?;
                                            if let Some(finish_time) = winning_time {
                                                builder.push(" tie their race with a time of ");
                                                builder.push(English.format_duration(finish_time, true));
                                            } else {
                                                builder.push(" both did not finish");
                                            }
                                            results_channel.say(&*ctx.global_state.discord_ctx.read().await, builder
                                                .push(" <https://")
                                                .push(ctx.global_state.host)
                                                .push(&ctx.data().await.url)
                                                .push('>')
                                                .build()
                                            ).await.to_racetime()?;
                                        } else {
                                            let winner = Team::from_racetime(&mut transaction, event.series, &event.event, winner).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let loser = Team::from_racetime(&mut transaction, event.series, &event.event, loser).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            results_channel.say(&*ctx.global_state.discord_ctx.read().await, builder
                                                .mention_team(&mut transaction, event.discord_guild, &winner).await.to_racetime()?
                                                .push(" (")
                                                .push(winning_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(English.format_duration(time, false))))
                                                .push(if winner.name_is_plural() { ") defeat " } else { ") defeats " })
                                                .mention_team(&mut transaction, event.discord_guild, &loser).await.to_racetime()?
                                                .push(" (")
                                                .push(losing_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(English.format_duration(time, false))))
                                                .push(") <https://")
                                                .push(ctx.global_state.host)
                                                .push(&ctx.data().await.url)
                                                .push('>')
                                                .build()
                                            ).await.to_racetime()?;
                                        }
                                    } else {
                                        unimplemented!() //TODO handle races with more than 2 teams
                                    }
                                }
                            }
                        }
                    }
                    transaction.commit().await.to_racetime()?;
                }
            },
            RaceStatusValue::Cancelled => {
                if let Some(OfficialRaceData { ref event, .. }) = self.official_data {
                    if let Some(organizer_channel) = event.discord_organizer_channel {
                        organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, MessageBuilder::default()
                            //TODO mention organizer role
                            .push("race cancelled: <https://")
                            .push(ctx.global_state.host)
                            .push(&ctx.data().await.url)
                            .push('>')
                            .build()
                        ).await.to_racetime()?;
                    }
                }
                self.unlock_spoiler_log(ctx).await?;
                if let Goal::Rsl = goal {
                    sqlx::query!("DELETE FROM rsl_seeds WHERE room = $1", format!("https://{}{}", ctx.global_state.host, ctx.data().await.url)).execute(&ctx.global_state.db_pool).await.to_racetime()?;
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
                        cal::EventKind::Async1 | cal::EventKind::Async2 => None,
                    },
                    Entrants::Three([ref team1, ref team2, ref team3]) => if let (Some(team1), Some(team2), Some(team3)) = (team1.name(&mut *transaction, discord_ctx).await.to_racetime()?, team2.name(&mut *transaction, discord_ctx).await.to_racetime()?, team3.name(&mut *transaction, discord_ctx).await.to_racetime()?) {
                        Some(Some(format!("{team1} vs {team2} vs {team3}"))) //TODO adjust for asyncs
                    } else {
                        None // no French translation available
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
                        (Some(phase), None) => Some(phase.to_owned()),
                        (None, Some(round)) => Some(round.to_owned()),
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
                        },
                        Entrants::Three([ref team1, ref team2, ref team3]) => format!(
                            "{}{} vs {} vs {}",
                            info_prefix.as_ref().map(|prefix| format!("{prefix}: ")).unwrap_or_default(),
                            team1.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team3.name(&mut *transaction, discord_ctx).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ), //TODO adjust for asyncs
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
                team_race: event.team_config().is_racetime_team_format(),
                invitational: !matches!(cal_event.race.entrants, Entrants::Open),
                unlisted: cal_event.is_first_async_half(),
                info_bot: String::default(),
                require_even_teams: true,
                start_delay: 15,
                time_limit: 24,
                time_limit_auto_complete: false,
                streaming_required: !cal_event.is_first_async_half(),
                auto_start: cal_event.is_first_async_half() || cal_event.race.video_urls.is_empty(),
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
                    let info_prefix = match (&cal_event.race.phase, &cal_event.race.round) {
                        (Some(phase), Some(round)) => Some(format!("{phase} {round}")),
                        (Some(phase), None) => Some(phase.to_owned()),
                        (None, Some(round)) => Some(round.to_owned()),
                        (None, None) => None,
                    };
                    let mut msg = MessageBuilder::default();
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
                    msg.push(" <");
                    msg.push(room_url);
                    msg.push('>');
                    msg.build()
                }
            };
            let _ = lock!(@read extra_room_tx).send(race_slug).await;
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

/// 2nd Mixed Pools Tournament seeds have a low success rate, so we keep one seed cached at all times.
async fn prepare_seeds(global_state: Arc<GlobalState>, mut seed_cache_rx: mpsc::Receiver<()>, mut shutdown: rocket::Shutdown) -> Result<(), Error> {
    'outer: loop {
        select! {
            () = &mut shutdown => break,
            Some(()) = seed_cache_rx.recv() => 'seed: loop {
                let mut seed_rx = global_state.clone().roll_seed(Goal::MixedPoolsS2.rando_version(), mp::s2_settings(), false);
                loop {
                    select! {
                        () = &mut shutdown => break 'outer,
                        Some(update) = seed_rx.recv() => match update {
                            SeedRollUpdate::Queued(_) |
                            SeedRollUpdate::MovedForward(_) |
                            SeedRollUpdate::WaitRateLimit(_) |
                            SeedRollUpdate::Started => {}
                            SeedRollUpdate::Done { seed, rsl_preset: _, send_spoiler_log: _ } => {
                                *lock!(global_state.cached_mixed_pools_seed) = Some(seed);
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
                        },
                    }
                }
            },
        }
    }
    Ok(())
}

async fn create_rooms(global_state: Arc<GlobalState>, mut shutdown: rocket::Shutdown) -> Result<(), Error> {
    loop {
        select! {
            () = &mut shutdown => break,
            _ = sleep(Duration::from_secs(60)) => { //TODO exact timing (coordinate with everything that can change the schedule)
                let new_room_lock = lock!(global_state.new_room_lock); // make sure a new room isn't handled before it's added to the database
                let mut transaction = global_state.db_pool.begin().await.to_racetime()?;
                let rooms_to_open = cal::Event::rooms_to_open(&mut transaction, &global_state.http_client, &global_state.startgg_token).await.to_racetime()?;
                for cal_event in rooms_to_open {
                    let Some(goal) = Goal::for_event(cal_event.race.series, &cal_event.race.event) else { continue };
                    if !goal.should_create_rooms() { continue }
                    let event = cal_event.race.event(&mut transaction).await.to_racetime()?;
                    if let Some(msg) = create_room(&mut transaction, &*global_state.discord_ctx.read().await, &global_state.host_info, &global_state.racetime_config.client_id, &global_state.racetime_config.client_secret, &global_state.extra_room_tx, &global_state.http_client, &cal_event, &event).await? {
                        let ctx = global_state.discord_ctx.read().await;
                        if cal_event.is_first_async_half() {
                            if let Some(channel) = event.discord_organizer_channel {
                                channel.say(&*ctx, &msg).await.to_racetime()?;
                            } else {
                                // DM Fenhl
                                UserId::new(86841168427495424).create_dm_channel(&*ctx).await.to_racetime()?.say(&*ctx, &msg).await.to_racetime()?;
                            }
                            for team in cal_event.active_teams() {
                                for member in team.members(&mut transaction).await.to_racetime()? {
                                    if let Some(discord) = member.discord {
                                        discord.id.create_dm_channel(&*ctx).await.to_racetime()?.say(&*ctx, &msg).await.to_racetime()?; //TODO different message? (e.g. “your race room is open”)
                                    }
                                }
                            }
                        } else {
                            if let Some(channel) = event.discord_race_room_channel {
                                channel.say(&*ctx, msg).await.to_racetime()?;
                            } else if let Some(thread) = cal_event.race.scheduling_thread {
                                thread.say(&*ctx, msg).await.to_racetime()?; //TODO different message? (e.g. “your race room is open”)
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
                drop(new_room_lock);
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
                *lock!(@write global_state.extra_room_tx) = bot.extra_room_sender();
                let () = bot.run_until::<Handler, _, _>(shutdown).await?;
                break Ok(())
            }
            Err(Error::Reqwest(e)) if e.status().map_or(false, |status| status.is_server_error()) => {
                if last_crash.elapsed() >= Duration::from_secs(60 * 60 * 24) {
                    wait_time = Duration::from_secs(1); // reset wait time after no crash for a day
                } else {
                    wait_time *= 2; // exponential backoff
                }
                eprintln!("failed to connect to racetime.gg (retrying in {}): {e} ({e:?})", English.format_duration(wait_time, true));
                //TODO notify if wait_time >= Duration::from_secs(2)
                sleep(wait_time).await;
                last_crash = Instant::now();
            }
            Err(e) => break Err(e),
        }
    }
}

pub(crate) async fn main(env: Environment, config: Config, shutdown: rocket::Shutdown, global_state: Arc<GlobalState>, seed_cache_rx: mpsc::Receiver<()>) -> Result<(), Error> {
    let ((), (), ()) = tokio::try_join!(
        prepare_seeds(global_state.clone(), seed_cache_rx, shutdown.clone()),
        create_rooms(global_state.clone(), shutdown.clone()),
        handle_rooms(global_state, if env.is_dev() { &config.racetime_bot_dev } else { &config.racetime_bot_production }, shutdown),
    )?;
    Ok(())
}
