use {
    chrono::Days,
    crate::{
        event::{
            Data,
            InfoError,
        },
        prelude::*,
    },
};

pub(crate) fn next_s2_race_after(min_time: DateTime<impl TimeZone>) -> DateTime<Utc> {
    let mut time = Utc.with_ymd_and_hms(2025, 10, 18, 20, 0, 0).single().expect("wrong hardcoded datetime");
    while time <= min_time {
        let date = time.date_naive().checked_add_days(Days::new(14)).unwrap();
        time = date.and_hms_opt(20, 0, 0).unwrap().and_utc();
    }
    time
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => Some(html! {
            article {
                p {
                    : "This is the first tournament season of Battle Royale, a game mode played on 1-hit KO where players complete challenges in the seed to score points without dying. This season is organised by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1JB_CfbUFQwoTuV8RHniG1nfiXWki4n4NMFlKXDCp5P8/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn s1_enter_form() -> RawHtml<String> {
    html! {
        article {
            p {
                : "To enter this tournament, either join the live qualifier on ";
                : format_datetime(Utc.with_ymd_and_hms(2024, 3, 9, 19, 0, 0).single().expect("wrong hardcoded datetime"), DateTimeFormat { long: true, running_text: true });
                : " or play the qualifier async (see ";
                a(href = "https://discord.com/channels/274180765816848384/1208046928504553483/1213524850627317830") : "this Discord message";
                : " for details).";
            }
            p {
                : "Note: This page is not official. See ";
                a(href = "https://docs.google.com/document/d/1JB_CfbUFQwoTuV8RHniG1nfiXWki4n4NMFlKXDCp5P8/edit") : "the official document";
                : " for details.";
            }
        }
    }
}

pub(crate) fn s2_enter_form() -> RawHtml<String> {
    html! {
        article {
            p {
                : "To enter this tournament, request the ";
                strong : "@battle royale";
                : " role on ";
                a(href = "https://discord.gg/ootrandomizer") : "the OoT Randomizer Discord server";
                : ". See ";
                a(href = "https://discord.com/channels/274180765816848384/1208046928504553483/1416697838007488572") : "this Discord message";
                : " for details.";
            }
        }
    }
}

pub(crate) fn s2_settings() -> (seed::Settings, serde_json::Map<String, serde_json::Value>) {
    (
        collect![
            format!("password_lock") => json!(true),
            format!("bridge") => json!("dungeons"),
            format!("bridge_rewards") => json!(3),
            format!("trials") => json!(0),
            format!("shuffle_ganon_bosskey") => json!("remove"),
            format!("open_forest") => json!("open"),
            format!("open_kakariko") => json!("open"),
            format!("open_door_of_time") => json!(true),
            format!("zora_fountain") => json!("open"),
            format!("gerudo_fortress") => json!("fast"),
            format!("dungeon_shortcuts_choice") => json!("choice"),
            format!("dungeon_shortcuts") => json!([
                "Deku Tree",
                "Jabu Jabus Belly",
                "Forest Temple",
                "Water Temple",
                "Shadow Temple",
                "Spirit Temple",
            ]),
            format!("starting_age") => json!("random"),
            format!("shuffle_dungeon_entrances") => json!("simple"),
            format!("spawn_positions") => json!([
                "child",
                "adult",
            ]),
            format!("free_bombchu_drops") => json!(false),
            format!("shopsanity") => json!("4"),
            format!("shuffle_scrubs") => json!("low"),
            format!("adult_trade_start") => json!([
                "Prescription",
                "Eyeball Frog",
                "Eyedrops",
                "Claim Check",
            ]),
            format!("shuffle_mapcompass") => json!("startwith"),
            format!("shuffle_bosskeys") => json!("vanilla"),
            format!("disabled_locations") => json!([
                "Deku Theater Mask of Truth",
                "Kak 40 Gold Skulltula Reward",
                "Kak 50 Gold Skulltula Reward",
                "GC Deku Scrub Grotto Left",
                "GC Deku Scrub Grotto Center",
                "GC Deku Scrub Grotto Right",
            ]),
            format!("allowed_tricks") => json!([
                "logic_visible_collisions",
                "logic_grottos_without_agony",
                "logic_fewer_tunic_requirements",
                "logic_rusted_switches",
                "logic_man_on_roof",
                "logic_windmill_poh",
                "logic_crater_bean_poh_with_hovers",
                "logic_dc_jump",
                "logic_lens_botw",
                "logic_child_deadhand",
                "logic_forest_vines",
                "logic_lens_shadow",
                "logic_lens_shadow_platform",
                "logic_lens_bongo",
                "logic_lens_spirit",
                "logic_lens_gtg",
                "logic_lens_castle",
            ]),
            format!("starting_inventory") => json!([
                "ocarina",
                "zeldas_letter",
            ]),
            format!("start_with_consumables") => json!(true),
            format!("start_with_rupees") => json!(true),
            format!("skip_reward_from_rauru") => json!(true),
            format!("no_escape_sequence") => json!(true),
            format!("no_guard_stealth") => json!(true),
            format!("no_epona_race") => json!(true),
            format!("skip_some_minigame_phases") => json!(true),
            format!("free_scarecrow") => json!(true),
            format!("fast_bunny_hood") => json!(true),
            format!("ruto_already_f1_jabu") => json!(true),
            format!("chicken_count") => json!(1),
            format!("big_poe_count") => json!(1),
            format!("hint_dist") => json!("useless"),
            format!("misc_hints") => json!([
                "altar",
                "dampe_diary",
                "ganondorf",
                "warp_songs_and_owls",
                "20_skulltulas",
                "30_skulltulas",
                "40_skulltulas",
                "50_skulltulas",
                "unique_merchants",
            ]),
            format!("correct_chest_appearances") => json!("both"),
            format!("correct_potcrate_appearances") => json!("textures_content"),
            format!("damage_multiplier") => json!("ohko"),
            format!("junk_ice_traps") => json!("off"),
            format!("ice_trap_appearance") => json!("junk_only"),
        ],
        collect![
            format!("settings") => json!({
                "starting_items": {
                    "Bottle with Fairy": 1,
                    "Ocarina": 1,
                    "Zeldas Letter": 1,
                },
                "hint_dist_user": {
                    "name":                  "Battle Royale S2",
                    "gui_name":              "Battle Royale S2",
                    "description":           "Battle Royale S2",
                    "add_locations":         [
                        { "location": "Deku Theater Skull Mask", "types": ["always"] },
                        { "location": "DMT Biggoron", "types": ["always"] },
                        { "location": "DMC Deku Scrub", "types": ["always"] },
                    ],
                    "remove_locations":      [
                        { "location": "Sheik at Temple", "types": ["always", "sometimes"] },
                        { "location": "Sheik in Forest", "types": ["sometimes"] },
                        { "location": "Sheik in Crater", "types": ["sometimes"] },
                        { "location": "Sheik in Ice Cavern", "types": ["sometimes"] },
                        { "location": "Sheik at Colossus", "types": ["sometimes"] },
                        { "location": "Song from Royal Familys Tomb", "types": ["sometimes"] },
                        { "location": "Kak Anju as Child", "types": ["sometimes"] },
                        { "location": "Ice Cavern Iron Boots Chest", "types": ["sometimes"] },
                        { "location": "Shadow Temple Freestanding Key", "types": ["sometimes"] },
                        { "location": "Shadow Temple Spike Walls Room", "types": ["dual"] },
                        { "location": "Spirit Temple Adult Lower", "types": ["dual"] },
                        { "location": "Spirit Temple Mirror Shield Chest", "types": ["sometimes"] },
                        { "location": "Spirit Temple Silver Gauntlets Chest", "types": ["sometimes"] },
                        { "location": "Gerudo Training Ground Maze Path Final Chest", "types": ["sometimes"] },
                        { "location": "Water Temple Boss Key Chest", "types": ["sometimes"] },
                        { "location": "Water Temple River Chest", "types": ["sometimes"] },
                        { "location": "Fire Temple Lower Loop", "types": ["dual"] },
                        { "location": "Bottom of the Well Dead Hand Room", "types": ["dual"] },
                        { "location": "GC Pot Freestanding PoH", "types": ["sometimes"] },
                        { "location": "GC Maze Left Chest", "types": ["sometimes"] },
                        { "location": "GV Chest", "types": ["sometimes"] },
                        { "location": "Kak 20 Gold Skulltula Reward", "types": ["sometimes"] },
                        { "location": "Graveyard Royal Familys Tomb Chest", "types": ["sometimes"] },
                        { "location": "HC Great Fairy Reward", "types": ["sometimes"] },
                        { "location": "OGC Great Fairy Reward", "types": ["sometimes"] },
                        { "location": "Deku Theater Rewards", "types": ["dual_always"] },
                        { "location": "GF HBA 1500 Points", "types": ["sometimes"] },
                        { "location": "Market Bombchu Bowling Rewards", "types": ["dual"] },
                        { "location": "ZR Frogs Rewards", "types": ["dual"] },
                        { "location": "Graveyard Dampe Race Rewards", "types": ["dual"] },
                        { "location": "Dodongos Cavern Upper Business Scrubs", "types": ["dual"] },
                    ],
                    "add_items":             [],
                    "remove_items":          [
                        { "item": "Zeldas Lullaby", "types": ["woth", "goal"] },
                    ],
                    "dungeons_woth_limit":   40,
                    "dungeons_barren_limit": 2,
                    "one_hint_per_goal":     true,
                    "named_items_required":  true,
                    "vague_named_items":     false,
                    "use_default_goals":     true,
                    "distribution":          {
                        "trial":           {"order": 1, "weight": 0.0, "fixed": 0, "copies": 2},
                        "always":          {"order": 2, "weight": 0.0, "fixed": 6, "copies": 2},
                        "entrance_always": {"order": 3, "weight": 0.0, "fixed": 0, "copies": 2},
                        "barren":          {"order": 4, "weight": 0.0, "fixed": 4, "copies": 2},
                        "goal":            {"order": 5, "weight": 0.0, "fixed": 5, "copies": 2},
                        "entrance":        {"order": 6, "weight": 0.0, "fixed": 0, "copies": 2},
                        "dual":            {"order": 7, "weight": 0.0, "fixed": 2, "copies": 2},
                        "sometimes":       {"order": 8, "weight": 0.0, "fixed": 3, "copies": 2},
                        "random":          {"order": 9, "weight": 9.0, "fixed": 0, "copies": 2},
                        "item":            {"order": 0, "weight": 0.0, "fixed": 0, "copies": 2},
                        "song":            {"order": 0, "weight": 0.0, "fixed": 0, "copies": 2},
                        "overworld":       {"order": 0, "weight": 0.0, "fixed": 0, "copies": 2},
                        "dungeon":         {"order": 0, "weight": 0.0, "fixed": 0, "copies": 2},
                        "junk":            {"order": 0, "weight": 0.0, "fixed": 0, "copies": 2},
                        "named-item":      {"order": 0, "weight": 0.0, "fixed": 0, "copies": 2},
                        "woth":            {"order": 0, "weight": 0.0, "fixed": 0, "copies": 2},
                    },
                    "groups":                [],
                    "disabled":              [],
                },
            }),
            format!("item_pool") => json!({
                "Nayrus Love": 0,
                "Biggoron Sword": 0,
            }),
        ],
    )
}
