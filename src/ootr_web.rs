//! A client for the ootrandomizer.com API, documented at <https://ootrandomizer.com/api/docs>

use {
    reqwest::{
        IntoUrl,
        StatusCode,
    },
    serde_json::Value as Json,
    serde_with::{
        DeserializeFromStr,
        DisplayFromStr,
        json::JsonString,
    },
    tokio::sync::{
        Semaphore,
        TryAcquireError,
    },
    crate::{
        prelude::*,
        racetime_bot::{
            SeedRollUpdate,
            UnlockSpoilerLog,
            VersionedBranch,
            VersionedRslPreset,
        },
    },
};

/// Randomizer versions that are known to exist on the ootrandomizer.com API despite not being listed by the version endpoint since supplementary versions weren't tracked at the time.
const KNOWN_GOOD_VERSIONS: [ootr_utils::Version; 4] = [
    ootr_utils::Version::from_branch(ootr_utils::Branch::DevR, 6, 2, 238, 1),
    ootr_utils::Version::from_branch(ootr_utils::Branch::DevR, 7, 1, 83, 1), // commit 578a64f4c78a831cde4215e0ac31565d3bf9bc46
    ootr_utils::Version::from_branch(ootr_utils::Branch::DevR, 7, 1, 143, 1), // commit 06390ece7e38fce1dd02ca60a28a7b1ff9fceb10
    ootr_utils::Version::from_branch(ootr_utils::Branch::DevFenhl, 6, 9, 14, 2),
];

const MULTIWORLD_RATE_LIMIT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)] HeaderToStr(#[from] reqwest::header::ToStrError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("there is nothing waiting for this seed anymore")]
    ChannelClosed,
    #[error("ootrandomizer.com API did not respond with expected patch file header")]
    PatchPathHeader,
    #[error("attempted to roll a random settings seed on web, but this branch isn't available with hidden settings on web")]
    RandomSettings,
    #[error("max retries exceeded")]
    Retries {
        num_retries: u8,
        last_error: Option<String>,
    },
    #[error("seed status API endpoint returned unknown value {0}")]
    UnexpectedSeedStatus(u8),
}

impl From<mpsc::error::SendError<SeedRollUpdate>> for Error {
    fn from(_: mpsc::error::SendError<SeedRollUpdate>) -> Self {
        Self::ChannelClosed
    }
}

struct VersionsResponse {
    currently_active_version: Option<ootr_utils::Version>,
    available_versions: Vec<ootr_utils::Version>,
}

