use {
    std::{
        fmt,
        io::prelude::*,
        path::Path,
        process::Stdio,
        sync::Arc,
        time::Duration,
    },
    async_trait::async_trait,
    collect_mac::collect,
    enum_iterator::{
        Sequence,
        all,
    },
    itertools::Itertools as _,
    racetime::{
        Error,
        handler::{
            RaceContext,
            RaceHandler,
        },
        model::*,
    },
    rand::prelude::*,
    semver::Version,
    serde::Deserialize,
    serde_json::{
        Value as Json,
        json,
    },
    serde_plain::derive_fromstr_from_deserialize,
    tokio::{
        fs,
        io::AsyncWriteExt as _,
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
            sleep_until,
        },
    },
    crate::{
        config::ConfigRaceTime,
        seed,
        util::{
            format_duration,
            natjoin_str,
        },
    },
};
#[cfg(unix)] use xdg::BaseDirectories;
#[cfg(windows)] use directories::UserDirs;

#[cfg(unix)] const PYTHON: &str = "python3";
#[cfg(windows)] const PYTHON: &str = "py";

const RANDO_VERSION: Version = Version::new(6, 2, 158); //TODO decide on an official version for the tournament

#[derive(Debug, thiserror::Error)]
enum RollError {
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
    DoneWeb(u32),
    /// Seed rolling failed.
    Error(RollError),
}

struct MwSeedQueue {
    http_client: reqwest::Client,
    ootr_api_key: String,
    next_seed: Mutex<Instant>,
    seed_rollers: Semaphore,
    waiting: Mutex<Vec<mpsc::UnboundedSender<()>>>,
}

impl MwSeedQueue {
    pub fn new(http_client: reqwest::Client, ootr_api_key: String) -> Self {
        Self {
            next_seed: Mutex::new(Instant::now() + Duration::from_secs(5 * 60)), // we have to wait 5 minutes between starting each seed
            seed_rollers: Semaphore::new(2), // we're allowed to roll a maximum of 2 multiworld seeds at the same time
            waiting: Mutex::default(),
            http_client, ootr_api_key,
        }
    }

    async fn get_version(&self, branch: &str) -> Result<Version, RollError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct VersionResponse {
            currently_active_version: Version,
        }

