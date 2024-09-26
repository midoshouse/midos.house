use {
    std::hash::Hasher,
    noisy_float::prelude::*,
    racetime::model::RaceStatusValue,
    crate::{
        event::{
            Data,
            DataError,
            Role,
            SignupStatus,
            Tab,
        },
        prelude::*,
    },
};

#[derive(Clone, Copy)]
pub(crate) enum QualifierKind {
    None,
    Rank,
    Single {
        show_times: bool,
    },
    Standard,
    Sgl2023Online,
    Sgl2024Online,
    SongsOfHope,
}

pub(crate) enum MemberUser {
    MidosHouse(User),
    RaceTime {
        id: String,
        url: String,
        name: String,
    },
}

impl PartialEq for MemberUser {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MidosHouse(user1), Self::MidosHouse(user2)) => user1.id == user2.id,
            (Self::MidosHouse(_), Self::RaceTime { .. }) | (Self::RaceTime { .. }, Self::MidosHouse(_)) => false,
            (Self::RaceTime { id: id1, .. }, Self::RaceTime { id: id2, .. }) => id1 == id2,
        }
    }
}

impl Eq for MemberUser {}

impl Hash for MemberUser {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::MidosHouse(user) => {
                false.hash(state);
                user.id.hash(state);
            }
            Self::RaceTime { id, .. } => {
                true.hash(state);
                id.hash(state);
            }
        }
    }
}

impl PartialOrd for MemberUser {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MemberUser {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::MidosHouse(user1), Self::MidosHouse(user2)) => user1.id.cmp(&user2.id),
            (Self::MidosHouse(_), Self::RaceTime { .. }) => Less,
            (Self::RaceTime { .. }, Self::MidosHouse(_)) => Greater,
            (Self::RaceTime { id: id1, .. }, Self::RaceTime { id: id2, .. }) => id1.cmp(id2),
        }
    }
}

