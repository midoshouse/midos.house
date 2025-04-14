use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => Some(html! {
            article {
                p {
                    : "This is an archive of the 1st season of the Mixed Pools tournament, organized by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ".";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://docs.google.com/spreadsheets/d/1EoRh37QOKbTT86Jdo97KnvdJ66Y7oKjbIplwYY8qRYs/edit#gid=130670252") : "Swiss pairings and results, and tiebreaker results";
                    }
                }
            }
        }),
        "2" => Some(html! {
            article {
                p {
                    : "This is the 2nd season of the Mixed Pools tournament, organized by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". Join ";
                    a(href = "https://discord.gg/cpvPMTPZtP") : "the Discord server";
                    : " for details.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://docs.google.com/document/d/1OBzytLL3YFNKpa2ly6U7UEh4irOmhnPHaGI8zGfSrsA/edit") : "Tournament format, rules, and settings";
                    }
                    li {
                        a(href = "https://docs.google.com/spreadsheets/d/1nz43jWsDrTgsnMzdLdXI13l9J6b8xHx9Ycpp8PAv9E8/edit?resourcekey#gid=148749353") : "Swiss pairings and results";
                    }
                }
            }
        }),
        "3" => Some(html! {
            article {
                p {
                    : "This is the 3rd season of the Mixed Pools tournament, organized by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". Join ";
                    a(href = "https://discord.gg/cpvPMTPZtP") : "the Discord server";
                    : " for details.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://docs.google.com/document/d/1OBzytLL3YFNKpa2ly6U7UEh4irOmhnPHaGI8zGfSrsA/edit") : "Tournament format, rules, and settings";
                    }
                    li {
                        a(href = "https://docs.google.com/spreadsheets/d/1DqKrmcLhWwfIRpTMZ-NqGGFma_jgCSkyHyP8kaJg6OA/edit?resourcekey#gid=1885752948") : "Swiss pairings and results";
                    }
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn s2_settings() -> seed::Settings {
    collect![
        format!("user_message") => json!("2nd Mixed Pools Tournament"),
        format!("bridge") => json!("open"),
        format!("bridge_medallions") => json!(2),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("dungeons"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("open_deku") => json!(true),
        format!("open_forest") => json!(true),
        format!("require_gohma") => json!(false),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("zora_fountain") => json!("open"),
        format!("gerudo_fortress") => json!("open"),
        format!("starting_age") => json!("random"),
        format!("shuffle_interior_entrances") => json!("all"),
        format!("shuffle_grotto_entrances") => json!(true),
        format!("shuffle_dungeon_entrances") => json!("all"),
        format!("shuffle_bosses") => json!("full"),
        format!("shuffle_overworld_entrances") => json!(true),
        format!("mix_entrance_pools") => json!([
            "Interior",
            "GrottoGrave",
            "Dungeon",
            "Overworld",
            "Boss",
        ]),
        format!("shuffle_gerudo_valley_river_exit") => json!("balanced"),
        format!("owl_drops") => json!("balanced"),
        format!("warp_songs") => json!("balanced"),
        format!("shuffle_child_spawn") => json!("balanced"),
        format!("shuffle_adult_spawn") => json!("balanced"),
        format!("exclusive_one_ways") => json!(true),
        format!("free_bombchu_drops") => json!(false),
        format!("shopsanity") => json!("4"),
        format!("shuffle_scrubs") => json!("low"),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
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
            "logic_deku_b1_webs_with_bow",
            "logic_visible_collisions",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
        ]),
        format!("starting_items") => json!([
            "ocarina",
            "zeldas_letter",
        ]),
        format!("start_with_consumables") => json!(true),
        format!("start_with_rupees") => json!(true),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("complete_mask_quest") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("correct_chest_appearances") => json!("both"),
        format!("correct_potcrate_appearances") => json!("off"),
        format!("clearer_item_models") => json!(false),
        format!("hint_dist") => json!("mixed_pools"),
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

pub(crate) fn s3_settings() -> seed::Settings {
    collect![
        format!("user_message") => json!("3rd Mixed Pools Tournament"),
        format!("bridge") => json!("open"),
        format!("bridge_medallions") => json!(2),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("dungeons"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("open_deku") => json!(true),
        format!("open_forest") => json!(true),
        format!("require_gohma") => json!(false),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("zora_fountain") => json!("open"),
        format!("gerudo_fortress") => json!("open"),
        format!("dungeon_shortcuts_choice") => json!("choice"),
        format!("dungeon_shortcuts") => json!([
            "Jabu Jabus Belly",
        ]),
        format!("starting_age") => json!("random"),
        format!("shuffle_interior_entrances") => json!("all"),
        format!("shuffle_grotto_entrances") => json!(true),
        format!("shuffle_dungeon_entrances") => json!("all"),
        format!("shuffle_bosses") => json!("full"),
        format!("shuffle_ganon_tower") => json!(true),
        format!("shuffle_overworld_entrances") => json!(true),
        format!("mix_entrance_pools") => json!([
            "Interior",
            "GrottoGrave",
            "Dungeon",
            "Overworld",
            "Boss",
        ]),
        format!("shuffle_gerudo_valley_river_exit") => json!("balanced"),
        format!("owl_drops") => json!("balanced"),
        format!("warp_songs") => json!("balanced"),
        format!("shuffle_child_spawn") => json!("balanced"),
        format!("shuffle_adult_spawn") => json!("balanced"),
        format!("exclusive_one_ways") => json!(true),
        format!("free_bombchu_drops") => json!(false),
        format!("shopsanity") => json!("4"),
        format!("shuffle_scrubs") => json!("low"),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
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
            "logic_deku_b1_webs_with_bow",
            "logic_visible_collisions",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
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
        format!("skip_some_minigame_phases") => json!(true),
        format!("complete_mask_quest") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("correct_chest_appearances") => json!("both"),
        format!("correct_potcrate_appearances") => json!("off"),
        format!("clearer_item_models") => json!([]),
        format!("hint_dist") => json!("mixed_pools"),
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

pub(crate) fn s4_settings() -> seed::Settings {
    collect![
        format!("password_lock") => json!(true),
        format!("user_message") => json!("4th Mixed Pools Tournament"),
        format!("bridge") => json!("open"),
        format!("bridge_medallions") => json!(2),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("dungeons"),
        format!("open_deku") => json!(true),
        format!("open_forest") => json!(true),
        format!("require_gohma") => json!(false),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("zora_fountain") => json!("open"),
        format!("gerudo_fortress") => json!("open"),
        format!("starting_age") => json!("random"),
        format!("shuffle_interior_entrances") => json!("all"),
        format!("shuffle_grotto_entrances") => json!(true),
        format!("shuffle_dungeon_entrances") => json!("all"),
        format!("shuffle_bosses") => json!("full"),
        format!("shuffle_ganon_tower") => json!(true),
        format!("shuffle_overworld_entrances") => json!(true),
        format!("mix_entrance_pools") => json!([
            "Interior",
            "GrottoGrave",
            "Dungeon",
            "Overworld",
            "Boss",
        ]),
        format!("shuffle_gerudo_valley_river_exit") => json!("balanced"),
        format!("owl_drops") => json!("balanced"),
        format!("warp_songs") => json!("balanced"),
        format!("shuffle_child_spawn") => json!("balanced"),
        format!("shuffle_adult_spawn") => json!("balanced"),
        format!("exclusive_one_ways") => json!(true),
        format!("shopsanity") => json!("4"),
        format!("shuffle_scrubs") => json!("low"),
        format!("adult_trade_start") => json!([
            "Prescription",
            "Eyeball Frog",
            "Eyedrops",
            "Claim Check",
        ]),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("enhance_map_compass") => json!(true),
        format!("free_bombchu_drops") => json!(false),
        format!("disabled_locations") => json!([
            "Kak 30 Gold Skulltula Reward",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
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
            "logic_deku_b1_webs_with_bow",
            "logic_visible_collisions",
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
        format!("skip_some_minigame_phases") => json!(true),
        format!("complete_mask_quest") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("hint_dist") => json!("mixed_pools"),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs_and_owls",
            "20_skulltulas",
        ]),
        format!("correct_chest_appearances") => json!("both"),
        format!("minor_items_as_major_chest") => json!([
            "shields",
        ]),
        format!("correct_potcrate_appearances") => json!("off"),
        format!("clearer_item_models") => json!([]),
        format!("blue_fire_arrows") => json!(true),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("junk_only"),
    ]
}
