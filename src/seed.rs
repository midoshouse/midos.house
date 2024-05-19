use {
    futures::stream::{
        Stream,
        StreamExt as _,
    },
    rocket::{
        fs::NamedFile,
        http::{
            Header,
            hyper::header::{
                CONTENT_DISPOSITION,
                LINK,
            },
        },
        response::content::RawJson,
        uri,
    },
    rocket_util::OptSuffix,
    crate::prelude::*,
};

#[cfg(unix)] pub(crate) const DIR: &str = "/var/www/midos.house/seed";
#[cfg(windows)] pub(crate) const DIR: &str = "C:/Users/fenhl/games/zelda/oot/midos-house-seeds";

pub(crate) trait HashIconExt {
    fn to_html(&self) -> RawHtml<String>;
}

impl HashIconExt for HashIcon {
    fn to_html(&self) -> RawHtml<String> {
        html! {
            @match self {
                Self::DekuStick => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/deku-stick.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/deku-stick.png")));
                Self::DekuNut => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/deku-nut.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/deku-nut.png")));
                Self::Bow => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bow.svg"));
                Self::Slingshot => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/slingshot.svg"));
                Self::FairyOcarina => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/fairy-ocarina.svg"));
                Self::Bombchu => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bombchu.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/bombchu.png")));
                Self::Longshot => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/longshot.svg"));
                Self::Boomerang => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/boomerang.svg"));
                Self::LensOfTruth => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/lens-of-truth.svg"));
                Self::Beans => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/beans.svg"));
                Self::MegatonHammer => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/megaton-hammer.svg"));
                Self::BottledFish => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bottled-fish.png"));
                Self::BottledMilk => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bottled-milk.png"));
                Self::MaskOfTruth => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/mask-of-truth.svg"));
                Self::SoldOut => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/sold-out.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/sold-out.png")));
                Self::Cucco => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/cucco.png"));
                Self::Mushroom => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/mushroom.png"));
                Self::Saw => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/saw.png"));
                Self::Frog => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/frog.png"));
                Self::MasterSword => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/master-sword.svg"));
                Self::MirrorShield => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/mirror-shield.svg"));
                Self::KokiriTunic => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/kokiri-tunic.png"));
                Self::HoverBoots => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/hover-boots.png"));
                Self::SilverGauntlets => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/silver-gauntlets.svg"));
                Self::GoldScale => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/gold-scale.svg"));
                Self::StoneOfAgony => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/stone-of-agony.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/stone-of-agony.png")));
                Self::SkullToken => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/skull-token.svg"));
                Self::HeartContainer => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/heart-container.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/heart-container.png")));
                Self::BossKey => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/boss-key.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/boss-key.png")));
                Self::Compass => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/compass.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/compass.png")));
                Self::Map => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/map.png"), srcset = format!("{} 10x", static_url!("hash-icon-500/map.png")));
                Self::BigMagic => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/big-magic.svg"));
            }
        }
    }
}

#[derive(Default, Debug, Clone)]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) struct Data {
    pub(crate) file_hash: Option<[HashIcon; 5]>,
    pub(crate) files: Option<Files>,
}

#[derive(Debug, Clone)]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) enum Files {
    MidosHouse {
        file_stem: Cow<'static, str>,
        locked_spoiler_log_path: Option<String>,
    },
    OotrWeb {
        id: i64,
        gen_time: DateTime<Utc>,
        file_stem: Cow<'static, str>,
    },
    TriforceBlitz {
        uuid: Uuid,
    },
    TfbSotd {
        date: NaiveDate,
        ordinal: u64,
    },
}

