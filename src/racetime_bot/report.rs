use {
    nonempty_collections::nev,
    tokio::sync::RwLockReadGuard,
    crate::{
        prelude::*,
        racetime_bot::*,
    },
};

pub(crate) trait Score {
    type SortKey: Ord;

    fn is_dnf(&self) -> bool;
    fn sort_key(&self) -> Self::SortKey;
    fn time_window(&self, other: &Self) -> Option<Duration>;
    fn format(&self, language: Language) -> Cow<'_, str>;
    fn as_duration(&self) -> Option<Option<Duration>>;
}

impl Score for Option<Duration> {
    type SortKey = (bool, Option<Duration>);

    fn is_dnf(&self) -> bool {
        self.is_none()
    }

    fn sort_key(&self) -> Self::SortKey {
        (
            self.is_none(), // sort DNF last
            *self,
        )
    }

    fn time_window(&self, other: &Self) -> Option<Duration> {
        Some((*self)? - (*other)?)
    }

    fn format(&self, language: Language) -> Cow<'_, str> {
        match language {
            French => self.map_or(Cow::Borrowed("forfait"), |time| Cow::Owned(French.format_duration(time, false))),
            _ => self.map_or(Cow::Borrowed("DNF"), |time| Cow::Owned(English.format_duration(time, false))),
        }
    }

    fn as_duration(&self) -> Option<Option<Duration>> {
        Some(*self)
    }
}

impl Score for tfb::Score {
    type SortKey = (Reverse<u8>, Duration);

    fn is_dnf(&self) -> bool {
        self.pieces == 0
    }

    fn sort_key(&self) -> Self::SortKey {
        (
            Reverse(self.pieces),
            self.last_collection_time,
        )
    }

    fn time_window(&self, other: &Self) -> Option<Duration> {
        (self.pieces == other.pieces).then(|| self.last_collection_time - other.last_collection_time)
    }

    fn format(&self, _: Language) -> Cow<'_, str> {
        Cow::Owned(self.to_string())
    }

    fn as_duration(&self) -> Option<Option<Duration>> {
        None
    }
}

