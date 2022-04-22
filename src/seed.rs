use {
    std::{
        borrow::Cow,
        io,
        num::NonZeroU8,
        path::Path,
    },
    chrono::prelude::*,
    futures::stream::{
        Stream,
        StreamExt as _,
        TryStreamExt as _,
    },
    horrorshow::{
        RenderBox,
        box_html,
    },
    serde::{
        Deserialize,
        Serialize,
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

pub(crate) const DIR: &str = "/var/www/midos.house/seed";

#[derive(Deserialize, Serialize)]
enum HashIcon {
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

derive_display_from_serialize!(HashIcon);

impl HashIcon {
    fn to_html(&self) -> Box<dyn RenderBox + '_> {
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
            Self::StoneOfAgony => box_html! {
                img(class = "hash-icon", alt = self.to_string(), src = format!("/static/hash-icon/{url_part}.png"), srcset = format!("/static/hash-icon-500/{url_part}.png 10x"));
            },
            _ => box_html! {
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
    pub(crate) id: usize,
    pub(crate) gen_time: DateTime<Utc>,
}

#[derive(Deserialize)]
pub(crate) struct SpoilerLog {
    file_hash: [HashIcon; 5],
    pub(crate) settings: SpoilerLogSettings,
    pub(crate) locations: SpoilerLogLocations,
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
}

pub(crate) async fn table(seeds: impl Stream<Item = Data>) -> io::Result<Box<dyn RenderBox + Send>> {
    let now = Utc::now();
    let seeds = seeds.then(|seed| async move {
        // ootrandomizer.com seeds are deleted after 90 days
        let web_id = seed.web.as_ref().and_then(|web| (web.gen_time > now - chrono::Duration::days(90)).then(|| web.id));
        let spoiler_file_name = format!("{}_Spoiler.json", seed.file_stem);
        let spoiler_path = Path::new(DIR).join(&spoiler_file_name);
        let spoiler_contents = serde_json::from_str(&fs::read_to_string(&spoiler_path).await?)?;
        io::Result::Ok((seed, web_id, spoiler_file_name, spoiler_contents))
    }).try_collect::<Vec<(_, _, _, SpoilerLog)>>().await?;
    Ok(box_html! {
        table {
            thead {
                tr {
                    th : "Hash";
                    th : "Patch File";
                    th : "Spoiler Log";
                }
            }
            tbody {
                @for (seed, web_id, spoiler_file_name, spoiler_contents) in seeds {
                    tr {
                        td {
                            @for hash_icon in spoiler_contents.file_hash {
                                : hash_icon.to_html();
                            }
                        }
                        @if let Some(web_id) = web_id {
                            td(colspan = "2") {
                                a(href = format!("https://ootrandomizer.com/seed/get?id={web_id}")) : "View";
                            }
                        } else {
                            td {
                                a(href = format!("/seed/{}.{}", seed.file_stem, if spoiler_contents.settings.world_count.get() > 1 { "zpfz" } else { "zpf" })) : "Download";
                            }
                            td {
                                a(href = format!("/seed/{}", spoiler_file_name)) : "View";
                            }
                        }
                    }
                }
            }
        }
    })
}
