use {
    std::cmp::Reverse,
    tokio::sync::RwLockReadGuard,
    crate::{
        prelude::*,
        racetime_bot::*,
    },
};

trait Score {
    type SortKey: Ord;

    fn is_dnf(&self) -> bool;
    fn sort_key(&self) -> Self::SortKey;
    fn time_window(&self, other: &Self) -> Option<Duration>;
    fn format(&self, language: Language) -> Cow<'_, str>;
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
}

async fn report_1v1<'a, S: Score>(mut transaction: Transaction<'a, Postgres>, ctx: &RaceContext<GlobalState>, cal_event: &cal::Event, event: &event::Data<'_>, mut teams: [(Team, S, Url); 2]) -> Result<Transaction<'a, Postgres>, Error> {
    teams.sort_unstable_by_key(|(_, time, _)| time.sort_key());
    let [(winner, winning_time, winning_room), (loser, losing_time, losing_room)] = teams;
    if winning_time.is_dnf() && losing_time.is_dnf() {
        if let Some(results_channel) = event.discord_race_results_channel.or(event.discord_organizer_channel) {
            let msg = if_chain! {
                if let French = event.language;
                if let Some(phase_round) = match (&cal_event.race.phase, &cal_event.race.round) {
                    (Some(phase), Some(round)) => if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut *transaction).await.to_racetime()? {
                        Some(Some(phase_round))
                    } else {
                        None // no translation
                    },
                    (Some(_), None) | (None, Some(_)) => None, // no translation
                    (None, None) => Some(None), // no phase/round
                };
                if cal_event.race.game.is_none();
                then {
                    let mut builder = MessageBuilder::default();
                    if let Some(phase_round) = phase_round {
                        builder.push_safe(phase_round);
                        builder.push(" : ");
                    }
                    builder.push("Ni ");
                    builder.mention_team(&mut transaction, event.discord_guild, &winner).await.to_racetime()?;
                    if winning_room != losing_room {
                        builder.push(" [<");
                        builder.push(winning_room.to_string());
                        builder.push(">]");
                    }
                    builder.push(" ni ");
                    builder.mention_team(&mut transaction, event.discord_guild, &loser).await.to_racetime()?;
                    if winning_room != losing_room {
                        builder.push(" [<");
                        builder.push(losing_room.to_string());
                        builder.push(">]");
                    }
                    builder.push(" n'ont fini");
                    if winning_room == losing_room {
                        builder.push(" <");
                        builder.push(winning_room);
                        builder.push('>');
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
                    builder.mention_team(&mut transaction, event.discord_guild, &winner).await.to_racetime()?;
                    if winning_room != losing_room {
                        builder.push(" [<");
                        builder.push(winning_room.to_string());
                        builder.push(">]");
                    }
                    builder.push(" and ");
                    builder.mention_team(&mut transaction, event.discord_guild, &loser).await.to_racetime()?;
                    if winning_room != losing_room {
                        builder.push(" [<");
                        builder.push(losing_room.to_string());
                        builder.push(">]");
                    }
                    builder.push(" both did not finish");
                    if winning_room == losing_room {
                        builder.push(" <");
                        builder.push(winning_room);
                        builder.push('>');
                    }
                    builder.build()
                }
            };
            results_channel.say(&*ctx.global_state.discord_ctx.read().await, msg).await.to_racetime()?;
        }
    } else if losing_time.time_window(&winning_time).is_some_and(|time_window| time_window <= event.retime_window) {
        if let Some(organizer_channel) = event.discord_organizer_channel {
            let mut msg = MessageBuilder::default();
            //TODO mention organizer role
            msg.push("race finished as a draw: <");
            msg.push(winning_room.to_string());
            if winning_room != losing_room {
                msg.push("> and <");
                msg.push(losing_room);
            }
            msg.push('>');
            if event.discord_race_results_channel.is_some() || matches!(cal_event.race.source, cal::Source::StartGG { .. }) {
                msg.push(" — please manually ");
                if let Some(results_channel) = event.discord_race_results_channel {
                    msg.push("post the announcement in ");
                    msg.mention(&results_channel);
                }
                if let Some(startgg_set_url) = cal_event.race.startgg_set_url().to_racetime()? {
                    if event.discord_race_results_channel.is_some() {
                        msg.push(" and ");
                    }
                    msg.push_named_link_no_preview("report the result on start.gg", startgg_set_url);
                }
                msg.push(" after adjusting the times");
            }
            //TODO note to manually initialize high seed for next game's draft (if any) and use `/post-status`
            organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, msg.build()).await.to_racetime()?;
        }
    } else {
        if let Some(results_channel) = event.discord_race_results_channel.or(event.discord_organizer_channel) {
            let msg = if_chain! {
                if let French = event.language;
                if let Some(phase_round) = match (&cal_event.race.phase, &cal_event.race.round) {
                    (Some(phase), Some(round)) => if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut *transaction).await.to_racetime()? {
                        Some(Some(phase_round))
                    } else {
                        None // no translation
                    },
                    (Some(_), None) | (None, Some(_)) => None, // no translation
                    (None, None) => Some(None), // no phase/round
                };
                if cal_event.race.game.is_none();
                then {
                    let mut builder = MessageBuilder::default();
                    if let Some(phase_round) = phase_round {
                        builder.push_safe(phase_round);
                        builder.push(" : ");
                    }
                    builder.mention_team(&mut transaction, event.discord_guild, &winner).await.to_racetime()?;
                    builder.push(" (");
                    builder.push(winning_time.format(French));
                    builder.push(')');
                    if winning_room != losing_room {
                        builder.push(" [<");
                        builder.push(winning_room.to_string());
                        builder.push(">]");
                    }
                    builder.push(if winner.name_is_plural() { " ont battu " } else { " a battu " });
                    builder.mention_team(&mut transaction, event.discord_guild, &loser).await.to_racetime()?;
                    builder.push(" (");
                    builder.push(losing_time.format(French));
                    builder.push(if winning_room == losing_room { ") <" } else { ") [<" });
                    builder.push(losing_room.to_string());
                    builder.push(if winning_room == losing_room { ">" } else { ">]" });
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
                    builder.mention_team(&mut transaction, event.discord_guild, &winner).await.to_racetime()?;
                    builder.push(" (");
                    builder.push(winning_time.format(English));
                    builder.push(')');
                    if winning_room != losing_room {
                        builder.push(" [<");
                        builder.push(winning_room.to_string());
                        builder.push(">]");
                    }
                    builder.push(if winner.name_is_plural() { " defeat " } else { " defeats " });
                    builder.mention_team(&mut transaction, event.discord_guild, &loser).await.to_racetime()?;
                    builder.push(" (");
                    builder.push(losing_time.format(English));
                    builder.push(if winning_room == losing_room { ") <" } else { ") [<" });
                    builder.push(losing_room.to_string());
                    builder.push(if winning_room == losing_room { ">" } else { ">]" });
                    builder.build()
                }
            };
            results_channel.say(&*ctx.global_state.discord_ctx.read().await, msg).await.to_racetime()?;
        }
        if cal_event.race.game.is_none() { //TODO also auto-report multi-game matches (report all games but the last as match progress)
            if let cal::Source::StartGG { ref set, .. } = cal_event.race.source {
                if let Some(winner_entrant_id) = winner.startgg_id {
                    startgg::query_uncached::<startgg::ReportOneGameResultMutation>(&ctx.global_state.http_client, &ctx.global_state.startgg_token, startgg::report_one_game_result_mutation::Variables {
                        set_id: set.clone(),
                        winner_entrant_id,
                    }).await.to_racetime()?;
                } else {
                    if let Some(organizer_channel) = event.discord_organizer_channel {
                        let mut msg = MessageBuilder::default();
                        //TODO mention organizer role
                        msg.push("failed to report race result to start.gg: <https://");
                        msg.push(racetime_host());
                        msg.push(&ctx.data().await.url);
                        msg.push("> (winner has no start.gg entrant ID)");
                        organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, msg.build()).await.to_racetime()?;
                    }
                }
            }
        } //TODO debug errors returned from this mutation
        if_chain! {
            if let Some(draft_kind) = event.draft_kind();
            if let Some(next_game) = cal_event.race.next_game(&mut transaction, &ctx.global_state.http_client).await.to_racetime()?;
            then {
                //TODO if this game decides the match, delete next game instead of initializing draft
                let draft = Draft::for_next_game(&mut transaction, draft_kind, loser.id, winner.id).await.to_racetime()?;
                sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", sqlx::types::Json(&draft) as _, next_game.id as _).execute(&mut *transaction).await.to_racetime()?;
                if_chain! {
                    if let Some(guild_id) = event.discord_guild;
                    if let Some(scheduling_thread) = next_game.scheduling_thread;
                    // not automatically posting if the match might already be decided
                    //TODO remove this condition after implementing handling for decided matches (see TODO comment above)
                    if cal_event.race.game.expect("found next game for race without game number") <= cal_event.race.game_count(&mut transaction).await.to_racetime()? / 2;
                    let discord_ctx = ctx.global_state.discord_ctx.read().await;
                    let data = discord_ctx.data.read().await;
                    if let Some(command_ids) = data.get::<CommandIds>().and_then(|command_ids| command_ids.get(&guild_id).copied());
                    then {
                        let mut msg_ctx = draft::MessageContext::Discord {
                            teams: next_game.teams().cloned().collect(),
                            team: Team::dummy(),
                            transaction, guild_id, command_ids,
                        };
                        scheduling_thread.say(&*discord_ctx, draft.next_step(draft_kind, next_game.game, &mut msg_ctx).await.to_racetime()?.message).await.to_racetime()?;
                        transaction = msg_ctx.into_transaction();
                    }
                }
            }
        }
    }
    Ok(transaction)
}