        //TODO rate limiting
        Ok(self.http_client.get("https://ootrandomizer.com/api/version")
            .query(&[("key", &*self.ootr_api_key), ("branch", branch)])
            .send().await?
            .error_for_status()?
            .json::<VersionResponse>().await?
            .currently_active_version)
    }

    async fn can_roll_on_web(&self, settings: &serde_json::Map<String, Json>) -> Result<bool, RollError> {
        if settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64")) != 1 { return Ok(false) } //TODO remove once the ootrandomizer.com API starts supporting multiworld seeds
        // check if randomizer version is available on web
        if let Ok(latest_web_version) = self.get_version("dev").await {
            if latest_web_version != RANDO_VERSION { // there is no endpoint for checking whether a given version is available on the website, so for now we assume that if the required version isn't the current one, it's not available
                println!("web version mismatch: we need {RANDO_VERSION} but latest is {latest_web_version}");
                return Ok(false)
            }
        } else {
            // the version API endpoint sometimes returns HTML instead of the expected JSON, fallback to generating locally when that happens
            return Ok(false)
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

    async fn roll_seed_web(&self, update_tx: mpsc::Sender<SeedRollUpdate>, _ /*settings*/: serde_json::Map<String, Json>) -> Result<u32, RollError> {
        for _ in 0..3 {
            let _ /*next_seed*/ = {
                let next_seed = self.next_seed.lock().await;
                let sleep = sleep_until(*next_seed);
                if !sleep.is_elapsed() {
                    update_tx.send(SeedRollUpdate::WaitRateLimit(*next_seed)).await?;
                }
                sleep.await;
                next_seed
            };
            update_tx.send(SeedRollUpdate::Started).await?;
            unimplemented!() //TODO roll the seed via the ootrandomizer.com API
            //TODO update and drop next_seed after receiving the response for the initial seed roll API request
        }
        Err(RollError::Retries)
    }

    fn roll_seed(self: Arc<Self>, settings: Mw3Settings) -> mpsc::Receiver<SeedRollUpdate> {
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

#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Wincon { #[default] Meds, Scrubs, Th }
#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Dungeons { #[default] Tournament, Skulls, Keyrings }
#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Er { #[default] Off, Dungeon }
#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Trials { #[default] Zero, Two }
#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Shops { #[default] Four, Off }
#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Scrubs { #[default] Affordable, Off }
#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Fountain { #[default] Closed, Open }
#[derive(Default, Clone, Copy, PartialEq, Eq, Sequence)] enum Spawn { #[default] Tot, Random }

impl Wincon { fn arg(&self) -> &'static str { match self { Self::Meds => "meds", Self::Scrubs => "scrubs", Self::Th => "th" } } }
impl Dungeons { fn arg(&self) -> &'static str { match self { Self::Tournament => "tournament", Self::Skulls => "skulls", Self::Keyrings => "keyrings" } } }
impl Er { fn arg(&self) -> &'static str { match self { Self::Off => "off", Self::Dungeon => "dungeon" } } }
impl Trials { fn arg(&self) -> &'static str { match self { Self::Zero => "0", Self::Two => "2" } } }
impl Shops { fn arg(&self) -> &'static str { match self { Self::Four => "4", Self::Off => "off" } } }
impl Scrubs { fn arg(&self) -> &'static str { match self { Self::Affordable => "affordable", Self::Off => "off" } } }
impl Fountain { fn arg(&self) -> &'static str { match self { Self::Closed => "closed", Self::Open => "open" } } }
impl Spawn { fn arg(&self) -> &'static str { match self { Self::Tot => "tot", Self::Random => "random" } } }

impl fmt::Display for Wincon { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Meds => write!(f, "default wincons"), Self::Scrubs => write!(f, "Scrubs wincons"), Self::Th => write!(f, "Triforce Hunt") } } }
impl fmt::Display for Dungeons { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Tournament => write!(f, "tournament dungeons"), Self::Skulls => write!(f, "dungeon tokens"), Self::Keyrings => write!(f, "keyrings") } } }
impl fmt::Display for Er { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Off => write!(f, "no ER"), Self::Dungeon => write!(f, "dungeon ER") } } }
impl fmt::Display for Trials { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Zero => write!(f, "0 trials"), Self::Two => write!(f, "2 trials") } } }
impl fmt::Display for Shops { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Four => write!(f, "shops 4"), Self::Off => write!(f, "no shops") } } }
impl fmt::Display for Scrubs { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Affordable => write!(f, "affordable scrubs"), Self::Off => write!(f, "no scrubs") } } }
impl fmt::Display for Fountain { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Closed => write!(f, "closed fountain"), Self::Open => write!(f, "open fountain") } } }
impl fmt::Display for Spawn { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Tot => write!(f, "ToT spawns"), Self::Random => write!(f, "random spawns & starting age") } } }

enum Team {
    HighSeed,
    LowSeed,
}

impl fmt::Display for Team {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HighSeed => write!(f, "Team A"),
            Self::LowSeed => write!(f, "Team B"),
        }
    }
}

enum DraftStep {
    GoFirst,
    Ban {
        prev_bans: u8,
        team: Team,
    },
    Pick {
        prev_picks: u8,
        team: Team,
    },
    Done(Mw3Settings),
}

#[derive(PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mw3Setting {
    Wincon,
    Dungeons,
    Er,
    Trials,
    Shops,
    Scrubs,
    Fountain,
    Spawn,
}

derive_fromstr_from_deserialize!(Mw3Setting);

#[derive(Default)]
struct Mw3Draft {
    went_first: Option<bool>,
    skipped_bans: u8,
    wincon: Option<Wincon>,
    dungeons: Option<Dungeons>,
    er: Option<Er>,
    trials: Option<Trials>,
    shops: Option<Shops>,
    scrubs: Option<Scrubs>,
    fountain: Option<Fountain>,
    spawn: Option<Spawn>,
}

impl Mw3Draft {
    fn pick_count(&self) -> u8 {
        self.skipped_bans
        + u8::from(self.wincon.is_some())
        + u8::from(self.dungeons.is_some())
        + u8::from(self.er.is_some())
        + u8::from(self.trials.is_some())
        + u8::from(self.shops.is_some())
        + u8::from(self.scrubs.is_some())
        + u8::from(self.fountain.is_some())
        + u8::from(self.spawn.is_some())
    }

