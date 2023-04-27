use {
    std::{
        borrow::Cow,
        collections::{
            HashMap,
            HashSet,
        },
        fmt,
        io::prelude::*,
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
    rand::prelude::*,
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
    serenity_utils::RwFuture,
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
        discord_bot::{
            Draft,
            DraftKind,
        },
        event::{
            self,
            Series,
            TeamConfig,
        },
        seed,
        series::{
            mw,
            ndos,
            rsl,
            tfb,
        },
        team::Team,
        user::User,
        util::{
            DurationUnit,
            Id,
            MessageBuilderExt as _,
            format_duration,
            io_error_from_reqwest,
            natjoin_str,
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

const CATEGORY: &str = "ootr";

/// Randomizer versions that are known to exist on the ootrandomizer.com API. Hardcoded because the API doesn't have a “does version x exist?” endpoint.
const KNOWN_GOOD_WEB_VERSIONS: [rando::Version; 4] = [
    rando::Version::from_dev(6, 2, 181),
    rando::Version::from_dev(6, 2, 205),
    rando::Version::from_branch(rando::Branch::DevR, 6, 2, 238, 1),
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
        return if let Some(user) = User::from_id(transaction, id).await? {
            if let Some(racetime_id) = user.racetime_id {
                Ok(racetime_id)
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

#[derive(Sequence)]
pub(crate) enum Goal {
    MultiworldS3,
    NineDaysOfSaws,
    PicRs2,
    Rsl,
    TriforceBlitz,
}

#[derive(Debug, thiserror::Error)]
#[error("this racetime.gg goal is not handled by Mido")]
pub(crate) struct GoalFromStrError;

impl Goal {
    fn matches_event(&self, series: Series, event: &str) -> bool {
        match self {
            Self::MultiworldS3 => series == Series::Multiworld && event == "3",
            Self::NineDaysOfSaws => series == Series::NineDaysOfSaws,
            Self::PicRs2 => series == Series::Pictionary && event == "rs2",
            Self::Rsl => series == Series::Rsl,
            Self::TriforceBlitz => series == Series::TriforceBlitz,
        }
    }

    pub(crate) fn is_custom(&self) -> bool {
        match self {
            Self::MultiworldS3 => true,
            Self::NineDaysOfSaws => true,
            Self::PicRs2 => true,
            Self::Rsl => false,
            Self::TriforceBlitz => false,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::MultiworldS3 => "3rd Multiworld Tournament",
            Self::NineDaysOfSaws => "9 Days of SAWS",
            Self::PicRs2 => "2nd Random Settings Pictionary Spoiler Log Race",
            Self::Rsl => "Random settings league",
            Self::TriforceBlitz => "Triforce Blitz",
        }
    }

    fn draft_kind(&self) -> DraftKind {
        match self {
            Self::MultiworldS3 => DraftKind::MultiworldS3,
            Self::NineDaysOfSaws | Self::PicRs2 | Self::Rsl | Self::TriforceBlitz => DraftKind::None,
        }
    }

    fn rando_version(&self) -> rando::Version {
        match self {
            Self::MultiworldS3 => rando::Version::from_dev(6, 2, 205),
            Self::NineDaysOfSaws => rando::Version::from_branch(rando::Branch::DevFenhl, 6, 9, 14, 2),
            Self::PicRs2 | Self::Rsl => panic!("randomizer version for this goal must be parsed from RSL script"),
            Self::TriforceBlitz => panic!("seeds for this goal must be rolled manually on the Triforce Blitz website"),
        }
    }

    async fn send_presets(&self, ctx: &RaceContext<GlobalState>) -> Result<(), Error> {
        match self {
            Self::MultiworldS3 => {
                ctx.send_message("!seed base: The settings used for the qualifier and tiebreaker asyncs.").await?;
                ctx.send_message("!seed random: Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.").await?;
                ctx.send_message("!seed draft: Pick the settings here in the chat. I don't enforce that the two teams have to be represented by different people.").await?;
                ctx.send_message("!seed (<setting> <value>)... (e.g. !seed trials 2 wincon scrubs): Pick a set of draftable settings without doing a full draft. Use “!settings” for a list of available settings.").await?;
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
            Self::PicRs2 => ctx.send_message("!seed: The settings used for the race").await?,
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
            Self::TriforceBlitz => ctx.send_message("!seed: official Triforce Blitz settings").await?, //TODO option to link the daily seed
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
    host: &'static str,
    racetime_config: ConfigRaceTime,
    extra_room_tx: RwLock<mpsc::Sender<String>>,
    db_pool: PgPool,
    http_client: reqwest::Client,
    startgg_token: String,
    ootr_api_client: OotrApiClient,
    discord_ctx: RwFuture<DiscordCtx>,
    clean_shutdown: Arc<Mutex<CleanShutdown>>,
}

impl GlobalState {
    pub(crate) fn new(new_room_lock: Arc<Mutex<()>>, racetime_config: ConfigRaceTime, db_pool: PgPool, http_client: reqwest::Client, ootr_api_key: String, startgg_token: String, host: &'static str, discord_ctx: RwFuture<DiscordCtx>, clean_shutdown: Arc<Mutex<CleanShutdown>>) -> Self {
        Self {
            host_info: racetime::HostInfo {
                hostname: Cow::Borrowed(host),
                ..racetime::HostInfo::default()
            },
            extra_room_tx: RwLock::new(mpsc::channel(1).0),
            ootr_api_client: OotrApiClient::new(http_client.clone(), ootr_api_key),
            new_room_lock, host, racetime_config, db_pool, http_client, startgg_token, discord_ctx, clean_shutdown,
        }
    }

    pub(crate) fn roll_seed(self: Arc<Self>, version: rando::Version, settings: serde_json::Map<String, Json>, spoiler_log: bool) -> mpsc::Receiver<SeedRollUpdate> {
        let (update_tx, update_rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let can_roll_on_web = match self.ootr_api_client.can_roll_on_web(None, &version, settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64").try_into().expect("too many worlds"))).await {
                Ok(can_roll_on_web) => can_roll_on_web,
                Err(e) => {
                    update_tx.send(SeedRollUpdate::Error(e)).await?;
                    return Ok(())
                }
            };
            let mw_permit = if can_roll_on_web && settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64")) > 1 {
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
            if can_roll_on_web {
                match self.ootr_api_client.roll_seed_web(update_tx.clone(), version, false, spoiler_log, settings).await {
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
            } else {
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
            let local_version_file = BufReader::new(File::open(rsl_script_path.join("rslversion.py")).await?);
            let mut lines = local_version_file.lines();
            let version = loop {
                let line = lines.next_line().await?.ok_or(RollError::RslVersion)?;
                if let Some((_, local_version)) = regex_captures!("^randomizer_version = '(.+)'$", &line) {
                    break local_version.parse()?
                }
            };
            let can_roll_on_web = self.ootr_api_client.can_roll_on_web(Some(&preset), &version, world_count).await?;
            // run the RSL script
            let _ = update_tx.send(SeedRollUpdate::Started).await;
            let outer_tries = if can_roll_on_web { 5 } else { 1 }; // when generating locally, retries are already handled by the RSL script
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
                if can_roll_on_web {
                    rsl_cmd.arg("--no_seed");
                }
                let output = rsl_cmd.current_dir(&rsl_script_path).output().await.at_command("RandomSettingsGenerator.py")?;
                match output.status.code() {
                    Some(0) => {}
                    Some(2) => return Err(RollError::Retries(15)),
                    _ => return Err(RollError::Wheel(wheel::Error::CommandExit { name: Cow::Borrowed("RandomSettingsGenerator.py"), output })),
                }
                if can_roll_on_web {
                    #[derive(Deserialize)]
                    struct Plando {
                        settings: serde_json::Map<String, Json>,
                    }

                    let plando_filename = BufRead::lines(&*output.stdout)
                        .filter_map_ok(|line| Some(regex_captures!("^Plando File: (.+)$", &line)?.1.to_owned()))
                        .next().ok_or(RollError::RslScriptOutput)??;
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
                    let (seed_id, gen_time, file_hash, file_stem) = match self.ootr_api_client.roll_seed_web(update_tx.clone(), version.clone(), true, spoiler_log, settings).await {
                        Ok(data) => data,
                        Err(RollError::Retries(_)) => continue,
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
                        .next().ok_or(RollError::RslScriptOutput)??;
                    let patch_path = rsl_script_path.join("patches").join(&patch_filename);
                    let spoiler_log_filename = BufRead::lines(&*output.stdout)
                        .filter_map_ok(|line| Some(regex_captures!("^Created spoiler log at: (.+)$", &line)?.1.to_owned()))
                        .next().ok_or(RollError::RslScriptOutput)??;
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
            let _ = update_tx.send(SeedRollUpdate::Error(RollError::Retries(15))).await;
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
    settings.insert(format!("create_patch_file"), json!(true));
    settings.insert(format!("create_compressed_rom"), json!(false));
    for _ in 0..3 {
        let rando_path = version.dir()?;
        let mut rando_process = Command::new(PYTHON).arg("OoTRandomizer.py").arg("--no_log").arg("--settings=-").current_dir(rando_path).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        rando_process.stdin.as_mut().expect("piped stdin missing").write_all(&serde_json::to_vec(&settings)?).await?;
        let output = rando_process.wait_with_output().await?;
        let stderr = if output.status.success() { BufRead::lines(&*output.stderr).try_collect::<_, Vec<_>, _>()? } else { continue };
        let patch_path = Path::new(stderr.iter().rev().find_map(|line| line.strip_prefix("Created patch file archive at: ")).ok_or(RollError::PatchPath)?);
        let spoiler_log_path = Path::new(stderr.iter().rev().find_map(|line| line.strip_prefix("Created spoiler log at: ")).ok_or(RollError::SpoilerLogPath)?);
        let patch_filename = patch_path.file_name().expect("patch file path with no file name");
        fs::rename(patch_path, Path::new(seed::DIR).join(patch_filename)).await?;
        return Ok((
            patch_filename.to_str().expect("non-UTF-8 patch filename").to_owned(),
            spoiler_log_path.to_owned(),
        ))
    }
    Err(RollError::Retries(3))
}

#[derive(Debug, thiserror::Error)]
#[cfg_attr(unix, derive(Protocol))]
#[cfg_attr(unix, async_proto(via = (String, String)))]
pub(crate) enum RollError {
    #[error(transparent)] Dir(#[from] rando::DirError),
    #[error(transparent)] Git(#[from] git2::Error),
    #[error(transparent)] Header(#[from] reqwest::header::ToStrError),
    #[error(transparent)] Io(#[from] std::io::Error),
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
    #[error("RSL script not found")]
    RslPath,
    #[error("max retries exceeded")]
    Retries(u8),
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
    async fn handle(self, db_pool: &PgPool, ctx: &RaceContext<GlobalState>, state: &ArcRwLock<RaceState>, race_id: Option<Id>, an: bool, description: &str) -> Result<(), Error> {
        match self {
            Self::Queued(0) => ctx.send_message("I'm already rolling other multiworld seeds so your seed has been queued. It is at the front of the queue so it will be rolled next.").await?,
            Self::Queued(1) => ctx.send_message("I'm already rolling other multiworld seeds so your seed has been queued. There is 1 seed in front of it in the queue.").await?,
            Self::Queued(pos) => ctx.send_message(&format!("I'm already rolling other multiworld seeds so your seed has been queued. There are {pos} seeds in front of it in the queue.")).await?,
            Self::MovedForward(0) => ctx.send_message("The queue has moved and your seed is now at the front so it will be rolled next.").await?,
            Self::MovedForward(1) => ctx.send_message("The queue has moved and there is only 1 more seed in front of yours.").await?,
            Self::MovedForward(pos) => ctx.send_message(&format!("The queue has moved and there are now {pos} seeds in front of yours.")).await?,
            Self::WaitRateLimit(duration) => ctx.send_message(&format!("Your seed will be rolled in {}.", format_duration(duration, true))).await?,
            Self::Started => ctx.send_message(&format!("Rolling {} {description}…", if an { "an" } else { "a" })).await?,
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
                                seed::Files::TriforceBlitz { .. } => unreachable!(), // no such thing as random settings Triforce Blitz
                            }
                        }
                    }
                }
                let seed_url = match seed.files {
                    seed::Files::MidosHouse { ref file_stem, .. } => format!("https://midos.house/seed/{file_stem}.{}", if let Some(world_count) = extra.world_count {
                        if world_count.get() > 1 { "zpfz" } else { "zpf" }
                    } else if Path::new(seed::DIR).join(format!("{file_stem}.zpfz")).exists() {
                        "zpfz"
                    } else {
                        "zpf"
                    }),
                    seed::Files::OotrWeb { id, .. } => format!("https://ootrandomizer.com/seed/get?id={id}"),
                    seed::Files::TriforceBlitz { uuid } => format!("https://www.triforceblitz.com/seed/{uuid}"),
                };
                ctx.send_message(&format!("@entrants Here is your seed: {seed_url}")).await?;
                if let seed::Files::MidosHouse { ref file_stem, .. } = seed.files {
                    if send_spoiler_log {
                        ctx.send_message(&format!("And here is the spoiler log: https://midos.house/seed/{file_stem}_Spoiler.json")).await?;
                    } else {
                        ctx.send_message(&format!("After the race, you can view the spoiler log at https://midos.house/seed/{file_stem}_Spoiler.json")).await?;
                    }
                } else {
                    if send_spoiler_log {
                        ctx.send_message("The spoiler log is also available on the seed page.").await?;
                    } else {
                        ctx.send_message("The spoiler log will be available on the seed page after the race.").await?;
                    }
                }
                ctx.set_bot_raceinfo(&format!(
                    "{}{}{seed_url}{}",
                    if let Some(preset) = rsl_preset { format!("{}\n", preset.race_info()) } else { String::default() },
                    extra.file_hash.map(|file_hash| format!("{}\n", format_hash(file_hash))).unwrap_or_default(),
                    if_chain! {
                        if let seed::Files::MidosHouse { ref file_stem, .. } = seed.files;
                        if send_spoiler_log;
                        then {
                            format!("\nhttps://midos.house/seed/{file_stem}_Spoiler.json")
                        } else {
                            String::default()
                        }
                    },
                )).await?;
                *lock!(@write state) = RaceState::Rolled(seed);
            }
            Self::Error(RollError::Retries(num_retries)) => {
                ctx.send_message(&format!("Sorry @entrants, the randomizer reported an error {num_retries} times, so I'm giving up on rolling the seed. Please try again. If this error persists, please report it to Fenhl.")).await?;
                *lock!(@write state) = RaceState::Init;
            }
            Self::Error(msg) => {
                eprintln!("seed roll error: {msg:?}");
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

    async fn can_roll_on_web(&self, rsl_preset: Option<&VersionedRslPreset>, version: &rando::Version, world_count: u8) -> Result<bool, RollError> {
        if world_count > 3 { return Ok(false) }
        if rsl_preset.is_some() && version.branch().web_name_random_settings().is_none() { return Ok(false) }
        // check if randomizer version is available on web
        if !KNOWN_GOOD_WEB_VERSIONS.contains(&version) {
            if version.supplementary().is_some() && !matches!(rsl_preset, Some(VersionedRslPreset::Xopar { .. })) {
                // The version API endpoint does not return the supplementary version number, so we can't be sure we have the right version unless it was manually checked and added to KNOWN_GOOD_WEB_VERSIONS.
                // For the RSL script's main branch, we assume the supplementary version number is correct since we dynamically get the version from the RSL script.
                // The dev-fenhl branch of the RSL script can point to versions not available on web, so we can't make this assumption there.
                return Ok(false)
            }
            if let Ok(latest_web_version) = self.get_version(version.branch(), rsl_preset.is_some()).await {
                if latest_web_version != *version.base() { // there is no endpoint for checking whether a given version is available on the website, so for now we assume that if the required version isn't the current one, it's not available
                    println!("web version mismatch on {} branch: we need {} but latest is {latest_web_version}", version.branch().web_name(rsl_preset.is_some()).expect("checked above"), version.base());
                    return Ok(false)
                }
            } else {
                // the version API endpoint sometimes returns HTML instead of the expected JSON, fallback to generating locally when that happens
                return Ok(false)
            }
        }
        Ok(true)
    }

    async fn roll_seed_web(&self, update_tx: mpsc::Sender<SeedRollUpdate>, version: rando::Version, random_settings: bool, spoiler_log: bool, settings: serde_json::Map<String, Json>) -> Result<(u64, DateTime<Utc>, [HashIcon; 5], String), RollError> {
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
        for _ in 0..3 {
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
            if !random_settings {
                update_tx.send(SeedRollUpdate::Started).await?;
            }
            let CreateSeedResponse { id } = self.post("https://ootrandomizer.com/api/v2/seed/create", Some(&[
                ("key", &*self.api_key),
                ("version", &*format!("{}_{}", version.branch().web_name(random_settings).ok_or(RollError::RandomSettingsWeb)?, version.base())),
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
                        io::copy_buf(&mut StreamReader::new(patch_response.bytes_stream().map_err(io_error_from_reqwest)), &mut File::create(Path::new(seed::DIR).join(&patch_file_name)).await?).await?;
                        return Ok((id, creation_timestamp, settings_log.file_hash, patch_file_stem.to_owned()))
                    }
                    2 => unreachable!(), // generated with link (not possible from API)
                    3 => break, // failed to generate
                    n => return Err(RollError::UnespectedSeedStatus(n)),
                }
            }
        }
        Err(RollError::Retries(3))
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

impl fmt::Display for Breaks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} every {}", format_duration(self.duration, true), format_duration(self.interval, true))
    }
}

#[derive(Default)]
enum RaceState {
    #[default]
    Init,
    Draft {
        state: mw::S3Draft,
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
    language: Option<&'static str>,
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
                    return sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM organizers WHERE series = $1 AND event = $2 AND organizer = $3) AS "exists!""#, event.series as _, &event.event, i64::from(user.id)).fetch_one(&ctx.global_state.db_pool).await
                }
            }
        }
        Ok(false)
    }

    async fn send_settings(&self, ctx: &RaceContext<GlobalState>) -> Result<(), Error> {
        let available_settings = {
            let state = lock!(@read self.race_state);
            if let RaceState::Draft { ref state, .. } = *state {
                state.available_settings()
            } else {
                mw::S3Draft::default().available_settings()
            }
        };
        for setting in available_settings {
            match setting {
                mw::S3Setting::Wincon => ctx.send_message("wincon: meds (default: 6 Medallion Bridge + Keysy BK), scrubs (3 Stone Bridge + LACS BK), or th (Triforce Hunt 25/30)").await?,
                mw::S3Setting::Dungeons => ctx.send_message("dungeons: tournament (default: keys shuffled in own dungeon), skulls (vanilla keys, dungeon tokens), or keyrings (small keyrings anywhere, vanilla boss keys)").await?,
                mw::S3Setting::Er => ctx.send_message("er: off (default) or dungeon").await?,
                mw::S3Setting::Trials => ctx.send_message("trials: 0 (default) or 2").await?,
                mw::S3Setting::Shops => ctx.send_message("shops: 4 (default) or off").await?,
                mw::S3Setting::Scrubs => ctx.send_message("scrubs: affordable (default) or off").await?,
                mw::S3Setting::Fountain => ctx.send_message("fountain: closed (default) or open").await?,
                mw::S3Setting::Spawn => ctx.send_message("spawn: tot (default: adult start, vanilla spawns) or random (random spawns and starting age)").await?,
            }
        }
        Ok(())
    }

    async fn advance_draft(&self, ctx: &RaceContext<GlobalState>) -> Result<(), Error> {
        let state = lock!(@write_owned self.race_state.clone());
        if let RaceState::Draft { state: ref draft, spoiler_log } = *state {
            match draft.next_step() {
                mw::DraftStep::GoFirst => ctx.send_message(&format!("{}, you have the higher seed. Choose whether you want to go !first or !second", self.high_seed_name)).await?,
                mw::DraftStep::Ban { prev_bans, team } => ctx.send_message(&format!("{}, lock a setting to its default using “!ban <setting>”, or use “!skip” if you don't want to ban anything.{}", team.choose(&self.high_seed_name, &self.low_seed_name), if prev_bans == 0 { " Use “!settings” for a list of available settings." } else { "" })).await?,
                mw::DraftStep::Pick { prev_picks, team } => ctx.send_message(&match prev_picks {
                    0 => format!("{}, pick a setting using “!draft <setting> <value>”", team.choose(&self.high_seed_name, &self.low_seed_name)),
                    1 => format!("{}, pick two settings.", team.choose(&self.high_seed_name, &self.low_seed_name)),
                    2 => format!("And your second pick?"),
                    3 => format!("{}, pick the final setting. You can also use “!skip” if you want to leave the settings as they are.", team.choose(&self.high_seed_name, &self.low_seed_name)),
                    _ => unreachable!(),
                }).await?,
                mw::DraftStep::Done(settings) => self.roll_seed(ctx, state, self.goal(ctx).await.rando_version(), settings.resolve(), spoiler_log, false, format!("seed with {settings}")),
            }
        } else {
            unreachable!()
        }
        Ok(())
    }

    fn roll_seed_inner(&self, ctx: &RaceContext<GlobalState>, mut state: OwnedRwLockWriteGuard<RaceState>, mut updates: mpsc::Receiver<SeedRollUpdate>, an: bool, description: String) {
        *state = RaceState::Rolling;
        drop(state);
        let db_pool = ctx.global_state.db_pool.clone();
        let ctx = ctx.clone();
        let state = self.race_state.clone();
        let id = self.official_data.as_ref().map(|official_data| official_data.cal_event.race.id.expect("handling room for official race without ID"));
        let official_start = self.official_data.as_ref().map(|official_data| official_data.cal_event.start().expect("handling room for official race without start time"));
        tokio::spawn(async move {
            let mut seed_state = None::<SeedRollUpdate>;
            if let Some(delay) = official_start.and_then(|start| (start - chrono::Duration::minutes(15) - Utc::now()).to_std().ok()) {
                ctx.send_message(&format!("Your {description} will be posted in {}.", format_duration(delay, true))).await?;
                let mut sleep = pin!(sleep_until(Instant::now() + delay).fuse());
                loop {
                    select! {
                        () = &mut sleep => {
                            if let Some(update) = seed_state.take() {
                                update.handle(&db_pool, &ctx, &state, id, an, &description).await?;
                            }
                        }
                        Some(update) = updates.recv() => seed_state = Some(update),
                    }
                }
            } else {
                while let Some(update) = updates.recv().await {
                    update.handle(&db_pool, &ctx, &state, id, an, &description).await?;
                }
                return Ok::<_, Error>(())
            }
        });
    }

    fn roll_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, version: rando::Version, settings: serde_json::Map<String, Json>, spoiler_log: bool, an: bool, description: String) {
        self.roll_seed_inner(ctx, state, Arc::clone(&ctx.global_state).roll_seed(version, settings, spoiler_log), an, description);
    }

    fn roll_rsl_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, preset: VersionedRslPreset, world_count: u8, spoiler_log: bool, an: bool, description: String) {
        self.roll_seed_inner(ctx, state, Arc::clone(&ctx.global_state).roll_rsl_seed(preset, world_count, spoiler_log), an, description);
    }

    async fn roll_tfb_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, spoiler_log: bool, an: bool, description: String) {
        self.roll_seed_inner(ctx, state, Arc::clone(&ctx.global_state).roll_tfb_seed(format!("https://{}{}", ctx.global_state.host, ctx.data().await.url), spoiler_log), an, description);
    }

    async fn queue_existing_seed(&self, ctx: &RaceContext<GlobalState>, state: OwnedRwLockWriteGuard<RaceState>, seed: seed::Data, an: bool, description: String) {
        let (tx, rx) = mpsc::channel(1);
        tx.send(SeedRollUpdate::Done { rsl_preset: None, send_spoiler_log: false, seed }).await.unwrap();
        self.roll_seed_inner(ctx, state, rx, an, description);
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
                    seed::Files::TriforceBlitz { .. } => {} // automatically unlocked by triforceblitz.com
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
            println!("race handler for {} started", lock!(@read race_data).url);
            let res = join_handle.await;
            let mut clean_shutdown = lock!(global_state.clean_shutdown);
            assert!(clean_shutdown.open_rooms.remove(&lock!(@read race_data).url));
            if clean_shutdown.requested && clean_shutdown.open_rooms.is_empty() {
                clean_shutdown.notifier.notify_waiters();
            }
            let () = res.unwrap();
            println!("race handler for {} stopped", lock!(@read race_data).url);
        });
        Ok(())
    }

    async fn new(ctx: &RaceContext<GlobalState>) -> Result<Self, Error> {
        let data = ctx.data().await;
        let goal = data.goal.name.parse().to_racetime()?;
        let new_room_lock = lock!(ctx.global_state.new_room_lock); // make sure a new room isn't handled before it's added to the database
        let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
        let (existing_seed, official_data, race_state, high_seed_name, low_seed_name, fpa_enabled) = if let Some(cal_event) = cal::Event::from_room(&mut transaction, &ctx.global_state.http_client, &ctx.global_state.startgg_token, format!("https://{}{}", ctx.global_state.host, ctx.data().await.url).parse()?).await.to_racetime()? {
            let mut entrants = Vec::default();
            for team in cal_event.active_teams() {
                let mut members = sqlx::query_scalar!(r#"SELECT racetime_id AS "racetime_id!: String" FROM users, team_members WHERE id = member AND team = $1 AND racetime_id IS NOT NULL"#, i64::from(team.id)).fetch(&mut transaction); //TODO don't invite pilots in TeamConfig::Pictionary
                while let Some(member) = members.try_next().await.to_racetime()? {
                    if let Some(entrant) = data.entrants.iter().find(|entrant| entrant.user.id == member) {
                        match entrant.status.value {
                            EntrantStatusValue::Requested => ctx.accept_request(&member).await?,
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
                        ctx.invite_user(&member).await?;
                    }
                    entrants.push(member);
                }
            }
            let event = cal_event.race.event(&mut transaction).await.to_racetime()?;
            ctx.send_message(&format!(
                "Welcome to {}! Learn more about the event at https://midos.house/event/{}/{}",
                if event.is_single_race() {
                    format!("the {}", event.display_name) //TODO remove “the” depending on event name
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
            )).await?;
            let (race_state, high_seed_name, low_seed_name) = match event.draft_kind() {
                DraftKind::MultiworldS3 => {
                    let Draft { high_seed, .. } = cal_event.race.draft.as_ref().expect("missing draft state");
                    let [high_seed_name, low_seed_name] = match cal_event.race.entrants {
                        Entrants::Open | Entrants::Count { .. } => [format!("Team A"), format!("Team B")],
                        Entrants::Named(_) => unimplemented!("official race with opaque participants text"), //TODO
                        Entrants::Two([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2)]) => if team1.id == *high_seed {
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
                        Entrants::Two([_, _]) => unimplemented!("official race with non-MH teams"), //TODO
                        Entrants::Three([_, _, _]) => unimplemented!("official race with 3 teams"), //TODO
                    };
                    (RaceState::Draft {
                        state: cal_event.race.draft.as_ref().map(|draft| draft.state.clone()).unwrap_or_default(),
                        spoiler_log: false,
                        //TODO restrict draft picks
                    }, high_seed_name, low_seed_name)
                }
                DraftKind::None => (RaceState::Init, format!("Team A"), format!("Team B")),
            };
            let mut restreams = HashMap::default();
            if let Some(video_url) = cal_event.race.video_url.clone() {
                restreams.insert(video_url, RestreamState {
                    language: Some("English"),
                    restreamer_racetime_id: cal_event.race.restreamer.clone(),
                    ready: false,
                });
            }
            if let Some(video_url_fr) = cal_event.race.video_url_fr.clone() {
                restreams.insert(video_url_fr, RestreamState {
                    language: Some("French"),
                    restreamer_racetime_id: cal_event.race.restreamer_fr.clone(),
                    ready: false,
                });
            }
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
                    ctx.send_message("Fair play agreement is active for this official race. Entrants may use the !fpa command during the race to notify of a crash. Race monitors should enable notifications using the bell 🔔 icon below chat.").await?; //TODO different message for monitorless FPA?
                    true
                } else {
                    false
                },
            )
        } else {
            let mut race_state = RaceState::Init;
            if let Some(ref info_bot) = data.info_bot {
                for section in info_bot.split(" | ") {
                    if let Some((_, file_stem)) = regex_captures!(r"^Seed: https://midos\.house/seed/(.+)\.zpfz?$", section) {
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
            if let RaceStatusValue::Pending | RaceStatusValue::InProgress = data.status.value {
                ctx.send_message("@entrants I just restarted and it looks like the race is already in progress. If the !breaks command was used, break notifications may be broken now. Sorry about that.").await?;
            } else {
                match race_state {
                    RaceState::Init => match goal {
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
                        Goal::TriforceBlitz => {
                            ctx.send_message("Welcome to Triforce Blitz!").await?;
                            ctx.send_message("Create a seed with !seed").await?; //TODO option to link the daily seed
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
        if let Some(OfficialRaceData { ref event, ref restreams, .. }) = this.official_data {
            if let Some(restreams_text) = natjoin_str(restreams.iter().map(|(video_url, state)| format!("in {} at {video_url}", state.language.expect("preset restreams should have languages assigned")))) {
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
                    format!("This race is being restreamed {restreams_text} — auto-start is disabled. Tournament organizers can use “!monitor” to become race monitors, then invite the restreamer{0} as race monitor{0} to allow them to force-start.", if restreams.len() == 1 { "" } else { "s" })
                } else if restreams.len() == 1 {
                    format!("This race is being restreamed {restreams_text} — auto-start is disabled. The restreamer can use “!ready” to unlock auto-start.")
                } else {
                    format!("This race is being restreamed {restreams_text} — auto-start is disabled. Restreamers can use “!ready” once the restream is ready. Auto-start will be unlocked once all restreams are ready.")
                };
                ctx.send_message(&text).await?;
            }
            if let Some(seed) = existing_seed {
                let state = lock!(@write_owned this.race_state.clone());
                this.queue_existing_seed(ctx, state, seed, false, format!("seed")).await;
            } else {
                match event.draft_kind() {
                    DraftKind::MultiworldS3 => {
                        let state = lock!(@read this.race_state);
                        if let RaceState::Draft { .. } = *state {
                            drop(state);
                            this.advance_draft(ctx).await?;
                        }
                    }
                    DraftKind::None => {
                        let state = lock!(@write_owned this.race_state.clone());
                        if let RaceState::Init = *state {
                            match goal {
                                Goal::MultiworldS3 => unreachable!(), // uses DraftKind::MultiworldS3
                                Goal::NineDaysOfSaws => unreachable!(), // 9dos series has concluded
                                Goal::PicRs2 => this.roll_rsl_seed(ctx, state, VersionedRslPreset::Fenhl {
                                    version: Some((Version::new(2, 3, 8), 10)),
                                    preset: RslDevFenhlPreset::Pictionary,
                                }, 1, true, false, format!("random settings Pictionary seed")),
                                Goal::Rsl => unreachable!(), // no official race rooms
                                Goal::TriforceBlitz => this.roll_tfb_seed(ctx, state, false, false, format!("Triforce Blitz seed")).await,
                            }
                        }
                    }
                }
            }
        }
        Ok(this)
    }

    async fn command(&mut self, ctx: &RaceContext<GlobalState>, cmd_name: String, args: Vec<String>, _is_moderator: bool, is_monitor: bool, msg: &ChatMessage) -> Result<(), Error> {
        let goal = self.goal(ctx).await;
        let reply_to = msg.user.as_ref().map_or("friend", |user| &user.name);
        match &*cmd_name.to_ascii_lowercase() {
            "ban" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = lock!(@write self.race_state);
                match *state {
                    RaceState::Init => match goal.draft_kind() {
                        DraftKind::MultiworldS3 => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                        DraftKind::None => ctx.send_message(&format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?,
                    },
                    RaceState::Draft { state: ref mut draft, .. } => if draft.went_first.is_none() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”")).await?;
                    } else if draft.pick_count() >= 2 {
                        ctx.send_message(&format!("Sorry {reply_to}, bans have already been chosen.")).await?;
                    } else {
                        match args[..] {
                            [] => {
                                drop(state);
                                ctx.send_message(&format!("Sorry {reply_to}, the setting is required. Use one of the following:")).await?;
                                self.send_settings(ctx).await?;
                            }
                            [ref setting] => {
                                if let Ok(setting) = setting.parse() {
                                    if draft.available_settings().contains(&setting) {
                                        match setting {
                                            mw::S3Setting::Wincon => draft.wincon = Some(mw::Wincon::default()),
                                            mw::S3Setting::Dungeons => draft.dungeons = Some(mw::Dungeons::default()),
                                            mw::S3Setting::Er => draft.er = Some(mw::Er::default()),
                                            mw::S3Setting::Trials => draft.trials = Some(mw::Trials::default()),
                                            mw::S3Setting::Shops => draft.shops = Some(mw::Shops::default()),
                                            mw::S3Setting::Scrubs => draft.scrubs = Some(mw::Scrubs::default()),
                                            mw::S3Setting::Fountain => draft.fountain = Some(mw::Fountain::default()),
                                            mw::S3Setting::Spawn => draft.spawn = Some(mw::Spawn::default()),
                                        }
                                        drop(state);
                                        self.advance_draft(ctx).await?;
                                    } else {
                                        ctx.send_message(&format!("Sorry {reply_to}, that setting is already locked in. Use “!skip” if you don't want to ban anything.")).await?;
                                    }
                                } else {
                                    drop(state);
                                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that setting. Use one of the following:")).await?;
                                    self.send_settings(ctx).await?;
                                }
                            }
                            [_, _, ..] => ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use “!ban <setting>”")).await?,
                        }
                    },
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "breaks" => match args[..] {
                [] => if let Some(breaks) = self.breaks {
                    ctx.send_message(&format!("Breaks are currently set to {breaks}. Disable with !breaks off")).await?;
                } else {
                    ctx.send_message("Breaks are currently disabled. Example command to enable: !breaks 5m every 2h30").await?;
                },
                [ref arg] if arg == "off" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                    self.breaks = None;
                    ctx.send_message("Breaks are now disabled.").await?;
                } else {
                    ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
                },
                _ => if let Ok(breaks) = args.join(" ").parse::<Breaks>() {
                    if breaks.duration < Duration::from_secs(60) {
                        ctx.send_message(&format!("Sorry {reply_to}, minimum break time (if enabled at all) is 1 minute. You can disable breaks entirely with !breaks off")).await?;
                    } else if breaks.interval < breaks.duration + Duration::from_secs(5 * 60) {
                        ctx.send_message(&format!("Sorry {reply_to}, there must be a minimum of 5 minutes between breaks since I notify runners 5 minutes in advance.")).await?;
                    } else if breaks.duration + breaks.interval >= Duration::from_secs(24 * 60 * 60) {
                        ctx.send_message(&format!("Sorry {reply_to}, race rooms are automatically closed after 24 hours so these breaks wouldn\'t work.")).await?;
                    } else {
                        self.breaks = Some(breaks);
                        ctx.send_message(&format!("Breaks set to {breaks}.")).await?;
                    }
                } else {
                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognise that format for breaks. Example commands: !breaks 5m every 2h30, !breaks off")).await?;
                },
            },
            "draft" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = lock!(@write self.race_state);
                match *state {
                    RaceState::Init => match goal.draft_kind() {
                        DraftKind::MultiworldS3 => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                        DraftKind::None => ctx.send_message(&format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?,
                    },
                    RaceState::Draft { state: ref mut draft, .. } => if draft.went_first.is_none() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”")).await?;
                    } else if draft.pick_count() < 2 {
                        ctx.send_message(&format!("Sorry {reply_to}, bans haven't been chosen yet, use “!ban <setting>”")).await?;
                    } else {
                        match args[..] {
                            [] => {
                                drop(state);
                                ctx.send_message(&format!("Sorry {reply_to}, the setting is required. Use one of the following:")).await?;
                                self.send_settings(ctx).await?;
                            }
                            [ref setting] => {
                                if let Ok(setting) = setting.parse() {
                                    ctx.send_message(&format!("Sorry {reply_to}, the value is required. Use {}", match setting {
                                        mw::S3Setting::Wincon => all::<mw::Wincon>().map(|option| format!("“!draft wincon {}”", option.arg())).join(" or "),
                                        mw::S3Setting::Dungeons => all::<mw::Dungeons>().map(|option| format!("“!draft dungeons {}”", option.arg())).join(" or "),
                                        mw::S3Setting::Er => all::<mw::Er>().map(|option| format!("“!draft er {}”", option.arg())).join(" or "),
                                        mw::S3Setting::Trials => all::<mw::Trials>().map(|option| format!("“!draft trials {}”", option.arg())).join(" or "),
                                        mw::S3Setting::Shops => all::<mw::Shops>().map(|option| format!("“!draft shops {}”", option.arg())).join(" or "),
                                        mw::S3Setting::Scrubs => all::<mw::Scrubs>().map(|option| format!("“!draft scrubs {}”", option.arg())).join(" or "),
                                        mw::S3Setting::Fountain => all::<mw::Fountain>().map(|option| format!("“!draft fountain {}”", option.arg())).join(" or "),
                                        mw::S3Setting::Spawn => all::<mw::Spawn>().map(|option| format!("“!draft spawn {}”", option.arg())).join(" or "),
                                    })).await?;
                                } else {
                                    drop(state);
                                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that setting. Use one of the following:")).await?;
                                    self.send_settings(ctx).await?;
                                }
                            }
                            [ref setting, ref value] => {
                                if let Ok(setting) = setting.parse() {
                                    if draft.available_settings().contains(&setting) {
                                        match setting {
                                            mw::S3Setting::Wincon => if let Some(value) = all::<mw::Wincon>().find(|option| option.arg() == value) { draft.wincon = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Wincon>().map(|option| format!("“!draft wincon {}”", option.arg())).join(" or "))).await? },
                                            mw::S3Setting::Dungeons => if let Some(value) = all::<mw::Dungeons>().find(|option| option.arg() == value) { draft.dungeons = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Dungeons>().map(|option| format!("“!draft dungeons {}”", option.arg())).join(" or "))).await? },
                                            mw::S3Setting::Er => if let Some(value) = all::<mw::Er>().find(|option| option.arg() == value) { draft.er = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Er>().map(|option| format!("“!draft er {}”", option.arg())).join(" or "))).await? },
                                            mw::S3Setting::Trials => if let Some(value) = all::<mw::Trials>().find(|option| option.arg() == value) { draft.trials = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Trials>().map(|option| format!("“!draft trials {}”", option.arg())).join(" or "))).await? },
                                            mw::S3Setting::Shops => if let Some(value) = all::<mw::Shops>().find(|option| option.arg() == value) { draft.shops = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Shops>().map(|option| format!("“!draft shops {}”", option.arg())).join(" or "))).await? },
                                            mw::S3Setting::Scrubs => if let Some(value) = all::<mw::Scrubs>().find(|option| option.arg() == value) { draft.scrubs = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Scrubs>().map(|option| format!("“!draft scrubs {}”", option.arg())).join(" or "))).await? },
                                            mw::S3Setting::Fountain => if let Some(value) = all::<mw::Fountain>().find(|option| option.arg() == value) { draft.fountain = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Fountain>().map(|option| format!("“!draft fountain {}”", option.arg())).join(" or "))).await? },
                                            mw::S3Setting::Spawn => if let Some(value) = all::<mw::Spawn>().find(|option| option.arg() == value) { draft.spawn = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Spawn>().map(|option| format!("“!draft spawn {}”", option.arg())).join(" or "))).await? },
                                        }
                                    } else {
                                        drop(state);
                                        ctx.send_message(&format!("Sorry {reply_to}, that setting is already locked in. Use one of the following:")).await?;
                                        self.send_settings(ctx).await?;
                                    }
                                } else {
                                    drop(state);
                                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that setting. Use one of the following:")).await?;
                                    self.send_settings(ctx).await?;
                                }
                            }
                            [_, _, _, ..] => ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use “!draft <setting> <value>”")).await?,
                        }
                    },
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "first" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = lock!(@write self.race_state);
                match *state {
                    RaceState::Init => match goal.draft_kind() {
                        DraftKind::MultiworldS3 => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                        DraftKind::None => ctx.send_message(&format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?,
                    },
                    RaceState::Draft { state: ref mut draft, .. } => if draft.went_first.is_some() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick has already been chosen.")).await?;
                    } else {
                        draft.went_first = Some(true);
                        drop(state);
                        self.advance_draft(ctx).await?;
                    },
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "fpa" => match args[..] {
                [] => if self.fpa_enabled {
                    if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                        ctx.send_message("FPA cannot be invoked before the race starts.").await?;
                    } else {
                        if let Some(OfficialRaceData { ref restreams, ref mut fpa_invoked, ref event, .. }) = self.official_data {
                            *fpa_invoked = true;
                            if restreams.is_empty() {
                                let player_team = if let TeamConfig::Solo = event.team_config() { "player" } else { "team" };
                                ctx.send_message(&format!("@everyone FPA has been invoked by {reply_to}. The {player_team} that did not call FPA can continue playing; the race will be retimed once completed.")).await?;
                            } else {
                                ctx.send_message(&format!("@everyone FPA has been invoked by {reply_to}. Please pause since this race is being restreamed.")).await?;
                            }
                        } else {
                            ctx.send_message(&format!("@everyone FPA has been invoked by {reply_to}.")).await?;
                        }
                    }
                } else {
                    ctx.send_message("Fair play agreement is not active. Race monitors may enable FPA for this race with !fpa on").await?;
                },
                [ref arg] => match &*arg.to_ascii_lowercase() {
                    "on" => if self.is_official() {
                        ctx.send_message("Fair play agreement is always active in official races.").await?;
                    } else if !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.send_message(&format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })).await?;
                    } else if self.fpa_enabled {
                        ctx.send_message("Fair play agreement is already activated.").await?;
                    } else {
                        self.fpa_enabled = true;
                        ctx.send_message("Fair play agreement is now active. @entrants may use the !fpa command during the race to notify of a crash. Race monitors should enable notifications using the bell 🔔 icon below chat.").await?;
                    },
                    "off" => if self.is_official() {
                        ctx.send_message(&format!("Sorry {reply_to}, but FPA can't be deactivated for official races.")).await?;
                    } else if !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.send_message(&format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })).await?;
                    } else if self.fpa_enabled {
                        self.fpa_enabled = false;
                        ctx.send_message("Fair play agreement is now deactivated.").await?;
                    } else {
                        ctx.send_message("Fair play agreement is not active.").await?;
                    },
                    _ => ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that subcommand. Use “!fpa on” or “!fpa off”, or just “!fpa” to invoke FPA.")).await?,
                },
                [_, _, ..] => ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use “!fpa on” or “!fpa off”, or just “!fpa” to invoke FPA.")).await?,
            },
            "lock" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                self.locked = true;
                ctx.send_message(&format!("Lock initiated. I will now only roll seeds for {}.", if self.is_official() { "race monitors or tournament organizers" } else { "race monitors" })).await?;
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })).await?;
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
                ctx.send_message(&format!("Sorry {reply_to}, only tournament organizers can do that.")).await?;
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, this command is only available for official races.")).await?;
            },
            "presets" => goal.send_presets(ctx).await?,
            "ready" => if let Some(OfficialRaceData { ref mut restreams, ref cal_event, ref event, .. }) = self.official_data {
                if let Some(state) = restreams.values_mut().find(|state| state.restreamer_racetime_id.as_ref() == Some(&msg.user.as_ref().expect("received !ready command from bot").id)) {
                    state.ready = true;
                } else {
                    ctx.send_message(&format!("Sorry {reply_to}, only restreamers can do that.")).await?;
                    return Ok(())
                }
                if restreams.values().all(|state| state.ready) {
                    ctx.send_message(&format!("All restreams ready, unlocking auto-start…")).await?;
                    let (access_token, _) = racetime::authorize_with_host(&ctx.global_state.host_info, &ctx.global_state.racetime_config.client_id, &ctx.global_state.racetime_config.client_secret, &ctx.global_state.http_client).await?;
                    racetime::StartRace {
                        goal: goal.as_str().to_owned(),
                        goal_is_custom: goal.is_custom(),
                        team_race: !matches!(event.team_config(), TeamConfig::Solo | TeamConfig::Pictionary),
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
                ctx.send_message(&format!("Sorry {reply_to}, this command is only available for official races.")).await?;
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
                                            team_race: !matches!(event.team_config(), TeamConfig::Solo | TeamConfig::Pictionary),
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
                    ctx.send_message(&format!("Sorry {reply_to}, this command is only available for official races.")).await?;
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })).await?;
            },
            "second" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = lock!(@write self.race_state);
                match *state {
                    RaceState::Init => match goal.draft_kind() {
                        DraftKind::MultiworldS3 => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                        DraftKind::None => ctx.send_message(&format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?,
                    },
                    RaceState::Draft { state: ref mut draft, .. } => if draft.went_first.is_some() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick has already been chosen.")).await?;
                    } else {
                        draft.went_first = Some(false);
                        drop(state);
                        self.advance_draft(ctx).await?;
                    },
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "seed" | "spoilerseed" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let spoiler_log = cmd_name.to_ascii_lowercase() == "spoilerseed";
                let mut state = lock!(@write_owned self.race_state.clone());
                match *state {
                    RaceState::Init => if self.locked && !self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                        ctx.send_message(&format!("Sorry {reply_to}, seed rolling is locked. Only {} may roll a seed for this race.", if self.is_official() { "race monitors or tournament organizers" } else { "race monitors" })).await?;
                    } else {
                        match goal {
                            Goal::MultiworldS3 => match args[..] {
                                [] => {
                                    ctx.send_message(&format!("Sorry {reply_to}, the preset is required. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                }
                                [ref arg] if arg == "base" => self.roll_seed(ctx, state, goal.rando_version(), mw::S3Settings::default().resolve(), spoiler_log, false, format!("seed with {}", mw::S3Settings::default())),
                                [ref arg] if arg == "random" => {
                                    let settings = mw::S3Settings::random(&mut thread_rng());
                                    self.roll_seed(ctx, state, goal.rando_version(), settings.resolve(), spoiler_log, false, format!("seed with {settings}"));
                                }
                                [ref arg] if arg == "draft" => {
                                    *state = RaceState::Draft { state: mw::S3Draft::default(), spoiler_log };
                                    drop(state);
                                    self.advance_draft(ctx).await?;
                                }
                                [ref arg] if arg.parse::<mw::S3Setting>().is_ok() => {
                                    drop(state);
                                    ctx.send_message(&format!("Sorry {reply_to}, you need to pair each setting with a value.")).await?;
                                    self.send_settings(ctx).await?;
                                }
                                [_] => {
                                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that preset. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                }
                                ref args => {
                                    let args = args.iter().map(|arg| arg.to_owned()).collect_vec();
                                    let mut settings = mw::S3Settings::default();
                                    let mut tuples = args.into_iter().tuples();
                                    for (setting, value) in &mut tuples {
                                        if let Ok(setting) = setting.parse() {
                                            match setting {
                                                mw::S3Setting::Wincon => if let Some(value) = all::<mw::Wincon>().find(|option| option.arg() == value) { settings.wincon = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Wincon>().map(|option| option.arg()).join(" or "))).await?; return Ok(()) },
                                                mw::S3Setting::Dungeons => if let Some(value) = all::<mw::Dungeons>().find(|option| option.arg() == value) { settings.dungeons = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Dungeons>().map(|option| option.arg()).join(" or "))).await?; return Ok(()) },
                                                mw::S3Setting::Er => if let Some(value) = all::<mw::Er>().find(|option| option.arg() == value) { settings.er = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Er>().map(|option| option.arg()).join(" or "))).await?; return Ok(()) },
                                                mw::S3Setting::Trials => if let Some(value) = all::<mw::Trials>().find(|option| option.arg() == value) { settings.trials = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Trials>().map(|option| option.arg()).join(" or "))).await?; return Ok(()) },
                                                mw::S3Setting::Shops => if let Some(value) = all::<mw::Shops>().find(|option| option.arg() == value) { settings.shops = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Shops>().map(|option| option.arg()).join(" or "),)).await?; return Ok(()) },
                                                mw::S3Setting::Scrubs => if let Some(value) = all::<mw::Scrubs>().find(|option| option.arg() == value) { settings.scrubs = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Scrubs>().map(|option| option.arg()).join(" or "))).await?; return Ok(()) },
                                                mw::S3Setting::Fountain => if let Some(value) = all::<mw::Fountain>().find(|option| option.arg() == value) { settings.fountain = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Fountain>().map(|option| option.arg()).join(" or "))).await?; return Ok(()) },
                                                mw::S3Setting::Spawn => if let Some(value) = all::<mw::Spawn>().find(|option| option.arg() == value) { settings.spawn = value; } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Spawn>().map(|option| option.arg()).join(" or "))).await?; return Ok(()) },
                                            }
                                        } else {
                                            drop(state);
                                            ctx.send_message(&format!("Sorry {reply_to}, I don't recognize one of those settings. Use one of the following:")).await?;
                                            self.send_settings(ctx).await?;
                                            return Ok(())
                                        }
                                    }
                                    if tuples.into_buffer().next().is_some() {
                                        drop(state);
                                        ctx.send_message(&format!("Sorry {reply_to}, you need to pair each setting with a value.")).await?;
                                        self.send_settings(ctx).await?;
                                    } else {
                                        self.roll_seed(ctx, state, goal.rando_version(), settings.resolve(), spoiler_log, false, format!("seed with {settings}"));
                                    }
                                }
                            },
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
                                                "item":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                                "song":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                                "overworld":       {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                                "dungeon":         {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                                "junk":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                                                "named-item":      {"order": 9, "weight": 0.0, "fixed":   0, "copies": 2},
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
                                    self.roll_seed(ctx, state, goal.rando_version(), settings, spoiler_log, false, format!("{description} seed"));
                                } else {
                                    ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that preset. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                },
                                [_, _, ..] => {
                                    ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use one of the following:")).await?;
                                    goal.send_presets(ctx).await?;
                                }
                            }
                            Goal::PicRs2 => self.roll_rsl_seed(ctx, state, VersionedRslPreset::Fenhl {
                                version: Some((Version::new(2, 3, 8), 10)),
                                preset: RslDevFenhlPreset::Pictionary,
                            }, 1, true, false, format!("random settings Pictionary seed")),
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
                                    [_, _, _, ..] => {
                                        ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use one of the following:")).await?;
                                        goal.send_presets(ctx).await?;
                                        return Ok(())
                                    }
                                };
                                let (an, description) = match preset {
                                    rsl::Preset::League => (false, format!("Random Settings League seed")),
                                    rsl::Preset::Beginner => (false, format!("random settings Beginner seed")),
                                    rsl::Preset::Intermediate => (false, format!("random settings Intermediate seed")),
                                    rsl::Preset::Ddr => (false, format!("random settings DDR seed")),
                                    rsl::Preset::CoOp => (false, format!("random settings co-op seed")),
                                    rsl::Preset::Multiworld => (false, format!("random settings multiworld seed for {world_count} players")),
                                    rsl::Preset::S6Test => (true, format!("RSL season 6 test seed")),
                                };
                                self.roll_rsl_seed(ctx, state, VersionedRslPreset::Xopar { version: None, preset }, world_count, spoiler_log, an, description);
                            }
                            Goal::TriforceBlitz => self.roll_tfb_seed(ctx, state, spoiler_log, false, format!("Triforce Blitz seed")).await, //TODO option to link the daily seed
                        }
                    },
                    RaceState::Draft { .. } => ctx.send_message(&format!("Sorry {reply_to}, settings are already being drafted.")).await?,
                    RaceState::Rolling => ctx.send_message(&format!("Sorry {reply_to}, but I'm already rolling a seed for this room. Please wait.")).await?,
                    RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, but I already rolled a seed. Check the race info!")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "settings" => self.send_settings(ctx).await?,
            "skip" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = lock!(@write self.race_state);
                match *state {
                    RaceState::Init => match goal.draft_kind() {
                        DraftKind::MultiworldS3 => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                        DraftKind::None => ctx.send_message(&format!("Sorry {reply_to}, this event doesn't have a settings draft.")).await?,
                    },
                    RaceState::Draft { state: ref mut draft, .. } => if draft.went_first.is_none() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”")).await?;
                    } else if let 0 | 1 | 5 = draft.pick_count() {
                        draft.skipped_bans += 1;
                        drop(state);
                        self.advance_draft(ctx).await?;
                    } else {
                        ctx.send_message(&format!("Sorry {reply_to}, this part of the draft can't be skipped.")).await?;
                    },
                    RaceState::Rolling | RaceState::Rolled(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "unlock" => if self.can_monitor(ctx, is_monitor, msg).await.to_racetime()? {
                self.locked = false;
                ctx.send_message("Lock released. Anyone may now roll a seed.").await?;
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, only {} can do that.", if self.is_official() { "race monitors and tournament organizers" } else { "race monitors" })).await?;
            },
            _ => ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that command.")).await?, //TODO “did you mean”? list of available commands with !help?
        }
        Ok(())
    }

    async fn race_data(&mut self, ctx: &RaceContext<GlobalState>, _old_race_data: RaceData) -> Result<(), Error> {
        let data = ctx.data().await;
        if let Some(OfficialRaceData { ref entrants, .. }) = self.official_data {
            for entrant in &data.entrants {
                if entrant.status.value == EntrantStatusValue::Requested && entrants.contains(&entrant.user.id) {
                    ctx.accept_request(&entrant.user.id).await?;
                }
            }
        }
        if !self.start_saved {
            if let (Goal::Rsl, Some(start)) = (self.goal(ctx).await, data.started_at) {
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
                                    ctx.send_message("@entrants Reminder: Next break in 5 minutes."),
                                    sleep(Duration::from_secs(5 * 60)),
                                );
                                if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), false).await { break }
                                let msg = format!("@entrants Break time! Please pause for {}.", format_duration(breaks.duration, true));
                                let (_, ()) = tokio::join!(
                                    ctx.send_message(&msg),
                                    sleep(breaks.duration),
                                );
                                if !Self::should_handle_inner(&*ctx.data().await, ctx.global_state.clone(), false).await { break }
                                let (_, ()) = tokio::join!(
                                    ctx.send_message("@entrants Break ended. You may resume playing."),
                                    sleep(breaks.interval - breaks.duration - Duration::from_secs(5 * 60)),
                                );
                            }
                        })
                    });
                }
                match self.goal(&ctx).await {
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
                    Goal::MultiworldS3 | Goal::NineDaysOfSaws | Goal::Rsl => {}
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
                                            let entrant1 = User::from_racetime(&mut transaction, winner).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let entrant2 = User::from_racetime(&mut transaction, loser).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let mut builder = MessageBuilder::default();
                                            builder.mention_user(&entrant1);
                                            builder.push(" and ");
                                            builder.mention_user(&entrant2);
                                            if let Some(finish_time) = winning_time {
                                                builder.push(" tie their race with a time of ");
                                                builder.push(format_duration(finish_time, true));
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
                                            let winner = User::from_racetime(&mut transaction, winner).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let loser = User::from_racetime(&mut transaction, loser).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let mut msg = MessageBuilder::default();
                                            if let Some(game) = cal_event.race.game {
                                                msg.push("game ");
                                                msg.push(game.to_string());
                                                msg.push(": ");
                                            }
                                            results_channel.say(&*ctx.global_state.discord_ctx.read().await, msg
                                                .mention_user(&winner)
                                                .push(" (")
                                                .push(winning_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(format_duration(time, false))))
                                                .push(") defeats ")
                                                .mention_user(&loser)
                                                .push(" (")
                                                .push(losing_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(format_duration(time, false))))
                                                .push(") <https://")
                                                .push(ctx.global_state.host)
                                                .push(&ctx.data().await.url)
                                                .push('>')
                                                .build()
                                            ).await.to_racetime()?;
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
                                        if winning_time == losing_time {
                                            let team1 = Team::from_racetime(&mut transaction, event.series, &event.event, winner).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let team2 = Team::from_racetime(&mut transaction, event.series, &event.event, loser).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                            let mut builder = MessageBuilder::default();
                                            builder.mention_team(&mut transaction, event.discord_guild, &team1).await.to_racetime()?;
                                            builder.push(" and ");
                                            builder.mention_team(&mut transaction, event.discord_guild, &team2).await.to_racetime()?;
                                            if let Some(finish_time) = winning_time {
                                                builder.push(" tie their race with a time of ");
                                                builder.push(format_duration(finish_time, true));
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
                                            let mut msg = MessageBuilder::default();
                                            if let Some(game) = cal_event.race.game {
                                                msg.push("game ");
                                                msg.push(game.to_string());
                                                msg.push(": ");
                                            }
                                            results_channel.say(&*ctx.global_state.discord_ctx.read().await, msg
                                                .mention_team(&mut transaction, event.discord_guild, &winner).await.to_racetime()?
                                                .push(" (")
                                                .push(winning_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(format_duration(time, false))))
                                                .push(if winner.name_is_plural() { ") defeat " } else { ") defeats " })
                                                .mention_team(&mut transaction, event.discord_guild, &loser).await.to_racetime()?
                                                .push(" (")
                                                .push(losing_time.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(format_duration(time, false))))
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
                if let Goal::Rsl = self.goal(ctx).await {
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

pub(crate) async fn create_room(transaction: &mut Transaction<'_, Postgres>, host_info: &racetime::HostInfo, client_id: &str, client_secret: &str, http_client: &reqwest::Client, cal_event: &cal::Event, event: &event::Data<'_>) -> Result<Option<(String, String)>, Error> {
    let Some(goal) = all::<Goal>().find(|goal| goal.matches_event(cal_event.race.series, &cal_event.race.event)) else { return Ok(None) };
    match racetime::authorize_with_host(host_info, client_id, client_secret, http_client).await {
        Ok((access_token, _)) => {
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
                        team1.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        team2.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    ),
                    cal::EventKind::Async1 => format!(
                        "{} (async): {} vs {}",
                        info_prefix.clone().unwrap_or_default(),
                        team1.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        team2.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    ),
                    cal::EventKind::Async2 => format!(
                        "{} (async): {} vs {}",
                        info_prefix.clone().unwrap_or_default(),
                        team2.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        team1.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    ),
                },
                Entrants::Three([ref team1, ref team2, ref team3]) => format!(
                    "{}{} vs {} vs {}",
                    info_prefix.as_ref().map(|prefix| format!("{prefix}: ")).unwrap_or_default(),
                    team1.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team2.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team3.name(&mut *transaction).await.to_racetime()?.unwrap_or(Cow::Borrowed("(unnamed)")),
                ), //TODO adjust for asyncs
            };
            if let Some(game) = cal_event.race.game {
                info_user.push_str(", game ");
                info_user.push_str(&game.to_string());
            }
            let race_slug = racetime::StartRace {
                goal: goal.as_str().to_owned(),
                goal_is_custom: goal.is_custom(),
                team_race: !matches!(event.team_config(), TeamConfig::Solo | TeamConfig::Pictionary),
                invitational: !matches!(cal_event.race.entrants, Entrants::Open),
                unlisted: cal_event.is_first_async_half(),
                info_bot: String::default(),
                require_even_teams: true,
                start_delay: 15,
                time_limit: 24,
                time_limit_auto_complete: false,
                streaming_required: !cal_event.is_first_async_half(),
                auto_start: cal_event.is_first_async_half() || (cal_event.race.video_url.is_none() && cal_event.race.video_url_fr.is_none()),
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
                cal::EventKind::Normal => { sqlx::query!("UPDATE races SET room = $1 WHERE id = $2", room_url.to_string(), i64::from(cal_event.race.id.expect("created from ID"))).execute(&mut *transaction).await.to_racetime()?; }
                cal::EventKind::Async1 => { sqlx::query!("UPDATE races SET async_room1 = $1 WHERE id = $2", room_url.to_string(), i64::from(cal_event.race.id.expect("created from ID"))).execute(&mut *transaction).await.to_racetime()?; }
                cal::EventKind::Async2 => { sqlx::query!("UPDATE races SET async_room2 = $1 WHERE id = $2", room_url.to_string(), i64::from(cal_event.race.id.expect("created from ID"))).execute(&mut *transaction).await.to_racetime()?; }
            }
            let mut msg = MessageBuilder::default();
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
            Ok(Some((race_slug, msg.build())))
        }
        Err(Error::Reqwest(e)) if e.status().map_or(false, |status| status.is_server_error()) => {
            // racetime.gg's auth endpoint has been known to return server errors intermittently.
            // In that case, we simply try again in the next iteration of the sleep loop.
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

async fn create_rooms(global_state: Arc<GlobalState>, mut shutdown: rocket::Shutdown) -> Result<(), Error> {
    loop {
        select! {
            () = &mut shutdown => break,
            _ = sleep(Duration::from_secs(60)) => {
                let new_room_lock = lock!(global_state.new_room_lock); // make sure a new room isn't handled before it's added to the database
                let mut transaction = global_state.db_pool.begin().await.to_racetime()?;
                let rooms_to_open = cal::Event::rooms_to_open(&mut transaction, &global_state.http_client, &global_state.startgg_token).await.to_racetime()?;
                for cal_event in rooms_to_open {
                    let Some(goal) = all::<Goal>().find(|goal| goal.matches_event(cal_event.race.series, &cal_event.race.event)) else { continue };
                    if let Goal::Rsl = goal { continue } // we're currently not opening rooms for RSL bracket matches despite having a goal for it
                    let event = cal_event.race.event(&mut transaction).await.to_racetime()?;
                    if let Some((race_slug, msg)) = create_room(&mut transaction, &global_state.host_info, &global_state.racetime_config.client_id, &global_state.racetime_config.client_secret, &global_state.http_client, &cal_event, &event).await? {
                        let _ = lock!(@read global_state.extra_room_tx).send(race_slug).await;
                        if cal_event.is_first_async_half() {
                            if let Some(channel) = event.discord_organizer_channel {
                                channel.say(&*global_state.discord_ctx.read().await, msg).await.to_racetime()?;
                            }
                            //TODO DM participants on Discord
                        } else if let Some(channel) = event.discord_race_room_channel {
                            channel.say(&*global_state.discord_ctx.read().await, msg).await.to_racetime()?;
                        } else if let Some(thread) = cal_event.race.scheduling_thread {
                            thread.say(&*global_state.discord_ctx.read().await, msg).await.to_racetime()?; //TODO different message? (e.g. “your race room is open”)
                        } else if let Some(channel) = event.discord_organizer_channel {
                            channel.say(&*global_state.discord_ctx.read().await, msg).await.to_racetime()?;
                        } else {
                            // DM Fenhl
                            let ctx = global_state.discord_ctx.read().await;
                            UserId::new(86841168427495424).create_dm_channel(&*ctx).await.to_racetime()?.say(&*ctx, msg).await.to_racetime()?;
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
                eprintln!("failed to connect to racetime.gg: {e} ({e:?})");
                //TODO notify if wait_time >= Duration::from_secs(2)
                sleep(wait_time).await;
                last_crash = Instant::now();
            }
            Err(e) => break Err(e),
        }
    }
}

pub(crate) async fn main(env: Environment, config: Config, shutdown: rocket::Shutdown, global_state: Arc<GlobalState>) -> Result<(), Error> {
    let ((), ()) = tokio::try_join!(
        create_rooms(global_state.clone(), shutdown.clone()),
        handle_rooms(global_state, if env.is_dev() { &config.racetime_bot_dev } else { &config.racetime_bot_production }, shutdown),
    )?;
    Ok(())
}