pub(crate) struct SignupsMember {
    role: Role,
    pub(crate) user: MemberUser,
    is_confirmed: bool,
    pub(crate) qualifier_time: Option<Duration>,
    qualifier_vod: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum Qualification {
    Single {
        qualified: bool,
    },
    TriforceBlitz {
        qualified: bool,
        pieces: i16,
    },
    Multiple {
        num_qualifiers: usize,
        score: R64,
    },
}

pub(crate) struct SignupsTeam {
    pub(crate) team: Option<Team>,
    pub(crate) members: Vec<SignupsMember>,
    pub(crate) qualification: Qualification,
    hard_settings_ok: bool,
    mq_ok: bool,
}

pub(crate) async fn signups_sorted(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, me: Option<&User>, data: &Data<'_>, qualifier_kind: QualifierKind) -> Result<Vec<SignupsTeam>, cal::Error> {
    let mut signups = match qualifier_kind {
        QualifierKind::Standard | QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online => {
            let mut scores = HashMap::<_, Vec<_>>::default();
            for race in Race::for_event(transaction, http_client, data).await? {
                if race.phase.as_ref().map_or(true, |phase| phase != "Qualifier") { continue }
                let Ok(room) = race.rooms().exactly_one() else { continue };
                let room_data = http_client.get(format!("{room}/data"))
                    .send().await?
                    .detailed_error_for_status().await?
                    .json_with_text_in_error::<RaceData>().await?;
                if room_data.status.value != RaceStatusValue::Finished { continue }
                let mut entrants = room_data.entrants;
                if let QualifierKind::Sgl2023Online = qualifier_kind {
                    entrants.retain(|entrant| entrant.user.id != "yMewn83Vj3405Jv7"); // user was banned
                    if race.id == Id::from(17171498007470059483_u64) {
                        entrants.retain(|entrant| entrant.user.id != "JrM6PoY6LQWRdm5v"); // result was annulled
                    }
                }
                entrants.sort_unstable_by_key(|entrant| (entrant.finish_time.is_none(), entrant.finish_time));
                let num_entrants = entrants.len();
                let finish_times = entrants.iter().filter_map(|entrant| entrant.finish_time).collect_vec();
                for entrant in entrants {
                    scores.entry(MemberUser::RaceTime {
                        id: entrant.user.id,
                        url: entrant.user.url,
                        name: entrant.user.name,
                    }).or_default().push(r64(if let Some(finish_time) = entrant.finish_time {
                        match qualifier_kind {
                            QualifierKind::Standard => {
                                // https://docs.google.com/document/d/1IHrOGxFQpt3HpQ-9kQ6AVAARc04x6c96N1aHnHfHaKM/edit
                                let t_average = finish_times[0..7].iter().sum::<Duration>() / 7;
                                let t_j_h = Duration::from_secs(8 * 60).mul_f64(1.0.min(0.0.max((Duration::from_secs((2 * 60 + 30) * 60) - t_average).div_duration_f64(Duration::from_secs((2 * 60 + 30) * 60) - Duration::from_secs((60 + 40) * 60)))));
                                let t_jet = Duration::from_secs(8 * 60).min(t_j_h.mul_f64(0.0.max((finish_time - t_average).div_duration_f64(Duration::from_secs(8 * 60)) * 0.35)));
                                let t_g_h = Duration::from_secs_f64(finish_times[0..7].iter().map(|finish_time| finish_time.abs_diff(t_average).as_secs_f64().powi(2)).sum::<f64>() / 7.0);
                                let sigma_finish = t_g_h.div_duration_f64(t_average);
                                let t_gamble = Duration::from_secs(5 * 60).min(t_g_h.mul_f64(0.0.max((finish_time - t_average).div_duration_f64(t_g_h) * 0.0.max(sigma_finish / 0.035 - 1.0) * 0.3)));
                                ((1.0 - (finish_time - t_average - t_jet - t_gamble).div_duration_f64(t_average)) * 1000.0).clamp(100.0, 1100.0)
                            }
                            QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online => {
                                let par_cutoff = if num_entrants < 20 { 3 } else { 4 };
                                let par_time = finish_times[0..par_cutoff].iter().sum::<Duration>() / par_cutoff as u32;
                                (100.0 * (2.0 - (finish_time.as_secs_f64() / par_time.as_secs_f64()))).clamp(10.0, 110.0)
                            }
                            _ => unreachable!("checked by outer match"),
                        }
                    } else {
                        0.0
                    }));
                }
            }
            let teams = Team::for_event(&mut *transaction, data.series, &data.event).await?;
            let scores = if data.is_started(&mut *transaction).await? {
                let mut entrant_scores = Vec::with_capacity(teams.len());
                for team in &teams {
                    let user = team.members(&mut *transaction).await?.into_iter().exactly_one().expect("SGL-style qualifiers in team-based event");
                    let id = user.racetime.as_ref().expect("SGL-style qualifiers with entrant without racetime.gg account").id.clone();
                    entrant_scores.push((MemberUser::MidosHouse(user), scores.remove(&MemberUser::RaceTime { id, url: String::default(), name: String::default() }).expect("Unqualified entrant with SGL-style qualifiers")));
                }
                Either::Left(entrant_scores.into_iter())
            } else {
                let opt_outs = sqlx::query_scalar!("SELECT racetime_id FROM opt_outs WHERE series = $1 AND event = $2", data.series as _, &data.event).fetch_all(&mut **transaction).await?;
                Either::Right(
                    scores.into_iter()
                        .filter(move |(user, _)| match user {
                            MemberUser::RaceTime { id, .. } => !opt_outs.contains(id),
                            MemberUser::MidosHouse(_) => true,
                        })
                )
            };
            let mut signups = Vec::with_capacity(scores.size_hint().0);
            for (user, mut scores) in scores {
                signups.push(SignupsTeam {
                    team: None, //TODO
                    members: vec![SignupsMember {
                        role: Role::None,
                        is_confirmed: match &user {
                            MemberUser::MidosHouse(user) => 'is_confirmed: {
                                for team in &teams {
                                    if team.member_ids(&mut *transaction).await?.contains(&user.id) {
                                        break 'is_confirmed true
                                    }
                                }
                                false
                            }
                            MemberUser::RaceTime { id, .. } => 'is_confirmed: {
                                for team in &teams {
                                    if team.members(&mut *transaction).await?.iter().any(|member| member.racetime.as_ref().is_some_and(|racetime| racetime.id == *id)) {
                                        break 'is_confirmed true
                                    }
                                }
                                false
                            }
                        },
                        qualifier_time: None,
                        qualifier_vod: None,
                        user,
                    }],
                    qualification: match qualifier_kind {
                        QualifierKind::Standard => {
                            let num_qualifiers = scores.len();
                            scores.truncate(8); // only count the first 8 qualifiers chronologically
                            scores.sort_unstable();
                            if num_qualifiers >= 5 {
                                scores.pop(); // remove best score
                            }
                            scores.truncate(4); // remove up to 3 worst scores
                            Qualification::Multiple {
                                num_qualifiers,
                                score: scores.iter().copied().sum::<R64>(), // overall score is sum of remaining scores
                            }
                        }
                        QualifierKind::Sgl2023Online => {
                            let num_qualifiers = scores.len();
                            scores.truncate(5); // only count the first 5 qualifiers chronologically
                            scores.sort_unstable();
                            if num_qualifiers >= 4 {
                                scores.pop(); // remove best score
                            }
                            if num_qualifiers >= 5 {
                                scores.swap_remove(0); // remove worst score
                            }
                            Qualification::Multiple {
                                num_qualifiers,
                                score: scores.iter().copied().sum::<R64>() / r64(scores.len().max(3) as f64), // overall score is average of remaining scores
                            }
                        }
                        QualifierKind::Sgl2024Online => {
                            let num_qualifiers = scores.len();
                            scores.truncate(6); // only count the first 6 qualifiers chronologically
                            scores.sort_unstable();
                            if num_qualifiers >= 4 {
                                scores.swap_remove(0); // remove worst score
                            }
                            Qualification::Multiple {
                                num_qualifiers,
                                score: scores.iter().copied().sum::<R64>() / r64(scores.len().max(3) as f64), // overall score is average of remaining scores
                            }
                        }
                        _ => unreachable!("checked by outer match"),
                    },
                    hard_settings_ok: false,
                    mq_ok: false,
                });
            }
            signups
        }
        QualifierKind::SongsOfHope => {
            #[derive(Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
            enum QualificationLevel {
                Qualified,
                ChoppinBlock,
                #[default]
                None,
            }

            let mut entrant_data = HashMap::<_, (u8, _)>::default();
            for race in Race::for_event(transaction, http_client, data).await? {
                if race.phase.as_ref().map_or(true, |phase| phase != "Qualifier") { continue }
                let Ok(room) = race.rooms().exactly_one() else { continue };
                let room_data = http_client.get(format!("{room}/data"))
                    .send().await?
                    .detailed_error_for_status().await?
                    .json_with_text_in_error::<RaceData>().await?;
                if room_data.status.value != RaceStatusValue::Finished { continue }
                let mut entrants = room_data.entrants;
                entrants.retain(|entrant| entrant_data.entry(MemberUser::RaceTime {
                    id: entrant.user.id.clone(),
                    url: entrant.user.url.clone(),
                    name: entrant.user.name.clone(),
                }).or_default().0 < 2);
                let num_entrants = entrants.len();
                entrants.sort_unstable_by_key(|entrant| (entrant.finish_time.is_none(), entrant.finish_time));
                for (placement, entrant) in entrants.into_iter().enumerate() {
                    let (num_qualifiers, qualification_level) = entrant_data.entry(MemberUser::RaceTime {
                        id: entrant.user.id,
                        url: entrant.user.url,
                        name: entrant.user.name,
                    }).or_default();
                    if *num_qualifiers < 2 {
                        *num_qualifiers += 1;
                        *qualification_level = if placement < 3 {
                            QualificationLevel::Qualified
                        } else if placement < num_entrants / 2 {
                            QualificationLevel::ChoppinBlock
                        } else {
                            QualificationLevel::None
                        }.min(*qualification_level);
                    }
                }
            }
            let num_qualified = entrant_data.values().filter(|(_, qualification_level)| *qualification_level == QualificationLevel::Qualified).count();
            let choppin_block_finished = if_chain! {
                if let Ok(race) = Race::for_event(transaction, http_client, data).await?.into_iter()
                    .filter(|race| race.phase.as_ref().is_some_and(|phase| phase == "Choppin Block"))
                    .exactly_one();
                if let Ok(room) = race.rooms().exactly_one();
                let room_data = http_client.get(format!("{room}/data"))
                    .send().await?
                    .detailed_error_for_status().await?
                    .json_with_text_in_error::<RaceData>().await?;
                if room_data.status.value == RaceStatusValue::Finished;
                then {
                    let mut entrants = room_data.entrants;
                    entrants.retain(|entrant| entrant_data.entry(MemberUser::RaceTime {
                        id: entrant.user.id.clone(),
                        url: entrant.user.url.clone(),
                        name: entrant.user.name.clone(),
                    }).or_default().1 == QualificationLevel::ChoppinBlock);
                    entrants.sort_unstable_by_key(|entrant| (entrant.finish_time.is_none(), entrant.finish_time));
                    for entrant in entrants.drain(..entrants.len().min(32 - num_qualified)) {
                        entrant_data.entry(MemberUser::RaceTime {
                            id: entrant.user.id,
                            url: entrant.user.url,
                            name: entrant.user.name,
                        }).or_default().1 = QualificationLevel::Qualified;
                    }
                    true
                } else {
                    false
                }
            };
            entrant_data.into_iter()
                .filter(|(_, (_, qualification_level))|
                    *qualification_level == QualificationLevel::Qualified
                    || !choppin_block_finished && *qualification_level == QualificationLevel::ChoppinBlock
                )
                .map(|(user, (_, qualification_level))| SignupsTeam {
                    team: None, //TODO
                    members: vec![SignupsMember {
                        role: Role::None,
                        is_confirmed: false, //TODO
                        qualifier_time: None,
                        qualifier_vod: None,
                        user,
                    }],
                    qualification: Qualification::Single { qualified: qualification_level == QualificationLevel::Qualified },
                    hard_settings_ok: false,
                    mq_ok: false,
                }).collect()
        }
        QualifierKind::None | QualifierKind::Rank | QualifierKind::Single { .. } => {
            struct TeamRow {
                team: Team,
                hard_settings_ok: bool,
                mq_ok: bool,
                pieces: Option<i16>,
                qualified: bool,
            }

            let teams = if let QualifierKind::Rank = qualifier_kind {
                // teams are manually ranked so include ones that haven't submitted qualifier asyncs
                sqlx::query!(r#"SELECT id AS "id: Id<Teams>", name, racetime_slug, startgg_id AS "startgg_id: startgg::ID", plural_name, hard_settings_ok, mq_ok, restream_consent, mw_impl AS "mw_impl: mw::Impl", qualifier_rank FROM teams WHERE
                    series = $1
                    AND event = $2
                    AND NOT resigned
                    AND (
                        EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                        OR NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                    )
                "#, data.series as _, &data.event, me.as_ref().map(|me| PgSnowflake(me.id)) as _).fetch(&mut **transaction)
                    .map_ok(|row| TeamRow {
                        team: Team {
                            id: row.id,
                            name: row.name,
                            racetime_slug: row.racetime_slug,
                            startgg_id: row.startgg_id,
                            plural_name: row.plural_name,
                            restream_consent: row.restream_consent,
                            mw_impl: row.mw_impl,
                            qualifier_rank: row.qualifier_rank,
                        },
                        hard_settings_ok: row.hard_settings_ok,
                        mq_ok: row.mq_ok,
                        pieces: None,
                        qualified: false,
                    })
                    .try_collect::<Vec<_>>().await?
            } else {
                sqlx::query!(r#"SELECT id AS "id: Id<Teams>", name, racetime_slug, startgg_id AS "startgg_id: startgg::ID", plural_name, submitted IS NOT NULL AS "qualified!", pieces, hard_settings_ok, mq_ok, restream_consent, mw_impl AS "mw_impl: mw::Impl", qualifier_rank FROM teams LEFT OUTER JOIN async_teams ON (id = team) WHERE
                    series = $1
                    AND event = $2
                    AND NOT resigned
                    AND (
                        EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                        OR NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                    )
                    AND (kind = 'qualifier' OR kind IS NULL)
                "#, data.series as _, &data.event, me.as_ref().map(|me| PgSnowflake(me.id)) as _).fetch(&mut **transaction)
                    .map_ok(|row| TeamRow {
                        team: Team {
                            id: row.id,
                            name: row.name,
                            racetime_slug: row.racetime_slug,
                            startgg_id: row.startgg_id,
                            plural_name: row.plural_name,
                            restream_consent: row.restream_consent,
                            mw_impl: row.mw_impl,
                            qualifier_rank: row.qualifier_rank,
                        },
                        hard_settings_ok: row.hard_settings_ok,
                        mq_ok: row.mq_ok,
                        pieces: row.pieces,
                        qualified: row.qualified,
                    })
                    .try_collect().await?
            };
            let roles = data.team_config.roles();
            let mut signups = Vec::with_capacity(teams.len());
            for team in teams {
                let mut members = Vec::with_capacity(roles.len());
                for &(role, _) in roles {
                    let row = sqlx::query!(r#"
                        SELECT member AS "id: Id<Users>", status AS "status: SignupStatus", time, vod
                        FROM team_members LEFT OUTER JOIN async_players ON (member = player AND series = $1 AND event = $2 AND kind = 'qualifier')
                        WHERE team = $3 AND role = $4
                    "#, data.series as _, &data.event, team.team.id as _, role as _).fetch_one(&mut **transaction).await?;
                    let is_confirmed = row.status.is_confirmed();
                    let user = User::from_id(&mut **transaction, row.id).await?.ok_or(DataError::NonexistentUser)?;
                    members.push(SignupsMember {
                        user: MemberUser::MidosHouse(user),
                        qualifier_time: row.time.map(decode_pginterval).transpose().map_err(DataError::PgInterval)?,
                        qualifier_vod: row.vod,
                        role, is_confirmed,
                    });
                }
                signups.push(SignupsTeam {
                    team: Some(team.team),
                    qualification: if let Some(pieces) = team.pieces {
                        Qualification::TriforceBlitz { qualified: team.qualified, pieces }
                    } else {
                        Qualification::Single { qualified: team.qualified }
                    },
                    hard_settings_ok: team.hard_settings_ok,
                    mq_ok: team.mq_ok,
                    members,
                });
            }
            signups
        }
    };
    signups.sort_unstable_by(|SignupsTeam { team: team1, members: members1, qualification: qualification1, .. }, SignupsTeam { team: team2, members: members2, qualification: qualification2, .. }| {
        match qualifier_kind {
            QualifierKind::None | QualifierKind::Single { show_times: false } | QualifierKind::SongsOfHope => {
                let qualified1 = match qualification1 {
                    Qualification::Single { qualified } | Qualification::TriforceBlitz { qualified, .. } => qualified,
                    Qualification::Multiple { .. } => unreachable!("Qualification::Multiple in QualifierKind::{{None, Single}}"),
                };
                let qualified2 = match qualification2 {
                    Qualification::Single { qualified } | Qualification::TriforceBlitz { qualified, .. } => qualified,
                    Qualification::Multiple { .. } => unreachable!("Qualification::Multiple in QualifierKind::{{None, Single}}"),
                };
                qualified2.cmp(&qualified1) // reversed to list qualified teams first
                .then_with(|| team1.cmp(&team2))
            }
            QualifierKind::Rank => {
                team1.as_ref().map_or(true, |team1| team1.qualifier_rank.is_none()).cmp(&team2.as_ref().map_or(true, |team2| team2.qualifier_rank.is_none())) // list qualified teams first
                .then_with(|| team1.as_ref().and_then(|team1| team1.qualifier_rank).cmp(&team2.as_ref().and_then(|team2| team2.qualifier_rank)))
                .then_with(|| team1.cmp(&team2))
            }
            QualifierKind::Single { show_times: true } => {
                #[derive(PartialEq, Eq, PartialOrd, Ord)]
                enum QualificationOrder {
                    Finished(Option<i16>, Duration),
                    DidNotFinish,
                    NotYetQualified,
                }

                impl QualificationOrder {
                    fn new(qualification: Qualification, members: &[SignupsMember]) -> Self {
                        match qualification {
                            Qualification::Single { qualified: false } | Qualification::TriforceBlitz { qualified: false, .. } => Self::NotYetQualified,
                            Qualification::Single { qualified: true } => if let Some(time) = members.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)) {
                                Self::Finished(None, time)
                            } else {
                                Self::DidNotFinish
                            },
                            Qualification::TriforceBlitz { qualified: true, pieces } => if let Some(time) = members.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)) {
                                Self::Finished(
                                    Some(-pieces), // list teams with more pieces first
                                    time,
                                )
                            } else {
                                Self::DidNotFinish
                            },
                            Qualification::Multiple { .. } => unreachable!("Qualification::Multiple in QualifierKind::Single"),
                        }
                    }
                }

                QualificationOrder::new(*qualification1, members1).cmp(&QualificationOrder::new(*qualification2, members2))
                .then_with(|| team1.cmp(&team2))
            }
            QualifierKind::Standard | QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online => {
                let (num1, score1) = match *qualification1 {
                    Qualification::Multiple { num_qualifiers, score } => (num_qualifiers, score),
                    _ => unreachable!("QualifierKind::Multiple must use Qualification::Multiple"),
                };
                let (num2, score2) = match *qualification2 {
                    Qualification::Multiple { num_qualifiers, score } => (num_qualifiers, score),
                    _ => unreachable!("QualifierKind::Multiple must use Qualification::Multiple"),
                };
                num2.min(3).cmp(&num1.min(match qualifier_kind { // list racers closer to reaching the required number of qualifiers first
                    QualifierKind::Standard => 5,
                    QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online => 3,
                    _ => unreachable!("checked by outer match"),
                }))
                .then_with(|| score2.cmp(&score1)) // list racers with higher scores first
                .then_with(|| members1.iter().map(|member| &member.user).cmp(members2.iter().map(|member| &member.user)))
            }
        }
    });
    Ok(signups)
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Cal(#[from] cal::Error),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Event(#[from] event::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] PgInterval(#[from] PgIntervalDecodeError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
}