#[derive(PartialEq, Eq)]
pub(crate) enum TeamLinks {
    Room(Url),
    AsyncForm(Vec<(&'static str, Option<Url>)>),
}

impl TeamLinks {
    fn format_discord(&self, msg: &mut MessageBuilder, is_bracketed: bool) {
        match self {
            Self::Room(url) => {
                msg.push('<');
                msg.push(url.as_str());
                msg.push('>');
            }
            Self::AsyncForm(vods) => {
                if !is_bracketed { msg.push('('); }
                if let [(_, vod)] = &**vods {
                    if let Some(vod) = vod {
                        msg.push_named_link_no_preview("vod", vod.as_str());
                    } else {
                        msg.push("no vod");
                    }
                } else {
                    msg.push("vods: ");
                    for (pos, (role, vod)) in vods.iter().with_position() {
                        if !pos.is_first {
                            msg.push(", ");
                        }
                        if let Some(vod) = vod {
                            msg.push_named_link_no_preview(*role, vod.as_str());
                        } else {
                            msg.push(*role);
                            msg.push(": no vod");
                        }
                    }
                }
                if !is_bracketed { msg.push(')'); }
            }
        }
    }
}

pub(crate) async fn report_1v1<'a, S: Score>(mut transaction: Transaction<'a, Postgres>, global: &GlobalState, cal_event: &cal::Event, event: &event::Data<'_>, breaks_used: bool, restreamed: bool, mut entrants: [(Entrant, S, TeamLinks); 2]) -> Result<Transaction<'a, Postgres>, Error> {
    entrants.sort_unstable_by_key(|(_, time, _)| time.sort_key());
    let [(winner, winning_time, winning_room), (loser, losing_time, losing_room)] = entrants;
    if winning_time.is_dnf() && losing_time.is_dnf() {
        if let Some(results_channel) = event.discord_race_results_channel.or(event.discord_organizer_channel) {
            let msg = if let French = event.language
                && let Some(phase_round) = match (&cal_event.race.phase, &cal_event.race.round) {
                    (Some(phase), Some(round)) => if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut *transaction).await? {
                        Some(Some(phase_round))
                    } else {
                        None // no translation
                    },
                    (Some(_), None) | (None, Some(_)) => None, // no translation
                    (None, None) => Some(None), // no phase/round
                }
                && cal_event.race.game.is_none()
            {
                let mut builder = MessageBuilder::default();
                if let Some(phase_round) = phase_round {
                    builder.push_safe(phase_round);
                    builder.push(" : ");
                }
                builder.push("Ni ");
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &winner).await?;
                if winning_room != losing_room {
                    builder.push(" [");
                    winning_room.format_discord(&mut builder, true);
                    builder.push(']');
                }
                builder.push(" ni ");
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &loser).await?;
                if winning_room != losing_room {
                    builder.push(" [");
                    losing_room.format_discord(&mut builder, true);
                    builder.push(']');
                }
                builder.push(" n'ont fini");
                if winning_room == losing_room {
                    builder.push(' ');
                    winning_room.format_discord(&mut builder, false);
                }
                builder.build()
            } else {
                let mut builder = MessageBuilder::default();
                let info_prefix = match (&cal_event.race.phase, &cal_event.race.round) {
                    (Some(phase), Some(round)) => Some(format!("{phase} {round}")),
                    (Some(phase), None) => Some(phase.clone()),
                    (None, Some(round)) => Some(round.clone()),
                    (None, None) => None,
                };
                match (info_prefix, cal_event.race.game) {
                    (Some(prefix), Some(game)) => {
                        builder.push_safe(prefix);
                        builder.push(", game ");
                        builder.push(game.to_string());
                        builder.push(": ");
                    }
                    (Some(prefix), None) => {
                        builder.push_safe(prefix);
                        builder.push(": ");
                    }
                    (None, Some(game)) => {
                        builder.push("game ");
                        builder.push(game.to_string());
                        builder.push(": ");
                    }
                    (None, None) => {}
                }
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &winner).await?;
                if winning_room != losing_room {
                    builder.push(" [");
                    winning_room.format_discord(&mut builder, true);
                    builder.push(']');
                }
                builder.push(" and ");
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &loser).await?;
                if winning_room != losing_room {
                    builder.push(" [");
                    losing_room.format_discord(&mut builder, true);
                    builder.push(']');
                }
                builder.push(" both did not finish");
                if winning_room == losing_room {
                    builder.push(' ');
                    winning_room.format_discord(&mut builder, false);
                }
                builder.build()
            };
            results_channel.say(discord_ctx!(global), msg).await?;
        }
    } else if losing_time.time_window(&winning_time).is_some_and(|time_window| time_window <= event.retime_window) {
        if let Some(organizer_channel) = event.discord_organizer_channel {
            let mut msg = MessageBuilder::default();
            msg.push("race finished as a draw: ");
            winning_room.format_discord(&mut msg, false);
            if winning_room != losing_room {
                msg.push(" and ");
                losing_room.format_discord(&mut msg, false);
            }
            if event.discord_race_results_channel.is_some() || matches!(cal_event.race.source, cal::Source::StartGG { .. } | cal::Source::StartGGSpeedGamingOnline { .. }) {
                msg.push(" — please manually ");
                if let Some(results_channel) = event.discord_race_results_channel {
                    msg.push("post the announcement in ");
                    msg.mention(&results_channel);
                }
                if let Some(startgg_report_url) = cal_event.race.startgg_report_url()? {
                    if event.discord_race_results_channel.is_some() {
                        msg.push(" and ");
                    }
                    msg.push_named_link_no_preview("report the result on start.gg", startgg_report_url);
                }
                msg.push(" after adjusting the times");
            }
            //TODO note to manually initialize high seed for next game's draft (if any) and use `/post-status`
            organizer_channel.say(discord_ctx!(global), msg.build()).await?;
        }
    } else {
        let results_channel = if event.manual_reporting_for_asyncs && winning_room != losing_room {
            event.discord_organizer_channel
        } else {
            event.discord_race_results_channel.or(event.discord_organizer_channel)
        };
        if let Some(results_channel) = results_channel {
            let msg = if let French = event.language
                && (!event.manual_reporting_for_asyncs || winning_room == losing_room)
                && let Some(phase_round) = match (&cal_event.race.phase, &cal_event.race.round) {
                    (Some(phase), Some(round)) => if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut *transaction).await? {
                        Some(Some(phase_round))
                    } else {
                        None // no translation
                    },
                    (Some(_), None) | (None, Some(_)) => None, // no translation
                    (None, None) => Some(None), // no phase/round
                }
                && cal_event.race.game.is_none()
                && !breaks_used
            {
                let mut builder = MessageBuilder::default();
                if let Some(phase_round) = phase_round {
                    builder.push_safe(phase_round);
                    builder.push(" : ");
                }
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &winner).await?;
                builder.push(" (");
                builder.push(winning_time.format(French));
                builder.push(')');
                if winning_room != losing_room {
                    builder.push(" [");
                    winning_room.format_discord(&mut builder, true);
                    builder.push(']');
                }
                builder.push(if winner.name_is_plural() { " ont battu " } else { " a battu " });
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &loser).await?;
                builder.push(" (");
                builder.push(losing_time.format(French));
                builder.push(if winning_room == losing_room { ") " } else { ") [" });
                losing_room.format_discord(&mut builder, winning_room != losing_room);
                if winning_room != losing_room {
                    builder.push(']');
                }
                builder.build()
            } else {
                let mut builder = MessageBuilder::default();
                if event.manual_reporting_for_asyncs && winning_room != losing_room {
                    builder.push("asynced race finished: ");
                }
                let info_prefix = match (&cal_event.race.phase, &cal_event.race.round) {
                    (Some(phase), Some(round)) => Some(format!("{phase} {round}")),
                    (Some(phase), None) => Some(phase.clone()),
                    (None, Some(round)) => Some(round.clone()),
                    (None, None) => None,
                };
                match (info_prefix, cal_event.race.game) {
                    (Some(prefix), Some(game)) => {
                        builder.push_safe(prefix);
                        builder.push(", game ");
                        builder.push(game.to_string());
                        builder.push(": ");
                    }
                    (Some(prefix), None) => {
                        builder.push_safe(prefix);
                        builder.push(": ");
                    }
                    (None, Some(game)) => {
                        builder.push("game ");
                        builder.push(game.to_string());
                        builder.push(": ");
                    }
                    (None, None) => {}
                }
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &winner).await?;
                builder.push(" (");
                builder.push(winning_time.format(English));
                if breaks_used && !winning_time.is_dnf() {
                    builder.push(if restreamed { " including breaks" } else { " adjusted for breaks" });
                }
                builder.push(')');
                if winning_room != losing_room {
                    builder.push(" [");
                    winning_room.format_discord(&mut builder, true);
                    builder.push(']');
                }
                builder.push(if winner.name_is_plural() { " defeat " } else { " defeats " });
                builder.mention_entrant_long(&mut transaction, event.discord_guild, &loser).await?;
                builder.push(" (");
                builder.push(losing_time.format(English));
                if breaks_used && !losing_time.is_dnf() {
                    builder.push(if restreamed { " including breaks" } else { " adjusted for breaks" });
                }
                builder.push(if winning_room == losing_room { ") " } else { ") [" });
                losing_room.format_discord(&mut builder, winning_room != losing_room);
                if winning_room != losing_room {
                    builder.push(']');
                }
                if (event.manual_reporting_for_asyncs && winning_room != losing_room) && (event.discord_race_results_channel.is_some() || matches!(cal_event.race.source, cal::Source::StartGG { .. } | cal::Source::StartGGSpeedGamingOnline { .. })) {
                    builder.push(" — please manually ");
                    if let Some(results_channel) = event.discord_race_results_channel {
                        builder.push("post the announcement in ");
                        builder.mention(&results_channel);
                    }
                    if let Some(startgg_report_url) = cal_event.race.startgg_report_url()? {
                        if event.discord_race_results_channel.is_some() {
                            builder.push(" and ");
                        }
                        builder.push_named_link_no_preview("report the result on start.gg", startgg_report_url);
                    }
                    builder.push(" after adjusting the times");
                }
                //TODO note to manually initialize high seed for next game's draft (if any) and use `/post-status`
                builder.build()
            };
            results_channel.say(discord_ctx!(global), msg).await?;
        }
        if !event.manual_reporting_for_asyncs || winning_room == losing_room {
            let match_decided = match cal_event.race.source {
                cal::Source::Manual | cal::Source::Sheet { .. } | cal::Source::SpeedGamingOnline { .. } | cal::Source::SpeedGamingInPerson { .. } => None,
                cal::Source::Challonge { .. } => None, //TODO
                cal::Source::League { id } => {
                    if let (Some(winner), Some(loser), Some(winning_time), Some(losing_time)) = (
                        match &winner {
                            Entrant::MidosHouseTeam(team) => team.members(&mut transaction).await?.into_iter().exactly_one().ok().and_then(|member| member.racetime).map(|racetime| racetime.id),
                            Entrant::MidosHouseTeamMember { member, .. } => member.racetime.as_ref().map(|racetime| racetime.id.clone()),
                            Entrant::Discord { racetime_id, .. } | Entrant::Named { racetime_id, .. } => racetime_id.clone(),
                        },
                        match &loser {
                            Entrant::MidosHouseTeam(team) => team.members(&mut transaction).await?.into_iter().exactly_one().ok().and_then(|member| member.racetime).map(|racetime| racetime.id),
                            Entrant::MidosHouseTeamMember { member, .. } => member.racetime.as_ref().map(|racetime| racetime.id.clone()),
                            Entrant::Discord { racetime_id, .. } | Entrant::Named { racetime_id, .. } => racetime_id.clone(),
                        },
                        winning_time.as_duration(),
                        losing_time.as_duration(),
                    ) {
                        let mut form = collect![as HashMap<_, _>:
                            "id" => id.to_string(),
                            "racetimeRoom" => as_variant!(winning_room, TeamLinks::Room).expect("attempted to report League race played as form-based async").to_string(),
                            "fpa" => format!("0"), //TODO also report races with FPA calls
                            "winner" => winner,
                            "loser" => loser,
                        ];
                        if let Some(winning_time) = winning_time {
                            form.insert("winningTime", winning_time.as_secs().to_string());
                        }
                        if let Some(losing_time) = losing_time {
                            form.insert("losingTime", losing_time.as_secs().to_string());
                        }
                        let request = global.http_client.post("https://league.ootrandomizer.com/reportResultFromMidoHouse")
                            .bearer_auth(&global.config.league_api_key)
                            .form(&form);
                        println!("reporting result to League website: {:?}", serde_urlencoded::to_string(&form));
                        request.send().await?.detailed_error_for_status().await?;
                    }
                    None
                }
                cal::Source::StartGG { ref set, .. } | cal::Source::StartGGSpeedGamingOnline { ref set, .. } => if let Entrant::MidosHouseTeam(Team { startgg_id: Some(winner_entrant_id), .. }) | Entrant::MidosHouseTeamMember { team: Team { startgg_id: Some(winner_entrant_id), .. }, .. } = &winner {
                    if let Some(game) = cal_event.race.game {
                        if let Some(access_token) = &global.config.startgg {
                            let startgg::set_scores_query::ResponseData {
                                set: Some(startgg::set_scores_query::SetScoresQuerySet {
                                    games,
                                    set_games_type: Some(set_games_type),
                                }),
                            } = startgg::query_uncached::<startgg::SetScoresQuery>(&global.http_client, access_token, startgg::set_scores_query::Variables {
                                set_id: set.clone(),
                            }).await? else {
                                return Err(Error::GraphQLQueryResponse(set.clone()))
                            };
                            let mut game_data = games.into_iter().flatten().map(|game| {
                                let Some(startgg::set_scores_query::SetScoresQuerySetGames { order_num: Some(game_num), winner_id: Some(winner_id) }) = game else { return Err(Error::GraphQLQueryResponse(set.clone())) };
                                Ok(startgg::report_multi_game_result_mutation::BracketSetGameDataInput {
                                    winner_id: Some(startgg::ID(winner_id.to_string())),
                                    entrant1_score: None,
                                    entrant2_score: None,
                                    stage_id: None,
                                    selections: None,
                                    game_num,
                                })
                            }).try_collect::<_, Vec<_>, _>()?;
                            game_data.push(startgg::report_multi_game_result_mutation::BracketSetGameDataInput {
                                winner_id: Some(winner_entrant_id.clone()),
                                game_num: game.into(),
                                entrant1_score: None,
                                entrant2_score: None,
                                stage_id: None,
                                selections: None,
                            });
                            let game_count = cal_event.race.game_count(&mut transaction).await?;
                            let (overall_winner, overall_won_games) = game_data.iter().map(|game| game.winner_id.as_ref().expect("missing game winner ID")).counts().into_iter().max_by_key(|(_, count)| *count).expect("no game winners");
                            let match_decided = match set_games_type {
                                1 => i16::try_from(overall_won_games).ok().is_none_or(|overall_won_games| overall_won_games > game_count / 2), // best of
                                2 => i16::try_from(game_data.len()).ok().is_none_or(|overall_games| overall_games >= game_count), // total games
                                _ => return Err(Error::GraphQLQueryResponse(set.clone())),
                            };
                            startgg::query_uncached::<startgg::ReportMultiGameResultMutation>(&global.http_client, access_token, startgg::report_multi_game_result_mutation::Variables {
                                set_id: set.clone(),
                                winner_entrant_id: match_decided.then(|| overall_winner.clone()),
                                game_data,
                            }).await?;
                            Some(match_decided)
                        } else {
                            if let Some(organizer_channel) = event.discord_organizer_channel {
                                let mut msg = MessageBuilder::default();
                                msg.push("failed to report race result to start.gg: ");
                                winning_room.format_discord(&mut msg, false);
                                if winning_room != losing_room {
                                    msg.push(" / ");
                                    losing_room.format_discord(&mut msg, false);
                                }
                                msg.push(" (start.gg integration is down)");
                                organizer_channel.say(discord_ctx!(global), msg.build()).await?;
                            }
                            None
                        }
                    } else {
                        if let Some(access_token) = &global.config.startgg {
                            startgg::query_uncached::<startgg::ReportOneGameResultMutation>(&global.http_client, access_token, startgg::report_one_game_result_mutation::Variables {
                                set_id: set.clone(),
                                winner_entrant_id: winner_entrant_id.clone(),
                            }).await?;
                            Some(true)
                        } else {
                            if let Some(organizer_channel) = event.discord_organizer_channel {
                                let mut msg = MessageBuilder::default();
                                msg.push("failed to report race result to start.gg: ");
                                winning_room.format_discord(&mut msg, false);
                                if winning_room != losing_room {
                                    msg.push(" / ");
                                    losing_room.format_discord(&mut msg, false);
                                }
                                msg.push(" (start.gg integration is down)");
                                organizer_channel.say(discord_ctx!(global), msg.build()).await?;
                            }
                            None
                        }
                    }
                } else {
                    if let Some(organizer_channel) = event.discord_organizer_channel {
                        let mut msg = MessageBuilder::default();
                        msg.push("failed to report race result to start.gg: ");
                        winning_room.format_discord(&mut msg, false);
                        if winning_room != losing_room {
                            msg.push(" / ");
                            losing_room.format_discord(&mut msg, false);
                        }
                        msg.push(" (winner has no start.gg entrant ID)");
                        organizer_channel.say(discord_ctx!(global), msg.build()).await?;
                    }
                    None
                },
            };
            if let Some(true) = match_decided {
                if let Some(next_race) = cal_event.race.next_game(&mut transaction, &global.http_client).await? {
                    let mut races_to_delete = nev![next_race];
                    while let Some(next_race) = races_to_delete.last().next_game(&mut transaction, &global.http_client).await? {
                        races_to_delete.push(next_race);
                    }
                    sqlx::query_scalar!(r#"UPDATE races SET ignored = TRUE WHERE id = ANY($1)"#, &races_to_delete.into_iter().map(|race| i64::from(race.id)).collect_vec()).execute(&mut *transaction).await?;
                }
            } else {
                if let Entrant::MidosHouseTeam(winner) = winner
                    && let Entrant::MidosHouseTeam(loser) = loser
                    && let Some(draft_kind) = event.draft_kind()
                    && let Some(next_game) = cal_event.race.next_game(&mut transaction, &global.http_client).await?
                {
                    let draft = Draft::for_next_game(&mut transaction, draft_kind, loser.id, winner.id).await?;
                    sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", sqlx::types::Json(&draft) as _, next_game.id as _).execute(&mut *transaction).await?;
                    if let Some(guild_id) = event.discord_guild
                        && let Some(scheduling_thread) = next_game.scheduling_thread
                        && (match_decided == Some(false) || cal_event.race.game.expect("found next game for race without game number") <= cal_event.race.game_count(&mut transaction).await? / 2)
                        && let discord_ctx = discord_ctx!(global)
                        && let data = discord_ctx.data.read().await
                        && let Some(Some(command_ids)) = data.get::<CommandIds>().and_then(|command_ids| command_ids.get(&guild_id).copied())
                    {
                        let mut msg_ctx = draft::MessageContext::Discord {
                            entrants: next_game.entrants.to_vec(),
                            team: Team::dummy(),
                            transaction, guild_id, command_ids,
                        };
                        scheduling_thread.say(discord_ctx, draft.next_step(draft_kind, next_game.game, &mut msg_ctx).await?.message).await?;
                        transaction = msg_ctx.into_transaction();
                    }
                }
            }
        }
    }
    Ok(transaction)
}

