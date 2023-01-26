use {
    std::{
        borrow::Cow,
        fmt,
        iter,
        marker::PhantomData,
        num::NonZeroU8,
        path::Path,
    },
    chrono::prelude::*,
    futures::stream::{
        Stream,
        StreamExt as _,
    },
    itertools::Itertools as _,
    lazy_regex::regex_captures,
    rocket::response::content::RawHtml,
    rocket_util::{
        ToHtml,
        html,
    },
    serde::{
        Deserialize,
        Deserializer,
        Serialize,
        de::{
            Error as _,
            value::MapDeserializer,
        },
    },
    serde_plain::derive_display_from_serialize,
    tokio::pin,
    wheel::fs,
    crate::{
        favicon::{
            Bridge,
            CorrectChestAppearances,
            LacsCondition,
            MinorItemsAsMajorChest,
            ShuffleGanonBosskey,
            SpoilerLogLocations,
        },
        http::static_url,
    },
};

#[cfg(unix)] pub(crate) const DIR: &str = "/var/www/midos.house/seed";
#[cfg(windows)] pub(crate) const DIR: &str = "C:/Users/fenhl/games/zelda/oot/midos-house-seeds";

#[derive(Debug, Clone, Copy, sqlx::Type, Deserialize, Serialize)]
#[sqlx(type_name = "hash_icon")]
pub(crate) enum HashIcon {
    #[serde(rename = "Deku Stick")]
    #[sqlx(rename = "Deku Stick")]
    DekuStick,
    #[serde(rename = "Deku Nut")]
    #[sqlx(rename = "Deku Nut")]
    DekuNut,
    Bow,
    Slingshot,
    #[serde(rename = "Fairy Ocarina")]
    #[sqlx(rename = "Fairy Ocarina")]
    FairyOcarina,
    Bombchu,
    Longshot,
    Boomerang,
    #[serde(rename = "Lens of Truth")]
    #[sqlx(rename = "Lens of Truth")]
    LensOfTruth,
    Beans,
    #[serde(rename = "Megaton Hammer")]
    #[sqlx(rename = "Megaton Hammer")]
    MegatonHammer,
    #[serde(rename = "Bottled Fish")]
    #[sqlx(rename = "Bottled Fish")]
    BottledFish,
    #[serde(rename = "Bottled Milk")]
    #[sqlx(rename = "Bottled Milk")]
    BottledMilk,
    #[serde(rename = "Mask of Truth")]
    #[sqlx(rename = "Mask of Truth")]
    MaskOfTruth,
    #[serde(rename = "SOLD OUT")]
    #[sqlx(rename = "SOLD OUT")]
    SoldOut,
    Cucco,
    Mushroom,
    Saw,
    Frog,
    #[serde(rename = "Master Sword")]
    #[sqlx(rename = "Master Sword")]
    MasterSword,
    #[serde(rename = "Mirror Shield")]
    #[sqlx(rename = "Mirror Shield")]
    MirrorShield,
    #[serde(rename = "Kokiri Tunic")]
    #[sqlx(rename = "Kokiri Tunic")]
    KokiriTunic,
    #[serde(rename = "Hover Boots")]
    #[sqlx(rename = "Hover Boots")]
    HoverBoots,
    #[serde(rename = "Silver Gauntlets")]
    #[sqlx(rename = "Silver Gauntlets")]
    SilverGauntlets,
    #[serde(rename = "Gold Scale")]
    #[sqlx(rename = "Gold Scale")]
    GoldScale,
    #[serde(rename = "Stone of Agony")]
    #[sqlx(rename = "Stone of Agony")]
    StoneOfAgony,
    #[serde(rename = "Skull Token")]
    #[sqlx(rename = "Skull Token")]
    SkullToken,
    #[serde(rename = "Heart Container")]
    #[sqlx(rename = "Heart Container")]
    HeartContainer,
    #[serde(rename = "Boss Key")]
    #[sqlx(rename = "Boss Key")]
    BossKey,
    Compass,
    Map,
    #[serde(rename = "Big Magic")]
    #[sqlx(rename = "Big Magic")]
    BigMagic,
}