    fn next_step(&self) -> DraftStep {
        if let Some(went_first) = self.went_first {
            match self.pick_count() {
                prev_bans @ 0..=1 => DraftStep::Ban {
                    team: match (prev_bans, went_first) {
                        (0, true) | (1, false) => Team::HighSeed,
                        (0, false) | (1, true) => Team::LowSeed,
                        (2.., _) => unreachable!(),
                    },
                    prev_bans,
                },
                n @ 2..=5 => DraftStep::Pick {
                    prev_picks: n - 2,
                    team: match (n, went_first) {
                        (2, true) | (3, false) | (4, false) | (5, true) => Team::HighSeed,
                        (2, false) | (3, true) | (4, true) | (5, false) => Team::LowSeed,
                        (0..=1 | 6.., _) => unreachable!(),
                    },
                },
                6.. => DraftStep::Done(Mw3Settings {
                    wincon: self.wincon.unwrap_or_default(),
                    dungeons: self.dungeons.unwrap_or_default(),
                    er: self.er.unwrap_or_default(),
                    trials: self.trials.unwrap_or_default(),
                    shops: self.shops.unwrap_or_default(),
                    scrubs: self.scrubs.unwrap_or_default(),
                    fountain: self.fountain.unwrap_or_default(),
                    spawn: self.spawn.unwrap_or_default(),
                }),
            }
        } else {
            DraftStep::GoFirst
        }
    }

    fn available_settings(&self) -> Vec<Mw3Setting> {
        let mut buf = Vec::with_capacity(8);
        if self.wincon.is_none() { buf.push(Mw3Setting::Wincon) }
        if self.dungeons.is_none() { buf.push(Mw3Setting::Dungeons) }
        if self.er.is_none() { buf.push(Mw3Setting::Er) }
        if self.trials.is_none() { buf.push(Mw3Setting::Trials) }
        if self.shops.is_none() { buf.push(Mw3Setting::Shops) }
        if self.scrubs.is_none() { buf.push(Mw3Setting::Scrubs) }
        if self.fountain.is_none() { buf.push(Mw3Setting::Fountain) }
        if self.spawn.is_none() { buf.push(Mw3Setting::Spawn) }
        buf
    }
}

#[derive(Default, Clone, Copy)]
struct Mw3Settings {
    wincon: Wincon,
    dungeons: Dungeons,
    er: Er,
    trials: Trials,
    shops: Shops,
    scrubs: Scrubs,
    fountain: Fountain,
    spawn: Spawn,
}

impl Mw3Settings {
    fn random(rng: &mut impl Rng) -> Self {
        let mut draft = Mw3Draft::default();
        loop {
            match draft.next_step() {
                DraftStep::GoFirst => draft.went_first = Some(rng.gen()),
                DraftStep::Ban { .. } => {
                    let available_settings = draft.available_settings();
                    let idx = rng.gen_range(0..=available_settings.len());
                    if let Some(setting) = available_settings.get(idx) {
                        match setting {
                            Mw3Setting::Wincon => draft.wincon = Some(Wincon::default()),
                            Mw3Setting::Dungeons => draft.dungeons = Some(Dungeons::default()),
                            Mw3Setting::Er => draft.er = Some(Er::default()),
                            Mw3Setting::Trials => draft.trials = Some(Trials::default()),
                            Mw3Setting::Shops => draft.shops = Some(Shops::default()),
                            Mw3Setting::Scrubs => draft.scrubs = Some(Scrubs::default()),
                            Mw3Setting::Fountain => draft.fountain = Some(Fountain::default()),
                            Mw3Setting::Spawn => draft.spawn = Some(Spawn::default()),
                        }
                    } else {
                        draft.skipped_bans += 1;
                    }
                }
                DraftStep::Pick { .. } => match draft.available_settings().choose(rng).expect("no more picks in DraftStep::Pick") {
                    Mw3Setting::Wincon => draft.wincon = Some(all().choose(rng).expect("setting values empty")),
                    Mw3Setting::Dungeons => draft.dungeons = Some(all().choose(rng).expect("setting values empty")),
                    Mw3Setting::Er => draft.er = Some(all().choose(rng).expect("setting values empty")),
                    Mw3Setting::Trials => draft.trials = Some(all().choose(rng).expect("setting values empty")),
                    Mw3Setting::Shops => draft.shops = Some(all().choose(rng).expect("setting values empty")),
                    Mw3Setting::Scrubs => draft.scrubs = Some(all().choose(rng).expect("setting values empty")),
                    Mw3Setting::Fountain => draft.fountain = Some(all().choose(rng).expect("setting values empty")),
                    Mw3Setting::Spawn => draft.spawn = Some(all().choose(rng).expect("setting values empty")),
                },
                DraftStep::Done(settings) => break settings,
            }
        }
    }

