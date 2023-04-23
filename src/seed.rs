use {
    std::{
        borrow::Cow,
        num::NonZeroU8,
        path::{
            Path,
            PathBuf,
        },
    },
    chrono::prelude::*,
    futures::stream::{
        Stream,
        StreamExt as _,
    },
    if_chain::if_chain,
    ootr_utils::spoiler::{
        HashIcon,
        SpoilerLog,
    },
    rocket::{
        http::uri::Origin,
        response::content::RawHtml,
    },
    rocket_util::html,
    serde::Deserialize,
    tokio::pin,
    uuid::Uuid,
    wheel::fs,
    crate::http::static_url,
};
#[cfg(unix)] use async_proto::Protocol;

#[cfg(unix)] pub(crate) const DIR: &str = "/var/www/midos.house/seed";
#[cfg(windows)] pub(crate) const DIR: &str = "C:/Users/fenhl/games/zelda/oot/midos-house-seeds";

pub(crate) trait HashIconExt {
    fn to_html(&self) -> RawHtml<String>;
}

impl HashIconExt for HashIcon {
    fn to_html(&self) -> RawHtml<String> {
        html! {
            @match self {
                Self::DekuStick => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/deku-stick.png"), srcset = concat!(static_url!("hash-icon-500/deku-stick.png"), " 10x"));
                Self::DekuNut => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/deku-nut.png"), srcset = concat!(static_url!("hash-icon-500/deku-nut.png"), " 10x"));
                Self::Bow => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bow.png"));
                Self::Slingshot => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/slingshot.png"));
                Self::FairyOcarina => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/fairy-ocarina.png"));
                Self::Bombchu => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bombchu.png"), srcset = concat!(static_url!("hash-icon-500/bombchu.png"), " 10x"));
                Self::Longshot => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/longshot.png"));
                Self::Boomerang => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/boomerang.png"));
                Self::LensOfTruth => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/lens-of-truth.png"));
                Self::Beans => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/beans.png"));
                Self::MegatonHammer => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/megaton-hammer.png"));
                Self::BottledFish => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bottled-fish.png"));
                Self::BottledMilk => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/bottled-milk.png"));
                Self::MaskOfTruth => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/mask-of-truth.png"));
                Self::SoldOut => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/sold-out.png"), srcset = concat!(static_url!("hash-icon-500/sold-out.png"), " 10x"));
                Self::Cucco => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/cucco.png"));
                Self::Mushroom => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/mushroom.png"));
                Self::Saw => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/saw.png"));
                Self::Frog => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/frog.png"));
                Self::MasterSword => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/master-sword.png"), srcset = concat!(static_url!("hash-icon-500/master-sword.png"), " 10x"));
                Self::MirrorShield => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/mirror-shield.png"));
                Self::KokiriTunic => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/kokiri-tunic.png"));
                Self::HoverBoots => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/hover-boots.png"));
                Self::SilverGauntlets => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/silver-gauntlets.png"));
                Self::GoldScale => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/gold-scale.png"));
                Self::StoneOfAgony => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/stone-of-agony.png"), srcset = concat!(static_url!("hash-icon-500/stone-of-agony.png"), " 10x"));
                Self::SkullToken => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/skull-token.png"));
                Self::HeartContainer => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/heart-container.png"), srcset = concat!(static_url!("hash-icon-500/heart-container.png"), " 10x"));
                Self::BossKey => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/boss-key.png"), srcset = concat!(static_url!("hash-icon-500/boss-key.png"), " 10x"));
                Self::Compass => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/compass.png"), srcset = concat!(static_url!("hash-icon-500/compass.png"), " 10x"));
                Self::Map => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/map.png"), srcset = concat!(static_url!("hash-icon-500/map.png"), " 10x"));
                Self::BigMagic => img(class = "hash-icon", alt = self.to_string(), src = static_url!("hash-icon/big-magic.png"));
            }
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) struct Data {
    pub(crate) file_hash: Option<[HashIcon; 5]>,
    pub(crate) files: Files,
}

#[derive(Debug, Clone)]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) enum Files {
    MidosHouse {
        file_stem: Cow<'static, str>,
        locked_spoiler_log_path: Option<String>,
    },
    OotrWeb {
        id: u64,
        gen_time: DateTime<Utc>,
        file_stem: Cow<'static, str>,
    },
    TriforceBlitz {
        uuid: Uuid,
    },
}

impl Data {
    pub(crate) async fn extra(&self, now: DateTime<Utc>) -> Result<ExtraData, ExtraDataError> {
        /// If some other part of the log like settings or version number can't be parsed, we may still be able to read the file hash from the log
        #[derive(Deserialize)]
        struct SparseSpoilerLog {
            file_hash: [HashIcon; 5],
        }

        if_chain! {
            if self.file_hash.is_none() || match self.files {
                Files::MidosHouse { .. } => true,
                Files::OotrWeb { gen_time, .. } => gen_time <= now - chrono::Duration::days(90),
                Files::TriforceBlitz { .. } => false,
            };
            if let Some((spoiler_path, spoiler_file_name)) = match self.files {
                Files::MidosHouse { locked_spoiler_log_path: Some(ref spoiler_path), .. } => Some((PathBuf::from(spoiler_path), None)),
                Files::MidosHouse { locked_spoiler_log_path: None, ref file_stem } | Files::OotrWeb { ref file_stem, .. } => {
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
                        (Some(log.file_hash), Some(log.settings.world_count))
                    } else {
                        (
                            self.file_hash.or_else(|| serde_json::from_str::<SparseSpoilerLog>(&log).ok().map(|log| log.file_hash)),
                            None,
                        )
                    }
                } else {
                    (self.file_hash, None)
                };
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

pub(crate) fn table_empty_cells(spoiler_logs: bool) -> RawHtml<String> {
    html! {
        td;
        td;
        @if spoiler_logs {
            td;
        }
    }
}

pub(crate) async fn table_cells(now: DateTime<Utc>, seed: &Data, spoiler_logs: bool, add_hash_url: Option<Origin<'_>>) -> Result<RawHtml<String>, ExtraDataError> {
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
            Files::OotrWeb { id, gen_time, .. } if gen_time > now - chrono::Duration::days(90) => td(colspan? = spoiler_logs.then_some("2")) {
                a(href = format!("https://ootrandomizer.com/seed/get?id={id}")) : "View";
            }
            Files::OotrWeb { ref file_stem, .. } | Files::MidosHouse { ref file_stem, .. } => {
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
            Files::TriforceBlitz { uuid } => td(colspan? = spoiler_logs.then_some("2")) {
                a(href = format!("https://www.triforceblitz.com/seed/{uuid}")) : "View";
            }
        }
    })
}

pub(crate) async fn table(seeds: impl Stream<Item = Data>, spoiler_logs: bool) -> Result<RawHtml<String>, ExtraDataError> {
    pin!(seeds);
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