async fn report_ffa(ctx: &RaceContext<GlobalState>, cal_event: &cal::Event, event: &event::Data<'_>, room: Url) -> Result<(), Error> {
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
        builder.push(room.to_string());
        builder.push('>');
        results_channel.say(&*ctx.global_state.discord_ctx.read().await, builder.build()).await.to_racetime()?;
    }
    Ok(())
}

impl Handler {
    pub(super) async fn check_tfb_finish(&self, ctx: &RaceContext<GlobalState>) -> Result<(), Error> {
        let data = ctx.data().await;
        let Some(OfficialRaceData { ref cal_event, ref event, fpa_invoked, ref scores, .. }) = self.official_data else { return Ok(()) };
        if let Some(scores) = data.entrants.iter().map(|entrant| match entrant.status.value {
            EntrantStatusValue::Dnf => Some(tfb::Score::default()),
            EntrantStatusValue::Done => scores.get(&entrant.user.id).and_then(|&score| score),
            _ => None,
        }.map(|score| (entrant.user.id.clone(), score))).collect() {
            ctx.say("All scores received. Thank you for playing Triforce Blitz, see you next race!").await?;
            self.official_race_finished(ctx, data, cal_event, event, fpa_invoked, Some(scores)).await?;
        }
        Ok(())
    }

