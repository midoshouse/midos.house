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
        "8" => Some(html! {
            article {
                p {
                    : "Season 8 of the Scrubs tournament is organized by Froppy, picks, ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". ";
                    a(href = "https://discord.gg/hpHngEY") : "Join the Discord server";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

#[derive(Deserialize)]
pub(crate) struct Qualifiers {
    pub(crate) qualifiers: Vec<Qualifier>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Qualifier {
    pub(crate) id: Uuid,
    pub(crate) number: u32,
    pub(crate) start_date: DateTime<Utc>,
    pub(crate) end_date: Option<DateTime<Utc>>,
    pub(crate) racetime_room_url: Option<Url>,
}

#[derive(Deserialize)]
pub(crate) struct Monitor {
    pub(crate) monitor: Option<User>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct User {
    pub(crate) racetime_url: Url,
}

#[derive(Debug, thiserror::Error, IsNetworkError)]
pub(crate) enum ImportError {
    #[error(transparent)] Calendar(#[from] cal::Error),
    #[error(transparent)] #[is_network_error = false] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("HTTP error{}: {}", if let Some(url) = .0.url() { format!(" at {url}") } else { String::default() }, .0)]
    Http(#[from] reqwest::Error),
}

pub(crate) async fn import(global: &GlobalState, transaction: &mut Transaction<'_, Postgres>, event: &Data<'_>, scrubs_id: Uuid) -> Result<(), ImportError> {
    let mut races = Vec::default();
    for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE series = $1 AND event = $2"#, event.series as _, &event.event).fetch_all(&mut **transaction).await? {
        races.push(Race::from_id(&mut *transaction, &global.http_client, id).await?);
    }
    let Qualifiers { qualifiers } = global.http_client.get("https://scrubs-tournament-mgmt-web-gamma.vercel.app/api/v1/qualifiers")
        .query(&[("tournamentId", scrubs_id)])
        .header("x-api-key", &global.config.scrubs_api_key)
        .send().await?
        .detailed_error_for_status().await?
        .json_with_text_in_error().await?;
    for qualifier in qualifiers {
        let mut new_race = Race {
            id: Id::dummy(),
            series: event.series,
            event: event.event.to_string(),
            source: cal::Source::Scrubs { id: qualifier.id },
            entrants: Entrants::Open,
            phase: Some(format!("Live Qualifier")),
            round: Some(qualifier.number.to_string()),
            game: None,
            scheduling_thread: None,
            schedule: RaceSchedule::Live {
                start: qualifier.start_date,
                end: qualifier.end_date,
                room: qualifier.racetime_room_url,
            },
            schedule_updated_at: None,
            fpa_invoked: false,
            draft: None,
            seed: seed::Data::default(), //TODO get from Scrubs API
            video_urls: HashMap::default(),
            restreamers: HashMap::default(),
            commentators: HashMap::default(),
            trackers: HashMap::default(),
            last_edited_by: None,
            last_edited_at: None,
            ignored: false, //TODO check `status` field from Scrubs API? (what are the possible values?)
            schedule_locked: false,
            notified: false,
            async_notified1: false,
            async_notified2: false,
            async_notified3: false,
        };
        if let Some(race) = races.iter_mut().find(|race| if let cal::Source::Scrubs { id } = race.source { id == qualifier.id } else { false }) {
            if !race.schedule_locked {
                let is_upcoming = !race.has_any_room(); // stop automatically updating certain fields once a room is open
                *race = Race {
                    id: race.id,
                    schedule: if is_upcoming { new_race.schedule } else { mem::take(&mut race.schedule) },
                    schedule_updated_at: race.schedule_updated_at,
                    seed: mem::take(&mut race.seed),
                    video_urls: mem::take(&mut race.video_urls),
                    restreamers: mem::take(&mut race.restreamers),
                    last_edited_at: race.last_edited_at,
                    last_edited_by: race.last_edited_by,
                    notified: race.notified,
                    async_notified1: race.async_notified1,
                    async_notified2: race.async_notified2,
                    async_notified3: race.async_notified3,
                    ..new_race //TODO refactor to default to existing race and only update fields derived from Scrubs API
                };
            }
            race
        } else {
            new_race.id = Id::<Races>::new(&mut *transaction).await?;
            races.push(new_race);
            races.last_mut().expect("just pushed")
        }.save(&mut *transaction).await?;
    }
    Ok(())
}

pub(crate) fn s5_settings() -> seed::Settings {
    collect![
        format!("bridge") => json!("dungeons"),
        format!("bridge_hearts") => json!(10),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("remove"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("enhance_map_compass") => json!(true),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("dungeon_shortcuts") => json!([
            "Dodongos Cavern",
            "Jabu Jabus Belly",
            "Forest Temple",
            "Fire Temple",
            "Water Temple",
            "Shadow Temple",
            "Spirit Temple",
        ]),
        format!("starting_age") => json!("random"),
        format!("empty_dungeons_mode") => json!("count"),
        format!("empty_dungeons_count") => json!(3),
        format!("spawn_positions") => json!([
            "child",
            "adult",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("disabled_locations") => json!([
            "Sheik in Ice Cavern",
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
            "GF HBA 1500 Points",
        ]),
        format!("allowed_tricks") => json!([
            "logic_grottos_without_agony",
            "logic_fewer_tunic_requirements",
            "logic_rusted_switches",
            "logic_man_on_roof",
            "logic_windmill_poh",
            "logic_crater_bean_poh_with_hovers",
            "logic_deku_b1_skip",
            "logic_dc_jump",
            "logic_lens_botw",
            "logic_child_deadhand",
            "logic_forest_vines",
            "logic_lens_shadow",
            "logic_lens_spirit",
            "logic_lens_gtg",
            "logic_lens_castle",
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
        format!("correct_potcrate_appearances") => json!("off"),
        format!("hint_dist") => json!("scrubs"),
        format!("misc_hints") => json!([
            "ganondorf",
            "20_skulltulas",
            "30_skulltulas",
        ]),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("adult_trade_start") => json!([
            "Claim Check",
        ]),
    ]
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

pub(crate) fn s7_settings() -> seed::Settings {
    collect![
        format!("password_lock") => json!(true),
        format!("bridge") => json!("dungeons"),
        format!("trials") => json!(0),
        format!("hint_dist") => json!("scrubs"),
        format!("misc_hints") => json!([
            "ganondorf",
            "20_skulltulas",
            "30_skulltulas",
            "warp_songs_and_owls",
            "big_poes",
        ]),
        format!("open_forest") => json!("closed_deku"),
        format!("starting_age") => json!("random"),
        format!("user_message") => json!("Scrub Tournament"),
        format!("big_poe_count") => json!(1),
        format!("chicken_count") => json!(3),
        format!("no_epona_race") => json!(true),
        format!("open_kakariko") => json!("open"),
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
        format!("junk_ice_traps") => json!("off"),
        format!("starting_songs") => json!([
            "prelude",
        ]),
        format!("fast_bunny_hood") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("spawn_positions") => json!([
            "child",
            "adult",
        ]),
        format!("blue_fire_arrows") => json!(true),
        format!("fast_shadow_boat") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("adult_trade_start") => json!([
            "Claim Check",
        ]),
        format!("open_door_of_time") => json!("open"),
        format!("tcg_requires_lens") => json!(true),
        format!("disabled_locations") => json!([
            "Sheik at Temple",
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
            "GF HBA 1500 Points",
            "Ganons Castle Light Trial First Left Chest",
            "Ganons Castle Light Trial Second Left Chest",
            "Ganons Castle Light Trial Third Left Chest",
            "Ganons Castle Light Trial First Right Chest",
            "Ganons Castle Light Trial Second Right Chest",
            "Ganons Castle Light Trial Third Right Chest",
            "Ganons Castle Light Trial Invisible Enemies Chest",
            "Ganons Castle Light Trial Lullaby Chest",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("no_escape_sequence") => json!(true),
        format!("scarecrow_behavior") => json!("free"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("starting_equipment") => json!([
            "deku_shield",
            "hylian_shield",
            "defense",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "farores_wind",
            "lens",
            "zeldas_letter",
        ]),
        format!("empty_dungeons_mode") => json!("count"),
        format!("enhance_map_compass") => json!(true),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("empty_dungeons_count") => json!(3),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("shuffle_ganon_bosskey") => json!("remove"),
        format!("skip_reward_from_rauru") => json!(true),
        format!("start_with_consumables") => json!(true),
        format!("correct_chest_appearances") => json!("both"),
        format!("skip_some_minigame_phases") => json!(true),
        format!("key_appearance_match_dungeon") => json!(true),
    ]
}

pub(crate) fn s8_settings() -> seed::Settings {
    collect![
        format!("user_message") => json!("Scrub Tournament"),
        format!("password_lock") => json!(true),
        format!("bridge") => json!("dungeons"),
        format!("bridge_rewards") => json!(7),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("dungeons"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!("open"),
        format!("gerudo_fortress") => json!("fast"),
        format!("starting_age") => json!("random"),
        format!("empty_dungeons_mode") => json!("count"),
        format!("empty_dungeons_count") => json!(3),
        format!("spawn_positions") => json!([
            "child",
            "adult",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("adult_trade_start") => json!([
            "Claim Check",
        ]),
        format!("shuffle_map") => json!("startwith"),
        format!("shuffle_compass") => json!("startwith"),
        format!("enhance_map_compass") => json!([
            "map_mq",
            "compass_reward",
        ]),
        format!("disabled_locations") => json!([
            "Sheik at Temple",
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
            "hylian_shield",
            "defense",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "farores_wind",
            "lens",
            "zeldas_letter",
        ]),
        format!("starting_songs") => json!([
            "prelude",
        ]),
        format!("add_random_starting_items") => json!(true),
        format!("random_starting_items_exclude") => json!([
            "songs",
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
        format!("scarecrow_behavior") => json!("free"),
        format!("fast_bunny_hood") => json!(true),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("fast_shadow_boat") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("hint_dist") => json!("scrubs"),
        format!("misc_hints") => json!([
            "ganondorf",
            "20_skulltulas",
            "30_skulltulas",
            "warp_songs_and_owls",
            "big_poes",
        ]),
        format!("correct_chest_appearances") => json!("both"),
        format!("key_appearance_match_dungeon") => json!(true),
        format!("blue_fire_arrows") => json!(true),
        format!("tcg_requires_lens") => json!(true),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("junk_only"),
    ]
}
