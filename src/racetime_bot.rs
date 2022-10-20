use {
    std::{
        fmt,
        io::prelude::*,
        path::{
            Path,
            PathBuf,
        },
        process::Stdio,
        sync::Arc,
        time::Duration,
    },
    async_trait::async_trait,
    chrono::prelude::*,
    enum_iterator::all,
    futures::stream::TryStreamExt as _,
    itertools::Itertools as _,
    lazy_regex::regex_captures,
    racetime::{
        Error,
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
    serde_json::json,
    serde_with::{
        DisplayFromStr,
        json::JsonString,
        serde_as,
    },
    serenity::{
        client::Context as DiscordCtx,
        utils::MessageBuilder,
    },
    serenity_utils::RwFuture,
    sqlx::{
        PgPool,
        types::Json,
    },
    tokio::{
        fs::{
            self,
            File,
        },
        io::{
            self,
            AsyncWriteExt as _,
        },
        process::Command,
        select,
        sync::{
            Mutex,
            OwnedRwLockWriteGuard,
            RwLock,
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
    wheel::traits::ReqwestResponseExt as _,
    crate::{
        Environment,
        cal::{
            Race,
            RaceKind,
        },
        config::Config,
        discord_bot::Draft,
        event::{
            self,
            Series,
            mw,
        },
        seed::{
            self,
            HashIcon,
            SpoilerLog,
        },
        util::{
            MessageBuilderExt as _,
            format_duration,
            io_error_from_reqwest,
        },
    },
};
#[cfg(unix)] use xdg::BaseDirectories;
#[cfg(windows)] use directories::UserDirs;

#[cfg(unix)] const PYTHON: &str = "python3";
#[cfg(windows)] const PYTHON: &str = "py";

const CATEGORY: &str = "ootr";

const RANDO_VERSION: Version = Version::new(6, 2, 205);
/// Randomizer versions that are known to exist on the ootrandomizer.com API. Hardcoded because the API doesn't have a “does version x exist?” endpoint.
const KNOWN_GOOD_WEB_VERSIONS: [Version; 2] = [
    Version::new(6, 2, 181),
    Version::new(6, 2, 205),
];

const MULTIWORLD_RATE_LIMIT: Duration = Duration::from_secs(20);

struct GlobalState {
    /// Locked while event rooms are being created. Wait with handling new rooms while it's held.
    new_room_lock: Mutex<()>,
    host: &'static str,
    db_pool: PgPool,
    http_client: reqwest::Client,
    startgg_token: String,
    mw_seed_queue: MwSeedQueue,
}

impl GlobalState {
    fn new(db_pool: PgPool, http_client: reqwest::Client, ootr_api_key: String, startgg_token: String, host: &'static str) -> Self {
        Self {
            new_room_lock: Mutex::default(),
            mw_seed_queue: MwSeedQueue::new(http_client.clone(), ootr_api_key),
            host, db_pool, http_client, startgg_token,
        }
    }

    fn roll_seed(self: Arc<Self>, settings: mw::S3Settings) -> mpsc::Receiver<SeedRollUpdate> {
        let settings = settings.resolve();
        let (update_tx, update_rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let permit = match self.mw_seed_queue.seed_rollers.try_acquire() {
                Ok(permit) => permit,
                Err(TryAcquireError::Closed) => unreachable!(),
                Err(TryAcquireError::NoPermits) => {
                    let (mut pos, mut pos_rx) = {
                        let mut waiting = self.mw_seed_queue.waiting.lock().await;
                        let pos = waiting.len();
                        let (pos_tx, pos_rx) = mpsc::unbounded_channel();
                        waiting.push(pos_tx);
                        (pos, pos_rx)
                    };
                    update_tx.send(SeedRollUpdate::Queued(pos)).await?;
                    while pos > 0 {
                        let () = pos_rx.recv().await.expect("queue position notifier closed");
                        pos -= 1;
                        update_tx.send(SeedRollUpdate::MovedForward(pos)).await?;
                    }
                    let mut waiting = self.mw_seed_queue.waiting.lock().await;
                    let permit = self.mw_seed_queue.seed_rollers.acquire().await.expect("seed queue semaphore closed");
                    waiting.remove(0);
                    for tx in &*waiting {
                        let _ = tx.send(());
                    }
                    permit
                }
            };
            let can_roll_on_web = match self.mw_seed_queue.can_roll_on_web(&settings).await {
                Ok(can_roll_on_web) => can_roll_on_web,
                Err(e) => {
                    update_tx.send(SeedRollUpdate::Error(e)).await?;
                    return Ok(())
                }
            };
            if can_roll_on_web {
                match self.mw_seed_queue.roll_seed_web(update_tx.clone(), settings).await {
                    Ok((seed_id, file_hash)) => update_tx.send(SeedRollUpdate::DoneWeb(seed_id, file_hash)).await?,
                    Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?,
                }
                drop(permit);
            } else {
                drop(permit); //TODO skip queue entirely?
                update_tx.send(SeedRollUpdate::Started).await?;
                match self.mw_seed_queue.roll_seed_locally(settings).await {
                    Ok((patch_filename, spoiler_log_path)) => update_tx.send(SeedRollUpdate::DoneLocal(patch_filename, spoiler_log_path)).await?,
                    Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?,
                }
            }
            Ok::<_, mpsc::error::SendError<_>>(())
        });
        update_rx
    }
}

#[derive(Debug, thiserror::Error)]
enum RollError {
    #[error(transparent)] Header(#[from] reqwest::header::ToStrError),
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error(transparent)] Json(#[from] serde_json::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[cfg(unix)] #[error(transparent)] Xdg(#[from] xdg::BaseDirectoriesError),
    #[error("there is nothing waiting for this seed anymore")]
    ChannelClosed,
    #[error("randomizer did not report patch location")]
    PatchPath,
    #[error("randomizer version not found")]
    RandoPath,
    #[error("max retries exceeded")]
    Retries,
    #[error("randomizer did not report spoiler log location")]
    SpoilerLogPath,
    #[error("seed status API endpoint returned unknown value {0}")]
    UnespectedSeedStatus(u8),
}

impl From<mpsc::error::SendError<SeedRollUpdate>> for RollError {
    fn from(_: mpsc::error::SendError<SeedRollUpdate>) -> Self {
        Self::ChannelClosed
    }
}

enum SeedRollUpdate {
    /// The seed rollers are busy and the seed has been queued.
    Queued(usize),
    /// A seed in front of us is done and we've moved to a new position in the queue.
    MovedForward(usize),
    /// We've cleared the queue but have to wait for the rate limit to expire.
    WaitRateLimit(Instant),
    /// We've cleared the queue and are now being rolled.
    Started,
    /// The seed has been rolled locally, includes the patch filename.
    DoneLocal(String, PathBuf),
    /// The seed has been rolled on ootrandomizer.com, includes the seed ID.
    DoneWeb(u64, [HashIcon; 5]),
    /// Seed rolling failed.
    Error(RollError),
}

impl SeedRollUpdate {
    async fn handle(self, ctx: &RaceContext, state: &Arc<RwLock<RaceState>>, settings: mw::S3Settings) -> Result<(), Error> {
        match self {
            Self::Queued(0) => ctx.send_message("I'm already rolling other seeds so your seed has been queued. It is at the front of the queue so it will be rolled next.").await?,
            Self::Queued(1) => ctx.send_message("I'm already rolling other seeds so your seed has been queued. There is 1 seed in front of it in the queue.").await?,
            Self::Queued(pos) => ctx.send_message(&format!("I'm already rolling other seeds so your seed has been queued. There are {pos} seeds in front of it in the queue.")).await?,
            Self::MovedForward(0) => ctx.send_message("The queue has moved and your seed is now at the front so it will be rolled next.").await?,
            Self::MovedForward(1) => ctx.send_message("The queue has moved and there is only 1 more seed in front of yours.").await?,
            Self::MovedForward(pos) => ctx.send_message(&format!("The queue has moved and there are now {pos} seeds in front of yours.")).await?,
            Self::WaitRateLimit(until) => ctx.send_message(&format!("Your seed will be rolled in {}.", format_duration(until - Instant::now(), true))).await?,
            Self::Started => ctx.send_message(&format!("Rolling a seed with {settings}…")).await?,
            Self::DoneLocal(patch_filename, spoiler_log_path) => {
                let spoiler_filename = spoiler_log_path.file_name().expect("spoiler log path with no file name").to_str().expect("non-UTF-8 spoiler filename").to_owned();
                let file_hash = serde_json::from_str::<SpoilerLog>(&fs::read_to_string(&spoiler_log_path).await?)?.file_hash;
                *state.write().await = RaceState::RolledLocally(spoiler_log_path);
                let seed_url = format!("https://midos.house/seed/{patch_filename}");
                ctx.send_message(&format!("@entrants Here is your seed: {seed_url}")).await?;
                ctx.send_message(&format!("After the race, you can view the spoiler log at https://midos.house/seed/{spoiler_filename}")).await?;
                ctx.set_bot_raceinfo(&format!("{}\n{seed_url}", format_hash(file_hash))).await?;
            }
            Self::DoneWeb(seed_id, file_hash) => {
                *state.write().await = RaceState::RolledWeb(seed_id);
                let seed_url = format!("https://ootrandomizer.com/seed/get?id={seed_id}");
                ctx.send_message(&format!("@entrants Here is your seed: {seed_url}")).await?;
                ctx.send_message("The spoiler log will be available on the seed page after the race.").await?;
                ctx.set_bot_raceinfo(&format!("{}\n{seed_url}", format_hash(file_hash))).await?;
            }
            Self::Error(msg) => {
                eprintln!("seed roll error: {msg:?}");
                ctx.send_message("Sorry @entrants, something went wrong while rolling the seed. Please report this error to Fenhl.").await?;
            }
        }
        Ok(())
    }
}

struct MwSeedQueue {
    http_client: reqwest::Client,
    ootr_api_key: String,
    next_request: Mutex<Instant>,
    next_seed: Mutex<Instant>,
    seed_rollers: Semaphore,
    waiting: Mutex<Vec<mpsc::UnboundedSender<()>>>,
}

impl MwSeedQueue {
    pub fn new(http_client: reqwest::Client, ootr_api_key: String) -> Self {
        Self {
            next_request: Mutex::new(Instant::now() + Duration::from_millis(500)),
            next_seed: Mutex::new(Instant::now() + MULTIWORLD_RATE_LIMIT),
            seed_rollers: Semaphore::new(2), // we're allowed to roll a maximum of 2 multiworld seeds at the same time
            waiting: Mutex::default(),
            http_client, ootr_api_key,
        }
    }

    async fn get(&self, uri: impl IntoUrl, query: Option<&(impl Serialize + ?Sized)>) -> reqwest::Result<reqwest::Response> {
        let mut next_request = self.next_request.lock().await;
        sleep_until(*next_request).await;
        let mut builder = self.http_client.get(uri);
        if let Some(query) = query {
            builder = builder.query(query);
        }
        let res = builder.send().await;
        *next_request = Instant::now() + Duration::from_millis(500);
        res
    }

    async fn post(&self, uri: impl IntoUrl, query: Option<&(impl Serialize + ?Sized)>, json: Option<&(impl Serialize + ?Sized)>) -> reqwest::Result<reqwest::Response> {
        let mut next_request = self.next_request.lock().await;
        sleep_until(*next_request).await;
        let mut builder = self.http_client.post(uri);
        if let Some(query) = query {
            builder = builder.query(query);
        }
        if let Some(json) = json {
            builder = builder.json(json);
        }
        let res = builder.send().await;
        *next_request = Instant::now() + Duration::from_millis(500);
        res
    }

    async fn get_version(&self, branch: &str) -> Result<Version, RollError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct VersionResponse {
            currently_active_version: Version,
        }

        Ok(self.get("https://ootrandomizer.com/api/version", Some(&[("key", &*self.ootr_api_key), ("branch", branch)])).await?
            .detailed_error_for_status().await?
            .json_with_text_in_error::<VersionResponse>().await?
            .currently_active_version)
    }

    async fn can_roll_on_web(&self, settings: &serde_json::Map<String, serde_json::Value>) -> Result<bool, RollError> {
        if settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64")) > 3 { return Ok(false) }
        // check if randomizer version is available on web
        if !KNOWN_GOOD_WEB_VERSIONS.contains(&RANDO_VERSION) {
            if let Ok(latest_web_version) = self.get_version("dev").await {
                if latest_web_version != RANDO_VERSION { // there is no endpoint for checking whether a given version is available on the website, so for now we assume that if the required version isn't the current one, it's not available
                    println!("web version mismatch: we need {RANDO_VERSION} but latest is {latest_web_version}");
                    return Ok(false)
                }
            } else {
                // the version API endpoint sometimes returns HTML instead of the expected JSON, fallback to generating locally when that happens
                return Ok(false)
            }
        }
        Ok(true)
    }

    async fn roll_seed_locally(&self, mut settings: serde_json::Map<String, serde_json::Value>) -> Result<(String, PathBuf), RollError> {
        settings.insert(format!("create_patch_file"), json!(true));
        settings.insert(format!("create_compressed_rom"), json!(false));
        for _ in 0..3 {
            #[cfg(unix)] let rando_path = BaseDirectories::new()?.find_data_file(Path::new("midos-house").join(format!("rando-dev-{RANDO_VERSION}"))).ok_or(RollError::RandoPath)?;
            #[cfg(windows)] let rando_path = UserDirs::new().ok_or(RollError::RandoPath)?.home_dir().join("git").join("github.com").join("TestRunnerSRL").join("OoT-Randomizer").join("tag").join(RANDO_VERSION.to_string());
            let mut rando_process = Command::new(PYTHON).arg("OoTRandomizer.py").arg("--no_log").arg("--settings=-").current_dir(rando_path).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
            rando_process.stdin.as_mut().expect("piped stdin missing").write_all(&serde_json::to_vec(&settings)?).await?;
            let output = rando_process.wait_with_output().await?;
            let stderr = if output.status.success() { output.stderr.lines().try_collect::<_, Vec<_>, _>()? } else { continue };
            let patch_path = Path::new(stderr.iter().rev().filter_map(|line| line.strip_prefix("Created patch file archive at: ")).next().ok_or(RollError::PatchPath)?);
            let spoiler_log_path = Path::new(stderr.iter().rev().filter_map(|line| line.strip_prefix("Created spoiler log at: ")).next().ok_or(RollError::SpoilerLogPath)?);
            let patch_filename = patch_path.file_name().expect("patch file path with no file name");
            fs::rename(patch_path, Path::new(seed::DIR).join(patch_filename)).await?;
            return Ok((
                patch_filename.to_str().expect("non-UTF-8 patch filename").to_owned(),
                spoiler_log_path.to_owned(),
            ))
        }
        Err(RollError::Retries)
    }

    async fn roll_seed_web(&self, update_tx: mpsc::Sender<SeedRollUpdate>, settings: serde_json::Map<String, serde_json::Value>) -> Result<(u64, [HashIcon; 5]), RollError> {
        #[serde_as]
        #[derive(Deserialize)]
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
            #[serde_as(as = "JsonString")]
            settings_log: SettingsLog,
        }

        for _ in 0..3 {
            let mut next_seed = {
                let next_seed = self.next_seed.lock().await;
                if let Some(duration) = next_seed.checked_duration_since(Instant::now()) {
                    update_tx.send(SeedRollUpdate::WaitRateLimit(*next_seed)).await?;
                    sleep(duration).await;
                }
                next_seed
            };
            update_tx.send(SeedRollUpdate::Started).await?;
            let seed_id = self.post("https://ootrandomizer.com/api/v2/seed/create", Some(&[("key", &*self.ootr_api_key), ("version", &*format!("dev_{RANDO_VERSION}")), ("locked", "1")]), Some(&settings)).await?
                .detailed_error_for_status().await?
                .json_with_text_in_error::<CreateSeedResponse>().await?
                .id;
            *next_seed = Instant::now() + MULTIWORLD_RATE_LIMIT;
            drop(next_seed);
            sleep(MULTIWORLD_RATE_LIMIT).await; // extra rate limiting rule
            loop {
                sleep(Duration::from_secs(1)).await;
                let resp = self.get(
                    "https://ootrandomizer.com/api/v2/seed/status",
                    Some(&[("key", &self.ootr_api_key), ("id", &seed_id.to_string())]),
                ).await?;
                if resp.status() == StatusCode::NO_CONTENT { continue }
                resp.error_for_status_ref()?;
                match resp.json_with_text_in_error::<SeedStatusResponse>().await?.status {
                    0 => continue, // still generating
                    1 => { // generated success
                        let file_hash = self.get("https://ootrandomizer.com/api/v2/seed/details", Some(&[("key", &self.ootr_api_key), ("id", &seed_id.to_string())])).await?
                            .detailed_error_for_status().await?
                            .json_with_text_in_error::<SeedDetailsResponse>().await?
                            .settings_log.file_hash;
                        let patch_response = self.get("https://ootrandomizer.com/api/v2/seed/patch", Some(&[("key", &self.ootr_api_key), ("id", &seed_id.to_string())])).await?
                            .detailed_error_for_status().await?;
                        let (_, patch_file_name) = regex_captures!("^attachment; filename=(.+)$", patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION).ok_or(RollError::PatchPath)?.to_str()?).ok_or(RollError::PatchPath)?;
                        let patch_file_name = patch_file_name.to_owned();
                        //let (_, patch_file_stem) = regex_captures!(r"^(.+)\.zpfz?$", patch_file_name).ok_or(RollError::PatchPath)?;
                        io::copy_buf(&mut StreamReader::new(patch_response.bytes_stream().map_err(io_error_from_reqwest)), &mut File::create(Path::new(seed::DIR).join(patch_file_name)).await?).await?;
                        return Ok((seed_id, file_hash))
                    }
                    2 => unreachable!(), // generated with link (not possible from API)
                    3 => break, // failed to generate
                    n => return Err(RollError::UnespectedSeedStatus(n)),
                }
            }
        }
        Err(RollError::Retries)
    }
}