pub(crate) struct SeedInfo {
    pub(crate) id: i64,
    pub(crate) gen_time: DateTime<Utc>,
    pub(crate) file_hash: [HashIcon; 5],
    pub(crate) file_stem: String,
    pub(crate) password: Option<[OcarinaNote; 6]>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeedDetailsResponse {
    pub(crate) spoiler_log: String,
}

pub(crate) struct ApiClient {
    http_client: reqwest::Client,
    api_key: String,
    api_key_encryption: String,
    next_request: Mutex<Instant>,
    mw_seed_rollers: Semaphore,
    waiting: Mutex<Vec<mpsc::UnboundedSender<()>>>,
}

impl ApiClient {
    pub(crate) fn new(http_client: reqwest::Client, api_key: String, api_key_encryption: String) -> Self {
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

    async fn head(&self, uri: impl IntoUrl + Clone, query: Option<&(impl Serialize + ?Sized)>) -> reqwest::Result<reqwest::Response> {
        lock!(next_request = self.next_request; {
            sleep_until(*next_request).await;
            let mut builder = self.http_client.head(uri.clone());
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

    async fn get_versions(&self, branch: Option<ootr_utils::Branch>, random_settings: bool) -> Result<VersionsResponse, Error> {
        #[derive(DeserializeFromStr)]
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

        impl VersionsResponseVersion {
            fn normalize(self, branch: Option<ootr_utils::Branch>) -> Option<ootr_utils::Version> {
                if let Some(supplementary) = self.supplementary.filter(|&supplementary| supplementary != 0) {
                    Some(ootr_utils::Version::from_branch(branch?, self.major, self.minor, self.patch, supplementary))
                } else if branch.map_or(true, |branch| branch == ootr_utils::Branch::Dev) {
                    Some(ootr_utils::Version::from_dev(self.major, self.minor, self.patch))
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
            branch.latest_web_name(random_settings).ok_or(Error::RandomSettings)?
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
    pub(crate) async fn can_roll_on_web(&self, rsl_preset: Option<&VersionedRslPreset>, version: &VersionedBranch, world_count: u8, unlock_spoiler_log: UnlockSpoilerLog) -> Option<ootr_utils::Version> {
        if world_count > 3 { return None }
        if let UnlockSpoilerLog::Progression = unlock_spoiler_log { return None }
        if rsl_preset.is_some() && version.branch().map_or(true, |branch| branch.latest_web_name_random_settings().is_none()) { return None }
        match version {
            VersionedBranch::Pinned(version) => {
                if matches!(rsl_preset, Some(VersionedRslPreset::Xopar { .. })) && *version == ootr_utils::Version::from_branch(ootr_utils::Branch::DevR, 7, 1, 181, 1) || *version == ootr_utils::Version::from_branch(ootr_utils::Branch::DevR, 8, 0, 1, 1) {
                    return Some(ootr_utils::Version::from_branch(
                        version.branch(),
                        version.base().major.try_into().expect("taken from existing ootr_utils::Version"),
                        version.base().minor.try_into().expect("taken from existing ootr_utils::Version"),
                        version.base().patch.try_into().expect("taken from existing ootr_utils::Version"),
                        0, // legacy devR/devRSL version which was not yet tagged with its supplementary version number but is only available in random settings mode (devRSL), not regularly (devR)
                    ))
                }
                let is_available = KNOWN_GOOD_VERSIONS.contains(version)
                    || self.get_versions((!version.is_release()).then(|| version.branch()), rsl_preset.is_some()).await
                        // the version API endpoint sometimes returns HTML instead of the expected JSON, fallback to generating locally when that happens
                        .is_ok_and(|VersionsResponse { available_versions, .. }| available_versions.contains(version));
                is_available.then(|| version.clone())
            }
            VersionedBranch::Latest(branch) => self.get_versions(Some(*branch), rsl_preset.is_some()).await.ok().and_then(|response| response.currently_active_version),
            VersionedBranch::Custom { .. } => None,
        }
    }

    pub(crate) async fn roll_seed_web(&self, update_tx: mpsc::Sender<SeedRollUpdate>, delay_until: Option<DateTime<Utc>>, version: ootr_utils::Version, random_settings: bool, unlock_spoiler_log: UnlockSpoilerLog, settings: serde_json::Map<String, Json>) -> Result<SeedInfo, Error> {
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

        #[derive(Deserialize)]
        struct PasswordResponse {
            pw: [OcarinaNote; 6],
        }

        let encrypt = version.is_release() && unlock_spoiler_log == UnlockSpoilerLog::Never;
        let api_key = if encrypt { &*self.api_key_encryption } else { &*self.api_key };
        let is_mw = settings.get("world_count").map_or(1, |world_count| world_count.as_u64().expect("world_count setting wasn't valid u64")) > 1;
        let password_lock = settings.get("password_lock").map_or(false, |password_lock| password_lock.as_bool().expect("password_lock setting wasn't a Boolean"));
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
        for attempt in 0u8.. {
            if attempt >= 3 && delay_until.map_or(true, |delay_until| Utc::now() >= delay_until) {
                drop(mw_permit);
                return Err(Error::Retries {
                    num_retries: attempt,
                    last_error: last_id.map(|id| format!("https://ootrandomizer.com/seed/get?id={id}")),
                })
            }
            if attempt == 0 && !random_settings {
                update_tx.send(SeedRollUpdate::Started).await?;
            }
            let CreateSeedResponse { id } = self.post("https://ootrandomizer.com/api/v2/seed/create", Some(&[
                ("key", api_key),
                ("version", &*version.to_string_web(random_settings).ok_or(Error::RandomSettings)?),
                if encrypt {
                    ("encrypt", "true")
                } else {
                    ("locked", if let UnlockSpoilerLog::Now = unlock_spoiler_log { "false" } else { "true" })
                },
                ("passwordLock", if password_lock { "true" } else { "false" }),
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
                        let (_, patch_file_name) = regex_captures!("^attachment; filename=(.+)$", patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION).ok_or(Error::PatchPathHeader)?.to_str()?).ok_or(Error::PatchPathHeader)?;
                        let patch_file_name = patch_file_name.to_owned();
                        let (_, patch_file_stem) = regex_captures!(r"^(.+)\.zpfz?$", &patch_file_name).ok_or(Error::PatchPathHeader)?;
                        let patch_path = Path::new(seed::DIR).join(&patch_file_name);
                        io::copy_buf(&mut StreamReader::new(patch_response.bytes_stream().map_err(io_error_from_reqwest)), &mut File::create(&patch_path).await?).await.at(patch_path)?;
                        return Ok(SeedInfo {
                            gen_time: creation_timestamp,
                            file_hash: settings_log.file_hash,
                            file_stem: patch_file_stem.to_owned(),
                            password: if password_lock {
                                let PasswordResponse { pw } = self.get("https://ootrandomizer.com/api/v2/seed/pw", Some(&[("key", api_key), ("id", &*id.to_string())])).await?
                                    .detailed_error_for_status().await?
                                    .json_with_text_in_error().await?;
                                Some(pw)
                            } else {
                                None
                            },
                            id,
                        })
                    }
                    2 => unreachable!(), // generated with link (not possible from API)
                    3 => break, // failed to generate
                    n => {
                        drop(mw_permit);
                        return Err(Error::UnexpectedSeedStatus(n))
                    }
                }
            }
        }
        Err(Error::Retries {
            num_retries: u8::MAX,
            last_error: last_id.map(|id| format!("https://ootrandomizer.com/seed/get?id={id}")),
        })
    }

    pub(crate) async fn patch_file_stem(&self, seed_id: i64) -> Result<String, Error> {
        let patch_response = self.head("https://ootrandomizer.com/api/v2/seed/patch", Some(&[("key", &self.api_key), ("id", &seed_id.to_string())])).await?
            .detailed_error_for_status().await?;
        let (_, file_stem) = regex_captures!(r"^attachment; filename=(.+)\.zpfz?$", patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION).ok_or(Error::PatchPathHeader)?.to_str()?).ok_or(Error::PatchPathHeader)?;
        Ok(file_stem.to_owned())
    }

    pub(crate) async fn unlock_spoiler_log(&self, seed_id: i64) -> Result<(), Error> {
        self.post("https://ootrandomizer.com/api/v2/seed/unlock", Some(&[("key", &self.api_key), ("id", &seed_id.to_string())]), None::<&()>, None).await?
            .detailed_error_for_status().await?;
        Ok(())
    }

    pub(crate) async fn seed_details(&self, seed_id: i64) -> Result<SeedDetailsResponse, Error> {
        Ok(
            self.get("https://ootrandomizer.com/api/v2/seed/details", Some(&[("key", &self.api_key), ("id", &seed_id.to_string())])).await?
                .detailed_error_for_status().await?
                .json_with_text_in_error().await?
        )
    }
}
