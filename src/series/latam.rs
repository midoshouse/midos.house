use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2025" => {
            let organizers = data.organizers(&mut *transaction).await?;
            Some(html! {
                article {
                    p(lang = "pt") {
                        : "Bem-vindos à primeira temporada da Copa Latinoamerica 2025! O torneio está sendo organizado por ";
                        : Portuguese.join_html_opt(&organizers);
                        : ". ";
                        a(href = "https://discord.gg/hRKZacDcTR") : "Junte-se ao nosso servidor do Discord";
                        : " para mais detalhes!";
                    }
                    p(lang = "es") {
                        : "Bienvenido a la primera temporada de la Copa Latinoamérica 2025. El torneo fue creado por ";
                        : Spanish.join_html_opt(&organizers);
                        : ". ";
                        a(href = "https://discord.gg/hRKZacDcTR") : "Únete a nuestro servidor de Discord";
                        : " para más informaciónes!";
                    }
                    p(lang = "en") {
                        : "Welcome to the first season of Copa Latinoamerica 2025! The tournament is organized by ";
                        : English.join_html_opt(organizers);
                        : ". Unfortunately only players from South America, Central America and Mexico can join the tournament. ";
                        a(href = "https://discord.gg/hRKZacDcTR") : "Join our Discord server";
                        : " for more details!";
                    }
                    h2(id = "recursos-resources") {
                        span(lang = "pt") : "Recursos";
                        : "/";
                        span(lang = "en") : "Resources";
                    }
                    ul {
                        li(lang = "pt") {
                            a(href = "https://docs.google.com/document/d/e/2PACX-1vSB2GudpNfGRM86PykriwYfKMXe8REukSM2yQB9BT-2cxib0fUq8WG8POPnIs53NHRwC45z92hTrcbm/pub") : "Documento de regras (PT-BR)";
                        }
                        li(lang = "es") {
                            a(href = "https://docs.google.com/document/d/e/2PACX-1vQcGvTa3OmvluwZiyRVzrdLGHW5_FQLQi08pbMIRY16bs68bJt2Zw60tQq-_um1JPZpNJBOpJLx9JhL/pub") : "Documento de reglas (ESP)";
                        }
                        li {
                            a(href = "https://docs.google.com/spreadsheets/d/1wJI5CPb6LRTlERnZ5lUYA146Ugyt__dSyzJ2DDGKb_M/edit") {
                                span(lang = "pt") : "Planilha de asyncs";
                                : "/";
                                span(lang = "en") : "Async sheet";
                            }
                        }
                        li(lang = "pt") {
                            a(href = "https://drive.google.com/drive/folders/1kI8HlW9FoP2iyfS0CWNmCbM4oENjbW0F") : "Gossip Stone Tracker (GST) para o torneio";
                        }
                    }
                }
            })
        }
        _ => None,
    })
}