    fn resolve(&self) -> serde_json::Map<String, Json> {
        let Self { wincon, dungeons, er, trials, shops, scrubs, fountain, spawn } = self;
        collect![
            format!("user_message") => json!("3rd Multiworld Tournament"),
            format!("world_count") => json!(3),
            format!("open_forest") => json!("open"),
            format!("open_kakariko") => json!("open"),
            format!("open_door_of_time") => json!(true),
            format!("zora_fountain") => match fountain {
                Fountain::Closed => json!("closed"),
                Fountain::Open => json!("open"),
            },
            format!("gerudo_fortress") => json!("fast"),
            format!("bridge") => match wincon {
                Wincon::Meds => json!("medallions"),
                Wincon::Scrubs => json!("stones"),
                Wincon::Th => json!("dungeons"),
            },
            format!("bridge_medallions") => json!(6),
            format!("bridge_stones") => json!(3),
            format!("bridge_rewards") => json!(4),
            format!("triforce_hunt") => json!(matches!(wincon, Wincon::Th)),
            format!("triforce_count_per_world") => json!(30),
            format!("triforce_goal_per_world") => json!(25),
            format!("trials") => match trials {
                Trials::Zero => json!(0),
                Trials::Two => json!(2),
            },
            format!("skip_child_zelda") => json!(true),
            format!("no_escape_sequence") => json!(true),
            format!("no_guard_stealth") => json!(true),
            format!("no_epona_race") => json!(true),
            format!("skip_some_minigame_phases") => json!(true),
            format!("free_scarecrow") => json!(true),
            format!("fast_bunny_hood") => json!(true),
            format!("start_with_rupees") => json!(true),
            format!("start_with_consumables") => json!(true),
            format!("big_poe_count") => json!(1),
            format!("shuffle_dungeon_entrances") => match er {
                Er::Off => json!("off"),
                Er::Dungeon => json!("simple"),
            },
            format!("spawn_positions") => json!(matches!(spawn, Spawn::Random)),
            format!("shuffle_scrubs") => match scrubs {
                Scrubs::Affordable => json!("low"),
                Scrubs::Off => json!("off"),
            },
            format!("shopsanity") => match shops {
                Shops::Four => json!("4"),
                Shops::Off => json!("off"),
            },
            format!("tokensanity") => match dungeons {
                Dungeons::Skulls => json!("dungeons"),
                Dungeons::Tournament | Dungeons::Keyrings => json!("off"),
            },
            format!("shuffle_mapcompass") => json!("startwith"),
            format!("shuffle_smallkeys") => match dungeons {
                Dungeons::Tournament => json!("dungeon"),
                Dungeons::Skulls => json!("vanilla"),
                Dungeons::Keyrings => json!("keysanity"),
            },
            format!("key_rings") => match dungeons {
                Dungeons::Keyrings => json!([
                    "Forest Temple",
                    "Fire Temple",
                    "Water Temple",
                    "Shadow Temple",
                    "Spirit Temple",
                    "Bottom of the Well",
                    "Gerudo Training Ground",
                    "Ganons Castle",
                ]),
                Dungeons::Tournament | Dungeons::Skulls => json!([]),
            },
            format!("shuffle_bosskeys") => match dungeons {
                Dungeons::Tournament => json!("dungeon"),
                Dungeons::Skulls | Dungeons::Keyrings => json!("vanilla"),
            },
            format!("shuffle_ganon_bosskey") => match wincon {
                Wincon::Meds => json!("remove"),
                Wincon::Scrubs => json!("on_lacs"),
                Wincon::Th => json!("triforce"),
            },
            format!("disabled_locations") => json!([
                "Deku Theater Mask of Truth",
                "Kak 40 Gold Skulltula Reward",
                "Kak 50 Gold Skulltula Reward"
            ]),
            format!("allowed_tricks") => json!([
                "logic_fewer_tunic_requirements",
                "logic_grottos_without_agony",
                "logic_child_deadhand",
                "logic_man_on_roof",
                "logic_dc_jump",
                "logic_rusted_switches",
                "logic_windmill_poh",
                "logic_crater_bean_poh_with_hovers",
                "logic_forest_vines",
                "logic_lens_botw",
                "logic_lens_castle",
                "logic_lens_gtg",
                "logic_lens_shadow",
                "logic_lens_shadow_platform",
                "logic_lens_bongo",
                "logic_lens_spirit",
                "logic_dc_scarecrow_gs"
            ]),
            format!("logic_earliest_adult_trade") => json!("claim_check"),
            format!("starting_equipment") => json!([
                "deku_shield"
            ]),
            format!("starting_items") => json!([
                "ocarina",
                "farores_wind",
                "lens"
            ]),
            format!("correct_chest_appearances") => json!("both"),
            format!("hint_dist") => json!("custom"),
            format!("hint_dist_user") => json!({
                "name":                  "mw3",
                "gui_name":              "MW Season 3",
                "description":           "Hints used for the Multiworld Tournament Season 3.",
                "add_locations":         [
                    { "location": "Sheik in Kakariko", "types": ["always"] },
                    { "location": "Song from Ocarina of Time", "types": ["always"] },
                    { "location": "Deku Theater Skull Mask", "types": ["always"] },
                    { "location": "DMC Deku Scrub", "types": ["always"] },
                    { "location": "Deku Tree GS Basement Back Room", "types": ["sometimes"] },
                    { "location": "Water Temple GS River", "types": ["sometimes"] },
                    { "location": "Spirit Temple GS Hall After Sun Block Room", "types": ["sometimes"] },
                ],
                "remove_locations":      [
                    { "location": "Sheik in Crater", "types": ["sometimes"] },
                    { "location": "Song from Royal Familys Tomb", "types": ["sometimes"] },
                    { "location": "Sheik in Forest", "types": ["sometimes"] },
                    { "location": "Sheik at Temple", "types": ["sometimes"] },
                    { "location": "Sheik at Colossus", "types": ["sometimes"] },
                    { "location": "LH Sun", "types": ["sometimes"] },
                    { "location": "GC Maze Left Chest", "types": ["sometimes"] },
                    { "location": "GV Chest", "types": ["sometimes"] },
                    { "location": "Graveyard Royal Familys Tomb Chest", "types": ["sometimes"] },
                    { "location": "GC Pot Freestanding PoH", "types": ["sometimes"] },
                    { "location": "LH Lab Dive", "types": ["sometimes"] },
                    { "location": "Fire Temple Megaton Hammer Chest", "types": ["sometimes"] },
                    { "location": "Fire Temple Scarecrow Chest", "types": ["sometimes"] },
                    { "location": "Water Temple Boss Key Chest", "types": ["sometimes"] },
                    { "location": "Water Temple GS Behind Gate", "types": ["sometimes"] },
                    { "location": "Gerudo Training Ground Maze Path Final Chest", "types": ["sometimes"] },
                    { "location": "Spirit Temple Silver Gauntlets Chest", "types": ["sometimes"] },
                    { "location": "Spirit Temple Mirror Shield Chest", "types": ["sometimes"] },
                    { "location": "Shadow Temple Freestanding Key", "types": ["sometimes"] },
                    { "location": "Ganons Castle Shadow Trial Golden Gauntlets Chest", "types": ["sometimes"] },
                ],
                "add_items":             [],
                "remove_items":          [
                    { "item": "Zeldas Lullaby", "types": ["woth", "goal"] },
                ],
                "dungeons_woth_limit":   40,
                "dungeons_barren_limit": 40,
                "named_items_required":  true,
                "vague_named_items":     false,
                "use_default_goals":     true,
                "upgrade_hints":         "on",
                "distribution": {
                    "trial":           {"order": 1, "weight": 0.0, "fixed":   0, "copies": 2},
                    "always":          {"order": 2, "weight": 0.0, "fixed":   0, "copies": 2},
                    "goal":            {"order": 3, "weight": 0.0, "fixed":   7, "copies": 2},
                    "sometimes":       {"order": 4, "weight": 0.0, "fixed": 100, "copies": 2},
                    "barren":          {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "entrance_always": {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "woth":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "entrance":        {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "random":          {"order": 0, "weight": 9.0, "fixed":   0, "copies": 0},
                    "item":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "song":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "overworld":       {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "dungeon":         {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "junk":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "named-item":      {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "dual_always":     {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                    "dual":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                }
            }),
            format!("ice_trap_appearance") => json!("junk_only"),
            format!("junk_ice_traps") => json!("off"),
            format!("starting_age") => match spawn {
                Spawn::Tot => json!("adult"),
                Spawn::Random => json!("random"),
            },
        ]
    }
}

impl fmt::Display for Mw3Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut not_default = Vec::with_capacity(8);
        if self.wincon != Wincon::default() { not_default.push(self.wincon.to_string()) }
        if self.dungeons != Dungeons::default() { not_default.push(self.dungeons.to_string()) }
        if self.er != Er::default() { not_default.push(self.er.to_string()) }
        if self.trials != Trials::default() { not_default.push(self.trials.to_string()) }
        if self.shops != Shops::default() { not_default.push(self.shops.to_string()) }
        if self.scrubs != Scrubs::default() { not_default.push(self.scrubs.to_string()) }
        if self.fountain != Fountain::default() { not_default.push(self.fountain.to_string()) }
        if self.spawn != Spawn::default() { not_default.push(self.spawn.to_string()) }
        if let Some(not_default) = natjoin_str(not_default) {
            not_default.fmt(f)
        } else {
            write!(f, "base settings")
        }
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
    Draft(Mw3Draft),
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
                    Mw3Setting::Wincon => ctx.send_message("wincon: meds (default: 6 Medallion Bridge + Keysy BK), scrubs (3 Stone Bridge + LACS BK), or th (Triforce Hunt 25/30)").await?,
                    Mw3Setting::Dungeons => ctx.send_message("dungeons: tournament (default: keys shuffled in own dungeon), skulls (vanilla keys, dungeon tokens), or keyrings (small keyrings anywhere, vanilla boss keys)").await?,
                    Mw3Setting::Er => ctx.send_message("er: off (default) or dungeon").await?,
                    Mw3Setting::Trials => ctx.send_message("trials: 0 (default) or 2").await?,
                    Mw3Setting::Shops => ctx.send_message("shops: 4 (default) or off").await?,
                    Mw3Setting::Scrubs => ctx.send_message("scrubs: affordable (default) or off").await?,
                    Mw3Setting::Fountain => ctx.send_message("fountain: closed (default) or open").await?,
                    Mw3Setting::Spawn => ctx.send_message("spawn: tot (default: adult start, vanilla spawns) or random (random spawns and starting age)").await?,
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
                DraftStep::GoFirst => ctx.send_message("Team A, you have the higher seed. Choose whether you want to go !first or !second").await?,
                DraftStep::Ban { prev_bans, team } => ctx.send_message(&format!("{team}, lock a setting to its default using “!ban <setting>”, or use “!skip” if you don't want to ban anything.{}", if prev_bans == 0 { " Use “!settings” for a list of available settings." } else { "" })).await?,
                DraftStep::Pick { prev_picks, team } => ctx.send_message(&match prev_picks {
                    0 => format!("{team}, pick a setting using “!draft <setting> <value>”"),
                    1 => format!("{team}, pick two settings."),
                    2 => format!("And your second pick?"),
                    3 => format!("{team}, pick the final setting. You can also use “!skip” if you want to leave the settings as they are."),
                    _ => unreachable!(),
                }).await?,
                DraftStep::Done(settings) => {
                    drop(state); //TODO retain lock
                    self.roll_seed(ctx, settings).await;
                }
            }
        } else {
            unreachable!()
        }
        Ok(())
    }

    async fn roll_seed(&mut self, ctx: &RaceContext, settings: Mw3Settings) {
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
                                            Mw3Setting::Wincon => draft.wincon = Some(Wincon::default()),
                                            Mw3Setting::Dungeons => draft.dungeons = Some(Dungeons::default()),
                                            Mw3Setting::Er => draft.er = Some(Er::default()),
                                            Mw3Setting::Trials => draft.trials = Some(Trials::default()),
                                            Mw3Setting::Shops => draft.shops = Some(Shops::default()),
                                            Mw3Setting::Scrubs => draft.scrubs = Some(Scrubs::default()),
                                            Mw3Setting::Fountain => draft.fountain = Some(Fountain::default()),
                                            Mw3Setting::Spawn => draft.spawn = Some(Spawn::default()),
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
                                        Mw3Setting::Wincon => all::<Wincon>().map(|option| format!("“!draft wincon {}”", option.arg())).join(" or "),
                                        Mw3Setting::Dungeons => all::<Dungeons>().map(|option| format!("“!draft dungeons {}”", option.arg())).join(" or "),
                                        Mw3Setting::Er => all::<Er>().map(|option| format!("“!draft er {}”", option.arg())).join(" or "),
                                        Mw3Setting::Trials => all::<Trials>().map(|option| format!("“!draft trials {}”", option.arg())).join(" or "),
                                        Mw3Setting::Shops => all::<Shops>().map(|option| format!("“!draft shops {}”", option.arg())).join(" or "),
                                        Mw3Setting::Scrubs => all::<Scrubs>().map(|option| format!("“!draft scrubs {}”", option.arg())).join(" or "),
                                        Mw3Setting::Fountain => all::<Fountain>().map(|option| format!("“!draft fountain {}”", option.arg())).join(" or "),
                                        Mw3Setting::Spawn => all::<Spawn>().map(|option| format!("“!draft spawn {}”", option.arg())).join(" or "),
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
                                            Mw3Setting::Wincon => if let Some(value) = all::<Wincon>().find(|option| option.arg() == value) { draft.wincon = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Wincon>().map(|option| format!("“!draft wincon {}”", option.arg())).join(" or "),)).await? },
                                            Mw3Setting::Dungeons => if let Some(value) = all::<Dungeons>().find(|option| option.arg() == value) { draft.dungeons = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Dungeons>().map(|option| format!("“!draft dungeons {}”", option.arg())).join(" or "),)).await? },
                                            Mw3Setting::Er => if let Some(value) = all::<Er>().find(|option| option.arg() == value) { draft.er = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Er>().map(|option| format!("“!draft er {}”", option.arg())).join(" or "),)).await? },
                                            Mw3Setting::Trials => if let Some(value) = all::<Trials>().find(|option| option.arg() == value) { draft.trials = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Trials>().map(|option| format!("“!draft trials {}”", option.arg())).join(" or "),)).await? },
                                            Mw3Setting::Shops => if let Some(value) = all::<Shops>().find(|option| option.arg() == value) { draft.shops = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Shops>().map(|option| format!("“!draft shops {}”", option.arg())).join(" or "),)).await? },
                                            Mw3Setting::Scrubs => if let Some(value) = all::<Scrubs>().find(|option| option.arg() == value) { draft.scrubs = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Scrubs>().map(|option| format!("“!draft scrubs {}”", option.arg())).join(" or "),)).await? },
                                            Mw3Setting::Fountain => if let Some(value) = all::<Fountain>().find(|option| option.arg() == value) { draft.fountain = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Fountain>().map(|option| format!("“!draft fountain {}”", option.arg())).join(" or "),)).await? },
                                            Mw3Setting::Spawn => if let Some(value) = all::<Spawn>().find(|option| option.arg() == value) { draft.spawn = Some(value); drop(state); self.advance_draft(ctx).await? } else { ctx.send_message(&format!("Sorry {reply_to}, I don't recognize that value. Use {}", all::<Spawn>().map(|option| format!("“!draft spawn {}”", option.arg())).join(" or "),)).await? },
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
                            self.roll_seed(ctx, Mw3Settings::default()).await;
                        }
                        ["random"] => {
                            drop(state);
                            let settings = Mw3Settings::random(&mut thread_rng());
                            self.roll_seed(ctx, settings).await;
                        }
                        ["draft"] => {
                            *state = RaceState::Draft(Mw3Draft::default());
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