async fn send_presets(ctx: &RaceContext) -> Result<(), Error> {
    ctx.send_message("!seed base: The settings used for the qualifier and tiebreaker asyncs.").await?;
    ctx.send_message("!seed random: Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.").await?;
    ctx.send_message("!seed draft: Pick the settings here in the chat. I don't enforce that the two teams have to be represented by different people.").await?;
    ctx.send_message("!seed (<setting> <value>)... (e.g. !seed trials 2 wincon scrubs): Pick a set of draftable settings without doing a full draft. Use “!settings” for a list of available settings.").await?;
    Ok(())
}

fn format_hash(file_hash: [HashIcon; 5]) -> impl fmt::Display {
    file_hash.into_iter().map(|icon| icon.to_racetime_emoji()).format(" ")
}

#[derive(Default)]
enum RaceState {
    #[default]
    Init,
    Draft(mw::S3Draft),
    Rolling,
    RolledLocally(PathBuf),
    RolledWeb(u64),
    SpoilerSent,
}

struct Handler {
    global_state: Arc<GlobalState>,
    official_race_start: Option<DateTime<Utc>>,
    high_seed_name: String,
    low_seed_name: String,
    fpa_enabled: bool,
    race_state: Arc<RwLock<RaceState>>,
}

impl Handler {
    fn is_official(&self) -> bool { self.official_race_start.is_some() }

