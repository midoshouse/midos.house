use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2026" => Some(html! {
            article {
                p {
                    : "This is the first Escape from Kakariko tournament, organized by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1ynb8ZRuudEzYcPOT1gOHxFTvaXC6F6lnnciA6jXVhWY/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn settings_2026() -> seed::Settings {
    collect![
        format!("user_message") => json!("Escape from Kakariko"),
        format!("password_lock") => json!(true),
        format!("reachable_locations") => json!("beatable"),
        format!("escape_from_kak") => json!(true),
        format!("triforce_hunt") => json!(true),
        format!("triforce_count_per_world") => json!(3),
        format!("triforce_goal_per_world") => json!(3),
        format!("triforce_blitz_maximum_empty_paths") => json!(0),
        format!("bridge") => json!("stones"),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("triforce"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("enhance_map_compass") => json!([
            "compass_reward",
        ]),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!("open"),
        format!("zora_fountain") => json!("open"),
        format!("gerudo_fortress") => json!("fast"),
        format!("dungeon_shortcuts_choice") => json!("choice"),
        format!("dungeon_shortcuts") => json!([
            "Forest Temple",
        ]),
        format!("empty_dungeons_mode") => json!("specific"),
        format!("empty_dungeons_count") => json!(1),
        format!("spawn_positions") => json!([
            "child",
            "adult",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("shuffle_song_items") => json!("any"),
        format!("shopsanity") => json!("0"),
        format!("tokensanity") => json!("dungeons"),
        format!("shuffle_scrubs") => json!("low"),
        format!("shuffle_freestanding_items") => json!("dungeons"),
        format!("shuffle_pots") => json!("dungeons"),
        format!("shuffle_empty_pots") => json!(true),
        format!("shuffle_crates") => json!("dungeons"),
        format!("shuffle_empty_crates") => json!(true),
        format!("plandomized_locations") => json!({
            "ToT Reward from Rauru": "Light Medallion",
            "Kak Bazaar Item 1": {
                "item": "Buy Hylian Shield",
                "price": 80,
            },
            "Kak Bazaar Item 2": {
                "item": "Buy Bombs (5) for 35 Rupees",
                "price": 35,
            },
            "Kak Bazaar Item 3": {
                "item": "Buy Deku Nut (5)",
                "price": 15,
            },
            "Kak Bazaar Item 4": {
                "item": "Buy Arrows (10)",
                "price": 20,
            },
            "Kak Bazaar Item 5": {
                "item": "Buy Goron Tunic",
                "price": 200,
            },
            "Kak Bazaar Item 6": {
                "item": "Buy Deku Stick (1)",
                "price": 10,
            },
            "Kak Bazaar Item 7": {
                "item": "Buy Zora Tunic",
                "price": 300,
            },
            "Kak Bazaar Item 8": {
                "item": "Buy Deku Shield",
                "price": 40,
            },
            "Ganons Castle Light Trial First Left Chest": "Prelude of Light",
            "Ganons Castle Light Trial Second Left Chest": "Eponas Song",
            "Ganons Castle Light Trial Third Left Chest": "Sarias Song",
            "Ganons Castle Light Trial First Right Chest": "Suns Song",
            "Ganons Castle Light Trial Second Right Chest": "Song of Storms",
            "Ganons Castle Light Trial Third Right Chest": "Minuet of Forest",
            "Ganons Castle Light Trial Invisible Enemies Chest": "Bolero of Fire",
            "Ganons Castle Light Trial Lullaby Chest": "Serenade of Water",
            "Ganons Castle Spirit Trial Crystal Switch Chest": "Nocturne of Shadow",
            "Ganons Castle Spirit Trial Invisible Chest": "Requiem of Spirit",
        }),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
            "Song from Impa",
            "Sheik in Kakariko",
            "Kak 10 Gold Skulltula Reward",
            "Kak 20 Gold Skulltula Reward",
            "Kak 30 Gold Skulltula Reward",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
            "Kak Windmill Freestanding PoH",
            "Spirit Temple Silver Gauntlets Chest",
            "Spirit Temple Mirror Shield Chest",
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
            "logic_visible_collisions",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
            "magic",
        ]),
        format!("starting_songs") => json!([
            "song_of_time",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "zeldas_letter",
            "farores_wind",
        ]),
        format!("start_with_consumables") => json!(true),
        format!("skip_reward_from_rauru") => json!(true),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("scarecrow_behavior") => json!("free"),
        format!("fast_bunny_hood") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("song_of_time_changes_age") => json!(true),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("lock_reverse_shadow") => json!(true),
        format!("fast_shadow_boat") => json!(true),
        format!("correct_chest_appearances") => json!("both"),
        format!("chest_textures_specific") => json!([
            "major",
            "bosskeys",
            "keys",
            "tokens",
            "hearts",
        ]),
        format!("correct_potcrate_appearances") => json!("textures_content"),
        format!("hint_dist") => json!("escape-from-kak"),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs_and_owls",
            "10_skulltulas",
            "20_skulltulas",
            "30_skulltulas",
            "40_skulltulas",
            "50_skulltulas",
        ]),
        format!("blue_fire_arrows") => json!(true),
        format!("tcg_requires_lens") => json!(true),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("adult_trade_start") => json!([
            "Prescription",
            "Eyeball Frog",
            "Eyedrops",
            "Claim Check",
        ]),
    ]
}
