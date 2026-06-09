use {
    indexmap::IndexMap,
    serde_json::Value as Json,
    crate::{
        event::{
            Data,
            InfoError,
        },
        prelude::*,
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2026" => Some(html! {
            article {
                p {
                    : "This is the first OOTR Spoiler Log Tournament, organized by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". See the official document (";
                    a(href = "https://docs.google.com/document/d/1FuTuwsDtguuxaF5sDmReWpt6o8MqyVwRov0osVoeiYA/edit") : "English";
                    : " • ";
                    a(href = "https://docs.google.com/document/d/1K8seiQIo3I2Zzs1Jphp42SPiZdAHbIlhL64b7pSouYo/edit") : "French";
                    : ") for details.";
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn settings_2026() -> seed::Settings {
    collect![
        format!("user_message") => json!("Spoiler Log Tournament"),
        format!("password_lock") => json!(true),
        format!("bridge") => json!("vanilla"),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("medallions"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!("open"),
        format!("gerudo_fortress") => json!("fast"),
        format!("starting_age") => json!("random"),
        format!("shuffle_dungeon_entrances") => json!("simple"),
        format!("spawn_positions") => json!([
            "child",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("adult_trade_start") => json!([
            "Prescription",
            "Eyeball Frog",
            "Eyedrops",
            "Claim Check",
        ]),
        format!("shuffle_map") => json!("startwith"),
        format!("shuffle_compass") => json!("startwith"),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
        ]),
        format!("allowed_tricks") => json!([
            "logic_grottos_without_agony",
            "logic_fewer_tunic_requirements",
            "logic_rusted_switches",
            "logic_man_on_roof",
            "logic_windmill_poh",
            "logic_crater_bean_poh_with_hovers",
            "logic_dc_jump",
            "logic_forest_vines",
            "logic_child_deadhand",
            "logic_lens_botw",
            "logic_lens_shadow",
            "logic_lens_shadow_platform",
            "logic_lens_bongo",
            "logic_lens_spirit",
            "logic_lens_gtg",
            "logic_lens_castle",
            "logic_deku_b1_webs_with_bow",
            "logic_dc_scarecrow_gs",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "zeldas_letter",
        ]),
        format!("random_starting_items_exclude") => json!([
            "bombchus",
            "shields",
            "deku_upgrades",
            "health_upgrades",
            "junk",
        ]),
        format!("random_starting_items_count") => json!(1),
        format!("start_with_consumables") => json!(true),
        format!("skip_reward_from_rauru") => json!("free"),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("scarecrow_behavior") => json!("fast"),
        format!("fast_bunny_hood") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("hints") => json!("none"),
        format!("hint_dist") => json!("tournament"),
        format!("misc_hints") => json!([]),
        format!("correct_chest_appearances") => json!("both"),
        format!("correct_potcrate_appearances") => json!("off"),
        format!("potcrate_textures_specific") => json!([]),
        format!("blue_fire_arrows") => json!(true),
        format!("tcg_requires_lens") => json!(true),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("anything"),
    ]
}

#[derive(Serialize)]
pub(crate) struct ProgressionSpoiler {
    entrances: IndexMap<Json, Json>,
    locations: serde_json::Map<String, Json>,
}

pub(crate) fn progression_spoiler(spoiler: Json) -> ProgressionSpoiler {
    fn replace(value: Json) -> Json {
        if value == "Child Spawn -> KF Links House" {
            json!("Child Spawn")
        } else if value == "DMC Fire Temple Entrance -> Fire Temple Lower" {
            json!("Fire Temple")
        } else if value == "Death Mountain -> Dodongos Cavern Beginning" {
            json!("Dodongos Cavern")
        } else if value == "Desert Colossus -> Spirit Temple Lobby" {
            json!("Spirit Temple")
        } else if value == "Gerudo Fortress -> Gerudo Training Ground Lobby" {
            json!("Gerudo Training Ground")
        } else if value == "Graveyard Warp Pad Region -> Shadow Temple Entryway" {
            json!("Shadow Temple")
        } else if value == "KF Outside Deku Tree -> Deku Tree Lobby" {
            json!("Deku Tree")
        } else if value == "Kakariko Village -> Bottom of the Well" {
            json!("Bottom of the Well")
        } else if value == "Lake Hylia -> Water Temple Lobby" {
            json!("Water Temple")
        } else if value == "SFM Forest Temple Entrance Ledge -> Forest Temple Lobby" {
            json!("Forest Temple")
        } else if value == "ZF Ice Ledge -> Ice Cavern Beginning" {
            json!("Ice Cavern")
        } else if value == "Zoras Fountain -> Jabu Jabus Belly Beginning" {
            json!("Jabu Jabus Belly")
        } else if value == json!({"region": "Gerudo Training Ground Lobby", "from": "Gerudo Fortress"}) {
            json!("Gerudo Training Ground")
        } else if value == json!({"region": "Jabu Jabus Belly Beginning", "from": "Zoras Fountain"}) {
            json!("Jabu Jabus Belly")
        } else if value == json!({"region": "Bottom of the Well", "from": "Kakariko Village"}) {
            json!("Bottom of the Well")
        } else if value == json!({"region": "Shadow Temple Entryway", "from": "Graveyard Warp Pad Region"}) {
            json!("Shadow Temple")
        } else if value == json!({"region": "Water Temple Lobby", "from": "Lake Hylia"}) {
            json!("Water Temple")
        } else if value == json!({"region": "Dodongos Cavern Beginning", "from": "Death Mountain"}) {
            json!("Dodongos Cavern")
        } else if value == json!({"region": "Spirit Temple Lobby", "from": "Desert Colossus"}) {
            json!("Spirit Temple")
        } else if value == json!({"region": "Ice Cavern Beginning", "from": "ZF Ice Ledge"}) {
            json!("Ice Cavern")
        } else if value == json!({"region": "Fire Temple Lower", "from": "DMC Fire Temple Entrance"}) {
            json!("Fire Temple")
        } else if value == json!({"region": "Forest Temple Lobby", "from": "SFM Forest Temple Entrance Ledge"}) {
            json!("Forest Temple")
        } else if value == json!({"region": "Deku Tree Lobby", "from": "KF Outside Deku Tree"}) {
            json!("Deku Tree")
        } else {
            value
        }
    }

    let Json::Object(mut spoiler) = spoiler else { panic!("spoiler log must be a JSON object") };
    ProgressionSpoiler {
        entrances: as_variant!(spoiler.remove("entrances").unwrap(), Json::Object).unwrap().into_iter().map(|(key, value)| (replace(Json::String(key)), replace(value))).collect(),
        locations: as_variant!(spoiler.remove("locations").unwrap(), Json::Object).unwrap(),
    }
}