pub(crate) fn settings_2025() -> (seed::Settings, serde_json::Map<String, serde_json::Value>) {
    (
        collect![
            format!("user_message") => json!("Copa Latinoamerica 2025"),
            format!("password_lock") => json!(true),
            format!("trials") => json!(0),
            format!("shuffle_ganon_bosskey") => json!("remove"),
            format!("open_forest") => json!("open"),
            format!("open_kakariko") => json!("open"),
            format!("open_door_of_time") => json!(true),
            format!("gerudo_fortress") => json!("fast"),
            format!("starting_age") => json!("adult"),
            format!("shuffle_dungeon_entrances") => json!("simple"),
            format!("spawn_positions") => json!([
                "child",
            ]),
            format!("free_bombchu_drops") => json!(false),
            format!("shopsanity") => json!("2"),
            format!("special_deal_price_distribution") => json!("uniform"),
            format!("adult_trade_start") => json!([
                "Claim Check",
            ]),
            format!("shuffle_mapcompass") => json!("startwith"),
            format!("enhance_map_compass") => json!(true),
            format!("disabled_locations") => json!([
                "Deku Theater Mask of Truth",
            ]),
            format!("allowed_tricks") => json!([
                "logic_visible_collisions",
                "logic_grottos_without_agony",
                "logic_fewer_tunic_requirements",
                "logic_rusted_switches",
                "logic_man_on_roof",
                "logic_windmill_poh",
                "logic_crater_bean_poh_with_hovers",
                "logic_deku_b1_webs_with_bow",
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
                "farores_wind",
                "zeldas_letter",
            ]),
            format!("start_with_consumables") => json!(true),
            format!("start_with_rupees") => json!(true),
            format!("skip_reward_from_rauru") => json!(true),
            format!("no_escape_sequence") => json!(true),
            format!("no_guard_stealth") => json!(true),
            format!("no_epona_race") => json!(true),
            format!("scarecrow_behavior") => json!("fast"),
            format!("fast_bunny_hood") => json!(true),
            format!("auto_equip_masks") => json!(true),
            format!("ruto_already_f1_jabu") => json!(true),
            format!("fast_shadow_boat") => json!(true),
            format!("big_poe_count") => json!(1),
            format!("hint_dist") => json!("tournament"),
            format!("misc_hints") => json!([
                "altar",
                "ganondorf",
                "warp_songs_and_owls",
                "30_skulltulas",
                "40_skulltulas",
                "50_skulltulas",
            ]),
            format!("correct_chest_appearances") => json!("both"),
            format!("chest_textures_specific") => json!([
                "major",
                "bosskeys",
                "keys",
            ]),
            format!("correct_potcrate_appearances") => json!("off"),
            format!("key_appearance_match_dungeon") => json!(true),
            format!("potcrate_textures_specific") => json!([]),
            format!("blue_fire_arrows") => json!(true),
            format!("tcg_requires_lens") => json!(true),
            format!("ice_trap_appearance") => json!("junk_only"),
        ],
        collect![
            format!("settings") => json!({
                "hint_dist_user": {
                    "name":                  "CLA",
                    "gui_name":              "CLA",
                    "description":           "Dicas da Copa Latinamerica 2025. 4 Paths, 2 Foolish (em Temple of Time), 1 Important Check (em Temple of Time), 3 Always (Ocarina of Time Song, Skull Mask e Frogs2), 3 Sometimes, 3 Dual Sometimes. Dicas de 30/40/50 skulltulas na Casa das Skulltulas",
                    "add_locations":         [
                        { "location": "Deku Theater Skull Mask", "types": ["always"] },
                    ],
                    "remove_locations":      [
                        { "location": "Song from Royal Familys Tomb", "types": ["sometimes"] },
                        { "location": "Sheik in Forest", "types": ["sometimes"] },
                        { "location": "Sheik at Temple", "types": ["sometimes"] },
                        { "location": "Sheik in Crater", "types": ["sometimes"] },
                        { "location": "Sheik at Colossus", "types": ["sometimes"] },
                        { "location": "GC Maze Left Chest", "types": ["sometimes"] },
                        { "location": "GV Chest", "types": ["sometimes"] },
                        { "location": "LH Sun", "types": ["sometimes"] },
                        { "location": "Gerudo Training Ground Maze Path Final Chest", "types": ["sometimes"] },
                        { "location": "Fire Temple Megaton Hammer Chest", "types": ["sometimes"] },
                        { "location": "Fire Temple Scarecrow Chest", "types": ["sometimes"] },
                        { "location": "Spirit Temple Silver Gauntlets Chest", "types": ["sometimes"] },
                        { "location": "Spirit Temple Mirror Shield Chest", "types": ["sometimes"] },
                        { "location": "HC Great Fairy Reward", "types": ["sometimes"] },
                        { "location": "OGC Great Fairy Reward", "types": ["sometimes"] },
                        { "location": "Sheik in Ice Cavern", "types": ["sometimes"] },
                        { "location": "Ice Cavern Iron Boots Chest", "types": ["sometimes"] },
                        { "location": "Water Temple River Chest", "types": ["sometimes"] },
                        { "location": "Ganons Castle Shadow Trial Golden Gauntlets Chest", "types": ["sometimes"] },
                        { "location": "Ice Cavern Iron Boots Chest", "types": ["sometimes"] },
                        { "location": "LH Adult Bean Destination Checks", "types": ["dual"] },
                        { "location": "Ganons Castle Spirit Trial Chests", "types": ["dual"] },
                        { "location": "LH Adult Bean Destination Checks", "types": ["dual"] },
                    ],
                    "add_items":             [],
                    "remove_items":          [
                        { "item": "Zeldas Lullaby", "types": ["goal"] },
                        { "item": "Minuet of Forest", "types": ["goal"] },
                        { "item": "Bolero of Fire", "types": ["goal"] },
                        { "item": "Nocturne of Shadow", "types": ["goal"] },
                        { "item": "Requiem of Spirit", "types": ["goal"] },
                    ],
                    "disabled": [
                        "HF (Cow Grotto)",
                        "HC (Storms Grotto)",
                        "HF (Near Market Grotto)",
                        "HF (Southeast Grotto)",
                        "HF (Open Grotto)",
                        "Kak (Open Grotto)",
                        "ZR (Open Grotto)",
                        "KF (Storms Grotto)",
                        "LW (Near Shortcuts Grotto)",
                        "DMT (Storms Grotto)",
                        "DMC (Upper Grotto)",
                    ],
                    "dungeons_barren_limit": 1,
                    "named_items_required":  true,
                    "excluded_goal_categories": ["ganon"],
                    "vague_named_items":     false,
                    "use_default_goals":     true,
                    "distribution":          {
                        "trial":           {"order": 9, "weight": 0.0, "fixed": 0, "copies": 0},
                        "always":          {"order": 2, "weight": 0.0, "fixed": 3, "copies": 2, "remove_stones": [
                            "ToT (Left)",
                            "ToT (Left-Center)",
                            "ToT (Right-Center)",
                        ]},
                        "named-item":      {"order": 3, "weight": 0.0, "fixed": 0, "copies": 0},
                        "goal":            {"order": 4, "weight": 0.0, "fixed": 4, "copies": 2, "remove_stones": [
                            "ToT (Left)",
                            "ToT (Left-Center)",
                            "ToT (Right-Center)",
                        ]},
                        "woth":            {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "barren":          {"order": 5, "weight": 0.0, "fixed": 2, "copies": 1, "priority_stones": [
                            "ToT (Left)",
                            "ToT (Left-Center)",
                        ]},
                        "song":            {"order": 6, "weight": 0.0, "fixed": 0, "copies": 0},
                        "dual":            {"order": 7, "weight": 0.0, "fixed": 3, "copies": 2, "remove_stones": [
                            "ToT (Left)",
                            "ToT (Left-Center)",
                            "ToT (Right-Center)",
                        ]},
                        "sometimes":       {"order": 8, "weight": 0.0, "fixed": 3, "copies": 2, "remove_stones": [
                            "ToT (Left)",
                            "ToT (Left-Center)",
                            "ToT (Right-Center)",
                        ]},
                        "junk":            {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "entrance_always": {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "entrance":        {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "random":          {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "item":            {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "overworld":       {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "dungeon":         {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "dual_always":     {"order": 0, "weight": 0.0, "fixed": 0, "copies": 0},
                        "important_check": {"order": 1, "weight": 0.0, "fixed": 1, "copies": 1, "priority_stones": [
                            "ToT (Right-Center)",
                        ]},
                    },
                },
            }),
            format!("randomized_settings") => json!({}),
            format!("entrances") => json!({
                "KF Outside Deku Tree -> Deku Tree Lobby": {"region": "Deku Tree Lobby", "from": "KF Outside Deku Tree"},
                "Death Mountain -> Dodongos Cavern Beginning": {"region": "Dodongos Cavern Beginning", "from": "Death Mountain"},
                "Zoras Fountain -> Jabu Jabus Belly Beginning": {"region": "Jabu Jabus Belly Beginning", "from": "Zoras Fountain"},
                "SFM Forest Temple Entrance Ledge -> Forest Temple Lobby": {"region": "Forest Temple Lobby", "from": "SFM Forest Temple Entrance Ledge"},
                "DMC Fire Temple Entrance -> Fire Temple Lower": {"region": "Fire Temple Lower", "from": "DMC Fire Temple Entrance"},
                "Lake Hylia -> Water Temple Lobby": {"region": "Water Temple Lobby", "from": "Lake Hylia"},
                "Graveyard Warp Pad Region -> Shadow Temple Entryway": {"region": "Shadow Temple Entryway", "from": "Graveyard Warp Pad Region"},
                "Desert Colossus -> Spirit Temple Lobby": {"region": "Spirit Temple Lobby", "from": "Desert Colossus"},
                "ZF Ice Ledge -> Ice Cavern Beginning": {"region": "Ice Cavern Beginning", "from": "ZF Ice Ledge"},
                "Gerudo Fortress -> Gerudo Training Ground Lobby": {"region": "Gerudo Training Ground Lobby", "from": "Gerudo Fortress"},
                "Kakariko Village -> Bottom of the Well": {"region": "Bottom of the Well", "from": "Kakariko Village"},
            }),
            format!("locations") => json!({}),
            format!("item_pool") => json!({"Progressive Wallet": 2}),
        ],
    )
}
