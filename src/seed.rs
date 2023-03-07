use {
    std::{
        borrow::Cow,
        path::Path,
    },
    chrono::prelude::*,
    futures::stream::{
        Stream,
        StreamExt as _,
    },
    ootr_utils::spoiler::{
        HashIcon,
        SpoilerLog,
    },
    rocket::response::content::RawHtml,
    rocket_util::html,
    serde::Deserialize,
    tokio::pin,
    wheel::fs,
    crate::http::static_url,
};

#[cfg(unix)] pub(crate) const DIR: &str = "/var/www/midos.house/seed";
#[cfg(windows)] pub(crate) const DIR: &str = "C:/Users/fenhl/games/zelda/oot/midos-house-seeds";

pub(crate) trait HashIconExt {
    fn to_html(&self) -> RawHtml<String>;
}

impl HashIconExt for HashIcon {
    fn to_html(&self) -> RawHtml<String> {
        let url_part = self.to_string().to_ascii_lowercase().replace(' ', "-");
        match self {
            Self::Bombchu |
            Self::BossKey |
            Self::Compass |
            Self::DekuNut |
            Self::DekuStick |
            Self::HeartContainer |
            Self::Map |
            Self::MasterSword |
            Self::SoldOut |
            Self::StoneOfAgony => html! {
                img(class = "hash-icon", alt = self.to_string(), src = static_url(&format!("hash-icon/{url_part}.png")), srcset = format!("{} 10x", static_url(&format!("hash-icon-500/{url_part}.png"))));
            },
            _ => html! {
                img(class = "hash-icon", alt = self.to_string(), src = static_url(&format!("hash-icon/{url_part}.png")));
            },
        }
    }
}

#[derive(Clone)]
pub(crate) struct Data {
    pub(crate) web: Option<OotrWebData>,
    pub(crate) file_hash: Option<[HashIcon; 5]>,
    pub(crate) file_stem: Cow<'static, str>,
}

#[derive(Clone, Copy)]
pub(crate) struct OotrWebData {
    pub(crate) id: u64,
    pub(crate) gen_time: DateTime<Utc>,
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

#[derive(Debug, thiserror::Error)]
pub(crate) enum TableCellsError {
    #[error(transparent)] Json(#[from] serde_json::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
}

pub(crate) async fn table_cells(now: DateTime<Utc>, seed: &Data, spoiler_logs: bool) -> Result<RawHtml<String>, TableCellsError> {
    /// If some other part of the log like settings or version number can't be parsed, we may still be able to read the file hash from the log
    #[derive(Deserialize)]
    struct SparseSpoilerLog {
        file_hash: [HashIcon; 5],
    }

    let (spoiler_file_name, spoiler_path_exists, file_hash, world_count) = if seed.file_hash.is_none() || seed.web.map_or(true, |web| web.gen_time <= now - chrono::Duration::days(90)) {
        let spoiler_file_name = format!("{}_Spoiler.json", seed.file_stem);
        let spoiler_path = Path::new(DIR).join(&spoiler_file_name);
        let spoiler_path_exists = spoiler_path.exists();
        let (file_hash, world_count) = if spoiler_path_exists {
            let log = fs::read_to_string(&spoiler_path).await?;
            if let Ok(log) = serde_json::from_str::<SpoilerLog>(&log) {
                (Some(log.file_hash), Some(log.settings.world_count))
            } else {
                (
                    seed.file_hash.or_else(|| serde_json::from_str::<SparseSpoilerLog>(&log).ok().map(|log| log.file_hash)),
                    None,
                )
            }
        } else {
            (seed.file_hash, None)
        };
        (Some(spoiler_file_name), spoiler_path_exists, file_hash, world_count)
    } else {
        (None, false, seed.file_hash, None)
    };
    Ok(html! {
        td(class = "hash") {
            @if let Some(file_hash) = seed.file_hash {
                @for hash_icon in file_hash {
                    : hash_icon.to_html();
                }
            } else {
                @if let Some(file_hash) = file_hash {
                    @for hash_icon in file_hash {
                        : hash_icon.to_html();
                    }
                }
            }
        }
        // ootrandomizer.com seeds are deleted after 90 days
        @if let Some(web) = seed.web.and_then(|web| (web.gen_time > now - chrono::Duration::days(90)).then_some(web)) {
            td(colspan? = spoiler_logs.then(|| "2")) {
                a(href = format!("https://ootrandomizer.com/seed/get?id={}", web.id)) : "View";
            }
        } else {
            td {
                a(href = format!("/seed/{}.{}", seed.file_stem, if let Some(world_count) = world_count {
                    if world_count.get() > 1 { "zpfz" } else { "zpf" }
                } else if Path::new(DIR).join(format!("{}.zpfz", seed.file_stem)).exists() {
                    "zpfz"
                } else {
                    "zpf"
                })) : "Download";
            }
            @if spoiler_logs {
                td {
                    @if spoiler_path_exists {
                        a(href = format!("/seed/{}", spoiler_file_name.expect("should be present since web seed missing or expired"))) : "View";
                    } else {
                        : "not found"; //TODO different message if the race is still in progress
                    }
                }
            }
        }
    })
}

pub(crate) async fn table(seeds: impl Stream<Item = Data>, spoiler_logs: bool) -> Result<RawHtml<String>, TableCellsError> {
    pin!(seeds);
    let now = Utc::now();
    Ok(html! {
        table {
            thead {
                tr : table_header_cells(spoiler_logs);
            }
            tbody {
                @while let Some(seed) = seeds.next().await {
                    tr : table_cells(now, &seed, spoiler_logs).await?;
                }
            }
        }
    })
}
