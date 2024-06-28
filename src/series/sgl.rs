use {
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
        "2023onl" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2023 SpeedGaming Live online OoTR tournament, organized by ";
                        : English.join_html(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://docs.google.com/document/d/1EACqBl8ZOreD6xT5jQ2HrdLOnpBpKyjS3FUYK8XFeqg/edit") : "Rules document";
                            }
                            li {
                                a(href = "https://sglive.speedgaming.org/") : "Main SGL event page";
                            }
                            li {
                                a(href = "https://discord.gg/YGzQsUp") : "Main SGL Discord";
                            }
                        }
                    }
                }
            })
        }
        "2023live" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2023 SpeedGaming Live in-person OoTR tournament, organized by ";
                        : English.join_html(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://docs.google.com/document/d/1EACqBl8ZOreD6xT5jQ2HrdLOnpBpKyjS3FUYK8XFeqg/edit") : "Rules document";
                            }
                            li {
                                a(href = "https://sglive.speedgaming.org/") : "Main SGL event page";
                            }
                            li {
                                a(href = "https://matcherino.com/t/sglive23") : "Matcherino";
                            }
                            li {
                                a(href = "https://discord.gg/YGzQsUp") : "Main SGL Discord";
                            }
                        }
                    }
                }
            })
        }
        "2024onl" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2024 SpeedGaming Live online OoTR tournament, organized by ";
                        : English.join_html(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://docs.google.com/document/d/1I0IcnGMqKr3QaCgg923SR_SxVu0iytIA_lOhN2ybj9w/edit") : "Rules document";
                            }
                            li {
                                a(href = "https://sglive.speedgaming.org/") : "Main SGL event page";
                            }
                            li {
                                a(href = "https://discord.gg/YGzQsUp") : "Main SGL Discord";
                            }
                        }
                    }
                }
            })
        }
        "2024live" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2024 SpeedGaming Live in-person OoTR tournament, organized by ";
                        : English.join_html(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://docs.google.com/document/d/1I0IcnGMqKr3QaCgg923SR_SxVu0iytIA_lOhN2ybj9w/edit") : "Rules document";
                            }
                            li {
                                a(href = "https://sglive.speedgaming.org/") : "Main SGL event page";
                            }
                            li {
                                a(href = "https://matcherino.com/t/sglive24") : "Matcherino";
                            }
                            li {
                                a(href = "https://discord.gg/YGzQsUp") : "Main SGL Discord";
                            }
                        }
                    }
                }
            })
        }
        _ => None,
    })
}

pub(crate) fn settings_2023() -> serde_json::Map<String, Json> {
    collect![
        format!("user_message") => json!("SGL 2023"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("bridge") => json!("stones"),
        format!("trials") => json!(0),
        format!("starting_age") => json!("adult"),
        format!("empty_dungeons_mode") => json!("rewards"),
        format!("empty_dungeons_rewards") => json!([
            "Light Medallion",
        ]),
        format!("spawn_positions") => json!([
            "child",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("shuffle_ganon_bosskey") => json!("on_lacs"),
        format!("enhance_map_compass") => json!(true),
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
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "zeldas_letter",
            "farores_wind",
        ]),
        format!("starting_songs") => json!([
            "prelude",
        ]),
        format!("start_with_consumables") => json!(true),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("fast_bunny_hood") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("correct_chest_appearances") => json!("both"),
        format!("minor_items_as_major_chest") => json!("bombchus"),
        format!("correct_potcrate_appearances") => json!("textures_content"),
        format!("hint_dist") => json!("sgl2023"),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs",
            "20_skulltulas",
            "30_skulltulas",
        ]),
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

pub(crate) fn settings_2024() -> serde_json::Map<String, Json> {
    collect![
        format!("user_message") => json!("SGL 2024 (Experimental)"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("bridge") => json!("stones"),
        format!("trials") => json!(0),
        format!("starting_age") => json!("adult"),
        format!("empty_dungeons_mode") => json!("rewards"),
        format!("empty_dungeons_rewards") => json!([
            "Light Medallion",
        ]),
        format!("spawn_positions") => json!([
            "child",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("shuffle_ganon_bosskey") => json!("on_lacs"),
        format!("enhance_map_compass") => json!(true),
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
            "logic_deku_b1_skip",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "zeldas_letter",
            "lens",
            "farores_wind",
        ]),
        format!("starting_songs") => json!([
            "prelude",
        ]),
        format!("start_with_consumables") => json!(true),
        format!("skip_reward_from_rauru") => json!(true),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("fast_bunny_hood") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("correct_chest_appearances") => json!("both"),
        format!("minor_items_as_major_chest") => json!([
            "bombchus",
        ]),
        format!("hint_dist") => json!("sgl2024"),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs",
            "20_skulltulas",
            "30_skulltulas",
        ]),
        format!("blue_fire_arrows") => json!(true),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("adult_trade_start") => json!([
            "Claim Check",
        ]),
    ]
}
