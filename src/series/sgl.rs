use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

/// Rate limit once per minute according to DMs with tsigma6
const RATE_LIMIT: Duration = Duration::from_secs(60);

static CACHE: LazyLock<Mutex<(Instant, Schedule)>> = LazyLock::new(|| Mutex::new((Instant::now() + RATE_LIMIT, Schedule::default())));

#[derive(Clone, Deserialize)]
pub(crate) struct RestreamMatch {
    players: Vec<Player>,
    pub(crate) id: i64,
    pub(crate) title: String,
}

impl RestreamMatch {
    pub(crate) async fn matches(&self, transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, race: &Race) -> Result<bool, cal::Error> {
        Ok(if race.phase.as_ref().is_some_and(|phase| phase == "Qualifier") {
            let Some((_, match_round)) = regex_captures!("^Qualifier #([0-9]+)$", &self.title) else { return Ok(false) };
            race.round.as_ref().is_some_and(|race_round| race_round == match_round)
        } else {
            match &race.entrants {
                Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => false,
                Entrants::Two(entrants) => {
                    if self.players.len() == 2 {
                        'permutations: for players in self.players.iter().permutations(2) {
                            for (entrant, player) in entrants.iter().zip_eq(players) {
                                if !player.matches(&mut *transaction, http_client, entrant).await? {
                                    continue 'permutations
                                }
                            }
                            return Ok(true)
                        }
                    }
                    false
                }
                Entrants::Three(entrants) => {
                    if self.players.len() == 3 {
                        'permutations: for players in self.players.iter().permutations(3) {
                            for (entrant, player) in entrants.iter().zip_eq(players) {
                                if !player.matches(&mut *transaction, http_client, entrant).await? {
                                    continue 'permutations
                                }
                            }
                            return Ok(true)
                        }
                    }
                    false
                }
            }
        })
    }
}

impl fmt::Display for RestreamMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.title.is_empty() {
            write!(f, "{}", self.players.iter().map(|player| &player.streaming_from).format(" vs "))
        } else {
            self.title.fmt(f)
        }
    }
}