impl Data {
    pub(crate) fn from_db(
        start: Option<DateTime<Utc>>,
        async_start1: Option<DateTime<Utc>>,
        async_start2: Option<DateTime<Utc>>,
        file_stem: Option<String>,
        locked_spoiler_log_path: Option<String>,
        web_id: Option<i64>,
        web_gen_time: Option<DateTime<Utc>>,
        tfb_uuid: Option<Uuid>,
        hash1: Option<HashIcon>,
        hash2: Option<HashIcon>,
        hash3: Option<HashIcon>,
        hash4: Option<HashIcon>,
        hash5: Option<HashIcon>,
    ) -> Self {
        Self {
            file_hash: match (hash1, hash2, hash3, hash4, hash5) {
                (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                (None, None, None, None, None) => None,
                _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
            },
            files: match (file_stem, locked_spoiler_log_path, web_id, web_gen_time, tfb_uuid) {
                (_, _, _, _, Some(uuid)) => Some(Files::TriforceBlitz { uuid }),
                (Some(file_stem), _, Some(id), Some(gen_time), None) => Some(Files::OotrWeb { id, gen_time, file_stem: Cow::Owned(file_stem) }),
                (Some(file_stem), locked_spoiler_log_path, Some(id), None, None) => Some(match (start, async_start1, async_start2) {
                    (Some(start), None, None) | (None, Some(start), None) | (None, None, Some(start)) => Files::OotrWeb { id, gen_time: start - TimeDelta::days(1), file_stem: Cow::Owned(file_stem) },
                    (None, Some(async_start1), Some(async_start2)) => Files::OotrWeb { id, gen_time: async_start1.min(async_start2) - TimeDelta::days(1), file_stem: Cow::Owned(file_stem) },
                    (_, _, _) => Files::MidosHouse { file_stem: Cow::Owned(file_stem), locked_spoiler_log_path },
                }),
                (Some(file_stem), locked_spoiler_log_path, None, _, None) => Some(Files::MidosHouse { file_stem: Cow::Owned(file_stem), locked_spoiler_log_path }),
                (None, _, _, _, None) => None,
            },
        }
    }

    pub(crate) async fn extra(&self, now: DateTime<Utc>) -> Result<ExtraData, ExtraDataError> {
        /// If some other part of the log like settings or version number can't be parsed, we may still be able to read the file hash from the log
        #[derive(Deserialize)]
        struct SparseSpoilerLog {
            file_hash: [HashIcon; 5],
        }

        if_chain! {
            if self.file_hash.is_none() || match self.files {
                Some(Files::MidosHouse { .. }) => true,
                Some(Files::OotrWeb { gen_time, .. }) => gen_time <= now - TimeDelta::days(90),
                Some(Files::TriforceBlitz { .. }) => false,
                Some(Files::TfbSotd { .. }) => false,
                None => false,
            };
            if let Some((spoiler_path, spoiler_file_name)) = match self.files {
                Some(Files::MidosHouse { locked_spoiler_log_path: Some(ref spoiler_path), .. }) => Some((PathBuf::from(spoiler_path), None)),
                Some(Files::MidosHouse { locked_spoiler_log_path: None, ref file_stem } | Files::OotrWeb { ref file_stem, .. }) => {
                    let spoiler_file_name = format!("{file_stem}_Spoiler.json");
                    Some((Path::new(DIR).join(&spoiler_file_name).to_owned(), Some(spoiler_file_name)))
                }
                _ => None,
            };
            then {
                let spoiler_path_exists = spoiler_path.exists();
                let (file_hash, world_count) = if spoiler_path_exists {
                    let log = fs::read_to_string(&spoiler_path).await?;
                    if let Ok(log) = serde_json::from_str::<SpoilerLog>(&log) {
                        (Some(log.file_hash), Some(log.settings[0].world_count))
                    } else {
                        (
                            self.file_hash.or_else(|| serde_json::from_str::<SparseSpoilerLog>(&log).ok().map(|log| log.file_hash)),
                            None,
                        )
                    }
                } else {
                    (self.file_hash, None)
                };
                //TODO if file_hash.is_none() and a patch file is available, read the file hash from the patched rom?
                return Ok(ExtraData {
                    spoiler_status: if spoiler_path_exists {
                        if let Some(spoiler_file_name) = spoiler_file_name {
                            SpoilerStatus::Unlocked(spoiler_file_name)
                        } else {
                            SpoilerStatus::Locked
                        }
                    } else {
                        SpoilerStatus::NotFound
                    },
                    file_hash, world_count,
                })
            }
        }
        //TODO if file_hash.is_none() and a patch file is available, read the file hash from the patched rom?
        Ok(ExtraData {
            spoiler_status: SpoilerStatus::NotFound,
            file_hash: self.file_hash,
            world_count: None,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExtraDataError {
    #[error(transparent)] Json(#[from] serde_json::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
}

impl IsNetworkError for ExtraDataError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Json(_) => false,
            Self::Wheel(e) => e.is_network_error(),
        }
    }
}

pub(crate) struct ExtraData {
    spoiler_status: SpoilerStatus,
    pub(crate) file_hash: Option<[HashIcon; 5]>,
    pub(crate) world_count: Option<NonZeroU8>,
}

enum SpoilerStatus {
    Unlocked(String),
    Locked,
    NotFound,
}

pub(crate) fn table_header_cells(spoiler_logs: bool) -> RawHtml<String> {
    html! {
        th : "Hash";
        th : "Patch File";
        @if spoiler_logs {
            th : "Spoiler Log";
        }
    }
}

pub(crate) async fn table_cells(now: DateTime<Utc>, seed: &Data, spoiler_logs: bool, add_hash_url: Option<rocket::http::uri::Origin<'_>>) -> Result<RawHtml<String>, ExtraDataError> {
    let extra = seed.extra(now).await?;
    Ok(html! {
        td {
            @if let Some(file_hash) = extra.file_hash {
                div(class = "hash") {
                    @for hash_icon in file_hash {
                        : hash_icon.to_html();
                    }
                }
            } else if let Some(add_hash_url) = add_hash_url {
                a(class = "button", href = add_hash_url.to_string()) : "Add";
            }
        }
        // ootrandomizer.com seeds are deleted after 90 days
        @match seed.files {
            Some(Files::OotrWeb { id, gen_time, .. }) if gen_time > now - TimeDelta::days(90) => td(colspan? = spoiler_logs.then_some("2")) {
                a(href = format!("https://ootrandomizer.com/seed/get?id={id}")) : "View";
            }
            Some(Files::OotrWeb { ref file_stem, .. } | Files::MidosHouse { ref file_stem, .. }) => {
                td {
                    a(href = format!("/seed/{file_stem}.{}", if let Some(world_count) = extra.world_count {
                        if world_count.get() > 1 { "zpfz" } else { "zpf" }
                    } else if Path::new(DIR).join(format!("{file_stem}.zpfz")).exists() {
                        "zpfz"
                    } else {
                        "zpf"
                    })) : "Download";
                }
                @if spoiler_logs {
                    td {
                        @match extra.spoiler_status {
                            SpoilerStatus::Unlocked(spoiler_file_name) => a(href = format!("/seed/{spoiler_file_name}")) : "View";
                            SpoilerStatus::Locked => : "locked";
                            SpoilerStatus::NotFound => : "not found";
                        }
                    }
                }
            }
            Some(Files::TriforceBlitz { uuid }) => td(colspan? = spoiler_logs.then_some("2")) {
                a(href = format!("https://www.triforceblitz.com/seed/{uuid}")) : "View";
            }
            Some(Files::TfbSotd { ordinal, .. }) => td(colspan? = spoiler_logs.then_some("2")) {
                a(href = format!("https://www.triforceblitz.com/seed/daily/{ordinal}")) : "View";
            }
            None => {
                td;
                @if spoiler_logs {
                    td;
                }
            }
        }
    })
}

pub(crate) async fn table(seeds: impl Stream<Item = Data>, spoiler_logs: bool) -> Result<RawHtml<String>, ExtraDataError> {
    let mut seeds = pin!(seeds);
    let now = Utc::now();
    Ok(html! {
        table(class = "seeds") {
            thead {
                tr : table_header_cells(spoiler_logs);
            }
            tbody {
                @while let Some(seed) = seeds.next().await {
                    tr : table_cells(now, &seed, spoiler_logs, None).await?;
                }
            }
        }
    })
}

#[derive(Responder)]
pub(crate) enum GetResponse {
    Page(RawHtml<String>),
    Patch {
        inner: NamedFile,
        content_disposition: Header<'static>,
    },
    Spoiler {
        inner: RawJson<Vec<u8>>,
        content_disposition: Header<'static>,
        link: Header<'static>,
    },
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum GetError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
}

impl<E: Into<GetError>> From<E> for StatusOrError<GetError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::get("/seed/<filename>")]
pub(crate) async fn get(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, filename: OptSuffix<'_, &str>) -> Result<GetResponse, StatusOrError<GetError>> {
    let OptSuffix(file_stem, suffix) = filename;
    if !regex_is_match!("^[0-9A-Za-z_-]+$", file_stem) { return Err(StatusOrError::Status(Status::NotFound)) }
    Ok(match suffix {
        Some(suffix @ ("zpf" | "zpfz")) => {
            let path = Path::new(DIR).join(format!("{file_stem}.{suffix}"));
            GetResponse::Patch {
                inner: match NamedFile::open(&path).await {
                    Ok(file) => file,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(StatusOrError::Status(Status::NotFound)),
                    Err(e) => return Err(e).at(path).map_err(|e| StatusOrError::Err(GetError::Wheel(e))),
                },
                content_disposition: Header::new(CONTENT_DISPOSITION.as_str(), "attachment"),
            }
        }
        Some("json") => {
            let spoiler = match fs::read(Path::new(DIR).join(format!("{file_stem}.json"))).await {
                Ok(spoiler) => spoiler,
                Err(wheel::Error::Io { inner, .. }) if inner.kind() == io::ErrorKind::NotFound => return Err(StatusOrError::Status(Status::NotFound)),
                Err(e) => return Err(e.into()),
            };
            let chests = match serde_json::from_slice::<SpoilerLog>(&spoiler) {
                Ok(spoiler) => ChestAppearances::from(spoiler),
                Err(e) => {
                    eprintln!("failed to add favicon to {file_stem}.json: {e} ({e:?})");
                    if let Environment::Production = **env {
                        wheel::night_report("/net/midoshouse/error", Some(&format!("failed to add favicon to {file_stem}.json: {e} ({e:?})"))).await?;
                    }
                    ChestAppearances::random()
                }
            };
            GetResponse::Spoiler {
                inner: RawJson(spoiler),
                content_disposition: Header::new(CONTENT_DISPOSITION.as_str(), "inline"),
                // may not work in all browsers, see https://bugzilla.mozilla.org/show_bug.cgi?id=1185705
                link: Header::new(LINK.as_str(), format!(r#"<{}>; rel="icon"; sizes="1024x1024""#, uri!(favicon::favicon_png(Suffix(chests.textures(), "png"))))),
            }
        }
        Some(_) => return Err(StatusOrError::Status(Status::NotFound)),
        None => {
            let mut transaction = pool.begin().await?;
            let patch_suffix = if fs::exists(Path::new(DIR).join(format!("{file_stem}.zpf"))).await? {
                "zpf"
            } else if fs::exists(Path::new(DIR).join(format!("{file_stem}.zpfz"))).await? {
                "zpfz"
            } else {
                return Err(StatusOrError::Status(Status::NotFound))
            };
            let spoiler_filename = format!("{file_stem}_Spoiler.json");
            let (spoiler_status, hash, chests) = match fs::read_json::<SpoilerLog>(Path::new(DIR).join(&spoiler_filename)).await {
                Ok(spoiler) => (SpoilerStatus::Unlocked(spoiler_filename), Some(spoiler.file_hash), ChestAppearances::from(spoiler)),
                Err(wheel::Error::Io { inner, .. }) if inner.kind() == io::ErrorKind::NotFound => if let Some(Some(locked_spoiler_log_path)) = sqlx::query_scalar!("SELECT locked_spoiler_log_path FROM races WHERE file_stem = $1", file_stem).fetch_optional(&mut *transaction).await? {
                    let spoiler = fs::read_json::<SpoilerLog>(locked_spoiler_log_path).await?;
                    (SpoilerStatus::Locked, Some(spoiler.file_hash), ChestAppearances::random()) // keeping chests random for locked spoilers to avoid leaking seed info
                } else {
                    (SpoilerStatus::NotFound, None, ChestAppearances::random())
                },
                Err(e) => return Err(e.into()),
            };
            GetResponse::Page(page(transaction, &me, &uri, PageStyle { kind: PageKind::Center, chests, ..PageStyle::default() }, "Seed — Mido's House", html! {
                @if let Some(hash) = hash {
                    h1(class = "hash") {
                        @for hash_icon in hash {
                            : hash_icon.to_html();
                        }
                    }
                } else {
                    h1 : "Seed";
                }
                @match spoiler_status {
                    SpoilerStatus::Unlocked(spoiler_filename) => div(class = "button-row") {
                        a(class = "button", href = format!("/seed/{file_stem}.{patch_suffix}")) : "Patch File";
                        a(class = "button", href = format!("/seed/{spoiler_filename}")) : "Spoiler Log";
                    }
                    SpoilerStatus::Locked => {
                        div(class = "button-row") {
                            a(class = "button", href = format!("/seed/{file_stem}.{patch_suffix}")) : "Patch File";
                        }
                        p : "Spoiler log locked (race is still in progress)";
                    }
                    SpoilerStatus::NotFound => {
                        div(class = "button-row") {
                            a(class = "button", href = format!("/seed/{file_stem}.{patch_suffix}")) : "Patch File";
                        }
                        p : "Spoiler log not found";
                    }
                }
            }).await?)
        }
    })
}
