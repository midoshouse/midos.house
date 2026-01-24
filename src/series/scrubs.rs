use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "5" => Some(html! {
            article {
                p {
                    : "Season 5 of the Scrubs tournament is organized by Froppy, picks, ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". ";
                    a(href = "https://discord.gg/hpHngEY") : "Join the Discord server";
                    : " for details.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://docs.google.com/spreadsheets/d/e/2PACX-1vRtASXFkNaSzqJoFSmjDpU2XfClRdogkRAgTsJ7RSCiZwUwkrXNcjF06fO_I8vMWfchkUKCrACXPmyE/pubhtml?gid=566134238") : "Qualifier scores & offline qualifier times";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/ootrs5minuet") : "Minuet brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/ootrs5bolero") : "Bolero brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/ootrs5serenade") : "Serenade brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/ootrs5nocturne") : "Nocturne brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/ootrs5requiem") : "Requiem brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/ootrs5prelude") : "Prelude brackets";
                    }
                }
            }
        }),
        "6" => Some(html! {
            article {
                p {
                    : "Season 6 of the Scrubs tournament is organized by Froppy, picks, ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". ";
                    a(href = "https://discord.gg/hpHngEY") : "Join the Discord server";
                    : " for details.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://docs.google.com/spreadsheets/d/e/2PACX-1vQ9W-LpqwoWzIIxMZZyFWDl7-MYQ7v_0z2Ntu8aebGxOJRQ4r5LpCap8jjSuyeeVl0Z_SCCGIZn28b5/pubhtml?gid=566134238") : "Qualifier scores & offline qualifier times";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s6Minuet") : "Minuet brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s6Bolero") : "Bolero brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s6Serenade") : "Serenade brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s6Nocturne") : "Nocturne brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s6Requiem") : "Requiem brackets";
                    }
                }
            }
        }),
        "7" => Some(html! {
            article {
                p {
                    : "Season 7 of the Scrubs tournament is organized by Froppy, picks, ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". ";
                    a(href = "https://discord.gg/hpHngEY") : "Join the Discord server";
                    : " for details.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://scrubscentral.challonge.com/s7gM") : "Minuet brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s7gB") : "Bolero brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s7gS") : "Serenade brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s7gN") : "Nocturne brackets";
                    }
                    li {
                        a(href = "https://scrubscentral.challonge.com/s7gR") : "Requiem brackets";
                    }
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn s6_settings() -> seed::Settings {
    collect![
        format!("user_message") => json!("Scrub Tournament"),
        format!("password_lock") => json!(true),
        format!("bridge") => json!("dungeons"),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("remove"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("enhance_map_compass") => json!(true),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("starting_age") => json!("random"),
        format!("empty_dungeons_mode") => json!("count"),
        format!("empty_dungeons_count") => json!(3),
        format!("spawn_positions") => json!([
            "child",
            "adult",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
            "GF HBA 1500 Points",
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
            "logic_lens_spirit",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
        ]),
        format!("starting_songs") => json!([
            "prelude",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "lens",
            "zeldas_letter",
        ]),
        format!("start_with_consumables") => json!(true),
        format!("start_with_rupees") => json!(true),
        format!("starting_hearts") => json!(4),
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
        format!("correct_chest_appearances") => json!("textures"),
        format!("hint_dist") => json!("scrubs"),
        format!("misc_hints") => json!([
            "ganondorf",
            "20_skulltulas",
            "30_skulltulas",
            "warp_songs_and_owls",
        ]),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("adult_trade_start") => json!([
            "Claim Check",
        ]),
    ]
}