async fn report_ffa(global: &GlobalState, cal_event: &cal::Event, event: &event::Data<'_>, room: Url) -> Result<(), Error> {
    if let Some(results_channel) = event.discord_race_results_channel.or(event.discord_organizer_channel) {
        let mut builder = MessageBuilder::default();
        let info_prefix = match (&cal_event.race.phase, &cal_event.race.round) {
            (Some(phase), Some(round)) => Some(format!("{phase} {round}")),
            (Some(phase), None) => Some(phase.clone()),
            (None, Some(round)) => Some(round.clone()),
            (None, None) => None,
        };
        match (info_prefix, cal_event.race.game) {
            (Some(prefix), Some(game)) => {
                builder.push_safe(prefix);
                builder.push(", game ");
                builder.push(game.to_string());
                builder.push(": ");
            }
            (Some(prefix), None) => {
                builder.push_safe(prefix);
                builder.push(": ");
            }
            (None, Some(game)) => {
                builder.push("game ");
                builder.push(game.to_string());
                builder.push(": ");
            }
            (None, None) => {}
        }
        builder.push("race finished: <");
        builder.push(room.as_str());
        builder.push('>');
        results_channel.say(discord_ctx!(global), builder.build()).await?;
    }
    Ok(())
}

impl Handler {
    #[must_use = "should set cleaned_up if this returns true"]
    pub(super) async fn check_tfb_finish(&self, ctx: &RaceContext<GlobalState>) -> Result<bool, Error> {
        let data = ctx.data().await;
        let Some(OfficialRaceData { ref cal_event, ref event, ref restreams, fpa_invoked, ref scores, .. }) = self.official_data else { return Ok(true) };
        let mut transaction = ctx.global_state.db_pool.begin().await?;
        let mut resolved_scores = HashMap::default();
        for rtgg_entrant in &data.entrants {
            let Some(mh_entrant) = Entrant::from_racetime(&mut transaction, event, rtgg_entrant).await? else { return Ok(false) };
            let score = match rtgg_entrant.status.value {
                EntrantStatusValue::Dnf => Some(tfb::Score::dnf(event.team_config)),
                EntrantStatusValue::Done => scores.get(&mh_entrant).copied().and_then(identity),
                _ => None,
            };
            let Some(score) = score else { return Ok(false) };
            resolved_scores.insert(mh_entrant, score);
        }
        transaction.commit().await?;
        ctx.say("All scores received. Thank you for playing Triforce Blitz, see you next race!").await?;
        self.official_race_finished(ctx, data, cal_event, event, fpa_invoked, self.breaks, !restreams.is_empty(), Some(resolved_scores)).await?;
        Ok(true)
    }

