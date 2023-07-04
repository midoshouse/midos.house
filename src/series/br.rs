use {
    collect_mac::collect,
    rand::prelude::*,
    rocket::response::content::RawHtml,
    rocket_util::html,
    serde_json::{
        Value as Json,
        json,
    },
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        event::{
            Data,
            InfoError,
        },
        lang::Language::{
            English,
            Portuguese,
        },
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => {
            let organizers = data.organizers(&mut *transaction).await?;
            Some(html! {
                article {
                    p(lang = "pt") {
                        : "Bem-vindo à primeira temporada da Copa do Brasil de Ocarina of Time Randomizer, idealizado por ";
                        : Portuguese.join_html(&organizers);
                        : ". Verifique o documento de regras para mais detalhes e ";
                        a(href = "https://discord.gg/hJcttRqFGA") : "entre em nosso Discord";
                        : " para ser atualizado acerca do andamento do torneio!";
                    }
                    p(lang = "en") {
                        : "Welcome to the first season of Copa do Brasil, created by ";
                        : English.join_html(organizers);
                        : ". See the rules document for details and ";
                        a(href = "https://discord.gg/hJcttRqFGA") : "join our Discord";
                        : " to stay tuned about the tournament!";
                    }
                }
            })
        }
        _ => None,
    })
}

pub(crate) fn s1_settings() -> serde_json::Map<String, Json> {
    let starting_song = ["minuet", "bolero", "serenade", "requiem", "nocturne", "prelude"].choose(&mut thread_rng()).unwrap();
    collect![
        format!("user_message") => json!("Copa do Brasil"),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("remove"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("starting_age") => json!("random"),
        format!("spawn_positions") => json!([
            "child",
            "adult",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("disabled_locations") => json!([
            "Song from Impa",
            "Deku Theater Mask of Truth",
        ]),
        format!("allowed_tricks") => json!([
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
        format!("starting_equipment") => json!([
            "deku_shield",
        ]),
        format!("starting_songs") => json!([
            starting_song,
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "zeldas_letter",
            "farores_wind",
        ]),
        format!("start_with_consumables") => json!(true),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("fast_bunny_hood") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("correct_chest_appearances") => json!("both"),
        format!("correct_potcrate_appearances") => json!("textures_content"),
        format!("hint_dist_user") => json!({
            "name":                  "tournament",
            "gui_name":              "Tournament",
            "description":           "Hint Distribution for the S6 Tournament. 5 Goal hints, 7 Sometimes hints, 8 Always hints (including Skull Mask and Sheik in Kakariko).",
            "add_locations":         [
                { "location": "Deku Theater Skull Mask", "types": ["always"] },
                { "location": "Sheik in Kakariko", "types": ["always"] },
            ],
            "remove_locations":      [
                {"location": "Sheik in Crater", "types": ["sometimes"] },
                {"location": "Song from Royal Familys Tomb", "types": ["sometimes"] },
                {"location": "Sheik in Forest", "types": ["sometimes"] },
                {"location": "Sheik at Temple", "types": ["sometimes"] },
                {"location": "Sheik at Colossus", "types": ["sometimes"] },
                {"location": "Sheik in Ice Cavern", "types": ["sometimes"] },
                {"location": "GF HBA 1500 Points", "types": ["sometimes"] },
                {"location": "GC Maze Left Chest", "types": ["sometimes"] },
                {"location": "GV Chest", "types": ["sometimes"] },
                {"location": "Graveyard Royal Familys Tomb Chest", "types": ["sometimes"] },
                {"location": "Water Temple River Chest", "types": ["sometimes"] },
                {"location": "Gerudo Training Ground Maze Path Final Chest", "types": ["sometimes"] },
                {"location": "Spirit Temple Silver Gauntlets Chest", "types": ["sometimes"] },
                {"location": "Spirit Temple Mirror Shield Chest", "types": ["sometimes"] },
                {"location": "Ice Cavern Iron Boots Chest", "types": ["sometimes"] },
                {"location": "Ganons Castle Shadow Trial Golden Gauntlets Chest", "types": ["sometimes"] },
                {"location": "Graveyard Dampe Race Rewards", "types": ["dual"] },
                {"location": "ZR Frogs Rewards", "types": ["dual"] },
                {"location": "Deku Theater Rewards", "types": ["dual"] },
                {"location": "Spirit Temple Child Top", "types": ["dual"] },
                {"location": "Spirit Temple Adult Lower", "types": ["dual"] },
                {"location": "Ganons Castle Spirit Trial Chests", "types": ["dual"] },
            ],
            "add_items":             [],
            "remove_items":          [
                { "item": "Zeldas Lullaby", "types": ["goal"] },
            ],
            "dungeons_barren_limit": 1,
            "named_items_required":  true,
            "vague_named_items":     false,
            "use_default_goals":     true,
            "distribution":          {
                "trial":           {"order": 1, "weight": 0.0, "fixed":   0, "copies": 2},
                "entrance_always": {"order": 2, "weight": 0.0, "fixed":   0, "copies": 2},
                "always":          {"order": 3, "weight": 0.0, "fixed":   0, "copies": 2},
                "goal":            {"order": 4, "weight": 0.0, "fixed":   5, "copies": 2},
                "barren":          {"order": 5, "weight": 0.0, "fixed":   3, "copies": 2},
                "entrance":        {"order": 6, "weight": 0.0, "fixed":   4, "copies": 2},
                "dual":            {"order": 7, "weight": 0.0, "fixed":   3, "copies": 2},
                "sometimes":       {"order": 8, "weight": 0.0, "fixed": 100, "copies": 2},
                "random":          {"order": 9, "weight": 9.0, "fixed":   0, "copies": 2},
                "item":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "song":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "overworld":       {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "dungeon":         {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "junk":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "named-item":      {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "woth":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "dual_always":     {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
                "important_check": {"order": 0, "weight": 0.0, "fixed":   0, "copies": 0},
            },
        }),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs_and_owls",
            "30_skulltulas",
            "40_skulltulas",
            "50_skulltulas",
        ]),
        format!("blue_fire_arrows") => json!(true),
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
