use {
    std::{
        borrow::Cow,
        iter,
    },
    collect_mac::collect,
    futures::stream,
    if_chain::if_chain,
    itertools::Itertools as _,
    ootr_utils::spoiler::HashIcon,
    rocket::{
        response::content::RawHtml,
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::html,
    serde_json::{
        Value as Json,
        json,
    },
    sqlx::{
        Postgres,
        Transaction,
    },
    url::Url,
    uuid::Uuid,
    crate::{
        event::{
            self,
            Data,
            Error,
            InfoError,
            Series,
            StatusContext,
        },
        lang::Language::English,
        seed,
        util::{
            DateTimeFormat,
            Id,
            form_field,
            format_datetime,
            full_form,
        },
    },
};

pub(crate) fn parse_seed_url(seed: &Url) -> Option<Uuid> {
    if_chain! {
        if let Some("triforceblitz.com" | "www.triforceblitz.com") = seed.host_str();
        if let Some(mut path_segments) = seed.path_segments();
        if path_segments.next() == Some("seed");
        if let Some(segment) = path_segments.next();
        if let Ok(uuid) = Uuid::parse_str(segment);
        if path_segments.next().is_none();
        then {
            Some(uuid)
        } else {
            None
        }
    }
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2" => Some(html! {
            article {
                p {
                    : "This is the 2nd season of the Triforce Blitz tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1p8HAwWsjsLW7tjfDl2SK-yQ35pVqbAS9GB72bkOIDFI/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn qualifier_async_rules() -> RawHtml<String> {
    html! {
        p : "Rules:";
        ol {
            li : "You must start the seed within 15 minutes of obtaining it and submit your time within 10 minutes of finishing. Any additional time taken will be added to your final time. If technical difficulties arise with obtaining the seed/submitting your time, please DM one of the Triforce Blitz Tournament Organizers to get it sorted out. (Discord role “Triforce Blitz Organisation” for pings)";
            li : "If you obtain a seed but do not submit a finish time before submissions close, it will count as a forfeit.";
            li {
                : "Requesting the seed for async will make you ";
                strong : "ineligible";
                : " to participate in the live qualifier on April 8th.";
            }
            li {
                : "To avoid accidental spoilers, the qualifier async ";
                strong : "CANNOT";
                : " be streamed. You must local record and upload to YouTube as an unlisted video.";
            }
            li {
                : "This should be run like an actual race. In the event of a technical issue, you are allowed to invoke the ";
                a(href = "https://docs.google.com/document/d/1BbvHJF8vtyrte76jpoCVQBTy9MYStpN3vr2PLdiCIMk/edit") : "Fair Play Agreement";
                : " and have up to a 15 minute time where you can try to catch back up. If you do this, you must fill out the appropriate field when submitting your time so it can be authenticated.";
            }
        }
    }
}

pub(crate) async fn status(transaction: &mut Transaction<'_, Postgres>, csrf: Option<&CsrfToken>, data: &Data<'_>, team_id: Option<Id>, ctx: &mut StatusContext<'_>) -> Result<RawHtml<String>, Error> {
    Ok(if let Some(async_kind) = data.active_async(&mut *transaction, team_id).await? {
        let async_row = sqlx::query!(r#"SELECT file_stem AS "file_stem!", hash1 AS "hash1: HashIcon", hash2 AS "hash2: HashIcon", hash3 AS "hash3: HashIcon", hash4 AS "hash4: HashIcon", hash5 AS "hash5: HashIcon" FROM asyncs WHERE series = $1 AND event = $2 AND kind = $3"#, data.series as _, &data.event, async_kind as _).fetch_one(&mut *transaction).await?;
        let team_row = if let Some(team_id) = team_id {
            sqlx::query!(r#"SELECT requested AS "requested!", submitted FROM async_teams WHERE team = $1 AND KIND = $2 AND requested IS NOT NULL"#, team_id as _, async_kind as _).fetch_optional(&mut *transaction).await?
        } else {
            None
        };
        if let Some(team_row) = team_row {
            if team_row.submitted.is_some() {
                if data.is_started(&mut *transaction).await? {
                    //TODO get this entrant's known matchup(s)
                    html! {
                        p : "Please schedule your matches using Discord threads in the scheduling channel.";
                    }
                } else {
                    //TODO if any vods are still missing, show form to add them
                    html! {
                        p : "Waiting for the start of the tournament and round 1 pairings. Keep an eye out for an announcement on Discord."; //TODO include start date?
                    }
                }
            } else {
                let seed = seed::Data {
                    file_hash: match (async_row.hash1, async_row.hash2, async_row.hash3, async_row.hash4, async_row.hash5) {
                        (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                        (None, None, None, None, None) => None,
                        _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
                    },
                    files: seed::Files::MidosHouse { file_stem: Cow::Owned(async_row.file_stem), locked_spoiler_log_path: None },
                };
                let seed_table = seed::table(stream::iter(iter::once(seed)), false).await?;
                let ctx = ctx.take_submit_async();
                let mut errors = ctx.errors().collect_vec();
                html! {
                    div(class = "info") {
                        p {
                            : "You requested the qualifier async on ";
                            : format_datetime(team_row.requested, DateTimeFormat { long: true, running_text: true });
                            : ".";
                        };
                        : seed_table;
                        p : "After playing the async, fill out the form below.";
                        : full_form(uri!(event::submit_async(data.series, &*data.event)), csrf, html! {
                            : form_field("pieces", &mut errors, html! {
                                label(for = "pieces") : "Number of Triforce Pieces found:";
                                input(type = "number", min = "0", max = "3", name = "pieces", value? = ctx.field_value("pieces"));
                            });
                            : form_field("time1", &mut errors, html! {
                                label(for = "time1") : "Time at which you found the most recent piece:";
                                input(type = "text", name = "time1", value? = ctx.field_value("time1")); //TODO h:m:s fields?
                                label(class = "help") : "(If you did not find any, leave this field blank.)";
                            });
                            : form_field("vod1", &mut errors, html! {
                                label(for = "vod1") : "VoD:";
                                input(type = "text", name = "vod1", value? = ctx.field_value("vod1"));
                                label(class = "help") : "(You must submit a link to an unlisted YouTube video upload. The link to a YouTube video becomes available as soon as you begin the upload process.)";
                            });
                            : form_field("fpa", &mut errors, html! {
                                label(for = "fpa") {
                                    : "If you would like to invoke the ";
                                    a(href = "https://docs.google.com/document/d/1BbvHJF8vtyrte76jpoCVQBTy9MYStpN3vr2PLdiCIMk/edit") : "Fair Play Agreement";
                                    : ", describe the break(s) you took below. Include the reason, starting time, and duration.";
                                }
                                textarea(name = "fpa"); //TODO fill from form context
                            });
                        }, errors, "Submit");
                    }
                }
            }
        } else {
            unimplemented!() //TODO redirect to enter page
        }
    } else {
        html! {
            p {
                : "To enter this tournament, play the qualifier, either live on ";
                : format_datetime(data.start(&mut *transaction).await?.expect("missing start time for tfb/2"), DateTimeFormat { long: true, running_text: true });
                : " or async starting on April 2.";
            }
        }
    })
}

pub(crate) fn jr_settings() -> serde_json::Map<String, Json> {
    collect![
        format!("user_message") => json!("Triforce Blitz - Jabu's Revenge"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("bridge") => json!("dungeons"),
        format!("bridge_rewards") => json!(2),
        format!("dungeon_shortcuts_choice") => json!("choice"),
        format!("dungeon_shortcuts") => json!([
            "Jabu Jabus Belly",
            "Forest Temple",
        ]),
        format!("triforce_blitz") => json!(true),
        format!("triforce_blitz_jabus_revenge") => json!(true),
        format!("free_bombchu_drops") => json!(false),
        format!("trials") => json!(0),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("fast_bunny_hood") => json!(true),
        format!("start_with_consumables") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("shuffle_dungeon_entrances") => json!(false),
        format!("tokensanity") => json!("dungeons"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("shuffle_bosskeys") => json!("remove"),
        format!("shuffle_ganon_bosskey") => json!("remove"),
        format!("disabled_locations") => json!([
            "Deku Theater Skull Mask",
            "Deku Theater Mask of Truth",
            "Kak 30 Gold Skulltula Reward",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
            "ZR Frogs Ocarina Game",
            "Jabu Jabus Belly Boomerang Chest",
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
            "logic_water_gold_scale_no_entry",
        ]),
        format!("adult_trade_start") => json!([
            "Claim Check",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
        ]),
        format!("starting_inventory") => json!([
            "ocarina",
            "zeldas_letter",
        ]),
        format!("ocarina_songs") => json!(false),
        format!("correct_chest_appearances") => json!("both"),
        format!("minor_items_as_major_chest") => json!("bombchus"),
        format!("hint_dist") => json!("triforce-blitz-s2"),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs_and_owls",
            "20_skulltulas",
        ]),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("junk_ice_traps") => json!("off"),
        format!("starting_age") => json!("adult"),
    ]
}
