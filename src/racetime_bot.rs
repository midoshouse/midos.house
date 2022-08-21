use {
    std::{
        io::prelude::*,
        path::Path,
        process::Stdio,
        sync::Arc,
        time::Duration,
    },
    async_trait::async_trait,
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
        RequestBuilder,
        StatusCode,
    },
    semver::Version,
    serde::Deserialize,
    serde_json::{
        Value as Json,
        json,
    },
    serde_with::{
        DisplayFromStr,
        json::JsonString,
        serde_as,
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
        sync::{
            Mutex,
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
    crate::{
        config::ConfigRaceTime,
        event::mw,
        seed::{
            self,
            SpoilerLog,
        },
        util::{
            format_duration,
            io_error_from_reqwest,
        },
    },
};
#[cfg(unix)] use xdg::BaseDirectories;
#[cfg(windows)] use directories::UserDirs;

#[cfg(unix)] const PYTHON: &str = "python3";
#[cfg(windows)] const PYTHON: &str = "py";

const RANDO_VERSION: Version = Version::new(6, 2, 181);
/// Randomizer versions that are known to exist on the ootrandomizer.com API. Hardcoded because the API doesn't have a “does version x exist?” endpoint.
const KNOWN_GOOD_WEB_VERSIONS: [Version; 1] = [Version::new(6, 2, 181)];

const MULTIWORLD_RATE_LIMIT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
enum RollError {
    #[error(transparent)] Header(#[from] reqwest::header::ToStrError),
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error(transparent)] Json(#[from] serde_json::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
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
    DoneLocal(String),
    /// The seed has been rolled on ootrandomizer.com, includes the seed ID.
    DoneWeb(u64),
    /// Seed rolling failed.
    Error(RollError),
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

    async fn get(&self, uri: impl IntoUrl) -> RequestBuilder {
        let mut next_request = self.next_request.lock().await;
        sleep_until(*next_request).await;
        *next_request = Instant::now() + Duration::from_millis(500);
        self.http_client.get(uri)
    }

    async fn post(&self, uri: impl IntoUrl) -> RequestBuilder {
        let mut next_request = self.next_request.lock().await;
        sleep_until(*next_request).await;
        *next_request = Instant::now() + Duration::from_millis(500);
        self.http_client.post(uri)
    }

    async fn get_version(&self, branch: &str) -> Result<Version, RollError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct VersionResponse {
            currently_active_version: Version,
        }

        //TODO rate limiting
        Ok(self.get("https://ootrandomizer.com/api/version").await
            .query(&[("key", &*self.ootr_api_key), ("branch", branch)])
            .send().await?
            .error_for_status()?
            .json::<VersionResponse>().await?
            .currently_active_version)
    }

    async fn can_roll_on_web(&self, settings: &serde_json::Map<String, Json>) -> Result<bool, RollError> {
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

    async fn roll_seed_locally(&self, mut settings: serde_json::Map<String, Json>) -> Result<String, RollError> {
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
            let patch_filename = patch_path.file_name().expect("spoiler log path with no file name");
            fs::rename(patch_path, Path::new(seed::DIR).join(patch_filename)).await?;
            let _ = spoiler_log_path; //TODO handle log unlocking after race
            return Ok(patch_filename.to_str().expect("non-UTF-8 patch filename").to_owned())
        }
        Err(RollError::Retries)
    }

    async fn roll_seed_web(&self, update_tx: mpsc::Sender<SeedRollUpdate>, settings: serde_json::Map<String, Json>) -> Result<u64, RollError> {
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

        #[serde_as]
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SeedDetailsResponse {
            #[serde_as(as = "Option<JsonString>")]
            spoiler_log: Option<SpoilerLog>, //TODO report missing spoiler log bug
        }

        for _ in 0..3 {
            let mut next_seed = {
                let next_seed = self.next_seed.lock().await;
                let sleep = sleep_until(*next_seed);
                if !sleep.is_elapsed() {
                    update_tx.send(SeedRollUpdate::WaitRateLimit(*next_seed)).await?;
                }
                sleep.await;
                next_seed
            };
            update_tx.send(SeedRollUpdate::Started).await?;
            let seed_id = self.post("https://ootrandomizer.com/api/v2/seed/create").await
                .query(&[("key", &*self.ootr_api_key), ("version", &*format!("dev_{RANDO_VERSION}")), ("locked", "1")])
                .json(&settings)
                .send().await?
                .error_for_status()?
                .json::<CreateSeedResponse>().await?
                .id;
            *next_seed = Instant::now() + MULTIWORLD_RATE_LIMIT;
            drop(next_seed);
            sleep(MULTIWORLD_RATE_LIMIT).await; // extra rate limiting rule
            loop {
                sleep(Duration::from_secs(1)).await;
                let resp = self.get("https://ootrandomizer.com/api/v2/seed/status").await
                    .query(&[("key", &self.ootr_api_key), ("id", &seed_id.to_string())])
                    .send().await?;
                if resp.status() == StatusCode::NO_CONTENT { continue }
                resp.error_for_status_ref()?;
                match resp.json::<SeedStatusResponse>().await?.status {
                    0 => continue, // still generating
                    1 => { // generated success
                        let _ /*file_hash*/ = self.get("https://ootrandomizer.com/api/v2/seed/details").await
                            .query(&[("key", &self.ootr_api_key), ("id", &seed_id.to_string())])
                            .send().await?
                            .error_for_status()?
                            .json::<SeedDetailsResponse>().await?
                            .spoiler_log.map(|spoiler_log| spoiler_log.file_hash);
                        let patch_response = self.get("https://ootrandomizer.com/api/v2/seed/patch").await
                            .query(&[("key", &self.ootr_api_key), ("id", &seed_id.to_string())])
                            .send().await?
                            .error_for_status()?;
                        let (_, patch_file_name) = regex_captures!("^attachment; filename=(.+)$", patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION).ok_or(RollError::PatchPath)?.to_str()?).ok_or(RollError::PatchPath)?;
                        let patch_file_name = patch_file_name.to_owned();
                        //let (_, patch_file_stem) = regex_captures!(r"^(.+)\.zpfz?$", patch_file_name).ok_or(RollError::PatchPath)?;
                        io::copy_buf(&mut StreamReader::new(patch_response.bytes_stream().map_err(io_error_from_reqwest)), &mut File::create(Path::new(seed::DIR).join(patch_file_name)).await?).await?;
                        //TODO also save spoiler log (to temp dir? Or set it to locked in the /seed handler?)
                        return Ok(seed_id)
                    }
                    2 => unreachable!(), // generated with link (not possible from API)
                    3 => break, // failed to generate
                    n => return Err(RollError::UnespectedSeedStatus(n)),
                }
            }
        }
        Err(RollError::Retries)
    }

    fn roll_seed(self: Arc<Self>, settings: mw::S3Settings) -> mpsc::Receiver<SeedRollUpdate> {
        let settings = settings.resolve();
        let (update_tx, update_rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let permit = match self.seed_rollers.try_acquire() {
                Ok(permit) => permit,
                Err(TryAcquireError::Closed) => unreachable!(),
                Err(TryAcquireError::NoPermits) => {
                    let (mut pos, mut pos_rx) = {
                        let mut waiting = self.waiting.lock().await;
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
                    let mut waiting = self.waiting.lock().await;
                    let permit = self.seed_rollers.acquire().await.expect("seed queue semaphore closed");
                    waiting.remove(0);
                    for tx in &*waiting {
                        let _ = tx.send(());
                    }
                    permit
                }
            };
            let can_roll_on_web = match self.can_roll_on_web(&settings).await {
                Ok(can_roll_on_web) => can_roll_on_web,
                Err(e) => {
                    update_tx.send(SeedRollUpdate::Error(e)).await?;
                    return Ok(())
                }
            };
            if can_roll_on_web {
                match self.roll_seed_web(update_tx.clone(), settings).await {
                    Ok(seed_id) => update_tx.send(SeedRollUpdate::DoneWeb(seed_id)).await?,
                    Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?,
                }
                drop(permit);
            } else {
                drop(permit); //TODO skip queue entirely?
                update_tx.send(SeedRollUpdate::Started).await?;
                match self.roll_seed_locally(settings).await {
                    Ok(patch_filename) => update_tx.send(SeedRollUpdate::DoneLocal(patch_filename)).await?,
                    Err(e) => update_tx.send(SeedRollUpdate::Error(e)).await?,
                }
            }
            Ok::<_, mpsc::error::SendError<_>>(())
        });
        update_rx
    }
}