#[derive(Clone, Deserialize)]
pub(crate) struct RestreamChannel {
    pub(crate) language: Language,
    pub(crate) slug: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Player {
    streaming_from: String,
}

impl Player {
    async fn matches(&self, transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, entrant: &Entrant) -> Result<bool, cal::Error> {
        Ok(match entrant {
            Entrant::MidosHouseTeam(team) => if_chain! {
                if let Ok(member) = team.members(transaction).await?.into_iter().exactly_one();
                if let Some(Some(user_data)) = member.racetime_user_data(http_client).await?;
                if let Some(twitch_name) = user_data.twitch_name;
                then {
                    twitch_name.eq_ignore_ascii_case(&self.streaming_from)
                } else {
                    false
                }
            },
            Entrant::Discord { twitch_username: None, .. } | Entrant::Named { twitch_username: None, .. } => false,
            Entrant::Discord { twitch_username: Some(username), .. } | Entrant::Named { twitch_username: Some(username), .. } => username.eq_ignore_ascii_case(&self.streaming_from),
        })
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Restream {
    pub(crate) match1: Option<RestreamMatch>,
    pub(crate) match2: Option<RestreamMatch>,
    pub(crate) channels: Vec<RestreamChannel>,
    pub(crate) when_countdown: DateTime<Utc>,
}

impl Restream {
    pub(crate) fn matches(&self) -> impl Iterator<Item = &RestreamMatch> {
        self.match1.iter().chain(&self.match2)
    }

    pub(crate) async fn update_race<'a>(&self, db_pool: &PgPool, mut transaction: Transaction<'a, Postgres>, discord_ctx: &DiscordCtx, event: &Data<'_>, cal_event: &mut cal::Event, id: i64) -> Result<Transaction<'a, Postgres>, event::Error> {
        if !cal_event.race.schedule_locked && !cal_event.race.has_any_room() { // don't mess with starting time if room already open; allow configuring vod URLs after the end of a restream
            assert!(matches!(mem::replace(&mut cal_event.race.source, cal::Source::SpeedGaming { id }), cal::Source::Manual | cal::Source::SpeedGaming { id: _ }));
            let schedule_changed = match cal_event.race.schedule {
                RaceSchedule::Live { start, .. } => (start != self.when_countdown).then_some(true),
                _ => Some(false),
            };
            cal_event.race.schedule.set_live_start(self.when_countdown);
            if let Some(was_scheduled) = schedule_changed {
                use {
                    serenity::all::{
                        CreateAllowedMentions,
                        CreateMessage,
                    },
                    crate::discord_bot::*,
                };

                if self.when_countdown - Utc::now() < TimeDelta::minutes(30) {
                    let (http_client, new_room_lock, racetime_host, racetime_config, extra_room_tx, clean_shutdown) = {
                        let data = discord_ctx.data.read().await;
                        (
                            data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone(),
                            data.get::<NewRoomLock>().expect("new room lock missing from Discord context").clone(),
                            data.get::<RacetimeHost>().expect("racetime.gg host missing from Discord context").clone(),
                            data.get::<ConfigRaceTime>().expect("racetime.gg config missing from Discord context").clone(),
                            data.get::<ExtraRoomTx>().expect("extra room sender missing from Discord context").clone(),
                            data.get::<CleanShutdown>().expect("clean shutdown state missing from Discord context").clone(),
                        )
                    };
                    lock!(new_room_lock = new_room_lock; {
                        if let Some((_, msg)) = racetime_bot::create_room(&mut transaction, discord_ctx, &racetime_host, &racetime_config.client_id, &racetime_config.client_secret, &extra_room_tx, &http_client, clean_shutdown, cal_event, &event).await? {
                            if let Some(channel) = event.discord_race_room_channel {
                                if let Some(thread) = cal_event.race.scheduling_thread {
                                    thread.say(discord_ctx, &msg).await?;
                                    channel.send_message(discord_ctx, CreateMessage::default().content(msg).allowed_mentions(CreateAllowedMentions::default())).await?;
                                } else {
                                    channel.say(discord_ctx, msg).await?;
                                }
                            } else if let Some(thread) = cal_event.race.scheduling_thread {
                                thread.say(discord_ctx, msg).await?;
                            } else if let Some(channel) = event.discord_organizer_channel {
                                channel.say(discord_ctx, msg).await?;
                            } else {
                                FENHL.create_dm_channel(discord_ctx).await?.say(discord_ctx, msg).await?;
                            }
                        } else {
                            let mut response_content = MessageBuilder::default();
                            response_content.push(if let Some(game) = cal_event.race.game { format!("Game {game}") } else { format!("This race") });
                            response_content.push(if was_scheduled { " has been rescheduled for " } else { " is now scheduled for " });
                            response_content.push_timestamp(self.when_countdown, serenity_utils::message::TimestampStyle::LongDateTime);
                            let msg = response_content
                                .push(". The race room will be opened momentarily.")
                                .build();
                            if let Some(thread) = cal_event.race.scheduling_thread {
                                thread.say(discord_ctx, msg).await?;
                            } else if let Some(channel) = event.discord_organizer_channel {
                                channel.say(discord_ctx, msg).await?;
                            } else {
                                FENHL.create_dm_channel(discord_ctx).await?.say(discord_ctx, msg).await?;
                            }
                        }
                        cal_event.race.save(&mut transaction).await?;
                        transaction.commit().await?;
                        transaction = db_pool.begin().await?;
                    })
                } else {
                    cal_event.race.save(&mut transaction).await?;
                    transaction.commit().await?;
                    transaction = db_pool.begin().await?;
                    if let Some(thread) = cal_event.race.scheduling_thread {
                        let msg = if_chain! {
                            if let French = event.language;
                            if cal_event.race.game.is_none();
                            then {
                                MessageBuilder::default()
                                    .push("Votre race a été planifiée pour le ")
                                    .push_timestamp(self.when_countdown, serenity_utils::message::TimestampStyle::LongDateTime)
                                    .push('.')
                                    .build()
                            } else {
                                let mut response_content = MessageBuilder::default();
                                response_content.push(if let Some(game) = cal_event.race.game { format!("Game {game}") } else { format!("This race") });
                                response_content.push(if was_scheduled { " has been rescheduled for " } else { " is now scheduled for " });
                                response_content.push_timestamp(self.when_countdown, serenity_utils::message::TimestampStyle::LongDateTime);
                                response_content.push('.');
                                response_content.build()
                            }
                        };
                        thread.say(discord_ctx, msg).await?;
                    }
                }
            }
            for channel in &self.channels {
                if let hash_map::Entry::Vacant(entry) = cal_event.race.video_urls.entry(channel.language) {
                    let video_url = Url::parse(&format!("https://twitch.tv/{}", channel.slug))?;
                    entry.insert(video_url);
                }
                //TODO register restreamer, if any
            }
        }
        Ok(transaction)
    }
}

pub(crate) type Schedule = Vec<Restream>;

pub(crate) async fn schedule(http_client: &reqwest::Client, event_slug: &str) -> wheel::Result<Schedule> {
    let now = Utc::now();
    lock!(cache = CACHE; {
        let (ref mut next_request, ref mut cache) = *cache;
        if *next_request <= Instant::now() {
            *cache = http_client.get("https://speedgaming.org/api/schedule")
                .query(&[
                    ("event", event_slug),
                    ("from", &now.to_rfc3339()), // no need to look for races created in the past minute since filters by start time with stream delay
                    ("to", &(now + TimeDelta::days(365)).to_rfc3339()), // required because the default is some very short interval (less than 1 week)
                ])
                .send().await?
                .detailed_error_for_status().await?
                .json_with_text_in_error().await?;
            *next_request = Instant::now() + RATE_LIMIT;
        }
        Ok(cache.clone())
    })
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2023onl" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2023 SpeedGaming Live online OoTR tournament, organized by ";
                        : English.join_html_opt(data.organizers(&mut *transaction).await?);
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
                        : English.join_html_opt(data.organizers(&mut *transaction).await?);
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
                        : English.join_html_opt(data.organizers(&mut *transaction).await?);
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
                        : English.join_html_opt(data.organizers(&mut *transaction).await?);
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
        "2025onl" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2025 SpeedGaming Live online OoTR tournament, organized by ";
                        : English.join_html_opt(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://docs.google.com/document/d/1SFmkuknmCqfO9EmTwMVKmKdema5OQ1InUlbuy16zsy8/edit") : "Rules document";
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
        "2025live" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2025 SpeedGaming Live in-person OoTR tournament, organized by ";
                        : English.join_html_opt(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://sglive.speedgaming.org/") : "Main SGL event page";
                            }
                            li {
                                a(href = "https://matcherino.com/t/sglive25") : "Matcherino";
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

pub(crate) fn settings_2023() -> seed::Settings {
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

pub(crate) fn settings_2024() -> seed::Settings {
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

pub(crate) fn settings_2025() -> seed::Settings {
    collect![
        format!("user_message") => json!("SGL 2025 Tournament"),
        format!("password_lock") => json!(true),
        format!("triforce_count_per_world") => json!(28),
        format!("triforce_goal_per_world") => json!(24),
        format!("bridge") => json!("stones"),
        format!("bridge_rewards") => json!(7),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("on_lacs"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("gerudo_fortress") => json!("fast"),
        format!("starting_age") => json!("adult"),
        format!("empty_dungeons_mode") => json!("rewards"),
        format!("empty_dungeons_rewards") => json!([
            "Light Medallion",
        ]),
        format!("spawn_positions") => json!([
            "child",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("adult_trade_start") => json!([
            "Claim Check",
        ]),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("enhance_map_compass") => json!(true),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
            "Sheik at Temple",
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
            "logic_deku_b1_skip",
        ]),
        format!("starting_equipment") => json!([
            "deku_shield",
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
        format!("start_with_consumables") => json!(true),
        format!("skip_reward_from_rauru") => json!(true),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("fast_bunny_hood") => json!(true),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("fast_shadow_boat") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("hint_dist_user") => json!({
            "name":                  "sgl2025",
            "gui_name":              "SGL 2025",
            "description":           "Hint distribution used for SGL 2025.",
            "add_locations":         [
                { "location": "Deku Theater Skull Mask", "types": ["always"] },
            ],
            "remove_locations":      [
                { "location": "Sheik in Crater", "types": ["sometimes"] },
                { "location": "Song from Royal Familys Tomb", "types": ["sometimes"] },
                { "location": "Sheik in Forest", "types": ["sometimes"] },
                { "location": "Sheik at Temple", "types": ["sometimes"] },
                { "location": "Sheik at Colossus", "types": ["sometimes"] },
                { "location": "LH Sun", "types": ["sometimes"] },
                { "location": "GF HBA 1500 Points", "types": ["sometimes"] },
                { "location": "GC Maze Left Chest", "types": ["sometimes"] },
                { "location": "GV Chest", "types": ["sometimes"] },
                { "location": "Graveyard Royal Familys Tomb Chest", "types": ["sometimes"] },
                { "location": "GC Pot Freestanding PoH", "types": ["sometimes"] },
                { "location": "LH Lab Dive", "types": ["sometimes"] },
                { "location": "Fire Temple Megaton Hammer Chest", "types": ["sometimes"] },
                { "location": "Water Temple Boss Key Chest", "types": ["sometimes"] },
                { "location": "Gerudo Training Ground Maze Path Final Chest", "types": ["sometimes"] },
                { "location": "Spirit Temple Silver Gauntlets Chest", "types": ["sometimes"] },
                { "location": "Spirit Temple Mirror Shield Chest", "types": ["sometimes"] },
                { "location": "Shadow Temple Freestanding Key", "types": ["sometimes"] },
                { "location": "Ice Cavern Iron Boots Chest", "types": ["sometimes"] },
                { "location": "DMT Biggoron", "types": ["sometimes"] },
                { "location": "Graveyard Dampe Race Rewards", "types": ["dual"] },
                { "location": "Graveyard Royal Family Tomb Contents", "types": ["dual"] },
                { "location": "Ice Cavern Final Room", "types": ["dual"] },
                { "location": "Fire Temple Lower Loop", "types": ["dual"] },
                { "location": "Spirit Temple Colossus Hands", "types": ["dual"] },
                { "location": "Spirit Temple Adult Lower", "types": ["dual"] },
                { "location": "Shadow Temple Invisible Blades Chests", "types": ["dual"] },
                { "location": "Shadow Temple Spike Walls Room", "types": ["dual"] },
                { "location": "Ganons Castle Spirit Trial Chests", "types": ["dual"] },
            ],
            "add_items":             [],
            "remove_items":          [
                { "item": "Zeldas Lullaby", "types": ["woth", "goal"] },
            ],
            "excluded_goal_categories": ["ganon"],
            "dungeons_woth_limit":   2,
            "dungeons_barren_limit": 1,
            "one_hint_per_goal":     true,
            "named_items_required":  true,
            "vague_named_items":     false,
            "distribution":          {
                "trial":           {"order": 1, "weight": 0.0, "fixed":   0, "copies": 2},
                "entrance_always": {"order": 2, "weight": 0.0, "fixed":   0, "copies": 2},
                "always":          {"order": 3, "weight": 0.0, "fixed":   0, "copies": 2, "remove_stones": [
                    "ToT (Left)",
                    "ToT (Left-Center)",
                    "ToT (Right-Center)",
                    "ToT (Right)",
                    "HF (Cow Grotto)",
                    "HC (Storms Grotto)",
                ]},
                "junk":            {"order": 4, "weight": 0.0, "fixed":   2, "copies": 1, "priority_stones": [
                    "HF (Cow Grotto)",
                    "HC (Storms Grotto)",
                ]},
                "barren":          {"order": 5, "weight": 0.0, "fixed":   4, "copies": 1, "priority_stones": [
                    "ToT (Left)",
                    "ToT (Left-Center)",
                    "ToT (Right-Center)",
                    "ToT (Right)",
                ]},
                "goal":            {"order": 6, "weight": 0.0, "fixed":   5, "copies": 2},
                "dual":            {"order": 7, "weight": 0.0, "fixed":   3, "copies": 2},
                "sometimes":       {"order": 8, "weight": 0.0, "fixed": 100, "copies": 2},
                "woth":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "entrance":        {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "random":          {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "item":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "song":            {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "overworld":       {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "dungeon":         {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
                "named-item":      {"order": 0, "weight": 0.0, "fixed":   0, "copies": 2},
            },
        }),
        format!("plandomized_locations") => json!({
            "ToT Reward from Rauru": "Forest Medallion",
        }),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs_and_owls",
            "20_skulltulas",
            "30_skulltulas",
        ]),
        format!("correct_chest_appearances") => json!("both"),
        format!("minor_items_as_major_chest") => json!([
            "bombchus",
        ]),
        format!("correct_potcrate_appearances") => json!("off"),
        format!("key_appearance_match_dungeon") => json!(true),
        format!("blue_fire_arrows") => json!(true),
        format!("tcg_requires_lens") => json!(true),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("junk_only"),
    ]
}
