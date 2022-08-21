use {
    std::{
        borrow::Cow,
        fmt,
        io,
        iter,
        marker::PhantomData,
        num::NonZeroU8,
        path::Path,
    },
    chrono::prelude::*,
    futures::stream::{
        Stream,
        StreamExt as _,
        TryStreamExt as _,
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
    tokio::fs,
    crate::favicon::{
        Bridge,
        CorrectChestAppearances,
        LacsCondition,
        ShuffleGanonBosskey,
        SpoilerLogLocations,
    },
};

#[cfg(unix)] pub(crate) const DIR: &str = "/var/www/midos.house/seed";
#[cfg(windows)] pub(crate) const DIR: &str = "C:/Users/fenhl/games/zelda/oot/midos-house-seeds";

#[derive(Deserialize, Serialize)]
pub(crate) enum HashIcon {
    #[serde(rename = "Deku Stick")]
    DekuStick,
    #[serde(rename = "Deku Nut")]
    DekuNut,
    Bow,
    Slingshot,
    #[serde(rename = "Fairy Ocarina")]
    FairyOcarina,
    Bombchu,
    Longshot,
    Boomerang,
    #[serde(rename = "Lens of Truth")]
    LensOfTruth,
    Beans,
    #[serde(rename = "Megaton Hammer")]
    MegatonHammer,
    #[serde(rename = "Bottled Fish")]
    BottledFish,
    #[serde(rename = "Bottled Milk")]
    BottledMilk,
    #[serde(rename = "Mask of Truth")]
    MaskOfTruth,
    #[serde(rename = "SOLD OUT")]
    SoldOut,
    Cucco,
    Mushroom,
    Saw,
    Frog,
    #[serde(rename = "Master Sword")]
    MasterSword,
    #[serde(rename = "Mirror Shield")]
    MirrorShield,
    #[serde(rename = "Kokiri Tunic")]
    KokiriTunic,
    #[serde(rename = "Hover Boots")]
    HoverBoots,
    #[serde(rename = "Silver Gauntlets")]
    SilverGauntlets,
    #[serde(rename = "Gold Scale")]
    GoldScale,
    #[serde(rename = "Stone of Agony")]
    StoneOfAgony,
    #[serde(rename = "Skull Token")]
    SkullToken,
    #[serde(rename = "Heart Container")]
    HeartContainer,
    #[serde(rename = "Boss Key")]
    BossKey,
    Compass,
    Map,
    #[serde(rename = "Big Magic")]
    BigMagic,
}

impl HashIcon {
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

derive_display_from_serialize!(HashIcon);

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
                img(class = "hash-icon", alt = self.to_string(), src = format!("/static/hash-icon/{url_part}.png"), srcset = format!("/static/hash-icon-500/{url_part}.png 10x"));
            },
            _ => html! {
                img(class = "hash-icon", alt = self.to_string(), src = format!("/static/hash-icon/{url_part}.png"));
            },
        }
    }
}

pub(crate) struct Data {
    pub(crate) web: Option<OotrWebData>,
    pub(crate) file_stem: Cow<'static, str>,
}

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
    pub(crate) invisible_chests: bool,
}

pub(crate) async fn table(seeds: impl Stream<Item = Data>, spoiler_logs: bool) -> io::Result<RawHtml<String>> {
    let now = Utc::now();
    let seeds = seeds.then(|seed| async move {
        // ootrandomizer.com seeds are deleted after 90 days
        let web_id = seed.web.as_ref().and_then(|web| (web.gen_time > now - chrono::Duration::days(90)).then(|| web.id));
        let spoiler_file_name = format!("{}_Spoiler.json", seed.file_stem);
        let spoiler_path = Path::new(DIR).join(&spoiler_file_name);
        let spoiler_contents = serde_json::from_str(&fs::read_to_string(&spoiler_path).await?)?;
        io::Result::Ok((seed, web_id, spoiler_file_name, spoiler_contents))
    }).try_collect::<Vec<(_, _, _, SpoilerLog)>>().await?;
    Ok(html! {
        table {
            thead {
                tr {
                    th : "Hash";
                    th : "Patch File";
                    @if spoiler_logs {
                        th : "Spoiler Log";
                    }
                }
            }
            tbody {
                @for (seed, web_id, spoiler_file_name, spoiler_contents) in seeds {
                    tr {
                        td {
                            @for hash_icon in spoiler_contents.file_hash {
                                : hash_icon;
                            }
                        }
                        @if let Some(web_id) = web_id {
                            td(colspan? = spoiler_logs.then(|| "2")) {
                                a(href = format!("https://ootrandomizer.com/seed/get?id={web_id}")) : "View";
                            }
                        } else {
                            td {
                                a(href = format!("/seed/{}.{}", seed.file_stem, if spoiler_contents.settings.world_count.get() > 1 { "zpfz" } else { "zpf" })) : "Download";
                            }
                            @if spoiler_logs {
                                td {
                                    a(href = format!("/seed/{}", spoiler_file_name)) : "View";
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}