async fn send_presets(ctx: &RaceContext) -> Result<(), Error> {
    ctx.send_message("!seed base: The settings used for the qualifier and tiebreaker asyncs.").await?;
    ctx.send_message("!seed random: Simulate a settings draft with both teams picking randomly. The settings are posted along with the seed.").await?;
    ctx.send_message("!seed draft: Pick the settings here in the chat. I don't enforce that the two teams have to be represented by different people, so you can also use this to decide on settings ahead of time.").await?;
    Ok(())
}

#[derive(Default)]
enum RaceState {
    #[default]
    Init,
    Draft(mw::S3Draft),
    Rolling,
    Rolled,
}

struct Handler {
    state: Arc<RwLock<RaceState>>,
    seed_queue: Arc<MwSeedQueue>,
}

impl Handler {
    async fn send_settings(&self, ctx: &RaceContext) -> Result<(), Error> {
        let state = self.state.read().await;
        if let RaceState::Draft(ref draft) = *state {
            for setting in draft.available_settings() {
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
        } else {
            unreachable!()
        }
        Ok(())
    }

    async fn advance_draft(&mut self, ctx: &RaceContext) -> Result<(), Error> {
        let state = self.state.read().await;
        if let RaceState::Draft(ref draft) = *state {
            match draft.next_step() {
                mw::DraftStep::GoFirst => ctx.send_message("Team A, you have the higher seed. Choose whether you want to go !first or !second").await?,
                mw::DraftStep::Ban { prev_bans, team } => ctx.send_message(&format!("{team}, lock a setting to its default using “!ban <setting>”, or use “!skip” if you don't want to ban anything.{}", if prev_bans == 0 { " Use “!settings” for a list of available settings." } else { "" })).await?,
                mw::DraftStep::Pick { prev_picks, team } => ctx.send_message(&match prev_picks {
                    0 => format!("{team}, pick a setting using “!draft <setting> <value>”"),
                    1 => format!("{team}, pick two settings."),
                    2 => format!("And your second pick?"),
                    3 => format!("{team}, pick the final setting. You can also use “!skip” if you want to leave the settings as they are."),
                    _ => unreachable!(),
                }).await?,
                mw::DraftStep::Done(settings) => {
                    drop(state); //TODO retain lock
                    self.roll_seed(ctx, settings).await;
                }
            }
        } else {
            unreachable!()
        }
        Ok(())
    }

    async fn roll_seed(&mut self, ctx: &RaceContext, settings: mw::S3Settings) {
        *self.state.write().await = RaceState::Rolling;
        let ctx = ctx.clone();
        let state = Arc::clone(&self.state);
        let mut updates = Arc::clone(&self.seed_queue).roll_seed(settings);
        tokio::spawn(async move {
            while let Some(update) = updates.recv().await {
                match update {
                    SeedRollUpdate::Queued(0) => ctx.send_message("I'm already rolling other seeds so your seed has been queued. It is at the front of the queue so it will be rolled next.").await?,
                    SeedRollUpdate::Queued(1) => ctx.send_message("I'm already rolling other seeds so your seed has been queued. There is 1 seed in front of it in the queue.").await?,
                    SeedRollUpdate::Queued(pos) => ctx.send_message(&format!("I'm already rolling other seeds so your seed has been queued. There are {pos} seeds in front of it in the queue.")).await?,
                    SeedRollUpdate::MovedForward(0) => ctx.send_message("The queue has moved and your seed is now at the front so it will be rolled next.").await?,
                    SeedRollUpdate::MovedForward(1) => ctx.send_message("The queue has moved and there is only 1 more seed in front of yours.").await?,
                    SeedRollUpdate::MovedForward(pos) => ctx.send_message(&format!("The queue has moved and there are now {pos} seeds in front of yours.")).await?,
                    SeedRollUpdate::WaitRateLimit(until) => ctx.send_message(&format!("Your seed will be rolled in {}.", format_duration(until - Instant::now()))).await?,
                    SeedRollUpdate::Started => ctx.send_message(&format!("Rolling a seed with {settings}…")).await?,
                    SeedRollUpdate::DoneLocal(patch_filename) => {
                        *state.write().await = RaceState::Rolled;
                        ctx.send_message(&format!("@entrants Here is your seed: https://midos.house/seed/{patch_filename}")).await?;
                        //ctx.send_message(&format!("After the race, you can view the spoiler log at https://midos.house/seed/{}_Spoiler.json", patch_filename.split_once('.').expect("patch filename with no suffix").0)).await?; //TODO add spoiler log unlocking feature
                        //TODO update raceinfo
                    }
                    SeedRollUpdate::DoneWeb(seed_id) => {
                        *state.write().await = RaceState::Rolled;
                        ctx.send_message(&format!("@entrants Here is your seed: https://ootrandomizer.com/seed/get?id={seed_id}")).await?;
                        //ctx.send_message("The spoiler log will be available on the seed page after the race.").await?; //TODO add spoiler log unlocking feature
                        //TODO update raceinfo
                    }
                    SeedRollUpdate::Error(msg) => {
                        eprintln!("seed roll error: {msg:?}");
                        ctx.send_message("Sorry @entrants, something went wrong while rolling the seed. Please report this error to Fenhl.").await?;
                    }
                }
            }
            Ok::<_, Error>(())
        });
    }
}

#[async_trait]
impl RaceHandler<MwSeedQueue> for Handler {
    fn should_handle(race_data: &RaceData) -> Result<bool, Error> {
        Ok(
            race_data.goal.name == "3rd Multiworld Tournament" //TODO don't hardcode (use a list shared with RandoBot?)
            && race_data.goal.custom
            && !matches!(race_data.status.value, RaceStatusValue::Finished | RaceStatusValue::Cancelled)
        )
    }