    pub(super) async fn official_race_finished(&self, ctx: &RaceContext<GlobalState>, data: RwLockReadGuard<'_, RaceData>, cal_event: &cal::Event, event: &event::Data<'_>, fpa_invoked: bool, breaks: Option<Breaks>, restreamed: bool, tfb_scores: Option<HashMap<Entrant, tfb::Score>>) -> Result<(), Error> {
        let stream_delay = match cal_event.race.entrants {
            Entrants::Open | Entrants::Count { .. } => event.open_stream_delay,
            Entrants::Two(_) | Entrants::Three(_) | Entrants::Named(_) => event.invitational_stream_delay,
        };
        sleep(stream_delay).await;
        let mut transaction = ctx.global_state.db_pool.begin().await?;
        if cal_event.is_private_async_part() {
            ctx.say("@entrants Please remember to send the videos of your run to a tournament organizer.").await?;
            if fpa_invoked {
                sqlx::query!("UPDATE races SET fpa_invoked = TRUE WHERE id = $1", cal_event.race.id as _).execute(&mut *transaction).await?;
            }
            if event.async_organizer_notifications && let Some(organizer_channel) = event.discord_organizer_channel {
                organizer_channel.say(discord_ctx!(ctx.global_state), MessageBuilder::default()
                    .push("first half of async finished")
                    .push(if fpa_invoked { " with FPA call" } else if event.manual_reporting_with_breaks && breaks.is_some() { " with breaks" } else { "" })
                    .push(": <https://")
                    .push(racetime_host())
                    .push(&ctx.data().await.url)
                    .push('>')
                    .build()
                ).await?;
            }
        } else if fpa_invoked {
            if let Some(organizer_channel) = event.discord_organizer_channel {
                let mut msg = MessageBuilder::default();
                msg.push("race finished with FPA call: <https://");
                msg.push(racetime_host());
                msg.push(&ctx.data().await.url);
                msg.push('>');
                if event.discord_race_results_channel.is_some() || matches!(cal_event.race.source, cal::Source::StartGG { .. } | cal::Source::StartGGSpeedGamingOnline { .. }) {
                    msg.push(" — please manually ");
                    if let Some(results_channel) = event.discord_race_results_channel {
                        msg.push("post the announcement in ");
                        msg.mention(&results_channel);
                    }
                    if let Some(startgg_report_url) = cal_event.race.startgg_report_url()? {
                        if event.discord_race_results_channel.is_some() {
                            msg.push(" and ");
                        }
                        msg.push_named_link_no_preview("report the result on start.gg", startgg_report_url);
                    }
                    msg.push(" after adjusting the times");
                }
                //TODO note to manually initialize high seed for next game's draft (if any) and use `/post-status`
                organizer_channel.say(discord_ctx!(ctx.global_state), msg.build()).await?;
            }
        } else if event.manual_reporting_with_breaks && breaks.is_some() {
            if let Some(organizer_channel) = event.discord_organizer_channel {
                let mut msg = MessageBuilder::default();
                msg.push("race finished with breaks: <https://");
                msg.push(racetime_host());
                msg.push(&ctx.data().await.url);
                msg.push('>');
                if event.discord_race_results_channel.is_some() || matches!(cal_event.race.source, cal::Source::StartGG { .. } | cal::Source::StartGGSpeedGamingOnline { .. }) {
                    msg.push(" — please manually ");
                    if let Some(results_channel) = event.discord_race_results_channel {
                        msg.push("post the announcement in ");
                        msg.mention(&results_channel);
                    }
                    if let Some(startgg_report_url) = cal_event.race.startgg_report_url()? {
                        if event.discord_race_results_channel.is_some() {
                            msg.push(" and ");
                        }
                        msg.push_named_link_no_preview("report the result on start.gg", startgg_report_url);
                    }
                    msg.push(" after adjusting the times");
                }
                //TODO note to manually initialize high seed for next game's draft (if any) and use `/post-status`
                organizer_channel.say(discord_ctx!(ctx.global_state), msg.build()).await?;
            }
        } else {
            match cal_event.race.entrants {
                Entrants::Open | Entrants::Count { .. } => {
                    let room = Url::parse(&format!("https://{}{}", racetime_host(), data.url))?;
                    report_ffa(&ctx.global_state, cal_event, event, room).await?;
                }
                Entrants::Named(_) => unimplemented!(),
                Entrants::Two(_) | Entrants::Three(_) => {
                    let mut entrant_times = HashMap::<_, Vec<_>>::default();
                    let mut entrant_rooms = HashMap::new();
                    let mut all_entrants_found = true;
                    if cal_event.is_public_async_part() {
                        for private_async_part in cal_event.race.cal_events().filter(|cal_event| cal_event.is_private_async_part()) {
                            let nonactive_entrant = private_async_part.active_entrants().exactly_one().map_err(|_| Error::ExactlyOne)?;
                            if let Some(ref room) = private_async_part.room() {
                                let data = ctx.global_state.http_client.get(format!("{}/data", room.to_string()))
                                    .send().await?
                                    .detailed_error_for_status().await?
                                    .json_with_text_in_error::<RaceData>().await?;
                                entrant_rooms.insert(nonactive_entrant.clone(), TeamLinks::Room(Url::clone(room)));
                                for entrant in &data.entrants {
                                    entrant_times.entry(nonactive_entrant.clone()).or_default().push(adjust_for_breaks(entrant.finish_time, breaks, restreamed));
                                }
                            } else {
                                let members = nonactive_entrant.member_ids_roles(&mut transaction).await?;
                                let data = sqlx::query!(
                                    r#"SELECT player AS "player: Id<Users>", vod, time FROM race_async_players WHERE race = $1 AND player = ANY($2)"#,
                                    cal_event.race.id as _,
                                    &members.iter().map(|(member_id, _)| i64::from(*member_id)).collect_vec(),
                                ).fetch_all(&mut *transaction).await?;
                                if members.iter().all(|(member_id, role)| !event.team_config.role_is_racing(*role) || data.iter().any(|row| row.player == *member_id)) {
                                    entrant_times.insert(nonactive_entrant.clone(), data.iter().map(|row| Ok::<_, Error>(adjust_for_breaks(row.time.map(decode_pginterval).transpose()?, breaks, restreamed))).try_collect()?);
                                    entrant_rooms.insert(nonactive_entrant.clone(), TeamLinks::AsyncForm({
                                        let mut links = data.into_iter().map(|row| {
                                            let (_, role) = *members.iter().find(|(member_id, _)| *member_id == row.player).expect("async submission from unexpected player");
                                            let (_, role_name) = *event.team_config.roles().iter().find(|(iter_role, _)| *iter_role == role).expect("unexpected team role");
                                            Ok::<_, Error>((role_name, row.vod.map(|vod| vod.parse()).transpose()?))
                                        }).collect::<Result<Vec<_>, _>>()?;
                                        links.sort_unstable();
                                        links
                                    }));
                                } else {
                                    all_entrants_found = false;
                                }
                            }
                        }
                        let active_entrant = cal_event.active_entrants().exactly_one().map_err(|_| Error::ExactlyOne)?;
                        entrant_rooms.insert(active_entrant.clone(), TeamLinks::Room(Url::parse(&format!("https://{}{}", racetime_host(), data.url))?));
                        for entrant in &data.entrants {
                            entrant_times.entry(active_entrant.clone()).or_default().push(adjust_for_breaks(entrant.finish_time, breaks, restreamed));
                        }
                    } else {
                        for rtgg_entrant in &data.entrants {
                            if let Some(mh_entrant) = Entrant::from_racetime(&mut transaction, event, rtgg_entrant).await? {
                                if let hash_map::Entry::Vacant(entry) = entrant_rooms.entry(mh_entrant.clone()) {
                                    entry.insert(TeamLinks::Room(Url::parse(&format!("https://{}{}", racetime_host(), data.url))?));
                                }
                                entrant_times.entry(mh_entrant).or_default().push(adjust_for_breaks(rtgg_entrant.finish_time, breaks, restreamed));
                            } else {
                                all_entrants_found = false;
                            }
                        }
                    }
                    if let Some(mut tfb_scores) = tfb_scores {
                        let mut teams = Vec::with_capacity(entrant_times.len());
                        for entrant in entrant_times.into_keys() {
                            teams.push((
                                entrant.clone(),
                                if let Some(tfb_score) = tfb_scores.remove(&entrant) {
                                    tfb_score
                                } else {
                                    return Err(Error::MissingTfbScore {
                                        room: Url::parse(&format!("https://{}{}", racetime_host(), data.url))?,
                                        entrant: entrant.clone(),
                                        tfb_scores,
                                    })
                                },
                                entrant_rooms.remove(&entrant).expect("each team should have a room"),
                            ));
                        }
                        if all_entrants_found && let Ok(teams) = teams.try_into() {
                            transaction = report_1v1(transaction, &ctx.global_state, cal_event, event, breaks.is_some(), restreamed, teams).await?;
                        } else { //TODO separate function for reporting 3-entrant results
                            let room = Url::parse(&format!("https://{}{}", racetime_host(), data.url))?;
                            report_ffa(&ctx.global_state, cal_event, event, room).await?;
                        }
                    } else {
                        let mut teams = Vec::with_capacity(entrant_times.len());
                        for (entrant, times) in entrant_times {
                            teams.push((
                                entrant.clone(),
                                times.iter().try_fold(Duration::default(), |acc, &time| Some(acc + time?)).map(|total| total / u32::try_from(times.len()).expect("too many team members")),
                                entrant_rooms.remove(&entrant).expect("each team should have a room"),
                            ));
                        }
                        if all_entrants_found && let Ok(teams) = teams.try_into() {
                            transaction = report_1v1(transaction, &ctx.global_state, cal_event, event, breaks.is_some(), restreamed, teams).await?;
                        } else { //TODO separate function for reporting 3-entrant results
                            let room = Url::parse(&format!("https://{}{}", racetime_host(), data.url))?;
                            report_ffa(&ctx.global_state, cal_event, event, room).await?;
                        }
                    }
                }
            }
        }
        transaction.commit().await?;
        Ok(())
    }
}