impl<E: Into<Error>> From<E> for StatusOrError<Error> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::get("/event/<series>/<event>/teams")]
pub(crate) async fn get(pool: &State<PgPool>, http_client: &State<reqwest::Client>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, **env, me.as_ref(), Tab::Teams, false).await?;
    let mut show_confirmed = false;
    let qualifier_kind = match (data.series, &*data.event) {
        (Series::SongsOfHope, "1") => QualifierKind::SongsOfHope,
        (Series::SpeedGaming, "2023onl" | "2024onl") | (Series::Standard, "8") => {
            show_confirmed = !data.is_started(&mut transaction).await? && Race::for_event(&mut transaction, http_client, &data).await?.into_iter().all(|race| race.phase.as_ref().map_or(true, |phase| phase != "Qualifier") || race.is_ended());
            match (data.series, &*data.event) {
                (Series::SpeedGaming, "2023onl") => QualifierKind::Sgl2023Online,
                (Series::SpeedGaming, "2024onl") => QualifierKind::Sgl2024Online,
                (Series::Standard, "8") => QualifierKind::Standard,
                _ => unreachable!("checked by outer match"),
            }
        }
        (_, _) => if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE series = $1 AND event = $2 AND qualifier_rank IS NOT NULL) AS "exists!""#, series as _, event).fetch_one(&mut *transaction).await? {
            QualifierKind::Rank
        } else if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM asyncs WHERE series = $1 AND event = $2 AND kind = 'qualifier') AS "exists!""#, series as _, event).fetch_one(&mut *transaction).await? {
            QualifierKind::Single {
                show_times: data.show_qualifier_times && (
                    sqlx::query_scalar!(r#"SELECT submitted IS NOT NULL AS "qualified!" FROM teams, async_teams, team_members WHERE async_teams.team = teams.id AND teams.series = $1 AND teams.event = $2 AND async_teams.team = team_members.team AND member = $3 AND kind = 'qualifier'"#, series as _, event, me.as_ref().map(|me| PgSnowflake(me.id)) as _).fetch_optional(&mut *transaction).await?.unwrap_or(false)
                    || data.is_started(&mut transaction).await?
                ),
            }
        } else {
            QualifierKind::None
        },
    };
    let show_restream_consent = if let Some(ref me) = me {
        data.organizers(&mut transaction).await?.contains(me) || data.restreamers(&mut transaction).await?.contains(me)
    } else {
        false
    };
    let roles = data.team_config.roles();
    let signups = signups_sorted(&mut transaction, http_client, me.as_ref(), &data, qualifier_kind).await?;
    let mut footnotes = Vec::default();
    let teams_label = if let TeamConfig::Solo = data.team_config { "Entrants" } else { "Teams" };
    let mut column_headers = Vec::default();
    if let QualifierKind::Rank | QualifierKind::Standard | QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online = qualifier_kind {
        column_headers.push(html! {
            th : "Qualifier Rank";
        });
    }
    if !matches!(data.team_config, TeamConfig::Solo) {
        column_headers.push(html! {
            th : "Team Name";
        });
    }
    for &(role, display_name) in roles {
        column_headers.push(html! {
            th(class? = role.css_class().filter(|_| data.team_config.has_distinct_roles())) : display_name;
        });
    }
    match qualifier_kind {
        QualifierKind::None | QualifierKind::Rank => {}
        QualifierKind::Single { show_times: false } | QualifierKind::SongsOfHope => column_headers.push(html! {
            th : "Qualified";
        }),
        QualifierKind::Single { show_times: true } => if series == Series::TriforceBlitz {
            column_headers.push(html! {
                th : "Pieces Found";
            });
        }
        QualifierKind::Standard | QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online => {
            column_headers.push(html! {
                th : "# Qualifiers";
            });
            column_headers.push(html! {
                th : "Qualifier Points";
            });
        }
    }
    if show_confirmed {
        column_headers.push(html! {
            th : "Confirmed";
        });
    }
    if let Some(draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4) = data.draft_kind() {
        column_headers.push(html! {
            th : "Advanced Settings OK";
        });
        column_headers.push(html! {
            th : "MQ OK";
        });
    }
    if show_restream_consent {
        column_headers.push(html! {
            th : "Restream Consent";
        });
    }
    let content = html! {
        : header;
        table {
            thead {
                tr {
                    @for header in &column_headers {
                        : header;
                    }
                }
            }
            tbody {
                @if signups.is_empty() {
                    tr {
                        td(colspan = column_headers.len()) {
                            i : "(no signups yet)";
                        }
                    }
                } else {
                    @for (signup_idx, SignupsTeam { team, members, qualification, hard_settings_ok, mq_ok }) in signups.into_iter().enumerate() {
                        tr {
                            @match qualifier_kind {
                                QualifierKind::Rank => td : team.as_ref().and_then(|team| team.qualifier_rank);
                                QualifierKind::Standard | QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online => td : (signup_idx + 1).to_string();
                                _ => {}
                            }
                            @if !matches!(data.team_config, TeamConfig::Solo) {
                                td {
                                    @if let Some(ref team) = team {
                                        : team.to_html(&mut transaction, **env, false).await?;
                                    }
                                    @if let (QualifierKind::Single { show_times: true }, Qualification::Single { qualified: true } | Qualification::TriforceBlitz { qualified: true, .. }) = (qualifier_kind, qualification) {
                                        br;
                                        small {
                                            @if let Some(time) = members.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)) {
                                                : English.format_duration(time / u32::try_from(members.len()).expect("too many team members"), false);
                                            } else {
                                                : "DNF";
                                            }
                                        }
                                    }
                                }
                            }
                            @for SignupsMember { role, user, is_confirmed, qualifier_time, qualifier_vod } in &members {
                                td(class? = role.css_class().filter(|_| data.team_config.has_distinct_roles())) {
                                    @match user {
                                        MemberUser::MidosHouse(user) => {
                                            : user;
                                            @if let Some(ref team) = team {
                                                @if *is_confirmed {
                                                    @if me.as_ref().map_or(false, |me| me == user) && members.iter().any(|member| !member.is_confirmed) {
                                                        : " ";
                                                        span(class = "button-row") {
                                                            form(action = uri!(crate::event::resign_post(series, event, team.id)).to_string(), method = "post") {
                                                                : csrf;
                                                                input(type = "submit", value = "Retract");
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    : " ";
                                                    @if me.as_ref().map_or(false, |me| me == user) {
                                                        span(class = "button-row") {
                                                            form(action = uri!(crate::event::confirm_signup(series, event, team.id)).to_string(), method = "post") {
                                                                : csrf;
                                                                input(type = "submit", value = "Accept");
                                                            }
                                                            form(action = uri!(crate::event::resign_post(series, event, team.id)).to_string(), method = "post") {
                                                                : csrf;
                                                                input(type = "submit", value = "Decline");
                                                            }
                                                            //TODO options to block sender or event
                                                        }
                                                    } else {
                                                        : "(unconfirmed)";
                                                    }
                                                }
                                            }
                                            @if let (QualifierKind::Single { show_times: true }, Qualification::Single { qualified: true } | Qualification::TriforceBlitz { qualified: true, .. }) = (qualifier_kind, qualification) {
                                                br;
                                                small {
                                                    @let time = if let Some(time) = qualifier_time { English.format_duration(*time, false) } else { format!("DNF") };
                                                    @if let Some(vod) = qualifier_vod {
                                                        @if let Some(Ok(vod_url)) = (!vod.contains(' ')).then(|| Url::parse(vod)) {
                                                            a(href = vod_url.to_string()) : time;
                                                        } else {
                                                            : time;
                                                            sup {
                                                                @let footnote_id = { footnotes.push(vod.clone()); footnotes.len() };
                                                                a(href = format!("#footnote{footnote_id}")) {
                                                                    : "[";
                                                                    : footnote_id;
                                                                    : "]";
                                                                }
                                                            };
                                                        }
                                                    } else {
                                                        : time;
                                                    }
                                                }
                                            }
                                        }
                                        MemberUser::RaceTime { url, name, .. } => a(href = format!("https://{}{url}", env.racetime_host())) : name;
                                    }
                                }
                            }
                            @match (qualifier_kind, qualification) {
                                (QualifierKind::None, _) | (QualifierKind::Rank, _) | (QualifierKind::Single { show_times: true }, Qualification::Single { .. }) => {}
                                (QualifierKind::Single { show_times: false } | QualifierKind::SongsOfHope, Qualification::Single { qualified } | Qualification::TriforceBlitz { qualified, .. }) => td {
                                    @if qualified {
                                        : "✓";
                                    }
                                }
                                (QualifierKind::Single { show_times: true }, Qualification::TriforceBlitz { pieces, .. }) => td : pieces;
                                (QualifierKind::Standard | QualifierKind::Sgl2023Online | QualifierKind::Sgl2024Online, Qualification::Multiple { num_qualifiers, score }) => {
                                    td(style = "text-align: right;") : num_qualifiers;
                                    td(style = "text-align: right;") : format!("{score:.2}");
                                }
                                (_, _) => @unreachable
                            }
                            @if show_confirmed {
                                td {
                                    @if members.iter().all(|member| member.is_confirmed) {
                                        : "✓";
                                    }
                                }
                            }
                            @if let Some(draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4) = data.draft_kind() {
                                td {
                                    @if hard_settings_ok {
                                        : "✓";
                                    }
                                }
                                td {
                                    @if mq_ok {
                                        : "✓";
                                    }
                                }
                            }
                            @if show_restream_consent {
                                td {
                                    @if team.map_or(false, |team| team.restream_consent) {
                                        : "✓";
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        @for (i, footnote) in footnotes.into_iter().enumerate() {
            p(id = format!("footnote{}", i + 1)) {
                : "[";
                : i + 1;
                : "]";
                @for word in footnote.split(' ') {
                    : " ";
                    @if let Ok(word_url) = Url::parse(word) {
                        a(href = word_url.to_string()) : word;
                    } else {
                        : word;
                    }
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(**env).await?, ..PageStyle::default() }, &format!("{teams_label} — {}", data.display_name), content).await?)
}