    async fn send_settings(&self, ctx: &RaceContext) -> Result<(), Error> {
        let available_settings = {
            let state = self.race_state.read().await;
            if let RaceState::Draft(ref draft) = *state {
                draft.available_settings()
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

    async fn advance_draft(&self, ctx: &RaceContext) -> Result<(), Error> {
        let state = self.race_state.clone().write_owned().await;
        if let RaceState::Draft(ref draft) = *state {
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
                mw::DraftStep::Done(settings) => self.roll_seed(ctx, state, settings).await,
            }
        } else {
            unreachable!()
        }
        Ok(())
    }

    async fn roll_seed(&self, ctx: &RaceContext, mut state: OwnedRwLockWriteGuard<RaceState>, settings: mw::S3Settings) {
        *state = RaceState::Rolling;
        drop(state);
        let ctx = ctx.clone();
        let state = Arc::clone(&self.race_state);
        let mut updates = Arc::clone(&self.global_state).roll_seed(settings);
        let mut official_start = self.official_race_start;
        tokio::spawn(async move {
            let mut seed_state = None::<SeedRollUpdate>;
            loop {
                if let Some(start) = official_start {
                    select! {
                        () = sleep((start - chrono::Duration::minutes(15) - Utc::now()).to_std().expect("official race room opened after seed roll deadline")) => {
                            official_start = None;
                            if let Some(update) = seed_state.take() {
                                update.handle(&ctx, &state, settings).await?;
                            } else {
                                panic!("no seed rolling progress after 15 minutes")
                            }
                        }
                        Some(update) = updates.recv() => seed_state = Some(update),
                    }
                } else {
                    while let Some(update) = updates.recv().await {
                        update.handle(&ctx, &state, settings).await?;
                    }
                    return Ok::<_, Error>(())
                }
            }
        });
    }
}

#[async_trait]
impl RaceHandler<GlobalState> for Handler {
    fn should_handle(race_data: &RaceData) -> Result<bool, Error> {
        Ok(
            race_data.goal.name == "3rd Multiworld Tournament" //TODO don't hardcode (use a list shared with RandoBot?)
            && race_data.goal.custom
            && !matches!(race_data.status.value, RaceStatusValue::Finished | RaceStatusValue::Cancelled)
        )
    }

    async fn new(ctx: &RaceContext, global_state: Arc<GlobalState>) -> Result<Self, Error> {
        let new_room_lock = global_state.new_room_lock.lock().await; // make sure a new room isn't handled before it's added to the database
        let mut transaction = global_state.db_pool.begin().await.map_err(|e| Error::Custom(Box::new(e)))?;
        let (official_race_start, race_state, high_seed_name, low_seed_name) = if let Some(race) = Race::from_room(&mut transaction, &global_state.http_client, &global_state.startgg_token, format!("https://{}{}", global_state.host, ctx.data().await.url).parse()?).await.map_err(|e| Error::Custom(Box::new(e)))? {
            for team in race.active_teams() {
                let mut members = sqlx::query_scalar!(r#"SELECT racetime_id AS "racetime_id!" FROM users, team_members WHERE id = member AND team = $1 AND racetime_id IS NOT NULL"#, i64::from(team.id)).fetch(&mut transaction);
                while let Some(member) = members.try_next().await.map_err(|e| Error::Custom(Box::new(e)))? {
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        loop {
                            let data = ctx.data().await;
                            if let Some(entrant) = data.entrants.iter().find(|entrant| entrant.user.id == member) {
                                match entrant.status.value {
                                    EntrantStatusValue::Requested => ctx.accept_request(&member).await.expect("failed to accept race join request"),
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
                                drop(data);
                                match ctx.invite_user(&member).await {
                                    Ok(()) => {}
                                    Err(e) => if Utc::now() + chrono::Duration::minutes(11) < race.start {
                                        sleep(Duration::from_secs(60)).await;
                                        continue
                                    } else {
                                        eprintln!("failed to invite {member}: {e:?}");
                                        let _ = ctx.send_message("Repeatedly failed to invite one of the racers. Please contact a category moderator for assistance.").await;
                                    },
                                }
                            }
                            break
                        }
                    });
                }
            }
            ctx.send_message(&format!("Welcome to this {} {} race! Learn more about the tournament at https://midos.house/event/mw/3", race.phase, race.round)).await?; //TODO don't hardcode event name/URL
            ctx.send_message("Fair play agreement is active for this official race. Entrants may use the !fpa command during the race to notify of a crash. Race monitors should enable notifications using the bell 🔔 icon below chat.").await?; //TODO different message for monitorless FPA?
            let (high_seed_name, low_seed_name) = if let Some(Draft { ref state, high_seed }) = race.draft {
                if let mw::DraftStep::Done(settings) = state.next_step() {
                    ctx.send_message(&format!("Your seed with {settings} will be posted in 15 minutes.")).await?;
                }
                if race.team1.id == high_seed {
                    (race.team1.name.clone().unwrap_or_else(|| format!("Team A")), race.team2.name.clone().unwrap_or_else(|| format!("Team B")))
                } else {
                    (race.team2.name.clone().unwrap_or_else(|| format!("Team A")), race.team1.name.clone().unwrap_or_else(|| format!("Team B")))
                }
            } else {
                (format!("Team A"), format!("Team B"))
            };
            (
                Some(race.start),
                RaceState::Draft(race.draft.map(|draft| draft.state).unwrap_or_default()), //TODO restrict draft picks
                high_seed_name,
                low_seed_name,
            )
        } else {
            ctx.send_message("Welcome! This is a practice room for the 3rd Multiworld Tournament. Learn more about the tournament at https://midos.house/event/mw/3").await?; //TODO don't hardcode event name/URL
            ctx.send_message("You can roll a seed using “!seed base”, “!seed random”, or “!seed draft”. You can also choose settings directly (e.g. !seed trials 2 wincon scrubs). For more info about these options, use “!presets”").await?; //TODO different presets depending on event
            (
                None,
                RaceState::default(),
                format!("Team A"),
                format!("Team B"),
            )
        };
        transaction.commit().await.map_err(|e| Error::Custom(Box::new(e)))?;
        drop(new_room_lock);
        let this = Self {
            race_state: Arc::new(RwLock::new(race_state)),
            fpa_enabled: official_race_start.is_some(),
            global_state, official_race_start, high_seed_name, low_seed_name,
        };
        if official_race_start.is_some() {
            this.advance_draft(ctx).await?;
        }
        Ok(this)
    }

    async fn command(&mut self, ctx: &RaceContext, cmd_name: String, args: Vec<String>, _is_moderator: bool, _is_monitor: bool, msg: &ChatMessage) -> Result<(), Error> {
        let reply_to = msg.user.as_ref().map_or("friend", |user| &user.name);
        match &*cmd_name.to_ascii_lowercase() {
            "ban" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = self.race_state.write().await;
                match *state {
                    RaceState::Init => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                    RaceState::Draft(ref mut draft) => if draft.went_first.is_none() {
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
                    RaceState::Rolling | RaceState::RolledLocally(_) | RaceState::RolledWeb(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "draft" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = self.race_state.write().await;
                match *state {
                    RaceState::Init => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                    RaceState::Draft(ref mut draft) => if draft.went_first.is_none() {
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
                    RaceState::Rolling | RaceState::RolledLocally(_) | RaceState::RolledWeb(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "first" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = self.race_state.write().await;
                match *state {
                    RaceState::Init => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                    RaceState::Draft(ref mut draft) => if draft.went_first.is_some() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick has already been chosen.")).await?;
                    } else {
                        draft.went_first = Some(true);
                        drop(state);
                        self.advance_draft(ctx).await?;
                    },
                    RaceState::Rolling | RaceState::RolledLocally(_) | RaceState::RolledWeb(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "fpa" => match args[..] {
                [] => if self.fpa_enabled {
                    if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                        ctx.send_message("FPA cannot be invoked before the race starts.").await?;
                    } else {
                        //TODO tag race as “don't auto-post results”
                        //TODO different message for restreamed races
                        ctx.send_message(&format!("@everyone FPA has been invoked by {reply_to}. The team that did not call FPA can continue playing; the race will be retimed once completed.")).await?;
                    }
                } else {
                    ctx.send_message("Fair play agreement is not active. Race monitors may enable FPA for this race with !fpa on").await?;
                },
                [ref arg] => match &arg[..] {
                    "on" => if self.is_official() {
                        ctx.send_message("Fair play agreement is always active in official races.").await?;
                    } else if self.fpa_enabled {
                        ctx.send_message("Fair play agreement is already activated.").await?;
                    } else {
                        self.fpa_enabled = true;
                        ctx.send_message("Fair play agreement is now active. @entrants may use the !fpa command during the race to notify of a crash. Race monitors should enable notifications using the bell 🔔 icon below chat.").await?;
                    },
                    "off" => if self.is_official() {
                        ctx.send_message(&format!("Sorry {reply_to}, but FPA can't be deactivated for official races.")).await?;
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
            "presets" => send_presets(ctx).await?,
            "second" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = self.race_state.write().await;
                match *state {
                    RaceState::Init => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                    RaceState::Draft(ref mut draft) => if draft.went_first.is_some() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick has already been chosen.")).await?;
                    } else {
                        draft.went_first = Some(false);
                        drop(state);
                        self.advance_draft(ctx).await?;
                    },
                    RaceState::Rolling | RaceState::RolledLocally(_) | RaceState::RolledWeb(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "seed" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = self.race_state.clone().write_owned().await;
                match *state {
                    RaceState::Init => match args[..] {
                        [] => {
                            ctx.send_message(&format!("Sorry {reply_to}, the preset is required. Use one of the following:")).await?;
                            send_presets(ctx).await?;
                        }
                        [ref arg] if arg == "base" => self.roll_seed(ctx, state, mw::S3Settings::default()).await,
                        [ref arg] if arg == "random" => {
                            let settings = mw::S3Settings::random(&mut thread_rng());
                            self.roll_seed(ctx, state, settings).await;
                        }
                        [ref arg] if arg == "draft" => {
                            *state = RaceState::Draft(mw::S3Draft::default());
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
                            send_presets(ctx).await?;
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
                                self.roll_seed(ctx, state, settings).await;
                            }
                        }
                    },
                    RaceState::Draft(_) => ctx.send_message(&format!("Sorry {reply_to}, settings are already being drafted.")).await?,
                    RaceState::Rolling => ctx.send_message(&format!("Sorry {reply_to}, but I'm already rolling a seed for this room. Please wait.")).await?,
                    RaceState::RolledLocally(_) | RaceState::RolledWeb(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, but I already rolled a seed. Check the race info!")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "settings" => self.send_settings(ctx).await?,
            "skip" => if let RaceStatusValue::Open | RaceStatusValue::Invitational = ctx.data().await.status.value {
                let mut state = self.race_state.write().await;
                match *state {
                    RaceState::Init => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                    RaceState::Draft(ref mut draft) => if draft.went_first.is_none() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”")).await?;
                    } else if let 0 | 1 | 5 = draft.pick_count() {
                        draft.skipped_bans += 1;
                        drop(state);
                        self.advance_draft(ctx).await?;
                    } else {
                        ctx.send_message(&format!("Sorry {reply_to}, this part of the draft can't be skipped.")).await?;
                    },
                    RaceState::Rolling | RaceState::RolledLocally(_) | RaceState::RolledWeb(_) | RaceState::SpoilerSent => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race or the draft is already completed.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            _ => ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that command.")).await?, //TODO “did you mean”? list of available commands with !help?
        }
        Ok(())
    }

    async fn race_data(&mut self, ctx: &RaceContext, _old_race_data: RaceData) -> Result<(), Error> {
        if let RaceStatusValue::Finished = ctx.data().await.status.value {
            //TODO also make sure this isn't the first half of an async
            let mut state = self.race_state.write().await;
            match *state {
                RaceState::RolledLocally(ref spoiler_log_path) => {
                    let spoiler_filename = spoiler_log_path.file_name().expect("spoiler log path with no file name");
                    fs::rename(&spoiler_log_path, Path::new(seed::DIR).join(spoiler_filename)).await?;
                    *state = RaceState::SpoilerSent;
                }
                RaceState::RolledWeb(seed_id) => {
                    self.global_state.mw_seed_queue.post("https://ootrandomizer.com/api/v2/seed/unlock", Some(&[("key", &self.global_state.mw_seed_queue.ootr_api_key), ("id", &seed_id.to_string())]), None::<&()>).await?
                        .detailed_error_for_status().await.map_err(|e| Error::Custom(Box::new(e)))?;
                    //TODO also save spoiler log to local archive
                    *state = RaceState::SpoilerSent;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

async fn create_rooms(global_state: Arc<GlobalState>, discord_ctx: RwFuture<DiscordCtx>, env: Environment, config: Config, mut shutdown: rocket::Shutdown) -> Result<(), Error> {
    let racetime_config = if env.is_dev() { &config.racetime_bot_dev } else { &config.racetime_bot_production };
    loop {
        select! {
            () = &mut shutdown => break,
            _ = sleep(Duration::from_secs(60)) => {
                let mut transaction = global_state.db_pool.begin().await.map_err(|e| Error::Custom(Box::new(e)))?;
                for row in sqlx::query!(r#"SELECT series AS "series: Series", event, startgg_set, draft_state AS "draft_state: Json<Draft>", start AS "start!", end_time FROM races WHERE room IS NULL AND start IS NOT NULL AND start > NOW() AND start <= NOW() + TIME '00:30:00'"#).fetch_all(&mut transaction).await.map_err(|e| Error::Custom(Box::new(e)))? {
                    let race = Race::new(&mut transaction, &global_state.http_client, &global_state.startgg_token, row.startgg_set, row.draft_state.clone().map(|Json(draft)| draft), row.start, row.end_time, None, RaceKind::Normal).await.map_err(|e| Error::Custom(Box::new(e)))?;
                    match racetime::authorize_with_host(global_state.host, &racetime_config.client_id, &racetime_config.client_secret, &global_state.http_client).await {
                        Ok((access_token, _)) => {
                            let new_room_lock = global_state.new_room_lock.lock().await; // make sure a new room isn't handled before it's added to the database
                            let race_slug = racetime::StartRace {
                                goal: format!("3rd Multiworld Tournament"), //TODO don't hardcode
                                goal_is_custom: true,
                                team_race: true,
                                invitational: true,
                                unlisted: false,
                                info_user: format!("{} {}: {} vs {}", race.phase, race.round, race.team1, race.team2),
                                info_bot: String::default(),
                                require_even_teams: true,
                                start_delay: 15,
                                time_limit: 24,
                                time_limit_auto_complete: false,
                                streaming_required: None,
                                auto_start: true, //TODO no autostart if restreamed
                                allow_comments: true,
                                hide_comments: true,
                                allow_prerace_chat: true,
                                allow_midrace_chat: true,
                                allow_non_entrant_chat: true,
                                chat_message_delay: 0,
                            }.start_with_host(global_state.host, &access_token, &global_state.http_client, CATEGORY).await?;
                            let room_url = Url::parse(&format!("https://{}/{CATEGORY}/{race_slug}", global_state.host))?;
                            sqlx::query!("UPDATE races SET room = $1 WHERE startgg_set = $2", room_url.to_string(), race.startgg_set).execute(&mut transaction).await.map_err(|e| Error::Custom(Box::new(e)))?;
                            transaction.commit().await.map_err(|e| Error::Custom(Box::new(e)))?;
                            drop(new_room_lock);
                            transaction = global_state.db_pool.begin().await.map_err(|e| Error::Custom(Box::new(e)))?;
                            if let Some(race) = Race::from_room(&mut transaction, &global_state.http_client, &global_state.startgg_token, room_url.clone()).await.map_err(|e| Error::Custom(Box::new(e)))? {
                                if let Some(event) = event::Data::new(&mut transaction, row.series, row.event).await.map_err(|e| Error::Custom(Box::new(e)))? {
                                    if let (Some(guild), Some(channel)) = (event.discord_guild, event.discord_race_room_channel) {
                                        channel.say(&*discord_ctx.read().await, MessageBuilder::default()
                                            .push_safe(race.phase)
                                            .push(' ')
                                            .push_safe(race.round)
                                            .push(": ")
                                            .mention_team(&mut transaction, guild, &race.team1).await.map_err(|e| Error::Custom(Box::new(e)))?
                                            .push(" vs ")
                                            .mention_team(&mut transaction, guild, &race.team2).await.map_err(|e| Error::Custom(Box::new(e)))?
                                            .push(' ')
                                            .push(room_url)
                                        ).await.map_err(|e| Error::Custom(Box::new(e)))?;
                                    }
                                }
                            }
                        }
                        Err(Error::Reqwest(e)) if e.status().map_or(false, |status| status.is_server_error()) => {
                            // racetime.gg's auth endpoint has been known to return server errors intermittently.
                            // In that case, we simply try again in the next iteration of the sleep loop.
                        }
                        Err(e) => return Err(e),
                    }
                }
                transaction.commit().await.map_err(|e| Error::Custom(Box::new(e)))?;
            }
        }
    }
    Ok(())
}

async fn handle_rooms(global_state: Arc<GlobalState>, env: Environment, config: Config, shutdown: rocket::Shutdown) -> Result<(), Error> {
    let racetime_config = if env.is_dev() { &config.racetime_bot_dev } else { &config.racetime_bot_production };
    let mut last_crash = Instant::now();
    let mut wait_time = Duration::from_secs(1);
    loop {
        match racetime::Bot::new_with_host(env.racetime_host(), CATEGORY, &racetime_config.client_id, &racetime_config.client_secret, global_state.clone()).await {
            Ok(bot) => {
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

pub(crate) async fn main(db_pool: PgPool, http_client: reqwest::Client, discord_ctx: RwFuture<DiscordCtx>, ootr_api_key: String, env: Environment, config: Config, shutdown: rocket::Shutdown) -> Result<(), Error> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let global_state = Arc::new(GlobalState::new(db_pool.clone(), http_client.clone(), ootr_api_key.clone(), startgg_token.to_owned(), env.racetime_host()));
    let ((), ()) = tokio::try_join!(
        create_rooms(global_state.clone(), discord_ctx, env, config.clone(), shutdown.clone()),
        handle_rooms(global_state, env, config, shutdown),
    )?;
    Ok(())
}