impl HashIcon {
    pub(crate) fn from_racetime_emoji(emoji: &str) -> Option<Self> {
        match emoji {
            "HashBeans" => Some(Self::Beans),
            "HashBigMagic" => Some(Self::BigMagic),
            "HashBombchu" => Some(Self::Bombchu),
            "HashBoomerang" => Some(Self::Boomerang),
            "HashBossKey" => Some(Self::BossKey),
            "HashBottledFish" => Some(Self::BottledFish),
            "HashBottledMilk" => Some(Self::BottledMilk),
            "HashBow" => Some(Self::Bow),
            "HashCompass" => Some(Self::Compass),
            "HashCucco" => Some(Self::Cucco),
            "HashDekuNut" => Some(Self::DekuNut),
            "HashDekuStick" => Some(Self::DekuStick),
            "HashFairyOcarina" => Some(Self::FairyOcarina),
            "HashFrog" => Some(Self::Frog),
            "HashGoldScale" => Some(Self::GoldScale),
            "HashHeart" => Some(Self::HeartContainer),
            "HashHoverBoots" => Some(Self::HoverBoots),
            "HashKokiriTunic" => Some(Self::KokiriTunic),
            "HashLensOfTruth" => Some(Self::LensOfTruth),
            "HashLongshot" => Some(Self::Longshot),
            "HashMap" => Some(Self::Map),
            "HashMaskOfTruth" => Some(Self::MaskOfTruth),
            "HashMasterSword" => Some(Self::MasterSword),
            "HashHammer" => Some(Self::MegatonHammer),
            "HashMirrorShield" => Some(Self::MirrorShield),
            "HashMushroom" => Some(Self::Mushroom),
            "HashSaw" => Some(Self::Saw),
            "HashSilvers" => Some(Self::SilverGauntlets),
            "HashSkullToken" => Some(Self::SkullToken),
            "HashSlingshot" => Some(Self::Slingshot),
            "HashSoldOut" => Some(Self::SoldOut),
            "HashStoneOfAgony" => Some(Self::StoneOfAgony),
            _ => None,
        }
    }

    pub(crate) fn to_racetime_emoji(&self) -> &'static str {
        match self {
            Self::Beans => "HashBeans",
            Self::BigMagic => "HashBigMagic",
            Self::Bombchu => "HashBombchu",
            Self::Boomerang => "HashBoomerang",
            Self::BossKey => "HashBossKey",
            Self::BottledFish => "HashBottledFish",
            Self::BottledMilk => "HashBottledMilk",
            Self::Bow => "HashBow",
            Self::Compass => "HashCompass",
            Self::Cucco => "HashCucco",
            Self::DekuNut => "HashDekuNut",
            Self::DekuStick => "HashDekuStick",
            Self::FairyOcarina => "HashFairyOcarina",
            Self::Frog => "HashFrog",
            Self::GoldScale => "HashGoldScale",
            Self::HeartContainer => "HashHeart",
            Self::HoverBoots => "HashHoverBoots",
            Self::KokiriTunic => "HashKokiriTunic",
            Self::LensOfTruth => "HashLensOfTruth",
            Self::Longshot => "HashLongshot",
            Self::Map => "HashMap",
            Self::MaskOfTruth => "HashMaskOfTruth",
            Self::MasterSword => "HashMasterSword",
            Self::MegatonHammer => "HashHammer",
            Self::MirrorShield => "HashMirrorShield",
            Self::Mushroom => "HashMushroom",
            Self::Saw => "HashSaw",
            Self::SilverGauntlets => "HashSilvers",
            Self::SkullToken => "HashSkullToken",
            Self::Slingshot => "HashSlingshot",
            Self::SoldOut => "HashSoldOut",
            Self::StoneOfAgony => "HashStoneOfAgony",
        }
    }
}