    async fn new(ctx: &RaceContext, seed_queue: Arc<MwSeedQueue>) -> Result<Self, Error> {
        //TODO different behavior for race rooms opened by the bot itself
        ctx.send_message("Welcome! This is a practice room for the 3rd Multiworld Tournament.").await?;
        ctx.send_message("You can roll a seed using “!seed base”, “!seed random”, or “!seed draft”. For more info about these options, use “!presets”").await?;
        ctx.send_message("Learn more about the tournament at https://midos.house/event/mw/3").await?;
        Ok(Self { state: Arc::default(), seed_queue })
    }

    async fn command(&mut self, ctx: &RaceContext, cmd_name: &str, args: Vec<&str>, _is_moderator: bool, _is_monitor: bool, msg: &ChatMessage) -> Result<(), Error> {
        let reply_to = msg.user.as_ref().map_or("friend", |user| &user.name);
        match cmd_name {
            "ban" => if matches!(ctx.data().await.status.value, RaceStatusValue::Open | RaceStatusValue::Invitational) {
                let mut state = self.state.write().await;
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
                            [setting] => {
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
                    RaceState::Rolling | RaceState::Rolled => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "draft" => if matches!(ctx.data().await.status.value, RaceStatusValue::Open | RaceStatusValue::Invitational) {
                let mut state = self.state.write().await;
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
                            [setting] => {
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
                            [setting, value] => {
                                if let Ok(setting) = setting.parse() {
                                    if draft.available_settings().contains(&setting) {
                                        match setting {
                                            mw::S3Setting::Wincon => if let Some(value) = all::<mw::Wincon>().find(|option| option.arg() == value) { draft.wincon = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Wincon>().map(|option| format!("“!draft wincon {}”", option.arg())).join(" or "),)).await? },
                                            mw::S3Setting::Dungeons => if let Some(value) = all::<mw::Dungeons>().find(|option| option.arg() == value) { draft.dungeons = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Dungeons>().map(|option| format!("“!draft dungeons {}”", option.arg())).join(" or "),)).await? },
                                            mw::S3Setting::Er => if let Some(value) = all::<mw::Er>().find(|option| option.arg() == value) { draft.er = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Er>().map(|option| format!("“!draft er {}”", option.arg())).join(" or "),)).await? },
                                            mw::S3Setting::Trials => if let Some(value) = all::<mw::Trials>().find(|option| option.arg() == value) { draft.trials = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Trials>().map(|option| format!("“!draft trials {}”", option.arg())).join(" or "),)).await? },
                                            mw::S3Setting::Shops => if let Some(value) = all::<mw::Shops>().find(|option| option.arg() == value) { draft.shops = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Shops>().map(|option| format!("“!draft shops {}”", option.arg())).join(" or "),)).await? },
                                            mw::S3Setting::Scrubs => if let Some(value) = all::<mw::Scrubs>().find(|option| option.arg() == value) { draft.scrubs = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Scrubs>().map(|option| format!("“!draft scrubs {}”", option.arg())).join(" or "),)).await? },
                                            mw::S3Setting::Fountain => if let Some(value) = all::<mw::Fountain>().find(|option| option.arg() == value) { draft.fountain = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Fountain>().map(|option| format!("“!draft fountain {}”", option.arg())).join(" or "),)).await? },
                                            mw::S3Setting::Spawn => if let Some(value) = all::<mw::Spawn>().find(|option| option.arg() == value) { draft.spawn = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<mw::Spawn>().map(|option| format!("“!draft spawn {}”", option.arg())).join(" or "),)).await? },
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
                    RaceState::Rolling | RaceState::Rolled => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "first" => if matches!(ctx.data().await.status.value, RaceStatusValue::Open | RaceStatusValue::Invitational) {
                let mut state = self.state.write().await;
                match *state {
                    RaceState::Init => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                    RaceState::Draft(ref mut draft) => if draft.went_first.is_some() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick has already been chosen.")).await?;
                    } else {
                        draft.went_first = Some(true);
                        drop(state);
                        self.advance_draft(ctx).await?;
                    },
                    RaceState::Rolling | RaceState::Rolled => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "presets" => send_presets(ctx).await?,
            "second" => if matches!(ctx.data().await.status.value, RaceStatusValue::Open | RaceStatusValue::Invitational) {
                let mut state = self.state.write().await;
                match *state {
                    RaceState::Init => ctx.send_message(&format!("Sorry {reply_to}, no draft has been started. Use “!seed draft” to start one.")).await?,
                    RaceState::Draft(ref mut draft) => if draft.went_first.is_some() {
                        ctx.send_message(&format!("Sorry {reply_to}, first pick has already been chosen.")).await?;
                    } else {
                        draft.went_first = Some(false);
                        drop(state);
                        self.advance_draft(ctx).await?;
                    },
                    RaceState::Rolling | RaceState::Rolled => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "seed" => if matches!(ctx.data().await.status.value, RaceStatusValue::Open | RaceStatusValue::Invitational) {
                let mut state = self.state.write().await;
                match *state {
                    RaceState::Init => match args[..] {
                        [] => {
                            ctx.send_message(&format!("Sorry {reply_to}, the preset is required. Use one of the following:")).await?;
                            send_presets(ctx).await?;
                        }
                        ["base"] => {
                            drop(state);
                            self.roll_seed(ctx, mw::S3Settings::default()).await;
                        }
                        ["random"] => {
                            drop(state);
                            let settings = mw::S3Settings::random(&mut thread_rng());
                            self.roll_seed(ctx, settings).await;
                        }
                        ["draft"] => {
                            *state = RaceState::Draft(mw::S3Draft::default());
                            drop(state);
                            self.advance_draft(ctx).await?;
                        }
                        [_] => {
                            ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that preset. Use one of the following:")).await?;
                            send_presets(ctx).await?;
                        }
                        [_, _, ..] => {
                            ctx.send_message(&format!("Sorry {reply_to}, I didn't quite understand that. Use one of the following:")).await?;
                            send_presets(ctx).await?;
                        }
                    },
                    RaceState::Draft(_) => ctx.send_message(&format!("Sorry {reply_to}, settings are already being drafted.")).await?,
                    RaceState::Rolling => ctx.send_message(&format!("Sorry {reply_to}, but I'm already rolling a seed for this room. Please wait.")).await?,
                    RaceState::Rolled => ctx.send_message(&format!("Sorry {reply_to}, but I already rolled a seed.")).await?, //TODO “Check the race info!”
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            "settings" => self.send_settings(ctx).await?,
            "skip" => if matches!(ctx.data().await.status.value, RaceStatusValue::Open | RaceStatusValue::Invitational) {
                let mut state = self.state.write().await;
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
                    RaceState::Rolling | RaceState::Rolled => ctx.send_message(&format!("Sorry {reply_to}, there is no settings draft this race.")).await?,
                }
            } else {
                ctx.send_message(&format!("Sorry {reply_to}, but the race has already started.")).await?;
            },
            _ => ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that command.")).await?, //TODO “did you mean”? list of available commands with !help?
        }
        Ok(())
    }
}

pub(crate) async fn main(http_client: reqwest::Client, ootr_api_key: String, host: &str, config: ConfigRaceTime, shutdown: rocket::Shutdown) -> Result<(), Error> {
    let bot = racetime::Bot::new_with_host(host, "ootr", &config.client_id, &config.client_secret, Arc::new(MwSeedQueue::new(http_client, ootr_api_key))).await?; //TODO automatically retry on server error
    let () = bot.run_until::<Handler, _, _>(shutdown).await?;
    Ok(())
}