    pub(super) async fn official_race_finished(&self, ctx: &RaceContext<GlobalState>, data: RwLockReadGuard<'_, RaceData>, cal_event: &cal::Event, event: &event::Data<'_>, fpa_invoked: bool, tfb_scores: Option<HashMap<String, tfb::Score>>) -> Result<(), Error> {
        let stream_delay = match cal_event.race.entrants {
            Entrants::Open | Entrants::Count { .. } => event.open_stream_delay,
            Entrants::Two(_) | Entrants::Three(_) | Entrants::Named(_) => event.invitational_stream_delay,
        };
        sleep(stream_delay).await;
        let mut transaction = ctx.global_state.db_pool.begin().await.to_racetime()?;
        if cal_event.is_private_async_part() {
            ctx.say("@entrants Please remember to send the videos of your run to a tournament organizer.").await?;
            if let Some(organizer_channel) = event.discord_organizer_channel {
                organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, MessageBuilder::default()
                    //TODO mention organizer role
                    .push("first half of async finished: <https://")
                    .push(racetime_host())
                    .push(&ctx.data().await.url)
                    .push('>')
                    .build()
                ).await.to_racetime()?;
            }
        } else if fpa_invoked {
            if let Some(organizer_channel) = event.discord_organizer_channel {
                let mut msg = MessageBuilder::default();
                //TODO mention organizer role
                msg.push("race finished with FPA call: <https://");
                msg.push(racetime_host());
                msg.push(&ctx.data().await.url);
                msg.push('>');
                if event.discord_race_results_channel.is_some() || matches!(cal_event.race.source, cal::Source::StartGG { .. }) {
                    msg.push(" — please manually ");
                    if let Some(results_channel) = event.discord_race_results_channel {
                        msg.push("post the announcement in ");
                        msg.mention(&results_channel);
                    }
                    if let Some(startgg_set_url) = cal_event.race.startgg_set_url().to_racetime()? {
                        if event.discord_race_results_channel.is_some() {
                            msg.push(" and ");
                        }
                        msg.push_named_link_no_preview("report the result on start.gg", startgg_set_url);
                    }
                    msg.push(" after adjusting the times");
                }
                //TODO note to manually initialize high seed for next game's draft (if any) and use `/post-status`
                organizer_channel.say(&*ctx.global_state.discord_ctx.read().await, msg.build()).await.to_racetime()?;
            }
        } else {
            match event.team_config {
                TeamConfig::Solo => match cal_event.race.entrants {
                    Entrants::Open | Entrants::Count { .. } => {
                        let room = Url::parse(&format!("https://{}{}", racetime_host(), data.url)).to_racetime()?;
                        report_ffa(ctx, cal_event, event, room).await?;
                    }
                    Entrants::Named(_) => unimplemented!(),
                    Entrants::Two(_) | Entrants::Three(_) => {
                        let room = Url::parse(&format!("https://{}{}", racetime_host(), data.url)).to_racetime()?;
                        if let Some(mut tfb_scores) = tfb_scores {
                            let mut teams = Vec::with_capacity(data.entrants.len());
                            for entrant in &data.entrants {
                                let user = User::from_racetime(&mut *transaction, &entrant.user.id).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                let team = Team::from_event_and_member(&mut transaction, event.series, &event.event, user.id).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                teams.push((team, tfb_scores.remove(&entrant.user.id).expect("missing TFB score"), room.clone()));
                            }
                            if let Ok(teams) = teams.try_into() {
                                transaction = report_1v1(transaction, ctx, cal_event, event, teams).await?;
                            } else { //TODO separate function for reporting 3-entrant results
                                report_ffa(ctx, cal_event, event, room).await?;
                            }
                        } else {
                            let mut teams = Vec::with_capacity(data.entrants.len());
                            for entrant in &data.entrants {
                                let user = User::from_racetime(&mut *transaction, &entrant.user.id).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                let team = Team::from_event_and_member(&mut transaction, event.series, &event.event, user.id).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?;
                                teams.push((team, entrant.finish_time, room.clone()));
                            }
                            if let Ok(teams) = teams.try_into() {
                                transaction = report_1v1(transaction, ctx, cal_event, event, teams).await?;
                            } else { //TODO separate function for reporting 3-entrant results
                                report_ffa(ctx, cal_event, event, room).await?;
                            }
                        }
                    }
                },
                TeamConfig::Pictionary => unimplemented!(), //TODO calculate like solo but report as teams
                _ => match cal_event.race.entrants {
                    Entrants::Open | Entrants::Count { .. } => {
                        let room = Url::parse(&format!("https://{}{}", racetime_host(), data.url)).to_racetime()?;
                        report_ffa(ctx, cal_event, event, room).await?;
                    }
                    Entrants::Named(_) => unimplemented!(),
                    Entrants::Two(_) | Entrants::Three(_) => {
                        let mut team_times = HashMap::<_, Vec<_>>::default();
                        let mut team_rooms = HashMap::new();
                        if cal_event.is_public_async_part() {
                            #[derive(Debug, thiserror::Error)]
                            #[error("ExactlyOneError while formatting result of last async half")]
                            struct ExactlyOneError;

                            for private_async_part in cal_event.race.cal_events().filter(|cal_event| cal_event.is_private_async_part()) {
                                if let Some(ref room) = private_async_part.room() {
                                    let nonactive_team = private_async_part.active_teams().exactly_one().map_err(|_| Error::Custom(Box::new(ExactlyOneError)))?;
                                    let data = ctx.global_state.http_client.get(format!("{}/data", room.to_string()))
                                        .send().await?
                                        .detailed_error_for_status().await.to_racetime()?
                                        .json_with_text_in_error::<RaceData>().await.to_racetime()?;
                                    team_rooms.insert(nonactive_team.racetime_slug.clone().expect("non-racetime.gg team"), Url::clone(room));
                                    for entrant in &data.entrants {
                                        team_times.entry(nonactive_team.racetime_slug.clone().expect("non-racetime.gg team")).or_default().push(entrant.finish_time);
                                    }
                                }
                            }
                            let active_team = cal_event.active_teams().exactly_one().map_err(|_| Error::Custom(Box::new(ExactlyOneError)))?;
                            team_rooms.insert(active_team.racetime_slug.clone().expect("non-racetime.gg team"), Url::parse(&format!("https://{}{}", racetime_host(), data.url)).to_racetime()?);
                            for entrant in &data.entrants {
                                team_times.entry(active_team.racetime_slug.clone().expect("non-racetime.gg team")).or_default().push(entrant.finish_time);
                            }
                        } else {
                            for entrant in &data.entrants {
                                if let Some(ref team) = entrant.team {
                                    if let hash_map::Entry::Vacant(entry) = team_rooms.entry(team.slug.clone()) {
                                        entry.insert(Url::parse(&format!("https://{}{}", racetime_host(), data.url)).to_racetime()?);
                                    }
                                    team_times.entry(team.slug.clone()).or_default().push(entrant.finish_time);
                                } else {
                                    unimplemented!("solo runner in team race") //TODO report error in organizer channel
                                }
                            }
                        }
                        let mut teams = Vec::with_capacity(team_times.len());
                        for (team_slug, times) in team_times {
                            teams.push((
                                Team::from_racetime(&mut transaction, event.series, &event.event, &team_slug).await.to_racetime()?.ok_or_else(|| Error::Custom(Box::new(sqlx::Error::RowNotFound)))?,
                                times.iter().try_fold(Duration::default(), |acc, &time| Some(acc + time?)).map(|total| total / u32::try_from(times.len()).expect("too many team members")),
                                team_rooms.remove(&team_slug).expect("each team should have a room"),
                            ));
                        }
                        if let Ok(teams) = teams.try_into() {
                            transaction = report_1v1(transaction, ctx, cal_event, event, teams).await?;
                        } else { //TODO separate function for reporting 3-entrant results
                            let room = Url::parse(&format!("https://{}{}", racetime_host(), data.url)).to_racetime()?;
                            report_ffa(ctx, cal_event, event, room).await?;
                        }
                    }
                },
            }
        }
        transaction.commit().await.to_racetime()?;
        Ok(())
    }
}