impl ToHtml for HashIcon {
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

derive_display_from_serialize!(HashIcon);

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

fn deserialize_multiworld<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Vec<T>, D::Error> {
    struct MultiworldVisitor<'de, T: Deserialize<'de>> {
        _marker: PhantomData<(&'de (), T)>,

    }

    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for MultiworldVisitor<'de, T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a multiworld map")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
            Ok(if let Some(first_key) = map.next_key()? {
                if let Some((_, world_number)) = regex_captures!("^World ([0-9]+)$", first_key) {
                    let world_number = world_number.parse::<usize>().expect("failed to parse world number");
                    let mut worlds = iter::repeat_with(|| None).take(world_number - 1).collect_vec();
                    worlds.push(map.next_value()?);
                    while let Some((key, value)) = map.next_entry()? {
                        let world_number = regex_captures!("^World ([0-9]+)$", key).expect("found mixed-format multiworld spoiler log").1.parse::<usize>().expect("failed to parse world number");
                        if world_number > worlds.len() {
                            if world_number > worlds.len() + 1 {
                                worlds.resize_with(world_number - 1, || None);
                            }
                            worlds.push(Some(value));
                        } else {
                            worlds[world_number - 1] = Some(value);
                        }
                    }
                    worlds.into_iter().map(|world| world.expect("missing entry for world")).collect()
                } else {
                    let mut new_map = iter::once((first_key.to_owned(), map.next_value()?)).collect::<serde_json::Map<_, _>>();
                    while let Some((key, value)) = map.next_entry()? {
                        new_map.insert(key, value);
                    }
                    vec![T::deserialize(MapDeserializer::new(new_map.into_iter())).map_err(A::Error::custom)?]
                }
            } else {
                Vec::default()
            })
        }
    }

    deserializer.deserialize_map(MultiworldVisitor { _marker: PhantomData })
}

#[derive(Deserialize)]
pub(crate) struct SpoilerLog {
    pub(crate) file_hash: [HashIcon; 5],
    #[serde(rename = ":version")]
    pub(crate) version: String,
    pub(crate) settings: SpoilerLogSettings,
    #[serde(deserialize_with = "deserialize_multiworld")]
    pub(crate) locations: Vec<SpoilerLogLocations>,
}

#[derive(Deserialize)]
pub(crate) struct SpoilerLogSettings {
    world_count: NonZeroU8,
    #[serde(default)]
    pub(crate) bridge: Bridge,
    #[serde(default)]
    pub(crate) bombchus_in_logic: bool,
    #[serde(default)]
    pub(crate) shuffle_ganon_bosskey: ShuffleGanonBosskey,
    #[serde(default)]
    pub(crate) lacs_condition: LacsCondition,
    #[serde(default)]
    pub(crate) correct_chest_sizes: bool,
    pub(crate) correct_chest_appearances: Option<CorrectChestAppearances>,
    #[serde(default)]
    pub(crate) minor_items_as_major_chest: MinorItemsAsMajorChest,
    #[serde(default)]
    pub(crate) invisible_chests: bool,
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
    let (spoiler_file_name, spoiler_path_exists, spoiler_contents) = if seed.file_hash.is_none() || seed.web.map_or(true, |web| web.gen_time <= now - chrono::Duration::days(90)) {
        let spoiler_file_name = format!("{}_Spoiler.json", seed.file_stem);
        let spoiler_path = Path::new(DIR).join(&spoiler_file_name);
        let spoiler_path_exists = spoiler_path.exists();
        (
            Some(spoiler_file_name),
            spoiler_path_exists,
            if spoiler_path_exists && (seed.file_hash.is_none() || seed.web.map_or(true, |web| web.gen_time <= now - chrono::Duration::days(90))) {
                Some(serde_json::from_str::<SpoilerLog>(&fs::read_to_string(&spoiler_path).await?)?)
            } else {
                None
            },
        )
    } else {
        (None, false, None)
    };
    Ok(html! {
        @if let Some(file_hash) = seed.file_hash {
            td(class = "hash") {
                @for hash_icon in file_hash {
                    : hash_icon;
                }
            }
        } else {
            td {
                @for hash_icon in spoiler_contents.as_ref().expect("should be present since file_hash is None").file_hash {
                    : hash_icon;
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
                a(href = format!("/seed/{}.{}", seed.file_stem, if let Some(ref spoiler) = spoiler_contents {
                    if spoiler.settings.world_count.get() > 1 { "zpfz" } else { "zpf" }
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
