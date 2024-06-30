use {
    std::cmp::{
        max_by_key,
        min_by_key,
    },
    ics::{
        ICalendar,
        parameters::TzIDParam,
        properties::{
            Description,
            DtEnd,
            DtStart,
            RRule,
            Summary,
            URL,
        },
    },
    reqwest::StatusCode,
    rocket_util::Response,
    sqlx::types::Json,
    yup_oauth2::{
        ServiceAccountAuthenticator,
        read_service_account_key,
    },
    crate::{
        discord_bot,
        event::{
            Tab,
            teams::{
                self,
                SignupsTeam,
            },
        },
        prelude::*,
    },
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    Manual,
    League {
        id: i32,
    },
    Sheet {
        timestamp: NaiveDateTime,
    },
    StartGG {
        event: String,
        set: startgg::ID,
    },
}

#[derive(Clone)]
pub(crate) enum Entrant {
    MidosHouseTeam(Team),
    Discord {
        id: UserId,
        racetime_id: Option<String>,
        twitch_username: Option<String>,
    },
    Named {
        name: String,
        racetime_id: Option<String>,
        twitch_username: Option<String>,
    },
}

impl Entrant {
    pub(crate) async fn name(&self, transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx) -> Result<Option<Cow<'_, str>>, discord_bot::Error> {
        Ok(match self {
            Self::MidosHouseTeam(team) => team.name(transaction).await?,
            Self::Discord { id, .. } => if let Some(user) = User::from_discord(&mut **transaction, *id).await? {
                Some(Cow::Owned(user.discord.unwrap().display_name))
            } else {
                let user = id.to_user(discord_ctx).await?;
                Some(Cow::Owned(user.global_name.unwrap_or(user.name)))
            },
            Self::Named { name, .. } => Some(Cow::Borrowed(name)),
        })
    }

    pub(crate) async fn to_html(&self, transaction: &mut Transaction<'_, Postgres>, env: Environment, discord_ctx: &DiscordCtx, running_text: bool) -> Result<RawHtml<String>, discord_bot::Error> {
        Ok(match self {
            Self::MidosHouseTeam(team) => team.to_html(transaction, env, running_text).await?,
            Self::Discord { id, racetime_id, .. } => {
                let url = if let Some(racetime_id) = racetime_id {
                    format!("https://racetime.gg/user/{racetime_id}")
                } else {
                    format!("https://discord.com/users/{id}")
                };
                if let Some(user) = User::from_discord(&mut **transaction, *id).await? {
                    html! {
                        a(href = url) : user.discord.unwrap().display_name;
                    }
                } else {
                    let user = id.to_user(discord_ctx).await?;
                    html! {
                        a(href = url) : user.global_name.unwrap_or(user.name);
                    }
                }
            }
            Self::Named { name, racetime_id: Some(racetime_id), .. } => html! {
                a(href = format!("https://racetime.gg/user/{racetime_id}")) : name;
            },
            Self::Named { name, racetime_id: None, .. } => name.to_html(),
        })
    }
}

impl PartialEq for Entrant {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MidosHouseTeam(lhs), Self::MidosHouseTeam(rhs)) => lhs == rhs,
            (Self::Discord { id: lhs, .. }, Self::Discord { id: rhs, .. }) => lhs == rhs,
            (Self::Named { name: lhs, .. }, Self::Named { name: rhs, .. }) => lhs == rhs,
            (Self::MidosHouseTeam(_), Self::Discord { .. } | Self::Named { .. }) |
            (Self::Discord { .. }, Self::MidosHouseTeam(_) | Self::Named { .. }) |
            (Self::Named { .. }, Self::MidosHouseTeam(_) | Self::Discord { .. }) => false,
        }
    }
}

impl Eq for Entrant {}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Entrants {
    Open,
    Count {
        total: u32,
        finished: u32,
    },
    Named(String),
    Two([Entrant; 2]),
    Three([Entrant; 3]),
}

impl Entrants {
    fn to_db(&self) -> ([Option<Id<Teams>>; 3], [Option<&String>; 3], [Option<UserId>; 2], [Option<&String>; 2], [Option<&String>; 2], [Option<u32>; 2]) {
        match *self {
            Entrants::Open => ([None; 3], [None; 3], [None; 2], [None; 2], [None; 2], [None; 2]),
            Entrants::Count { total, finished } => ([None; 3], [None; 3], [None; 2], [None; 2], [None; 2], [Some(total), Some(finished)]),
            Entrants::Named(ref entrants) => ([None; 3], [Some(entrants), None, None], [None; 2], [None; 2], [None; 2], [None; 2]),
            Entrants::Two([ref p1, ref p2]) => {
                let (team1, p1, p1_discord, p1_racetime, p1_twitch) = match p1 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None, None, None, None),
                    Entrant::Discord { id, racetime_id, twitch_username } => (None, None, Some(*id), racetime_id.as_ref(), twitch_username.as_ref()),
                    Entrant::Named { name, racetime_id, twitch_username } => (None, Some(name), None, racetime_id.as_ref(), twitch_username.as_ref()),
                };
                let (team2, p2, p2_discord, p2_racetime, p2_twitch) = match p2 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None, None, None, None),
                    Entrant::Discord { id, racetime_id, twitch_username } => (None, None, Some(*id), racetime_id.as_ref(), twitch_username.as_ref()),
                    Entrant::Named { name, racetime_id, twitch_username } => (None, Some(name), None, racetime_id.as_ref(), twitch_username.as_ref()),
                };
                ([team1, team2, None], [p1, p2, None], [p1_discord, p2_discord], [p1_racetime, p2_racetime], [p1_twitch, p2_twitch], [None; 2])
            }
            Entrants::Three([ref p1, ref p2, ref p3]) => {
                let (team1, p1) = match p1 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None),
                    Entrant::Named { name, racetime_id: None, twitch_username: None } => (None, Some(name)),
                    _ => unimplemented!(), //TODO
                };
                let (team2, p2) = match p2 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None),
                    Entrant::Named { name, racetime_id: None, twitch_username: None } => (None, Some(name)),
                    _ => unimplemented!(), //TODO
                };
                let (team3, p3) = match p3 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None),
                    Entrant::Named { name, racetime_id: None, twitch_username: None } => (None, Some(name)),
                    _ => unimplemented!(), //TODO
                };
                ([team1, team2, team3], [p1, p2, p3], [None; 2], [None; 2], [None; 2], [None; 2])
            }
        }
    }
}

#[derive(Clone)]
pub(crate) enum RaceSchedule {
    Unscheduled,
    Live {
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
        room: Option<Url>,
    },
    Async {
        start1: Option<DateTime<Utc>>,
        start2: Option<DateTime<Utc>>,
        start3: Option<DateTime<Utc>>,
        end1: Option<DateTime<Utc>>,
        end2: Option<DateTime<Utc>>,
        end3: Option<DateTime<Utc>>,
        room1: Option<Url>,
        room2: Option<Url>,
        room3: Option<Url>,
    },
}

impl RaceSchedule {
    fn new(
        live_start: Option<DateTime<Utc>>, async_start1: Option<DateTime<Utc>>, async_start2: Option<DateTime<Utc>>, async_start3: Option<DateTime<Utc>>,
        live_end: Option<DateTime<Utc>>, async_end1: Option<DateTime<Utc>>, async_end2: Option<DateTime<Utc>>, async_end3: Option<DateTime<Utc>>,
        live_room: Option<Url>, async_room1: Option<Url>, async_room2: Option<Url>, async_room3: Option<Url>,
    ) -> Self {
        match (live_start, async_start1, async_start2, async_start3) {
            (None, None, None, None) => Self::Unscheduled,
            (Some(start), None, None, None) => Self::Live {
                end: live_end,
                room: live_room,
                start,
            },
            (None, start1, start2, start3) => Self::Async {
                end1: async_end1,
                end2: async_end2,
                end3: async_end3,
                room1: async_room1,
                room2: async_room2,
                room3: async_room3,
                start1, start2, start3,
            },
            (Some(_), _, _, _) => unreachable!("both live and async starts included, should be prevented by SQL constraint"),
        }
    }

    fn end_time(&self, entrants: &Entrants) -> Option<DateTime<Utc>> {
        match *self {
            Self::Unscheduled => None,
            Self::Live { end, .. } => end,
            Self::Async { end1, end2, end3, .. } => Some(if let Entrants::Three(_) = entrants {
                end1?.max(end2?).max(end3?)
            } else {
                end1?.max(end2?)
            }),
        }
    }

    fn start_matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unscheduled, Self::Unscheduled) => true,
            (Self::Live { start: start_a, .. }, Self::Live { start: start_b, .. }) => start_a == start_b,
            (Self::Async { start1: start_a1, start2: start_a2, start3: start_a3, .. }, Self::Async { start1: start_b1, start2: start_b2, start3: start_b3, .. }) => start_a1 == start_b1 && start_a2 == start_b2 && start_a3 == start_b3,
            (Self::Unscheduled, _) | (Self::Live { .. }, _) | (Self::Async { .. }, _) => false, // ensure compile error on missing variants by listing each left-hand side individually
        }
    }

    fn cmp(&self, entrants_a: &Entrants, other: &Self, entrants_b: &Entrants) -> Ordering {
        let (mut starts_a, end_a) = match *self {
            Self::Unscheduled => ([None; 3], None),
            Self::Live { start, end, .. } => ([Some(start); 3], end),
            Self::Async { start1, start2, start3, end1, end2, end3, .. } => ([start1, start2, start3], if let Entrants::Three(_) = entrants_a {
                end1.and_then(|end1| Some(end1.max(end2?).max(end3?)))
            } else {
                end1.and_then(|end1| Some(end1.max(end2?)))
            }),
        };
        let (mut starts_b, end_b) = match *other {
            Self::Unscheduled => ([None; 3], None),
            Self::Live { start, end, .. } => ([Some(start); 3], end),
            Self::Async { start1, start2, start3, end1, end2, end3, .. } => ([start1, start2, start3], if let Entrants::Three(_) = entrants_b {
                end1.and_then(|end1| Some(end1.max(end2?).max(end3?)))
            } else {
                end1.and_then(|end1| Some(end1.max(end2?)))
            }),
        };
        let mut ordering = end_a.is_none().cmp(&end_b.is_none()) // races that have ended first
            .then_with(|| end_a.cmp(&end_b)); // races that ended earlier first
        if ordering.is_eq() {
            starts_a.sort_unstable();
            starts_b.sort_unstable();
            for (start_a, start_b) in starts_a.into_iter().zip_eq(starts_b) {
                ordering = ordering.then_with(|| start_a.is_none().cmp(&start_b.is_none())) // races with more starting times first
                    .then_with(|| start_a.cmp(&start_b)); // races with parts starting earlier first
            }
        }
        ordering
    }

    pub(crate) fn set_live_start(&mut self, new_start: DateTime<Utc>) {
        match self {
            Self::Live { start, .. } => *start = new_start,
            _ => *self = Self::Live { start: new_start, end: None, room: None },
        }
    }

    pub(crate) fn set_async_start1(&mut self, new_start: DateTime<Utc>) {
        match self {
            Self::Async { start1, .. } => *start1 = Some(new_start),
            _ => *self = Self::Async { start1: Some(new_start), start2: None, start3: None, end1: None, end2: None, end3: None, room1: None, room2: None, room3: None },
        }
    }

    pub(crate) fn set_async_start2(&mut self, new_start: DateTime<Utc>) {
        match self {
            Self::Async { start2, .. } => *start2 = Some(new_start),
            _ => *self = Self::Async { start1: None, start2: Some(new_start), start3: None, end1: None, end2: None, end3: None, room1: None, room2: None, room3: None },
        }
    }

    pub(crate) fn set_async_start3(&mut self, new_start: DateTime<Utc>) {
        match self {
            Self::Async { start3, .. } => *start3 = Some(new_start),
            _ => *self = Self::Async { start1: None, start2: None, start3: Some(new_start), end1: None, end2: None, end3: None, room1: None, room2: None, room3: None },
        }
    }
}

#[derive(Clone)]
pub(crate) struct Race {
    pub(crate) id: Id<Races>,
    pub(crate) series: Series,
    pub(crate) event: String,
    pub(crate) source: Source,
    pub(crate) entrants: Entrants,
    pub(crate) phase: Option<String>,
    pub(crate) round: Option<String>,
    pub(crate) game: Option<i16>,
    pub(crate) scheduling_thread: Option<ChannelId>,
    pub(crate) schedule: RaceSchedule,
    pub(crate) schedule_updated_at: Option<DateTime<Utc>>,
    pub(crate) draft: Option<Draft>,
    pub(crate) seed: seed::Data,
    pub(crate) video_urls: HashMap<Language, Url>,
    pub(crate) restreamers: HashMap<Language, String>,
    pub(crate) last_edited_by: Option<Id<Users>>,
    pub(crate) last_edited_at: Option<DateTime<Utc>>,
    pub(crate) ignored: bool,
    pub(crate) schedule_locked: bool,
}

impl Race {
    pub(crate) async fn from_id(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, id: Id<Races>) -> Result<Self, Error> {
        let row = sqlx::query!(r#"SELECT
            series AS "series: Series",
            event,
            league_id,
            sheet_timestamp,
            startgg_event,
            startgg_set AS "startgg_set: startgg::ID",
            game,
            team1 AS "team1: Id<Teams>",
            team2 AS "team2: Id<Teams>",
            team3 AS "team3: Id<Teams>",
            p1,
            p2,
            p3,
            p1_discord AS "p1_discord: PgSnowflake<UserId>",
            p2_discord AS "p2_discord: PgSnowflake<UserId>",
            p1_racetime,
            p2_racetime,
            p1_twitch,
            p2_twitch,
            total,
            finished,
            phase,
            round,
            scheduling_thread AS "scheduling_thread: PgSnowflake<ChannelId>",
            draft_state AS "draft_state: Json<Draft>",
            start,
            async_start1,
            async_start2,
            async_start3,
            end_time,
            async_end1,
            async_end2,
            async_end3,
            room,
            async_room1,
            async_room2,
            async_room3,
            schedule_updated_at,
            file_stem,
            locked_spoiler_log_path,
            web_id,
            web_gen_time,
            tfb_uuid,
            hash1 AS "hash1: HashIcon",
            hash2 AS "hash2: HashIcon",
            hash3 AS "hash3: HashIcon",
            hash4 AS "hash4: HashIcon",
            hash5 AS "hash5: HashIcon",
            video_url,
            restreamer,
            video_url_fr,
            restreamer_fr,
            video_url_de,
            restreamer_de,
            video_url_pt,
            restreamer_pt,
            last_edited_by AS "last_edited_by: Id<Users>",
            last_edited_at,
            ignored,
            schedule_locked
        FROM races WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
        let source = if let Some(id) = row.league_id {
            Source::League { id }
        } else if let Some(timestamp) = row.sheet_timestamp {
            Source::Sheet { timestamp }
        } else if let (Some(event), Some(set)) = (row.startgg_event, row.startgg_set) {
            Source::StartGG { event, set }
        } else {
            Source::Manual
        };
        let entrants = {
            let p1 = if let Some(team1) = row.team1 {
                Some(Entrant::MidosHouseTeam(Team::from_id(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?))
            } else if let Some(PgSnowflake(id)) = row.p1_discord {
                Some(Entrant::Discord {
                    racetime_id: row.p1_racetime,
                    twitch_username: row.p1_twitch,
                    id,
                })
            } else if let Some(name) = row.p1 {
                Some(Entrant::Named {
                    racetime_id: row.p1_racetime,
                    twitch_username: row.p1_twitch,
                    name,
                })
            } else {
                None
            };
            let p2 = if let Some(team2) = row.team2 {
                Some(Entrant::MidosHouseTeam(Team::from_id(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?))
            } else if let Some(PgSnowflake(id)) = row.p2_discord {
                Some(Entrant::Discord {
                    racetime_id: row.p2_racetime,
                    twitch_username: row.p2_twitch,
                    id,
                })
            } else if let Some(name) = row.p2 {
                Some(Entrant::Named {
                    racetime_id: row.p2_racetime,
                    twitch_username: row.p2_twitch,
                    name,
                })
            } else {
                None
            };
            let p3 = if let Some(team3) = row.team3 {
                Some(Entrant::MidosHouseTeam(Team::from_id(&mut *transaction, team3).await?.ok_or(Error::UnknownTeam)?))
            } else if let Some(name) = row.p3 {
                Some(Entrant::Named { racetime_id: None, twitch_username: None, name })
            } else {
                None
            };
            match [p1, p2, p3] {
                [Some(p1), Some(p2), Some(p3)] => Entrants::Three([p1, p2, p3]),
                [Some(p1), Some(p2), None] => Entrants::Two([p1, p2]),
                [Some(Entrant::Named { name, .. }), None, None] => Entrants::Named(name),
                [None, None, None] => if let (Some(total), Some(finished)) = (row.total, row.finished) {
                    Entrants::Count {
                        total: total as u32,
                        finished: finished as u32,
                    }
                } else {
                    Entrants::Open
                },
                _ => panic!("unexpected configuration of entrants"),
            }
        };

        macro_rules! update_end {
            ($var:ident, $room:ident, $query:literal) => {
                let $var = if let Some(end) = row.$var {
                    Some(end)
                } else if let Some(ref room) = row.$room {
                    let end = http_client.get(format!("{room}/data"))
                        .send().await?
                        .detailed_error_for_status().await?
                        .json_with_text_in_error::<RaceData>().await?
                        .ended_at;
                    if let Some(end) = end {
                        sqlx::query!($query, end, id as _).execute(&mut **transaction).await?;
                    }
                    end
                } else {
                    None
                };
            };
        }

        update_end!(end_time, room, "UPDATE races SET end_time = $1 WHERE id = $2");
        update_end!(async_end1, async_room1, "UPDATE races SET async_end1 = $1 WHERE id = $2");
        update_end!(async_end2, async_room2, "UPDATE races SET async_end2 = $1 WHERE id = $2");
        update_end!(async_end3, async_room3, "UPDATE races SET async_end3 = $1 WHERE id = $2");
        Ok(Self {
            series: row.series,
            event: row.event,
            phase: row.phase,
            round: row.round,
            game: row.game,
            scheduling_thread: row.scheduling_thread.map(|PgSnowflake(id)| id),
            schedule: RaceSchedule::new(
                row.start, row.async_start1, row.async_start2, row.async_start3,
                end_time, async_end1, async_end2, async_end3,
                row.room.map(|room| room.parse()).transpose()?, row.async_room1.map(|room| room.parse()).transpose()?, row.async_room2.map(|room| room.parse()).transpose()?, row.async_room3.map(|room| room.parse()).transpose()?,
            ),
            schedule_updated_at: row.schedule_updated_at,
            draft: row.draft_state.map(|Json(draft)| draft),
            seed: seed::Data::from_db(
                row.start,
                row.async_start1,
                row.async_start2,
                row.async_start3,
                row.file_stem,
                row.locked_spoiler_log_path,
                row.web_id,
                row.web_gen_time,
                row.tfb_uuid,
                row.hash1,
                row.hash2,
                row.hash3,
                row.hash4,
                row.hash5,
            ),
            video_urls: all().filter_map(|language| match language {
                English => row.video_url.clone(),
                French => row.video_url_fr.clone(),
                German => row.video_url_de.clone(),
                Portuguese => row.video_url_pt.clone(),
            }.map(|video_url| Ok::<_, Error>((language, video_url.parse()?)))).try_collect()?,
            restreamers: all().filter_map(|language| match language {
                English => row.restreamer.clone(),
                French => row.restreamer_fr.clone(),
                German => row.restreamer_de.clone(),
                Portuguese => row.restreamer_pt.clone(),
            }.map(|restreamer| (language, restreamer))).collect(),
            last_edited_by: row.last_edited_by,
            last_edited_at: row.last_edited_at,
            ignored: row.ignored,
            schedule_locked: row.schedule_locked,
            id, source, entrants,
        })
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, event: &event::Data<'_>) -> Result<Vec<Self>, Error> {
        async fn update_race(transaction: &mut Transaction<'_, Postgres>, found_race: &mut Race, race: Race) -> sqlx::Result<()> {
            if found_race.source != race.source { //TODO temporary code to add source data to existing races, remove once this has been applied
                let (league_id, sheet_timestamp, startgg_event, startgg_set) = match race.source {
                    Source::Manual => (None, None, None, None),
                    Source::League { id } => (Some(id), None, None, None),
                    Source::Sheet { timestamp } => (None, Some(timestamp), None, None),
                    Source::StartGG { ref event, ref set } => (None, None, Some(event), Some(set)),
                };
                sqlx::query!("UPDATE races SET league_id = $1, sheet_timestamp = $2, startgg_event = $3, startgg_set = $4 WHERE id = $5",
                    league_id,
                    sheet_timestamp,
                    startgg_event,
                    startgg_set as _,
                    found_race.id as _,
                ).execute(&mut **transaction).await?;
            }
            if !found_race.schedule.start_matches(&race.schedule) {
                match race.schedule {
                    RaceSchedule::Unscheduled => {
                        found_race.schedule = RaceSchedule::Unscheduled;
                        sqlx::query!("UPDATE races SET start = NULL, async_start1 = NULL, async_start2 = NULL, async_start3 = NULL WHERE id = $1", found_race.id as _).execute(&mut **transaction).await?;
                    }
                    RaceSchedule::Live { start, .. } => {
                        match found_race.schedule {
                            RaceSchedule::Unscheduled => found_race.schedule = race.schedule,
                            RaceSchedule::Live { start: ref mut old_start, .. } => *old_start = start,
                            RaceSchedule::Async { .. } => unimplemented!("race listed as async in database was rescheduled as live"), //TODO
                        }
                        sqlx::query!("UPDATE races SET start = $1, async_start1 = NULL, async_start2 = NULL, async_start3 = NULL WHERE id = $2", start, found_race.id as _).execute(&mut **transaction).await?;
                    },
                    RaceSchedule::Async { start1, start2, start3, .. } => {
                        match found_race.schedule {
                            RaceSchedule::Unscheduled => found_race.schedule = race.schedule,
                            RaceSchedule::Live { .. } => unimplemented!("race listed as live in database was rescheduled as async"), //TODO
                            RaceSchedule::Async { start1: ref mut old_start1, start2: ref mut old_start2, start3: ref mut old_start3, .. } => {
                                *old_start1 = start1;
                                *old_start2 = start2;
                                *old_start3 = start3;
                            }
                        }
                        sqlx::query!("UPDATE races SET start = NULL, async_start1 = $1, async_start2 = $2, async_start3 = $3 WHERE id = $4", start1, start2, start3, found_race.id as _).execute(&mut **transaction).await?;
                    }
                }
            }
            if race.video_urls.iter().any(|(language, new_url)| found_race.video_urls.get(language).map_or(true, |old_url| old_url != new_url)) {
                if found_race.video_urls.iter().all(|(language, old_url)| race.video_urls.get(language).map_or(true, |new_url| old_url == new_url)) { //TODO make sure manually entered restreams aren't changed automatically, then remove this condition
                    sqlx::query!("UPDATE races SET video_url = $1, video_url_fr = $2, video_url_de = $3, video_url_pt = $4 WHERE id = $5",
                        race.video_urls.get(&English).or_else(|| found_race.video_urls.get(&English)).map(|url| url.to_string()),
                        race.video_urls.get(&French).or_else(|| found_race.video_urls.get(&French)).map(|url| url.to_string()),
                        race.video_urls.get(&German).or_else(|| found_race.video_urls.get(&German)).map(|url| url.to_string()),
                        race.video_urls.get(&Portuguese).or_else(|| found_race.video_urls.get(&Portuguese)).map(|url| url.to_string()),
                        found_race.id as _,
                    ).execute(&mut **transaction).await?;
                }
            }
            Ok(())
        }

        async fn add_or_update_race(transaction: &mut Transaction<'_, Postgres>, races: &mut Vec<Race>, race: Race) -> sqlx::Result<()> {
            if let Some(found_race) = races.iter_mut().find(|iter_race|
                iter_race.series == race.series
                && iter_race.event == race.event
                && iter_race.phase == race.phase
                && iter_race.round == race.round
                && iter_race.game == race.game
                && iter_race.entrants == race.entrants
                && !iter_race.schedule_locked
            ) {
                update_race(transaction, found_race, race).await?;
            } else {
                // add race to database
                race.save(transaction).await?;
                races.push(race);
            }
            Ok(())
        }

        let now = Utc::now();
        let mut races = Vec::default();
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE series = $1 AND event = $2"#, event.series as _, &event.event).fetch_all(&mut **transaction).await? {
            races.push(Self::from_id(&mut *transaction, http_client, id).await?);
        }
        match event.series {
            Series::BattleRoyale => match &*event.event {
                "1" => {}
                _ => unimplemented!(),
            },
            Series::League => match &*event.event {
                "4" => {}
                "5" => {}
                "6" => {}
                "7" => {
                    let schedule = http_client.get("https://league.ootrandomizer.com/scheduleJson")
                        .send().await?
                        .detailed_error_for_status().await?
                        .json_with_text_in_error::<league::Schedule>().await?;
                    for race in schedule.matches {
                        if race.id <= 427 { continue } // seasons 5 and 6
                        let id = Id::<Races>::new(&mut *transaction).await?;
                        add_or_update_race(&mut *transaction, &mut races, Self {
                            series: event.series,
                            event: event.event.to_string(),
                            source: Source::League { id: race.id },
                            entrants: Entrants::Two([
                                race.player_a.into_entrant(),
                                race.player_b.into_entrant(),
                            ]),
                            phase: None,
                            round: Some(race.division),
                            game: None,
                            scheduling_thread: None,
                            schedule: RaceSchedule::Live {
                                start: race.time_utc,
                                end: None,
                                room: None,
                            },
                            schedule_updated_at: None,
                            draft: None,
                            seed: seed::Data::default(),
                            video_urls: if let Some(twitch_username) = race.restreamers.into_iter().filter_map(|restreamer| restreamer.twitch_username).at_most_one().expect("multiple restreams for a League race") {
                                //HACK: League schedule does not include language info, mark as English
                                iter::once((English, Url::parse(&format!("https://twitch.tv/{twitch_username}"))?)).collect()
                            } else {
                                HashMap::default()
                            },
                            restreamers: HashMap::default(),
                            last_edited_by: None,
                            last_edited_at: None,
                            ignored: match race.status {
                                league::MatchStatus::Canceled => true,
                                league::MatchStatus::Confirmed => false,
                            },
                            schedule_locked: false,
                            id,
                        }).await?;
                    }
                }
                _ => unimplemented!(),
            },
            Series::MixedPools => match &*event.event {
                "1" | "2" => {}
                "3" => for row in sheet_values(http_client.clone(), "1DqKrmcLhWwfIRpTMZ-NqGGFma_jgCSkyHyP8kaJg6OA", "Schedule!A2:F").await? {
                    let [timestamp, p1, p2, round, date_et, time_et] = &*row else { continue };
                    let id = Id::new(&mut *transaction).await?;
                    let (phase, round) = match &**round {
                        "Top 16" => (format!("Top 16"), format!("Round 1")),
                        "Quarterfinals" => (format!("Top 16"), format!("Quarterfinal")),
                        "Semifinals" => (format!("Top 16"), format!("Semifinal")),
                        "Finals" => (format!("Top 16"), format!("Final")),
                        _ => (format!("Swiss"), round.clone()),
                    };
                    add_or_update_race(&mut *transaction, &mut races, Self {
                        series: event.series,
                        event: event.event.to_string(),
                        source: Source::Sheet { timestamp: NaiveDateTime::parse_from_str(timestamp, "%d.%m.%Y %H:%M:%S")? },
                        entrants: Entrants::Two([
                            Entrant::Named { name: p1.clone(), racetime_id: None, twitch_username: None },
                            Entrant::Named { name: p2.clone(), racetime_id: None, twitch_username: None },
                        ]),
                        phase: Some(phase),
                        round: Some(round),
                        game: None,
                        scheduling_thread: None,
                        schedule: RaceSchedule::Live {
                            start: NaiveDateTime::parse_from_str(&format!("{date_et} at {time_et}"), "%d.%m.%Y at %H:%M:%S")?.and_local_timezone(America::New_York).single_ok()?.to_utc(),
                            end: None,
                            room: None,
                        },
                        schedule_updated_at: None,
                        draft: None,
                        seed: seed::Data::default(),
                        video_urls: HashMap::default(),
                        restreamers: HashMap::default(),
                        last_edited_by: None,
                        last_edited_at: None,
                        ignored: false,
                        schedule_locked: false,
                        id,
                    }).await?;
                },
                _ => unimplemented!(),
            },
            Series::Multiworld => match &*event.event {
                "1" => {} // no match data available
                "2" | "3" | "4" => {}
                _ => unimplemented!(),
            },
            Series::NineDaysOfSaws | Series::Pictionary => {
                let id = if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE series = $1 AND event = $2"#, event.series as _, &event.event).fetch_optional(&mut **transaction).await? {
                    id
                } else {
                    Id::<Races>::new(&mut *transaction).await?
                };
                let schedule = if let Some(start) = event.start(&mut *transaction).await? {
                    RaceSchedule::Live {
                        end: event.end,
                        room: event.url.clone(),
                        start,
                    }
                } else {
                    RaceSchedule::Unscheduled
                };
                add_or_update_race(&mut *transaction, &mut races, Self {
                    series: event.series,
                    event: event.event.to_string(),
                    source: Source::Manual,
                    entrants: Entrants::Open,
                    phase: None,
                    round: None,
                    game: None,
                    scheduling_thread: None,
                    schedule_updated_at: None,
                    draft: None,
                    seed: seed::Data::default(), //TODO
                    video_urls: event.video_url.iter().map(|video_url| (English, video_url.clone())).collect(), //TODO sync between event and race? Video URL fields for other languages on event::Data?
                    restreamers: HashMap::default(),
                    last_edited_by: None,
                    last_edited_at: None,
                    ignored: false,
                    schedule_locked: false,
                    id, schedule,
                }).await?;
            }
            Series::Rsl => match &*event.event {
                "1" => {} // no match data available
                "2" | "3" | "4" | "5" | "6" => {}
                _ => unimplemented!(),
            },
            Series::Scrubs => match &*event.event {
                "5" => {}
                _ => unimplemented!(),
            },
            Series::Standard => match &*event.event {
                "w" => {
                    let schedule = RaceSchedule::Live { start: s::next_na_weekly_after(now).to_utc(), end: None, room: None };
                    if !races.iter().any(|race| race.series == event.series && race.event == event.event && race.schedule.start_matches(&schedule)) {
                        let race = Race {
                            id: Id::new(&mut *transaction).await?,
                            series: event.series,
                            event: event.event.to_string(),
                            source: Source::Manual,
                            entrants: Entrants::Open,
                            phase: None,
                            round: Some(format!("NA Weekly")),
                            game: None,
                            scheduling_thread: None,
                            schedule_updated_at: None,
                            draft: None,
                            seed: seed::Data::default(),
                            video_urls: HashMap::default(),
                            restreamers: HashMap::default(),
                            last_edited_by: None,
                            last_edited_at: None,
                            ignored: false,
                            schedule_locked: false,
                            schedule,
                        };
                        race.save(&mut *transaction).await?;
                        races.push(race);
                    }
                    let schedule = RaceSchedule::Live { start: s::next_eu_weekly_after(now).to_utc(), end: None, room: None };
                    if !races.iter().any(|race| race.series == event.series && race.event == event.event && race.schedule.start_matches(&schedule)) {
                        let race = Race {
                            id: Id::new(&mut *transaction).await?,
                            series: event.series,
                            event: event.event.to_string(),
                            source: Source::Manual,
                            entrants: Entrants::Open,
                            phase: None,
                            round: Some(format!("EU Weekly")),
                            game: None,
                            scheduling_thread: None,
                            schedule_updated_at: None,
                            draft: None,
                            seed: seed::Data::default(),
                            video_urls: HashMap::default(),
                            restreamers: HashMap::default(),
                            last_edited_by: None,
                            last_edited_at: None,
                            ignored: false,
                            schedule_locked: false,
                            schedule,
                        };
                        race.save(&mut *transaction).await?;
                        races.push(race);
                    }
                }
                //TODO add archives of old Standard tournaments and Challenge Cups?
                "6" | "7" | "7cc" => {}
                _ => unimplemented!(),
            },
            | Series::CoOp //TODO add archives of seasons 1 and 2?
            | Series::CopaDoBrasil
            | Series::SongsOfHope
            | Series::SpeedGaming
            | Series::TournoiFrancophone
            | Series::TriforceBlitz
            | Series::WeTryToBeBetter
                => {} // these series are now scheduled via Mido's House
        }
        races.retain(|race| !race.ignored);
        races.sort_unstable();
        Ok(races)
    }

    pub(crate) async fn for_scheduling_channel(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, channel_id: ChannelId, game: Option<i16>, include_started: bool) -> Result<Vec<Self>, Error> {
        let mut races = Vec::default();
        let rows = match (game, include_started) {
            (None, false) => sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE scheduling_thread = $1 AND (start IS NULL OR start > NOW())"#, PgSnowflake(channel_id) as _).fetch_all(&mut **transaction).await?,
            (None, true) => sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE scheduling_thread = $1"#, PgSnowflake(channel_id) as _).fetch_all(&mut **transaction).await?,
            (Some(game), false) => sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE scheduling_thread = $1 AND game = $2 AND (start IS NULL OR start > NOW())"#, PgSnowflake(channel_id) as _, game).fetch_all(&mut **transaction).await?,
            (Some(game), true) => sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE scheduling_thread = $1 AND game = $2"#, PgSnowflake(channel_id) as _, game).fetch_all(&mut **transaction).await?,
        };
        for id in rows {
            races.push(Self::from_id(&mut *transaction, http_client, id).await?);
        }
        races.retain(|race| !race.ignored);
        races.sort_unstable();
        Ok(races)
    }

    pub(crate) async fn game_count(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<i16, Error> {
        let ([team1, team2, team3], [p1, p2, p3], [p1_discord, p2_discord], [p1_racetime, p2_racetime], [p1_twitch, p2_twitch], [total, finished]) = self.entrants.to_db();
        Ok(sqlx::query_scalar!(r#"SELECT game AS "game!" FROM races WHERE
            series = $1
            AND event = $2
            AND phase IS NOT DISTINCT FROM $3
            AND round IS NOT DISTINCT FROM $4
            AND game IS NOT NULL
            AND team1 IS NOT DISTINCT FROM $5
            AND team2 IS NOT DISTINCT FROM $6
            AND team3 IS NOT DISTINCT FROM $7
            AND p1 IS NOT DISTINCT FROM $8
            AND p2 IS NOT DISTINCT FROM $9
            AND p3 IS NOT DISTINCT FROM $10
            AND p1_discord IS NOT DISTINCT FROM $11
            AND p2_discord IS NOT DISTINCT FROM $12
            AND p1_racetime IS NOT DISTINCT FROM $13
            AND p2_racetime IS NOT DISTINCT FROM $14
            AND p1_twitch IS NOT DISTINCT FROM $15
            AND p2_twitch IS NOT DISTINCT FROM $16
            AND total IS NOT DISTINCT FROM $17
            AND finished IS NOT DISTINCT FROM $18
            ORDER BY game DESC LIMIT 1
        "#,
            self.series as _,
            self.event,
            self.phase,
            self.round,
            team1 as _,
            team2 as _,
            team3 as _,
            p1,
            p2,
            p3,
            p1_discord.map(PgSnowflake) as _,
            p2_discord.map(PgSnowflake) as _,
            p1_racetime,
            p2_racetime,
            p1_twitch,
            p2_twitch,
            total.map(|total| total as i32),
            finished.map(|finished| finished as i32),
        ).fetch_optional(&mut **transaction).await?.unwrap_or(1))
    }

    pub(crate) async fn next_game(&self, transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client) -> Result<Option<Self>, Error> {
        Ok(if_chain! {
            if let Some(game) = self.game;
            let ([team1, team2, team3], [p1, p2, p3], [p1_discord, p2_discord], [p1_racetime, p2_racetime], [p1_twitch, p2_twitch], [total, finished]) = self.entrants.to_db();
            if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE
                series = $1
                AND event = $2
                AND phase IS NOT DISTINCT FROM $3
                AND round IS NOT DISTINCT FROM $4
                AND game = $5
                AND team1 IS NOT DISTINCT FROM $6
                AND team2 IS NOT DISTINCT FROM $7
                AND team3 IS NOT DISTINCT FROM $8
                AND p1 IS NOT DISTINCT FROM $9
                AND p2 IS NOT DISTINCT FROM $10
                AND p3 IS NOT DISTINCT FROM $11
                AND p1_discord IS NOT DISTINCT FROM $12
                AND p2_discord IS NOT DISTINCT FROM $13
                AND p1_racetime IS NOT DISTINCT FROM $14
                AND p2_racetime IS NOT DISTINCT FROM $15
                AND p1_twitch IS NOT DISTINCT FROM $16
                AND p2_twitch IS NOT DISTINCT FROM $17
                AND total IS NOT DISTINCT FROM $18
                AND finished IS NOT DISTINCT FROM $19
            "#,
                self.series as _,
                self.event,
                self.phase,
                self.round,
                game + 1,
                team1 as _,
                team2 as _,
                team3 as _,
                p1,
                p2,
                p3,
                p1_discord.map(PgSnowflake) as _,
                p2_discord.map(PgSnowflake) as _,
                p1_racetime,
                p2_racetime,
                p1_twitch,
                p2_twitch,
                total.map(|total| total as i32),
                finished.map(|finished| finished as i32),
            ).fetch_optional(&mut **transaction).await?;
            then {
                Some(Self::from_id(&mut *transaction, http_client, id).await?)
            } else {
                None
            }
        })
    }

    pub(crate) async fn event(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<event::Data<'static>, event::DataError> {
        event::Data::new(transaction, self.series, self.event.clone()).await?.ok_or(event::DataError::Missing)
    }

    pub(crate) fn startgg_set_url(&self) -> Result<Option<Url>, url::ParseError> {
        Ok(if let Source::StartGG { ref event, set: startgg::ID(ref set), .. } = self.source {
            Some(format!("https://start.gg/{event}/set/{set}").parse()?)
        } else {
            None
        })
    }

    pub(crate) fn cal_events(&self) -> impl Iterator<Item = Event> + Send {
        match self.schedule {
            RaceSchedule::Unscheduled => Box::new(iter::empty()) as Box<dyn Iterator<Item = Event> + Send>,
            RaceSchedule::Live { .. } => Box::new(iter::once(Event { race: self.clone(), kind: EventKind::Normal })),
            RaceSchedule::Async { .. } => if let Entrants::Three(_) = self.entrants {
                Box::new([
                    Event { race: self.clone(), kind: EventKind::Async1 },
                    Event { race: self.clone(), kind: EventKind::Async2 },
                    Event { race: self.clone(), kind: EventKind::Async3 },
                ].into_iter()) as Box<dyn Iterator<Item = Event> + Send>
            } else {
                Box::new([
                    Event { race: self.clone(), kind: EventKind::Async1 },
                    Event { race: self.clone(), kind: EventKind::Async2 },
                ].into_iter())
            },
        }
    }

    /// The seed remains hidden until it's posted in the last calendar event of this race.
    pub(crate) fn show_seed(&self) -> bool {
        if let RaceSchedule::Unscheduled = self.schedule { return false }
        let now = Utc::now();
        self.cal_events().all(|event| event.is_private_async_part() || event.start().is_some_and(|start| start <= now + TimeDelta::minutes(15)) || event.end().is_some())
    }

    pub(crate) fn is_ended(&self) -> bool {
        // Since the end time of a race isn't known in advance, we assume that if a race has an end time, that end time is in the past.
        self.schedule.end_time(&self.entrants).is_some()
    }

    pub(crate) fn rooms(&self) -> impl Iterator<Item = Url> + Send {
        // hide room of private async parts until public part finished
        //TODO show to the team that played the private async part
        let all_ended = self.cal_events().all(|event| event.end().is_some());
        self.cal_events().filter(move |event| all_ended || !event.is_private_async_part()).filter_map(|event| event.room().cloned())
    }

    pub(crate) fn teams(&self) -> impl Iterator<Item = &Team> + Send {
        match self.entrants {
            Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => Box::new(iter::empty()) as Box<dyn Iterator<Item = &Team> + Send>,
            Entrants::Two([ref team1, ref team2]) => Box::new([team1, team2].into_iter().filter_map(as_variant!(Entrant::MidosHouseTeam))),
            Entrants::Three([ref team1, ref team2, ref team3]) => Box::new([team1, team2, team3].into_iter().filter_map(as_variant!(Entrant::MidosHouseTeam))),
        }
    }

    pub(crate) fn teams_opt(&self) -> Option<impl Iterator<Item = &Team> + Send> {
        match self.entrants {
            Entrants::Two([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2)]) => Some(Box::new([team1, team2].into_iter()) as Box<dyn Iterator<Item = &Team> + Send>),
            Entrants::Three([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2), Entrant::MidosHouseTeam(ref team3)]) => Some(Box::new([team1, team2, team3].into_iter())),
            Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) | Entrants::Two(_) | Entrants::Three(_) => None,
        }
    }

    pub(crate) async fn multistream_url(&self, transaction: &mut Transaction<'_, Postgres>, env: Environment, http_client: &reqwest::Client, event: &event::Data<'_>) -> Result<Option<Url>, Error> {
        async fn entrant_twitch_names<'a>(transaction: &mut Transaction<'_, Postgres>, env: Environment, http_client: &reqwest::Client, event: &event::Data<'_>, entrant: &'a Entrant) -> Result<Option<Vec<Cow<'a, str>>>, Error> {
            let mut channels = Vec::default();
            match entrant {
                Entrant::MidosHouseTeam(team) => for (member, role) in team.members_roles(&mut *transaction).await? {
                    if event.team_config.role_is_racing(role) {
                        if let Some(twitch_name) = member.racetime_user_data(env, http_client).await?.and_then(|racetime_user_data| racetime_user_data.twitch_name) {
                            channels.push(Cow::Owned(twitch_name));
                        } else {
                            return Ok(None)
                        }
                    }
                },
                Entrant::Discord { twitch_username: Some(twitch_name), .. } | Entrant::Named { twitch_username: Some(twitch_name), .. } => channels.push(Cow::Borrowed(&**twitch_name)),
                Entrant::Discord { twitch_username: None, racetime_id: Some(racetime_id), .. } | Entrant::Named { twitch_username: None, racetime_id: Some(racetime_id), .. } => {
                    let racetime_user_data = http_client.get(format!("https://{}/user/{racetime_id}/data", env.racetime_host()))
                        .send().await?
                        .detailed_error_for_status().await?
                        .json_with_text_in_error::<racetime::model::UserData>().await?;
                    if let Some(twitch_name) = racetime_user_data.twitch_name {
                        channels.push(Cow::Owned(twitch_name));
                    } else {
                        return Ok(None)
                    }
                }
                Entrant::Discord { twitch_username: None, racetime_id: None, id } => if_chain! {
                    if let Some(user) = User::from_discord(&mut **transaction, *id).await?;
                    if let Some(racetime_user_data) = user.racetime_user_data(env, http_client).await?;
                    if let Some(twitch_name) = racetime_user_data.twitch_name;
                    then {
                        channels.push(Cow::Owned(twitch_name));
                    } else {
                        return Ok(None)
                    }
                },
                Entrant::Named { twitch_username: None, racetime_id: None, .. } => return Ok(None),
            }
            Ok(Some(channels))
        }

        Ok(if let RaceSchedule::Live { room: Some(_), .. } = self.schedule {
            match self.entrants {
                Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => None,
                Entrants::Two(ref entrants) => {
                    let mut channels = Vec::default();
                    for entrant in entrants {
                        if let Some(twitch_names) = entrant_twitch_names(&mut *transaction, env, http_client, event, entrant).await? {
                            channels.extend(twitch_names);
                        } else {
                            return Ok(None)
                        }
                    }
                    let mut url = Url::parse("https://multistre.am/").unwrap();
                    url.path_segments_mut().unwrap().extend(&channels).push(match channels.len() {
                        0 => return Ok(None),
                        2 => "layout4",
                        4 => "layout12",
                        6 => "layout18",
                        _ => unimplemented!(),
                    });
                    Some(url)
                }
                Entrants::Three(ref entrants) => {
                    let mut channels = Vec::default();
                    for entrant in entrants {
                        if let Some(twitch_names) = entrant_twitch_names(&mut *transaction, env, http_client, event, entrant).await? {
                            channels.extend(twitch_names);
                        } else {
                            return Ok(None)
                        }
                    }
                    let mut url = Url::parse("https://multistre.am/").unwrap();
                    url.path_segments_mut().unwrap().extend(&channels).push(match channels.len() {
                        0 => return Ok(None),
                        3 => "layout7",
                        6 => "layout17",
                        _ => unimplemented!(),
                    });
                    Some(url)
                }
            }
        } else {
            None
        })
    }

    pub(crate) async fn player_video_urls(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<Vec<(User, Url)>, Error> {
        let rows = sqlx::query!(r#"SELECT player AS "player: Id<Users>", video FROM race_player_videos WHERE race = $1"#, self.id as _).fetch_all(&mut **transaction).await?;
        let mut tuples = Vec::with_capacity(rows.len());
        for row in rows {
            tuples.push((User::from_id(&mut **transaction, row.player).await?.expect("foreign key constraint violated"), row.video.parse()?));
        }
        Ok(tuples)
    }

    pub(crate) fn has_room_for(&self, team: &Team) -> bool {
        match &self.schedule {
            RaceSchedule::Unscheduled => false,
            RaceSchedule::Live { room, .. } => room.is_some(),
            RaceSchedule::Async { room1, room2, room3, .. } => match &self.entrants {
                Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => panic!("asynced race not with Entrants::Two or Entrants::Three"),
                Entrants::Two([team1, team2]) => {
                    if let Entrant::MidosHouseTeam(team1) = team1 {
                        if team == team1 {
                            return room1.is_some()
                        }
                    }
                    if let Entrant::MidosHouseTeam(team2) = team2 {
                        if team == team2 {
                            return room2.is_some()
                        }
                    }
                    false
                }
                Entrants::Three([team1, team2, team3]) => {
                    if let Entrant::MidosHouseTeam(team1) = team1 {
                        if team == team1 {
                            return room1.is_some()
                        }
                    }
                    if let Entrant::MidosHouseTeam(team2) = team2 {
                        if team == team2 {
                            return room2.is_some()
                        }
                    }
                    if let Entrant::MidosHouseTeam(team3) = team3 {
                        if team == team3 {
                            return room3.is_some()
                        }
                    }
                    false
                }
            },
        }
    }

    pub(crate) async fn save(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<()> {
        let (league_id, sheet_timestamp, startgg_event, startgg_set) = match self.source {
            Source::Manual => (None, None, None, None),
            Source::League { id } => (Some(id), None, None, None),
            Source::Sheet { timestamp } => (None, Some(timestamp), None, None),
            Source::StartGG { ref event, ref set } => (None, None, Some(event), Some(set)),
        };
        let ([team1, team2, team3], [p1, p2, p3], [p1_discord, p2_discord], [p1_racetime, p2_racetime], [p1_twitch, p2_twitch], [total, finished]) = self.entrants.to_db();
        let (start, [async_start1, async_start2, async_start3], end, [async_end1, async_end2, async_end3], room, [async_room1, async_room2, async_room3]) = match self.schedule {
            RaceSchedule::Unscheduled => (None, [None; 3], None, [None; 3], None, [None; 3]),
            RaceSchedule::Live { start, end, ref room } => (Some(start), [None; 3], end, [None; 3], room.as_ref(), [None; 3]),
            RaceSchedule::Async { start1, start2, start3, end1, end2, end3, ref room1, ref room2, ref room3 } => (None, [start1, start2, start3], None, [end1, end2, end3], None, [room1.as_ref(), room2.as_ref(), room3.as_ref()]),
        };
        let (web_id, web_gen_time, file_stem, locked_spoiler_log_path, tfb_uuid) = match self.seed.files {
            Some(seed::Files::MidosHouse { ref file_stem, ref locked_spoiler_log_path }) => (None, None, Some(file_stem), locked_spoiler_log_path.as_ref(), None),
            Some(seed::Files::OotrWeb { id, gen_time, ref file_stem }) => (Some(id), Some(gen_time), Some(file_stem), None, None),
            Some(seed::Files::TriforceBlitz { uuid }) => (None, None, None, None, Some(uuid)),
            Some(seed::Files::TfbSotd { .. }) => unimplemented!("Triforce Blitz seed of the day not supported for official races"),
            None => (None, None, None, None, None),
        };
        sqlx::query!("
            INSERT INTO races              (startgg_set, start, series, event, async_start2, async_start1, room, async_room1, async_room2, draft_state, async_end1, async_end2, end_time, team1, team2, web_id, web_gen_time, file_stem, hash1, hash2, hash3, hash4, hash5, game, id,  p1,  p2,  last_edited_by, last_edited_at, video_url, phase, round, p3,  startgg_event, scheduling_thread, total, finished, tfb_uuid, video_url_fr, restreamer, restreamer_fr, locked_spoiler_log_path, video_url_pt, restreamer_pt, p1_twitch, p2_twitch, p1_discord, p2_discord, team3, schedule_updated_at, video_url_de, restreamer_de, sheet_timestamp, league_id, p1_racetime, p2_racetime, async_start3, async_room3, async_end3)
            VALUES                         ($1,          $2,    $3,     $4,    $5,           $6,           $7,   $8,          $9,          $10,         $11,        $12,        $13,      $14,   $15,   $16,    $17,          $18,       $19,   $20,   $21,   $22,   $23,   $24,  $25, $26, $27, $28,            $29,            $30,       $31,   $32,   $33, $34,           $35,               $36,   $37,      $38,      $39,          $40,        $41,           $42,                     $43,          $44,           $45,       $46,       $47,        $48,        $49,   $50,                 $51,          $52,           $53,             $54,       $55,         $56,         $57,          $58,         $59       )
            ON CONFLICT (id) DO UPDATE SET (startgg_set, start, series, event, async_start2, async_start1, room, async_room1, async_room2, draft_state, async_end1, async_end2, end_time, team1, team2, web_id, web_gen_time, file_stem, hash1, hash2, hash3, hash4, hash5, game, id,  p1,  p2,  last_edited_by, last_edited_at, video_url, phase, round, p3,  startgg_event, scheduling_thread, total, finished, tfb_uuid, video_url_fr, restreamer, restreamer_fr, locked_spoiler_log_path, video_url_pt, restreamer_pt, p1_twitch, p2_twitch, p1_discord, p2_discord, team3, schedule_updated_at, video_url_de, restreamer_de, sheet_timestamp, league_id, p1_racetime, p2_racetime, async_start3, async_room3, async_end3)
            =                              ($1,          $2,    $3,     $4,    $5,           $6,           $7,   $8,          $9,          $10,         $11,        $12,        $13,      $14,   $15,   $16,    $17,          $18,       $19,   $20,   $21,   $22,   $23,   $24,  $25, $26, $27, $28,            $29,            $30,       $31,   $32,   $33, $34,           $35,               $36,   $37,      $38,      $39,          $40,        $41,           $42,                     $43,          $44,           $45,       $46,       $47,        $48,        $49,   $50,                 $51,          $52,           $53,             $54,       $55,         $56,         $57,          $58,         $59       )
        ",
            startgg_set as _,
            start,
            self.series as _,
            self.event,
            async_start2,
            async_start1,
            room.map(|url| url.to_string()),
            async_room1.map(|url| url.to_string()),
            async_room2.map(|url| url.to_string()),
            self.draft.as_ref().map(Json) as _,
            async_end1,
            async_end2,
            end,
            team1 as _,
            team2 as _,
            web_id.map(|web_id| web_id as i64),
            web_gen_time,
            file_stem.map(|file_stem| &**file_stem),
            self.seed.file_hash.map(|[hash1, _, _, _, _]| hash1) as _,
            self.seed.file_hash.map(|[_, hash2, _, _, _]| hash2) as _,
            self.seed.file_hash.map(|[_, _, hash3, _, _]| hash3) as _,
            self.seed.file_hash.map(|[_, _, _, hash4, _]| hash4) as _,
            self.seed.file_hash.map(|[_, _, _, _, hash5]| hash5) as _,
            self.game,
            self.id as _,
            p1,
            p2,
            self.last_edited_by as _,
            self.last_edited_at,
            self.video_urls.get(&English).map(|url| url.to_string()),
            self.phase,
            self.round,
            p3,
            startgg_event,
            self.scheduling_thread.map(PgSnowflake) as _,
            total.map(|total| total as i32),
            finished.map(|finished| finished as i32),
            tfb_uuid,
            self.video_urls.get(&French).map(|url| url.to_string()),
            self.restreamers.get(&English),
            self.restreamers.get(&French),
            locked_spoiler_log_path,
            self.video_urls.get(&Portuguese).map(|url| url.to_string()),
            self.restreamers.get(&Portuguese),
            p1_twitch,
            p2_twitch,
            p1_discord.map(PgSnowflake) as _,
            p2_discord.map(PgSnowflake) as _,
            team3 as _,
            self.schedule_updated_at,
            self.video_urls.get(&German).map(|url| url.to_string()),
            self.restreamers.get(&German),
            sheet_timestamp,
            league_id,
            p1_racetime,
            p2_racetime,
            async_start3,
            async_room3.map(|url| url.to_string()),
            async_end3,
        ).execute(&mut **transaction).await?;
        Ok(())
    }
}

impl PartialEq for Race {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Equal
    }
}

impl Eq for Race {}

impl PartialOrd for Race {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Race {
    fn cmp(&self, other: &Self) -> Ordering {
        self.schedule.cmp(&self.entrants, &other.schedule, &other.entrants)
            .then_with(|| self.series.to_str().cmp(other.series.to_str()))
            .then_with(|| self.event.cmp(&other.event))
            .then_with(|| self.phase.cmp(&other.phase))
            .then_with(|| self.round.cmp(&other.round))
            .then_with(|| self.source.cmp(&other.source))
            .then_with(|| self.game.cmp(&other.game))
            .then_with(|| self.id.cmp(&other.id))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EventKind {
    Normal,
    Async1,
    Async2,
    Async3,
}

#[derive(Clone)]
pub(crate) struct Event {
    pub(crate) race: Race,
    pub(crate) kind: EventKind,
}

impl Event {
    pub(crate) async fn from_room(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, room: Url) -> Result<Option<Self>, Error> {
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE room = $1 AND start IS NOT NULL"#, room.to_string()).fetch_optional(&mut **transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Normal,
            }))
        }
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE async_room1 = $1 AND async_start1 IS NOT NULL"#, room.to_string()).fetch_optional(&mut **transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Async1,
            }))
        }
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE async_room2 = $1 AND async_start2 IS NOT NULL"#, room.to_string()).fetch_optional(&mut **transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Async2,
            }))
        }
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE async_room3 = $1 AND async_start3 IS NOT NULL"#, room.to_string()).fetch_optional(&mut **transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Async3,
            }))
        }
        Ok(None)
    }

    pub(crate) async fn rooms_to_open(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client) -> Result<Vec<Self>, Error> {
        let mut events = Vec::default();
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE room IS NULL AND start IS NOT NULL AND start > NOW() AND (start <= NOW() + TIME '00:30:00' OR (team1 IS NULL AND p1_discord IS NULL AND p1 IS NULL AND start <= NOW() + TIME '01:00:00'))"#).fetch_all(&mut **transaction).await? {
            events.push(Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Normal,
            })
        }
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE async_room1 IS NULL AND async_start1 IS NOT NULL AND async_start1 > NOW() AND async_start1 <= NOW() + TIME '00:30:00'"#).fetch_all(&mut **transaction).await? {
            let event = Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Async1,
            };
            if event.race.event(&mut *transaction).await?.team_config.is_racetime_team_format() {
                events.push(event);
            } else {
                // racetime.gg doesn't support single-entrant races
                //TODO DM/post seed 15 minutes before start
            }
        }
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE async_room2 IS NULL AND async_start2 IS NOT NULL AND async_start2 > NOW() AND async_start2 <= NOW() + TIME '00:30:00'"#).fetch_all(&mut **transaction).await? {
            let event = Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Async2,
            };
            if event.race.event(&mut *transaction).await?.team_config.is_racetime_team_format() {
                events.push(event);
            } else {
                // racetime.gg doesn't support single-entrant races
                //TODO DM/post seed 15 minutes before start
            }
        }
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id<Races>" FROM races WHERE async_room3 IS NULL AND async_start3 IS NOT NULL AND async_start3 > NOW() AND async_start3 <= NOW() + TIME '00:30:00'"#).fetch_all(&mut **transaction).await? {
            let event = Self {
                race: Race::from_id(&mut *transaction, http_client, id).await?,
                kind: EventKind::Async3,
            };
            if event.race.event(&mut *transaction).await?.team_config.is_racetime_team_format() {
                events.push(event);
            } else {
                // racetime.gg doesn't support single-entrant races
                //TODO DM/post seed 15 minutes before start
            }
        }
        Ok(events)
    }

    pub(crate) fn active_teams(&self) -> impl Iterator<Item = &Team> + Send {
        match self.race.entrants {
            Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => Box::new(iter::empty()) as Box<dyn Iterator<Item = &Team> + Send>,
            Entrants::Two([ref team1, ref team2]) => Box::new([
                matches!(self.kind, EventKind::Normal | EventKind::Async1).then_some(team1),
                matches!(self.kind, EventKind::Normal | EventKind::Async2).then_some(team2),
            ].into_iter().filter_map(identity).filter_map(as_variant!(Entrant::MidosHouseTeam))),
            Entrants::Three([ref team1, ref team2, ref team3]) => Box::new([
                matches!(self.kind, EventKind::Normal | EventKind::Async1).then_some(team1),
                matches!(self.kind, EventKind::Normal | EventKind::Async2).then_some(team2),
                matches!(self.kind, EventKind::Normal | EventKind::Async3).then_some(team3),
            ].into_iter().filter_map(identity).filter_map(as_variant!(Entrant::MidosHouseTeam))),
        }
    }

    pub(crate) fn room(&self) -> Option<&Url> {
        match self.race.schedule {
            RaceSchedule::Unscheduled => None,
            RaceSchedule::Live { ref room, .. } => room.as_ref(),
            RaceSchedule::Async { ref room1, ref room2, ref room3, .. } => match self.kind {
                EventKind::Normal => unreachable!(),
                EventKind::Async1 => room1.as_ref(),
                EventKind::Async2 => room2.as_ref(),
                EventKind::Async3 => room3.as_ref(),
            },
        }
    }

    pub(crate) fn start(&self) -> Option<DateTime<Utc>> {
        match self.race.schedule {
            RaceSchedule::Unscheduled => None,
            RaceSchedule::Live { start, .. } => Some(start),
            RaceSchedule::Async { start1, start2, start3, .. } => match self.kind {
                EventKind::Normal => unreachable!(),
                EventKind::Async1 => start1,
                EventKind::Async2 => start2,
                EventKind::Async3 => start3,
            },
        }
    }

    pub(crate) fn end(&self) -> Option<DateTime<Utc>> {
        match self.race.schedule {
            RaceSchedule::Unscheduled => None,
            RaceSchedule::Live { end, .. } => end,
            RaceSchedule::Async { end1, end2, end3, .. } => match self.kind {
                EventKind::Normal => unreachable!(),
                EventKind::Async1 => end1,
                EventKind::Async2 => end2,
                EventKind::Async3 => end3,
            },
        }
    }

    pub(crate) fn is_private_async_part(&self) -> bool {
        match self.race.schedule {
            RaceSchedule::Unscheduled | RaceSchedule::Live { .. } => false,
            RaceSchedule::Async { start1, start2, start3, .. } => match self.race.entrants {
                Entrants::Two(_) => match self.kind {
                    EventKind::Async1 => start1.map_or(false, |start1| start2.map_or(true, |start2| start1 <= start2)),
                    EventKind::Async2 => start2.map_or(false, |start2| start1.map_or(true, |start1| start2 < start1)),
                    EventKind::Normal | EventKind::Async3 => unreachable!(),
                },
                Entrants::Three(_) => match self.kind {
                    EventKind::Async1 => start1.map_or(false, |start1| start2.map_or(true, |start2| start1 <= start2) || start3.map_or(true, |start3| start1 <= start3)),
                    EventKind::Async2 => start2.map_or(false, |start2| start1.map_or(true, |start1| start2 < start1) || start3.map_or(true, |start3| start2 <= start3)),
                    EventKind::Async3 => start3.map_or(false, |start3| start1.map_or(true, |start1| start3 < start1) || start2.map_or(true, |start2| start3 < start2)),
                    EventKind::Normal => unreachable!(),
                },
                _ => unreachable!(),
            },
        }
    }

    pub(crate) fn is_public_async_part(&self) -> bool {
        match self.race.schedule {
            RaceSchedule::Unscheduled | RaceSchedule::Live { .. } => false,
            RaceSchedule::Async { .. } => !self.is_private_async_part(),
        }
    }

    pub(crate) async fn should_create_room(&self, transaction: &mut Transaction<'_, Postgres>, event: &event::Data<'_>) -> Result<bool, event::DataError> {
        Ok(racetime_bot::Goal::for_event(self.race.series, &self.race.event).map_or(false, |goal| goal.should_create_rooms()) && (
            // don't create racetime.gg rooms for in-person races
            self.race.series != Series::SpeedGaming
            || !self.race.event.ends_with("live")
            || !event.is_started(transaction).await?
        ))
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] ChronoParse(#[from] chrono::format::ParseError),
    #[error(transparent)] Discord(#[from] discord_bot::Error),
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] SeedData(#[from] seed::ExtraDataError),
    #[error(transparent)] Sheets(#[from] SheetsError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] StartGG(#[from] startgg::Error),
    #[error(transparent)] TimeFromLocal(#[from] wheel::traits::TimeFromLocalError<DateTime<Tz>>),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("no team with this ID")]
    UnknownTeam,
    #[error("start.gg team ID {0} is not associated with a Mido's House team")]
    UnknownTeamStartGG(startgg::ID),
}

impl<E: Into<Error>> From<E> for StatusOrError<Error> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

impl IsNetworkError for Error {
    fn is_network_error(&self) -> bool {
        match self {
            Self::ChronoParse(_) => false,
            Self::Discord(_) => false,
            Self::Event(_) => false,
            Self::ParseInt(_) => false,
            Self::Reqwest(e) => e.is_network_error(),
            Self::SeedData(e) => e.is_network_error(),
            Self::Sheets(e) => e.is_network_error(),
            Self::Sql(_) => false,
            Self::StartGG(e) => e.is_network_error(),
            Self::TimeFromLocal(_) => false,
            Self::Url(_) => false,
            Self::Wheel(e) => e.is_network_error(),
            Self::UnknownTeam => false,
            Self::UnknownTeamStartGG(_) => false,
        }
    }
}

static SHEETS_CACHE: Lazy<Mutex<HashMap<(String, String), (Instant, Vec<Vec<String>>)>>> = Lazy::new(|| Mutex::default());

#[derive(Debug, thiserror::Error)]
enum SheetsUncachedError {
    #[error(transparent)] OAuth(#[from] yup_oauth2::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("empty token is not valid")]
    EmptyToken,
    #[error("OAuth token is expired")]
    TokenExpired,
}

impl IsNetworkError for SheetsUncachedError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::OAuth(_) => false,
            Self::Reqwest(e) => e.is_network_error(),
            Self::Wheel(e) => e.is_network_error(),
            Self::EmptyToken => false,
            Self::TokenExpired => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(crate) struct SheetsError {
    source: SheetsUncachedError,
    cache: SheetsCacheMissReason,
}

impl IsNetworkError for SheetsError {
    fn is_network_error(&self) -> bool {
        self.source.is_network_error()
    }
}

#[derive(Debug)]
enum SheetsCacheMissReason {
    Elapsed,
    Vacant,
}

async fn sheet_values(http_client: reqwest::Client, sheet_id: &str, range: &str) -> Result<Vec<Vec<String>>, SheetsError> {
    #[derive(Deserialize)]
    struct ValueRange {
        values: Vec<Vec<String>>,
    }

    async fn sheet_values_uncached(http_client: &reqwest::Client, sheet_id: &str, range: &str) -> Result<Vec<Vec<String>>, SheetsUncachedError> {
        let gsuite_secret = read_service_account_key("assets/google-client-secret.json").await.at("assets/google-client-secret.json")?;
        let auth = ServiceAccountAuthenticator::builder(gsuite_secret)
            .build().await.at_unknown()?;
        let token = auth.token(&["https://www.googleapis.com/auth/spreadsheets"]).await?;
        if token.is_expired() { return Err(SheetsUncachedError::TokenExpired) }
        let Some(token) = token.token() else { return Err(SheetsUncachedError::EmptyToken) };
        if token.is_empty() { return Err(SheetsUncachedError::EmptyToken) }
        let ValueRange { values } = http_client.get(&format!("https://sheets.googleapis.com/v4/spreadsheets/{sheet_id}/values/{range}"))
            .bearer_auth(token)
            .query(&[
                ("valueRenderOption", "FORMATTED_VALUE"),
                ("dateTimeRenderOption", "FORMATTED_STRING"),
                ("majorDimension", "ROWS"),
            ])
            .send().await?
            .detailed_error_for_status().await?
            .json_with_text_in_error::<ValueRange>().await?;
        Ok(values)
    }

    let key = (sheet_id.to_owned(), range.to_owned());
    lock!(cache = SHEETS_CACHE; Ok(match cache.entry(key) {
        hash_map::Entry::Occupied(mut entry) => {
            let (retrieved, values) = entry.get();
            if retrieved.elapsed() < Duration::from_secs(5 * 60) {
                values.clone()
            } else {
                match sheet_values_uncached(&http_client, sheet_id, range).await {
                    Ok(values) => {
                        entry.insert((Instant::now(), values.clone()));
                        values
                    }
                    Err(e) if e.is_network_error() && retrieved.elapsed() < Duration::from_secs(60 * 60) => values.clone(),
                    Err(source) => return Err(SheetsError { cache: SheetsCacheMissReason::Elapsed, source }),
                }
            }
        }
        hash_map::Entry::Vacant(entry) => {
            let values = sheet_values_uncached(&http_client, sheet_id, range).await.map_err(|source| SheetsError { cache: SheetsCacheMissReason::Vacant, source })?;
            entry.insert((Instant::now(), values.clone()));
            values
        }
    }))
}

trait IntoIcsTzid {
    fn into_tzid(self) -> TzIDParam<'static>;
}

impl IntoIcsTzid for Utc {
    fn into_tzid(self) -> TzIDParam<'static> {
        TzIDParam::new("Etc/UTC")
    }
}

impl IntoIcsTzid for Tz {
    fn into_tzid(self) -> TzIDParam<'static> {
        TzIDParam::new(self.name())
    }
}

fn dtstamp(datetime: DateTime<Utc>) -> String {
    datetime.to_utc().format("%Y%m%dT%H%M%SZ").to_string()
}

fn dtstart<Z: TimeZone + IntoIcsTzid>(datetime: DateTime<Z>) -> DtStart<'static> {
    let mut dtstart = DtStart::new(datetime.naive_local().format("%Y%m%dT%H%M%S").to_string());
    dtstart.add(datetime.timezone().into_tzid());
    dtstart
}

fn dtend<Z: TimeZone + IntoIcsTzid>(datetime: DateTime<Z>) -> DtEnd<'static> {
    let mut dtend = DtEnd::new(datetime.naive_local().format("%Y%m%dT%H%M%S").to_string());
    dtend.add(datetime.timezone().into_tzid());
    dtend
}

async fn add_event_races(transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, http_client: &reqwest::Client, cal: &mut ICalendar<'_>, event: &event::Data<'_>) -> Result<(), Error> {
    let now = Utc::now();
    let mut latest_instantiated_na_weekly = None;
    let mut latest_instantiated_eu_weekly = None;
    for race in Race::for_event(transaction, http_client, event).await?.into_iter() {
        for race_event in race.cal_events() {
            if let Some(start) = race_event.start() {
                let mut cal_event = ics::Event::new(format!("{}{}@midos.house",
                    race.id,
                    match race_event.kind {
                        EventKind::Normal => "",
                        EventKind::Async1 => "-1",
                        EventKind::Async2 => "-2",
                        EventKind::Async3 => "-3",
                    },
                ), dtstamp(now));
                let summary_prefix = match (&race.phase, &race.round) {
                    (Some(phase), Some(round)) => format!("{} {phase} {round}", event.short_name()),
                    (Some(phase), None) => format!("{} {phase}", event.short_name()),
                    (None, Some(round)) => format!("{} {round}", event.short_name()),
                    (None, None) => event.display_name.clone(),
                };
                let summary_prefix = match race.entrants {
                    Entrants::Open | Entrants::Count { .. } => summary_prefix,
                    Entrants::Named(ref entrants) => match race_event.kind {
                        EventKind::Normal => format!("{summary_prefix}: {entrants}"),
                        EventKind::Async1 | EventKind::Async2 | EventKind::Async3 => format!("{summary_prefix} (async): {entrants}"),
                    },
                    Entrants::Two([ref team1, ref team2]) => match race_event.kind {
                        EventKind::Normal => format!(
                            "{summary_prefix}: {} vs {}",
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async1 => format!(
                            "{summary_prefix} (async): {} vs {}",
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async2 => format!(
                            "{summary_prefix} (async): {} vs {}",
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async3 => unreachable!(),
                    },
                    Entrants::Three([ref team1, ref team2, ref team3]) => match race_event.kind {
                        EventKind::Normal => format!(
                            "{summary_prefix}: {} vs {} vs {}",
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team3.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async1 => format!(
                            "{summary_prefix} (async): {} vs {} vs {}",
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team3.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async2 => format!(
                            "{summary_prefix} (async): {} vs {} vs {}",
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team3.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async3 => format!(
                            "{summary_prefix} (async): {} vs {} vs {}",
                            team3.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                    },
                };
                cal_event.push(Summary::new(if let Some(game) = race.game {
                    format!("{summary_prefix}, game {game}")
                } else {
                    summary_prefix
                }));
                cal_event.push(dtstart(start));
                cal_event.push(dtend(race_event.end().unwrap_or_else(|| start + match event.series {
                    Series::TriforceBlitz => TimeDelta::hours(2),
                    Series::BattleRoyale => TimeDelta::hours(2) + TimeDelta::minutes(30),
                    Series::CoOp | Series::MixedPools | Series::Scrubs | Series::SpeedGaming | Series::WeTryToBeBetter => TimeDelta::hours(3),
                    Series::CopaDoBrasil | Series::League | Series::NineDaysOfSaws | Series::SongsOfHope | Series::Standard | Series::TournoiFrancophone => TimeDelta::hours(3) + TimeDelta::minutes(30),
                    Series::Multiworld | Series::Pictionary => TimeDelta::hours(4),
                    Series::Rsl => TimeDelta::hours(4) + TimeDelta::minutes(30),
                }))); //TODO better fallback duration estimates depending on participants
                let mut urls = Vec::default();
                for (language, video_url) in &race.video_urls {
                    urls.push((Cow::Owned(format!("{language} restream")), video_url.clone()));
                }
                if let Some(room) = race_event.room() {
                    urls.push((Cow::Borrowed("race room"), room.clone()));
                }
                if let Some(set_url) = race.startgg_set_url()? {
                    urls.push((Cow::Borrowed("start.gg set"), set_url));
                }
                if let Some((_, url)) = urls.get(0) {
                    cal_event.push(URL::new(url.to_string()));
                    urls.remove(0);
                    if !urls.is_empty() {
                        cal_event.push(Description::new(urls.into_iter().map(|(description, url)| format!("{description}: {url}")).join("\n")));
                    }
                } else {
                    cal_event.push(URL::new(uri!("https://midos.house", event::info(event.series, &*event.event)).to_string()));
                }
                cal.add_event(cal_event);
                if let (Series::Standard, "w", Some(round)) = (event.series, &*event.event, &race.round) {
                    match &**round {
                        "NA Weekly" => latest_instantiated_na_weekly = Some(start),
                        "EU Weekly" => latest_instantiated_eu_weekly = Some(start),
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(latest_instantiated_na_weekly) = latest_instantiated_na_weekly {
        let mut cal_event = ics::Event::new("weekly-na@midos.house", dtstamp(now));
        cal_event.push(Summary::new("NA Weekly"));
        let start = s::next_na_weekly_after(latest_instantiated_na_weekly);
        cal_event.push(dtstart(start));
        cal_event.push(dtend(start + TimeDelta::hours(3) + TimeDelta::minutes(30)));
        cal_event.push(RRule::new("FREQ=WEEKLY"));
        cal.add_event(cal_event);
    }
    if let Some(latest_instantiated_eu_weekly) = latest_instantiated_eu_weekly {
        let mut cal_event = ics::Event::new("weekly-eu@midos.house", dtstamp(now));
        cal_event.push(Summary::new("EU Weekly"));
        let start = s::next_eu_weekly_after(latest_instantiated_eu_weekly);
        cal_event.push(dtstart(start));
        cal_event.push(dtend(start + TimeDelta::hours(3) + TimeDelta::minutes(30)));
        cal_event.push(RRule::new("FREQ=WEEKLY"));
        cal.add_event(cal_event);
    }
    Ok(())
}

#[rocket::get("/calendar")]
pub(crate) async fn index_help(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> PageResult {
    page(pool.begin().await?, &me, &uri, PageStyle::default(), "Calendar — Mido's House", html! {
        p {
            : "A calendar of all races across all events can be found at ";
            code : uri!("https://midos.house", index).to_string();
            : " — by pasting this link into most calendar apps' “subscribe” feature instead of downloading it, you can get automatic updates as races are scheduled:";
        }
        ul {
            li {
                : "In Google Calendar, select ";
                a(href = "https://calendar.google.com/calendar/u/0/r/settings/addbyurl") : "Add calendar → From URL";
            }
            li {
                : "In Apple Calendar, press ";
                kbd : "⌥";
                kbd : "⌘";
                kbd : "S";
                : " or select File → New Calendar Subscription";
            }
            li : "In Mozilla Thunderbird, select New Calendar → On the Network. Paste the link into the “Location” field and click “Find Calendars”, then “Properties”. Enable “Read Only” and click “OK”, then “Subscribe”.";
        }
        //p : "You can also find calendar links for individual events on their pages."; //TODO
    }).await
}

#[rocket::get("/calendar.ics")]
pub(crate) async fn index(discord_ctx: &State<RwFuture<DiscordCtx>>, pool: &State<PgPool>, http_client: &State<reqwest::Client>) -> Result<Response<ICalendar<'static>>, Error> {
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for row in sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE listed"#).fetch_all(&mut *transaction).await? {
        let event = event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, &*discord_ctx.read().await, http_client, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/series/<series>/calendar.ics")]
pub(crate) async fn for_series(discord_ctx: &State<RwFuture<DiscordCtx>>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series) -> Result<Response<ICalendar<'static>>, Error> {
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for event in sqlx::query_scalar!(r#"SELECT event FROM events WHERE listed AND series = $1"#, series as _).fetch_all(&mut *transaction).await? {
        let event = event::Data::new(&mut transaction, series, event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, &*discord_ctx.read().await, http_client, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/event/<series>/<event>/calendar.ics")]
pub(crate) async fn for_event(discord_ctx: &State<RwFuture<DiscordCtx>>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series, event: &str) -> Result<Response<ICalendar<'static>>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    add_event_races(&mut transaction, &*discord_ctx.read().await, http_client, &mut cal, &event).await?;
    transaction.commit().await?;
    Ok(Response(cal))
}

pub(crate) async fn create_race_form(mut transaction: Transaction<'_, Postgres>, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, ctx: Context<'_>, is_3p: bool) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, env, me.as_ref(), Tab::Races, true).await?;
    let form = if me.is_some() {
        let teams = Team::for_event(&mut transaction, event.series, &event.event).await?;
        let mut team_data = Vec::with_capacity(teams.len());
        for team in teams {
            let name = if let Some(name) = team.name(&mut transaction).await? {
                name.into_owned()
            } else {
                format!("unnamed team ({})", English.join_str(team.members(&mut transaction).await?).unwrap_or_else(|| format!("no members")))
            };
            team_data.push((team.id.to_string(), name));
        }
        let phase_round_options = sqlx::query!("SELECT phase, round FROM phase_round_options WHERE series = $1 AND event = $2", event.series as _, &event.event).fetch_all(&mut *transaction).await?;
        let mut errors = ctx.errors().collect_vec();
        full_form(uri!(create_race_post(event.series, &*event.event)), csrf, html! {
            : form_field("team1", &mut errors, html! {
                label(for = "team1") {
                    @if let TeamConfig::Solo = event.team_config {
                        : "Player A:";
                    } else {
                        : "Team A:";
                    }
                }
                select(name = "team1") {
                    @for (id, name) in &team_data {
                        option(value = id, selected? = ctx.field_value("team1") == Some(id)) : name;
                    }
                }
            });
            : form_field("team2", &mut errors, html! {
                label(for = "team2") {
                    @if let TeamConfig::Solo = event.team_config {
                        : "Player B:";
                    } else {
                        : "Team B:";
                    }
                }
                select(name = "team2") {
                    @for (id, name) in &team_data {
                        option(value = id, selected? = ctx.field_value("team2") == Some(id)) : name;
                    }
                }
            });
            @if is_3p {
                : form_field("team3", &mut errors, html! {
                    label(for = "team3") {
                        @if let TeamConfig::Solo = event.team_config {
                            : "Player C:";
                        } else {
                            : "Team C:";
                        }
                    }
                    select(name = "team3") {
                        @for (id, name) in team_data {
                            option(value = id, selected? = ctx.field_value("team3") == Some(&id)) : name;
                        }
                    }
                });
            }
            @if phase_round_options.is_empty() {
                : form_field("phase", &mut errors, html! {
                    label(for = "phase") : "Phase:";
                    input(type = "text", name = "phase", value? = ctx.field_value("phase"));
                });
                : form_field("round", &mut errors, html! {
                    label(for = "round") : "Round:";
                    input(type = "text", name = "round", value? = ctx.field_value("round"));
                });
            } else {
                : form_field("phase_round", &mut errors, html! {
                    label(for = "phase_round") : "Round:";
                    select(name = "phase_round") {
                        @for row in phase_round_options {
                            @let option = format!("{} {}", row.phase, row.round);
                            option(value = &option, selected? = ctx.field_value("phase_round") == Some(&option)) : option;
                        }
                    }
                });
            }
            : form_field("game_count", &mut errors, html! {
                label(for = "game_count") : "Number of games in this match:";
                input(type = "number", min = "1", max = "255", name = "game_count", value = ctx.field_value("game_count").map_or_else(|| event.default_game_count.to_string(), |game_count| game_count.to_owned()));
                label(class = "help") {
                    : "(If some games end up not being necessary, use ";
                    code : "/delete-after";
                    : " in the scheduling thread to delete them.)";
                }
            });
        }, errors, "Create")
    } else {
        html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(create_race(event.series, &*event.event, Some(NonZeroU8::new(if ctx.field_value("team3").is_some() { 3 } else { 2 }).unwrap())))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to create a race.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await?, ..PageStyle::default() }, &format!("New Race — {}", event.display_name), html! {
        : header;
        h2 : "Create race";
        : form;
    }).await?)
}

#[rocket::get("/event/<series>/<event>/races/new?<players>")]
pub(crate) async fn create_race(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: String, players: Option<NonZeroU8>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let is_3p = match players.unwrap_or_else(|| NonZeroU8::new(2).unwrap()).get() {
        2 => false,
        3 => true,
        _ => return Err(StatusOrError::Status(Status::NotImplemented)),
    };
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(create_race_form(transaction, **env, me, uri, csrf.as_ref(), event, Context::default(), is_3p).await?))
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct CreateRaceForm {
    #[field(default = String::new())]
    csrf: String,
    team1: Id<Teams>,
    team2: Id<Teams>,
    team3: Option<Id<Teams>>,
    #[field(default = String::new())]
    phase: String,
    #[field(default = String::new())]
    round: String,
    #[field(default = String::new())]
    phase_round: String,
    game_count: i16,
}

#[rocket::post("/event/<series>/<event>/races/new", data = "<form>")]
pub(crate) async fn create_race_post(pool: &State<PgPool>, env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, CreateRaceForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if !event.organizers(&mut transaction).await?.contains(&me) {
        form.context.push_error(form::Error::validation("You must be an organizer of this event to add a race."));
    }
    match event.match_source() {
        MatchSource::Manual => {}
        MatchSource::League => form.context.push_error(form::Error::validation("This event's races are generated automatically from league.ootrandomizer.com and cannot be edited manually. Please contact Fenhl if a race needs to be added that's not listed at league.ootrandomizer.com.")),
        MatchSource::StartGG(_) => {} // allow adding tiebreakers which aren't tracked by start.gg
    }
    Ok(if let Some(ref value) = form.value {
        let team1 = Team::from_id(&mut transaction, value.team1).await?;
        if let Some(_) = team1 {
            //TODO validate that this team is for this event
        } else {
            form.context.push_error(form::Error::validation("There is no team with this ID.").with_name("team1"));
        }
        let team2 = Team::from_id(&mut transaction, value.team2).await?;
        if let Some(_) = team2 {
            //TODO validate that this team is for this event
        } else {
            form.context.push_error(form::Error::validation("There is no team with this ID.").with_name("team2"));
        }
        if team1 == team2 {
            form.context.push_error(form::Error::validation("Can't choose the same team twice.").with_name("team2"));
        }
        let team3 = if let Some(team3) = value.team3 {
            let team3 = Team::from_id(&mut transaction, team3).await?;
            if let Some(_) = team3 {
                //TODO validate that this team is for this event
            } else {
                form.context.push_error(form::Error::validation("There is no team with this ID.").with_name("team3"));
            }
            if team1 == team3 || team2 == team3 {
                form.context.push_error(form::Error::validation("Can't choose the same team twice.").with_name("team3"));
            }
            team3
        } else {
            None
        };
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(create_race_form(transaction, **env, Some(me), uri, csrf.as_ref(), event, form.context, team3.is_some()).await?)
        } else {
            let (phase, round) = if value.phase_round.is_empty() {
                (
                    (!value.phase.is_empty()).then(|| value.phase.clone()),
                    (!value.round.is_empty()).then(|| value.round.clone()),
                )
            } else {
                sqlx::query!("SELECT phase, round FROM phase_round_options WHERE series = $1 AND event = $2", event.series as _, &event.event).fetch_all(&mut *transaction).await?
                    .into_iter()
                    .find(|row| format!("{} {}", row.phase, row.round) == value.phase_round)
                    .map(|row| (Some(row.phase), Some(row.round)))
                    .unwrap_or_else(|| (None, Some(value.phase_round.clone())))
            };
            let [team1, team2] = [team1, team2].map(|team| team.expect("validated"));
            let draft = if team3.is_some() {
                None
            } else if let Some(draft_kind) = event.draft_kind() {
                let [high_seed, low_seed] = match draft_kind {
                    draft::Kind::S7 => [
                        min_by_key(&team1, &team2, |team| team.qualifier_rank).id,
                        max_by_key(&team1, &team2, |team| team.qualifier_rank).id,
                    ],
                    draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 => [team1.id, team2.id],
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 => {
                        let mut team_ids = [team1.id, team2.id];
                        team_ids.shuffle(&mut thread_rng());
                        team_ids
                    }
                };
                Some(Draft::new(&mut transaction, draft_kind, high_seed, low_seed).await?)
            } else {
                None
            };
            let mut scheduling_thread = None;
            for game in 1..=value.game_count {
                let mut race = Race {
                    id: Id::<Races>::new(&mut transaction).await?,
                    series: event.series,
                    event: event.event.to_string(),
                    source: Source::Manual,
                    entrants: if let Some(ref team3) = team3 {
                        Entrants::Three([
                            Entrant::MidosHouseTeam(team1.clone()),
                            Entrant::MidosHouseTeam(team2.clone()),
                            Entrant::MidosHouseTeam(team3.clone()),
                        ])
                    } else {
                        Entrants::Two([
                            Entrant::MidosHouseTeam(team1.clone()),
                            Entrant::MidosHouseTeam(team2.clone()),
                        ])
                    },
                    phase: phase.clone(),
                    round: round.clone(),
                    game: (value.game_count > 1).then_some(game),
                    schedule: RaceSchedule::Unscheduled,
                    schedule_updated_at: None,
                    draft: draft.clone(),
                    seed: seed::Data::default(),
                    video_urls: HashMap::default(),
                    restreamers: HashMap::default(),
                    last_edited_by: None,
                    last_edited_at: None,
                    ignored: false,
                    schedule_locked: false,
                    scheduling_thread,
                };
                if game == 1 {
                    transaction = discord_bot::create_scheduling_thread(&*discord_ctx.read().await, transaction, &mut race, value.game_count).await?;
                    scheduling_thread = race.scheduling_thread;
                }
                race.save(&mut transaction).await?;
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        let is_3p = form.context.field_value("team3").is_some();
        RedirectOrContent::Content(create_race_form(transaction, **env, Some(me), uri, csrf.as_ref(), event, form.context, is_3p).await?)
    })
}

enum ImportSkipReason {
    Exists,
    Preview,
    Slots,
    Participants,
    SetGamesType,
}

impl fmt::Display for ImportSkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists => write!(f, "already exists"),
            Self::Preview => write!(f, "is a preview"),
            Self::Slots => write!(f, "no match on slots"),
            Self::Participants => write!(f, "no match on participants"),
            Self::SetGamesType => write!(f, "unknown games type"),
        }
    }
}

pub(crate) async fn race_table(
    transaction: &mut Transaction<'_, Postgres>,
    discord_ctx: &DiscordCtx,
    env: Environment,
    http_client: &reqwest::Client,
    event: Option<&event::Data<'_>>,
    game_count: bool,
    show_multistreams: bool,
    can_create: bool,
    can_edit: bool,
    show_restream_consent: bool,
    races: &[Race],
) -> Result<RawHtml<String>, Error> {
    let has_games = game_count || races.iter().any(|race| race.game.is_some());
    let has_seeds = races.iter().any(|race| (race.seed.file_hash.is_some() || race.seed.files.is_some()) && race.show_seed());
    let has_buttons = can_create || can_edit;
    let now = Utc::now();
    Ok(html! {
        table {
            thead {
                tr {
                    th : "Start";
                    @if event.is_none() {
                        th : "Event";
                    }
                    th : "Round";
                    @if has_games {
                        @if game_count {
                            th : "Best of";
                        } else {
                            th : "Game";
                        }
                    }
                    th(colspan = "6") : "Entrants";
                    th : "Links";
                    @if has_seeds {
                        : seed::table_header_cells(true);
                    }
                    @if show_restream_consent {
                        th : "Restream Consent";
                    }
                    @if has_buttons {
                        th {
                            @if can_create {
                                @if let Some(event) = event {
                                    @match event.match_source() {
                                        MatchSource::Manual => a(class = "button", href = uri!(create_race(races[0].series, &*races[0].event, _)).to_string()) : "New Race";
                                        MatchSource::League => {}
                                        MatchSource::StartGG(_) => @if !event.auto_import {
                                            a(class = "button", href = uri!(import_races(races[0].series, &*races[0].event)).to_string()) : "Import";
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            tbody {
                @for race in races {
                    tr {
                        @let (event, show_event) = if let Some(event) = event {
                            (event.clone(), false)
                        } else {
                            (event::Data::new(&mut *transaction, race.series, &race.event).await?.expect("race for nonexistent event"), true)
                        };
                        td {
                            @match race.schedule {
                                RaceSchedule::Unscheduled => {}
                                RaceSchedule::Live { start, .. } => : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                RaceSchedule::Async { .. } => : "(async)";
                            }
                        }
                        @if show_event {
                            td(class = "small-table-content") {
                                a(href = uri!(event::info(event.series, &*event.event)).to_string()) : event.short_name();
                            }
                            td(class = "large-table-content") : event;
                        }
                        td {
                            : race.phase;
                            : " ";
                            : race.round;
                        }
                        @if has_games {
                            td {
                                @if let Some(game) = race.game {
                                    : game;
                                }
                            }
                        }
                        @match race.entrants {
                            Entrants::Open => td(colspan = "6") : "(open)";
                            Entrants::Count { total, finished } => td(colspan = "6") {
                                : total;
                                : " (";
                                : finished;
                                : " finishers)";
                            }
                            Entrants::Named(ref entrants) => td(colspan = "6") : entrants;
                            Entrants::Two([ref team1, ref team2]) => {
                                td(class = "vs1", colspan = "3") {
                                    : team1.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                    @if let RaceSchedule::Async { start1: Some(start), .. } = race.schedule {
                                        br;
                                        small {
                                            : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                        }
                                    }
                                }
                                td(class = "vs2", colspan = "3") {
                                    : team2.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                    @if let RaceSchedule::Async { start2: Some(start), .. } = race.schedule {
                                        br;
                                        small {
                                            : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                        }
                                    }
                                }
                            }
                            Entrants::Three([ref team1, ref team2, ref team3]) => {
                                td(colspan = "2") {
                                    : team1.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                    @if let RaceSchedule::Async { start1: Some(start), .. } = race.schedule {
                                        br;
                                        small {
                                            : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                        }
                                    }
                                }
                                td(colspan = "2") {
                                    : team2.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                    @if let RaceSchedule::Async { start2: Some(start), .. } = race.schedule {
                                        br;
                                        small {
                                            : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                        }
                                    }
                                }
                                td(colspan = "2") {
                                    : team3.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                    @if let RaceSchedule::Async { start3: Some(start), .. } = race.schedule {
                                        br;
                                        small {
                                            : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                        }
                                    }
                                }
                            }
                        }
                        td {
                            div(class = "favicon-container") {
                                @for (language, video_url) in &race.video_urls {
                                    a(class = "favicon", title = format!("{language} restream"), href = video_url.to_string()) : favicon(video_url);
                                }
                                @if show_multistreams && race.video_urls.is_empty() {
                                    @if let Some(multistream_url) = race.multistream_url(&mut *transaction, env, http_client, &event).await? {
                                        a(class = "favicon", title = "multistream", href = multistream_url.to_string()) : favicon(&multistream_url);
                                    }
                                }
                                @for (user, video_url) in race.player_video_urls(&mut *transaction).await? {
                                    a(class = "favicon", title = format!("{user}'s vod"), href = video_url.to_string()) : favicon(&video_url);
                                }
                                @if let Some(startgg_url) = race.startgg_set_url()? {
                                    a(class = "favicon", title = "start.gg set", href = startgg_url.to_string()) : favicon(&startgg_url);
                                }
                                @for room in race.rooms() {
                                    a(class = "favicon", title = "race room", href = room.to_string()) : favicon(&room);
                                }
                            }
                        }
                        @if has_seeds {
                            @if race.show_seed() {
                                : seed::table_cells(now, &race.seed, true, can_edit.then(|| uri!(cal::add_file_hash(race.series, &*race.event, race.id)))).await?;
                            } else {
                                // hide seed if unfinished async
                                //TODO show to the team that played the 1st async half
                                : seed::table_cells(now, &seed::Data::default(), true, None).await?;
                            }
                        }
                        @if show_restream_consent {
                            td {
                                @if race.teams().all(|team| team.restream_consent) {
                                    : "✓";
                                }
                            }
                        }
                        @if has_buttons {
                            td {
                                @if can_edit {
                                    a(class = "button", href = uri!(crate::cal::edit_race(race.series, &race.event, race.id)).to_string()) : "Edit";
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// Returns:
///
/// * A list of races to import. Only one race for each match is imported, with the `game` field specifying the total number of games in the match.
///   The caller is expected to duplicate this race to get the different games of the match, and create a single scheduling thread for the match.
///   A `game` value of `None` should be treated like `Some(1)`.
/// * A list of start.gg set IDs that were not imported, along with the reasons they were skipped.
async fn startgg_races_to_import(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, event: &event::Data<'_>, event_slug: &str) -> Result<(Vec<Race>, Vec<(startgg::ID, ImportSkipReason)>), Error> {
    async fn process_set(
        transaction: &mut Transaction<'_, Postgres>,
        http_client: &reqwest::Client,
        event: &event::Data<'_>,
        races: &mut Vec<Race>,
        startgg_event: &str,
        set: startgg::ID,
        phase: Option<String>,
        round: Option<String>,
        team1: Team,
        team2: Team,
        set_games_type: Option<i64>,
        total_games: Option<i64>,
        best_of: Option<i64>,
    ) -> Result<Option<ImportSkipReason>, Error> {
        races.push(Race {
            id: Id::new(&mut *transaction).await?,
            series: event.series,
            event: event.event.to_string(),
            source: Source::StartGG {
                event: startgg_event.to_owned(),
                set,
            },
            entrants: Entrants::Two([
                Entrant::MidosHouseTeam(team1.clone()),
                Entrant::MidosHouseTeam(team2.clone()),
            ]),
            game: match set_games_type {
                Some(1) => best_of.map(|best_of| best_of.try_into().expect("too many games")),
                Some(2) => total_games.map(|total_games| total_games.try_into().expect("too many games")),
                _ => return Ok(Some(ImportSkipReason::SetGamesType)),
            },
            scheduling_thread: None,
            schedule: RaceSchedule::Unscheduled,
            schedule_updated_at: None,
            draft: if let Some(draft_kind) = event.draft_kind() {
                let [high_seed, low_seed] = match draft_kind {
                    draft::Kind::S7 => [
                        min_by_key(&team1, &team2, |team| team.qualifier_rank).id,
                        max_by_key(&team1, &team2, |team| team.qualifier_rank).id,
                    ],
                    draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 => if phase.as_ref().is_some_and(|phase| phase == "Top 8") {
                        let seeding = [
                            Id::from(8429274534302278572_u64), // Anju's Secret
                            Id::from(12142947927479333421_u64), // ADD
                            Id::from(5548902498821246494_u64), // Donutdog!!! Wuff! Wuff!
                            Id::from(7622448514297787774_u64), // Snack Pack
                            Id::from(13644615382444869291_u64), // The Highest Gorons
                            Id::from(4984265622447250649_u64), // Bongo Akimbo
                            Id::from(592664405695569367_u64), // The Jhegsons
                            Id::from(14405144517033747435_u64), // Pandora's Brot
                        ];
                        let mut team_ids = [team1.id, team2.id];
                        team_ids.sort_unstable_by_key(|team| seeding.iter().position(|iter_team| iter_team == team));
                        team_ids
                    } else {
                        let qualifier_kind = teams::QualifierKind::Single { //TODO adjust to match teams::get?
                            show_times: event.show_qualifier_times && event.is_started(&mut *transaction).await?,
                        };
                        let signups = teams::signups_sorted(&mut *transaction, http_client, None, event, qualifier_kind).await?;
                        let SignupsTeam { members: members1, .. } = signups.iter().find(|SignupsTeam { team, .. }| team.as_ref().is_some_and(|team| *team == team1)).expect("match with team that didn't sign up");
                        let SignupsTeam { members: members2, .. } = signups.iter().find(|SignupsTeam { team, .. }| team.as_ref().is_some_and(|team| *team == team2)).expect("match with team that didn't sign up");
                        let avg1 = members1.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)).map(|total| total / u32::try_from(members1.len()).expect("too many team members"));
                        let avg2 = members2.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)).map(|total| total / u32::try_from(members2.len()).expect("too many team members"));
                        match [avg1, avg2] {
                            [Some(_), None] => [team1.id, team2.id],
                            [None, Some(_)] => [team2.id, team1.id],
                            [Some(avg1), Some(avg2)] if avg1 < avg2 => [team1.id, team2.id],
                            [Some(avg1), Some(avg2)] if avg1 > avg2 => [team2.id, team1.id],
                            _ => {
                                // tie broken by coin flip
                                let mut team_ids = [team1.id, team2.id];
                                team_ids.shuffle(&mut thread_rng());
                                team_ids
                            }
                        }
                    },
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 => unreachable!("this event does not use start.gg"),
                };
                Some(Draft::new(&mut *transaction, draft_kind, high_seed, low_seed).await?)
            } else {
                None
            },
            seed: seed::Data::default(),
            video_urls: HashMap::default(),
            restreamers: HashMap::default(),
            last_edited_by: None,
            last_edited_at: None,
            ignored: false,
            schedule_locked: false,
            phase, round,
        });
        Ok(None)
    }

    async fn process_page(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, event: &event::Data<'_>, event_slug: &str, page: i64, races: &mut Vec<Race>, skips: &mut Vec<(startgg::ID, ImportSkipReason)>) -> Result<i64, Error> {
        let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
        if let TeamConfig::Solo = event.team_config {
            let startgg::solo_event_sets_query::ResponseData {
                event: Some(startgg::solo_event_sets_query::SoloEventSetsQueryEvent {
                    sets: Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSets {
                        page_info: Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsPageInfo { total_pages: Some(total_pages) }),
                        nodes: Some(sets),
                    }),
                }),
            } = startgg::query_cached::<startgg::SoloEventSetsQuery>(http_client, startgg_token, startgg::solo_event_sets_query::Variables { event_slug: event_slug.to_owned(), page }).await? else { panic!("no match on query") };
            for set in sets.into_iter().filter_map(identity) {
                let startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodes { id: Some(id), phase_group, full_round_text, slots: Some(slots), set_games_type, total_games, round } = set else { panic!("unexpected set format") };
                if id.0.starts_with("preview") {
                    skips.push((id, ImportSkipReason::Preview));
                } else if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM races WHERE startgg_set = $1) AS "exists!""#, id as _).fetch_one(&mut **transaction).await? {
                    skips.push((id, ImportSkipReason::Exists));
                } else if let [
                    Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlots { entrant: Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrant { participants: Some(ref p1) }) }),
                    Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlots { entrant: Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrant { participants: Some(ref p2) }) }),
                ] = *slots {
                    if let [Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrantParticipants { id: Some(ref team1) })] = **p1 {
                        if let [Some(startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrantParticipants { id: Some(ref team2) })] = **p2 {
                            let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or_else(|| Error::UnknownTeamStartGG(team1.clone()))?;
                            let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or_else(|| Error::UnknownTeamStartGG(team2.clone()))?;
                            let best_of = phase_group.as_ref()
                                .and_then(|startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroup { rounds, .. }| rounds.as_ref())
                                .and_then(|rounds| rounds.iter().filter_map(Option::as_ref).find(|startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroupRounds { number, .. }| *number == round))
                                .and_then(|startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroupRounds { best_of, .. }| *best_of);
                            let phase = phase_group
                                .and_then(|startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroup { phase, .. }| phase)
                                .and_then(|startgg::solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroupPhase { name }| name);
                            if let Some(reason) = process_set(&mut *transaction, http_client, event, races, event_slug, id.clone(), phase, full_round_text, team1, team2, set_games_type, total_games, best_of).await? {
                                skips.push((id, reason));
                            }
                        } else {
                            skips.push((id, ImportSkipReason::Participants));
                        }
                    } else {
                        skips.push((id, ImportSkipReason::Participants));
                    }
                } else {
                    skips.push((id, ImportSkipReason::Slots));
                }
            }
            Ok(total_pages)
        } else {
            let startgg::team_event_sets_query::ResponseData {
                event: Some(startgg::team_event_sets_query::TeamEventSetsQueryEvent {
                    sets: Some(startgg::team_event_sets_query::TeamEventSetsQueryEventSets {
                        page_info: Some(startgg::team_event_sets_query::TeamEventSetsQueryEventSetsPageInfo { total_pages: Some(total_pages) }),
                        nodes: Some(sets),
                    }),
                }),
            } = startgg::query_cached::<startgg::TeamEventSetsQuery>(http_client, startgg_token, startgg::team_event_sets_query::Variables { event_slug: event_slug.to_owned(), page }).await? else { panic!("no match on query") };
            for set in sets.into_iter().filter_map(identity) {
                let startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodes { id: Some(id), phase_group, full_round_text, slots: Some(slots), set_games_type, total_games, round } = set else { panic!("unexpected set format") };
                if id.0.starts_with("preview") {
                    skips.push((id, ImportSkipReason::Preview));
                } else if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM races WHERE startgg_set = $1) AS "exists!""#, id as _).fetch_one(&mut **transaction).await? {
                    skips.push((id, ImportSkipReason::Exists));
                } else if let [
                    Some(startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlots { entrant: Some(startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlotsEntrant { id: Some(ref team1) }) }),
                    Some(startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlots { entrant: Some(startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlotsEntrant { id: Some(ref team2) }) }),
                ] = *slots {
                    let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or_else(|| Error::UnknownTeamStartGG(team1.clone()))?;
                    let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or_else(|| Error::UnknownTeamStartGG(team2.clone()))?;
                    let best_of = phase_group.as_ref()
                        .and_then(|startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroup { rounds, .. }| rounds.as_ref())
                        .and_then(|rounds| rounds.iter().filter_map(Option::as_ref).find(|startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroupRounds { number, .. }| *number == round))
                        .and_then(|startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroupRounds { best_of, .. }| *best_of);
                    let phase = phase_group
                        .and_then(|startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroup { phase, .. }| phase)
                        .and_then(|startgg::team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroupPhase { name }| name);
                    if let Some(reason) = process_set(&mut *transaction, http_client, event, races, event_slug, id.clone(), phase, full_round_text, team1, team2, set_games_type, total_games, best_of).await? {
                        skips.push((id, reason));
                    }
                } else {
                    skips.push((id, ImportSkipReason::Slots));
                }
            }
            Ok(total_pages)
        }
    }

    let mut races = Vec::default();
    let mut skips = Vec::default();
    let total_pages = process_page(&mut *transaction, http_client, env, config, event, event_slug, 1, &mut races, &mut skips).await?;
    for page in 2..=total_pages {
        process_page(&mut *transaction, http_client, env, config, event, event_slug, page, &mut races, &mut skips).await?;
    }
    Ok((races, skips))
}

pub(crate) async fn import_races_form(mut transaction: Transaction<'_, Postgres>, http_client: &reqwest::Client, discord_ctx: &DiscordCtx, env: Environment, config: &Config, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, env, me.as_ref(), Tab::Races, true).await?;
    let form = match event.match_source() {
        MatchSource::Manual => html! {
            article {
                p : "This event has no source for importing races configured.";
            }
        },
        MatchSource::League => html! {
            article {
                p {
                    : "Races for this event are automatically imported from ";
                    a(href = "https://league.ootrandomizer.com/") : "league.ootrandomizer.com";
                    : ".";
                }
            }
        },
        MatchSource::StartGG(event_slug) => if event.auto_import {
            html! {
                article {
                    p : "Races for this event are imported automatically every 5 minutes.";
                }
            }
        } else if me.is_some() {
            let (races, skips) = startgg_races_to_import(&mut transaction, http_client, env, config, &event, event_slug).await?;
            if races.is_empty() {
                html! {
                    article {
                        @if skips.is_empty() {
                            p : "start.gg did not list any matches for this event.";
                        } else {
                            p : "There are no races to import. The following matches have been skipped:";
                            table {
                                thead {
                                    tr {
                                        th : "start.gg match ID";
                                        th : "Reason";
                                    }
                                }
                                tbody {
                                    @for (set_id, reason) in skips {
                                        tr {
                                            td : set_id.0;
                                            td : reason.to_string();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                let table = race_table(&mut transaction, discord_ctx, env, http_client, Some(&event), true, false, false, false, false, &races).await?;
                let errors = ctx.errors().collect_vec();
                full_form(uri!(import_races_post(event.series, &*event.event)), csrf, html! {
                    p : "The following races will be imported:";
                    : table;
                }, errors, "Import")
            }
        } else {
            html! {
                article {
                    p {
                        a(href = uri!(auth::login(Some(uri!(import_races(event.series, &*event.event))))).to_string()) : "Sign in or create a Mido's House account";
                        : " to import races.";
                    }
                }
            }
        },
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await?, ..PageStyle::default() }, &format!("Import Races — {}", event.display_name), html! {
        : header;
        h2 : "Import races";
        : form;
    }).await?)
}

#[rocket::get("/event/<series>/<event>/races/import")]
pub(crate) async fn import_races(config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, discord_ctx: &State<RwFuture<DiscordCtx>>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: String) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(import_races_form(transaction, http_client, &*discord_ctx.read().await, **env, config, me, uri, csrf.as_ref(), event, Context::default()).await?))
}

#[rocket::post("/event/<series>/<event>/races/import", data = "<form>")]
pub(crate) async fn import_races_post(discord_ctx: &State<RwFuture<DiscordCtx>>, env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, EmptyForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if !event.organizers(&mut transaction).await?.contains(&me) {
        form.context.push_error(form::Error::validation("You must be an organizer to import races."));
    }
    let races = match event.match_source() {
        MatchSource::Manual => {
            form.context.push_error(form::Error::validation("This event has no source for importing races configured."));
            Vec::default()
        }
        MatchSource::League => {
            form.context.push_error(form::Error::validation("Races for this event are automatically imported from league.ootrandomizer.com."));
            Vec::default()
        }
        MatchSource::StartGG(event_slug) => {
            let (races, skips) = startgg_races_to_import(&mut transaction, http_client, **env, config, &event, event_slug).await?;
            if races.is_empty() {
                if skips.is_empty() {
                    form.context.push_error(form::Error::validation("start.gg did not list any matches for this event."));
                } else {
                    form.context.push_error(form::Error::validation("There are no races to import. Some matches have been skipped."));
                }
            }
            races
        }
    };
    Ok(if form.context.errors().next().is_some() {
        RedirectOrContent::Content(import_races_form(transaction, http_client, &*discord_ctx.read().await, **env, config, Some(me), uri, csrf.as_ref(), event, form.context).await?)
    } else {
        for race in races {
            transaction = import_race(transaction, &*discord_ctx.read().await, race).await?;
        }
        transaction.commit().await?;
        RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
    })
}

async fn import_race<'a>(mut transaction: Transaction<'a, Postgres>, discord_ctx: &DiscordCtx, race: Race) -> Result<Transaction<'a, Postgres>, event::Error> {
    let game_count = race.game.unwrap_or(1);
    let mut scheduling_thread = None;
    for game in 1..=game_count {
        let mut race = Race {
            id: Id::<Races>::new(&mut transaction).await?,
            game: (game_count > 1).then_some(game),
            scheduling_thread,
            ..race.clone()
        };
        if game == 1 {
            transaction = discord_bot::create_scheduling_thread(discord_ctx, transaction, &mut race, game_count).await?;
            scheduling_thread = race.scheduling_thread;
        }
        race.save(&mut transaction).await?;
    }
    Ok(transaction)
}

async fn auto_import_races_inner(db_pool: PgPool, http_client: reqwest::Client, env: Environment, config: Config, mut shutdown: rocket::Shutdown, discord_ctx: RwFuture<DiscordCtx>) -> Result<(), event::Error> {
    loop {
        let mut transaction = db_pool.begin().await?;
        for row in sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE auto_import AND start IS NOT NULL AND start <= NOW() AND (end_time IS NULL OR end_time > NOW())"#).fetch_all(&mut *transaction).await? {
            let event = event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during transaction");
            match event.match_source() {
                MatchSource::Manual => {}
                MatchSource::League => {}
                MatchSource::StartGG(event_slug) => {
                    let (races, _) = startgg_races_to_import(&mut transaction, &http_client, env, &config, &event, event_slug).await?;
                    for race in races {
                        transaction = import_race(transaction, &*discord_ctx.read().await, race).await?;
                    }
                }
            }
        }
        transaction.commit().await?;
        select! {
            () = &mut shutdown => break,
            () = sleep(Duration::from_secs(5 * 60)) => {}
        }
    }
    Ok(())
}

pub(crate) async fn auto_import_races(db_pool: PgPool, http_client: reqwest::Client, env: Environment, config: Config, shutdown: rocket::Shutdown, discord_ctx: RwFuture<DiscordCtx>) -> Result<(), event::Error> {
    let mut last_crash = Instant::now();
    let mut wait_time = Duration::from_secs(1);
    loop {
        match auto_import_races_inner(db_pool.clone(), http_client.clone(), env, config.clone(), shutdown.clone(), discord_ctx.clone()).await {
            Ok(()) => break Ok(()),
            Err(e) if e.is_network_error() => {
                if last_crash.elapsed() >= Duration::from_secs(60 * 60 * 24) {
                    wait_time = Duration::from_secs(1); // reset wait time after no crash for a day
                } else {
                    wait_time *= 2; // exponential backoff
                }
                if wait_time >= Duration::from_secs(2 * 60) {
                    eprintln!("failed to auto-import races (retrying in {}): {e} ({e:?})", English.format_duration(wait_time, true));
                    if wait_time >= Duration::from_secs(10 * 60) {
                        wheel::night_report("/net/midoshouse/error", Some(&format!("failed to auto-import races (retrying in {}): {e} ({e:?})", English.format_duration(wait_time, true)))).await?;
                    }
                }
                sleep(wait_time).await;
                last_crash = Instant::now();
            }
            Err(e) => {
                wheel::night_report("/net/midoshouse/error", Some(&format!("failed to auto-import races: {e} ({e:?})"))).await?;
                break Err(e)
            }
        }
    }
}

pub(crate) async fn edit_race_form(mut transaction: Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, race: Race, ctx: Option<Context<'_>>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, env, me.as_ref(), Tab::Races, true).await?;
    let fenhl = User::from_id(&mut *transaction, Id::<Users>::from(14571800683221815449_u64)).await?.ok_or(PageError::FenhlUserData)?;
    let form = if me.is_some() {
        let mut errors = ctx.as_ref().map(|ctx| ctx.errors().collect()).unwrap_or_default();
        full_form(uri!(edit_race_post(event.series, &*event.event, race.id)), csrf, html! {
            @match race.schedule {
                RaceSchedule::Unscheduled => {}
                RaceSchedule::Live { ref room, .. } => : form_field("room", &mut errors, html! {
                    label(for = "room") : "racetime.gg room:";
                    input(type = "text", name = "room", value? = if let Some(ref ctx) = ctx {
                        ctx.field_value("room").map(|room| room.to_string())
                    } else {
                        room.as_ref().map(|room| room.to_string())
                    });
                });
                RaceSchedule::Async { ref room1, ref room2, ref room3, .. } => {
                    : form_field("async_room1", &mut errors, html! {
                        label(for = "async_room1") : "racetime.gg room (team A):";
                        input(type = "text", name = "async_room1", value? = if let Some(ref ctx) = ctx {
                            ctx.field_value("async_room1").map(|room| room.to_string())
                        } else {
                            room1.as_ref().map(|room| room.to_string())
                        });
                    });
                    : form_field("async_room2", &mut errors, html! {
                        label(for = "async_room2") : "racetime.gg room (team B):";
                        input(type = "text", name = "async_room2", value? = if let Some(ref ctx) = ctx {
                            ctx.field_value("async_room2").map(|room| room.to_string())
                        } else {
                            room2.as_ref().map(|room| room.to_string())
                        });
                    });
                    @if let Entrants::Three(_) = race.entrants {
                        : form_field("async_room3", &mut errors, html! {
                            label(for = "async_room3") : "racetime.gg room (team C):";
                            input(type = "text", name = "async_room3", value? = if let Some(ref ctx) = ctx {
                                ctx.field_value("async_room3").map(|room| room.to_string())
                            } else {
                                room2.as_ref().map(|room| room.to_string())
                            });
                        });
                    }
                }
            }
            table {
                thead {
                    tr {
                        th;
                        th {
                            : "Restream URL";
                            br;
                            small(style = "font-weight: normal;") : "Please use the first available out of the following: Permanent Twitch highlight, YouTube or other video, Twitch past broadcast, Twitch channel.";
                        }
                        //TODO hide restreamers column if the race room exists
                        th {
                            : "Restreamer";
                            br;
                            small(style = "font-weight: normal;") : "racetime.gg profile URL, racetime.gg user ID, or Mido's House user ID. Enter “me” to assign yourself.";
                        }
                    }
                }
                tbody {
                    @for language in all::<Language>() {
                        tr {
                            th : language;
                            @let field_name = format!("video_urls.{}", language.short_code());
                            : form_table_cell(&field_name, &mut errors, html! {
                                input(type = "text", name = &field_name, value? = if let Some(ref ctx) = ctx {
                                    ctx.field_value(&*field_name).map(|room| room.to_string())
                                } else {
                                    race.video_urls.get(&language).map(|video_url| video_url.to_string())
                                });
                            });
                            //TODO hide restreamers column if the race room exists
                            @let field_name = format!("restreamers.{}", language.short_code());
                            : form_table_cell(&field_name, &mut errors, html! {
                                input(type = "text", name = &field_name, value? = if let Some(ref ctx) = ctx {
                                    ctx.field_value(&*field_name)
                                } else if me.as_ref().and_then(|me| me.racetime.as_ref()).map_or(false, |racetime| race.restreamers.get(&language).map_or(false, |restreamer| *restreamer == racetime.id)) {
                                    Some("me")
                                } else {
                                    race.restreamers.get(&language).map(|restreamer| restreamer.as_str()) //TODO display as racetime.gg profile URL
                                });
                            });
                        }
                    }
                }
            }
        }, errors, "Save")
    } else {
        html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(edit_race(event.series, &*event.event, race.id))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to edit this race.";
                }
            }
        }
    };
    let content = html! {
        : header;
        h2 : "Edit race";
        @match race.source {
            Source::Manual => p : "Source: Manually added";
            Source::League { id: _ } => p: "Source: league.ootrandomizer.com";
            Source::Sheet { timestamp } => p {
                : "Google Form submission timestamp: ";
                : timestamp.format("%d/%m/%Y %H:%M:%S").to_string();
                : " (unknown time zone)";
            }
            Source::StartGG { event, set: startgg::ID(set) } => {
                p {
                    : "start.gg event: ";
                    : event;
                }
                p {
                    : "start.gg match: ";
                    : set;
                }
            }
        }
        @match race.entrants {
            Entrants::Open => p : "Open entry";
            Entrants::Count { total, finished } => p {
                : total;
                : " entrants, ";
                : finished;
                : " finishers";
            }
            Entrants::Named(ref entrants) => p {
                : "Entrants: ";
                : entrants;
            }
            Entrants::Two([ref p1, ref p2]) => {
                p : "Entrants:";
                ol {
                    li : p1.to_html(&mut transaction, env, discord_ctx, false).await?;
                    li : p2.to_html(&mut transaction, env, discord_ctx, false).await?;
                }
            }
            Entrants::Three([ref p1, ref p2, ref p3]) => {
                p : "Entrants:";
                ol {
                    li : p1.to_html(&mut transaction, env, discord_ctx, false).await?;
                    li : p2.to_html(&mut transaction, env, discord_ctx, false).await?;
                    li : p3.to_html(&mut transaction, env, discord_ctx, false).await?;
                }
            }
        }
        @if let Some(phase) = race.phase {
            p {
                : "Phase: ";
                : phase;
            }
        }
        @if let Some(round) = race.round {
            p {
                : "Round: ";
                : round;
            }
        }
        @if let Some(game) = race.game {
            p {
                : "Game: ";
                : game;
            }
        }
        @match race.schedule {
            RaceSchedule::Unscheduled => p : "Not yet scheduled";
            RaceSchedule::Live { start, end, room: _ } => {
                p {
                    : "Start: ";
                    : format_datetime(start, DateTimeFormat { long: true, running_text: false });
                }
                @if let Some(end) = end {
                    p {
                        : "End: ";
                        : format_datetime(end, DateTimeFormat { long: true, running_text: false });
                    }
                } else {
                    p : "Not yet ended (will be updated automatically from the racetime.gg room, if any)";
                }
            }
            RaceSchedule::Async { start1, start2, start3, end1, end2, end3, room1: _, room2: _, room3: _ } => {
                @if let Some(start1) = start1 {
                    p {
                        : "Start (team A): ";
                        : format_datetime(start1, DateTimeFormat { long: true, running_text: false });
                    }
                } else {
                    p : "Team A not yet started";
                }
                @if let Some(start2) = start2 {
                    p {
                        : "Start (team B): ";
                        : format_datetime(start2, DateTimeFormat { long: true, running_text: false });
                    }
                } else {
                    p : "Team B not yet started";
                }
                @if let Entrants::Three(_) = race.entrants {
                    @if let Some(start3) = start3 {
                        p {
                            : "Start (team C): ";
                            : format_datetime(start3, DateTimeFormat { long: true, running_text: false });
                        }
                    } else {
                        p : "Team C not yet started";
                    }
                }
                @if let Some(end1) = end1 {
                    p {
                        : "End (team A): ";
                        : format_datetime(end1, DateTimeFormat { long: true, running_text: false });
                    }
                } else {
                    p : "Team A not yet ended (will be updated automatically from the racetime.gg room, if any)";
                }
                @if let Some(end2) = end2 {
                    p {
                        : "End (team B): ";
                        : format_datetime(end2, DateTimeFormat { long: true, running_text: false });
                    }
                } else {
                    p : "Team B not yet ended (will be updated automatically from the racetime.gg room, if any)";
                }
                @if let Entrants::Three(_) = race.entrants {
                    @if let Some(end3) = end3 {
                        p {
                            : "End (team C): ";
                            : format_datetime(end3, DateTimeFormat { long: true, running_text: false });
                        }
                    } else {
                        p : "Team C not yet ended (will be updated automatically from the racetime.gg room, if any)";
                    }
                }
            }
        }
        p {
            : "The data above is currently not editable for technical reasons. Please contact ";
            : fenhl;
            : " if you've spotted an error in it.";
        }
        : form;
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await?, ..PageStyle::default() }, &format!("Edit Race — {}", event.display_name), content).await?)
}

#[rocket::get("/event/<series>/<event>/races/<id>/edit")]
pub(crate) async fn edit_race(env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id<Races>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, id).await?;
    if race.series != event.series || race.event != event.event {
        return Ok(RedirectOrContent::Redirect(Redirect::permanent(uri!(edit_race(race.series, race.event, id)))))
    }
    Ok(RedirectOrContent::Content(edit_race_form(transaction, &*discord_ctx.read().await, **env, me, uri, csrf.as_ref(), event, race, None).await?))
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct EditRaceForm {
    #[field(default = String::new())]
    csrf: String,
    #[field(default = String::new())]
    room: String,
    #[field(default = String::new())]
    async_room1: String,
    #[field(default = String::new())]
    async_room2: String,
    #[field(default = String::new())]
    async_room3: String,
    #[field(default = HashMap::new())]
    video_urls: HashMap<Language, String>,
    #[field(default = HashMap::new())]
    restreamers: HashMap<Language, String>,
}

#[rocket::post("/event/<series>/<event>/races/<id>/edit", data = "<form>")]
pub(crate) async fn edit_race_post(discord_ctx: &State<RwFuture<DiscordCtx>>, env: &State<Environment>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id<Races>, form: Form<Contextual<'_, EditRaceForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, id).await?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if race.series != event.series || race.event != event.event {
        form.context.push_error(form::Error::validation("This race is not part of this event."));
    }
    if !me.is_archivist && !event.organizers(&mut transaction).await?.contains(&me) && !event.restreamers(&mut transaction).await?.contains(&me) {
        form.context.push_error(form::Error::validation("You must be an organizer, restreamer, or archivist to edit this race. If you would like to become an archivist, please contact Fenhl on Discord."));
    }
    Ok(if let Some(ref value) = form.value {
        let mut valid_room_urls = HashMap::new();
        match race.schedule {
            RaceSchedule::Unscheduled => {
                if !value.room.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("room"));
                }
                if !value.async_room1.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("async_room1"));
                }
                if !value.async_room2.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("async_room2"));
                }
                if !value.async_room3.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("async_room3"));
                }
            }
            RaceSchedule::Live { .. } => {
                if !value.room.is_empty() {
                    match Url::parse(&value.room) {
                        Ok(room) => if let Some(host) = room.host_str() {
                            if host == "racetime.gg" {
                                valid_room_urls.insert("room", room);
                            } else {
                                form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("room"));
                            }
                        } else {
                            form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("room"));
                        }
                        Err(e) => form.context.push_error(form::Error::validation(format!("Failed to parse race room URL: {e}")).with_name("room")),
                    }
                }
                if !value.async_room1.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("async_room1"));
                }
                if !value.async_room2.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("async_room2"));
                }
                if !value.async_room3.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("async_room3"));
                }
            }
            RaceSchedule::Async { .. } => {
                if !value.room.is_empty() {
                    form.context.push_error(form::Error::validation("The race room can't be added yet because the race isn't scheduled.").with_name("room"));
                }
                if !value.async_room1.is_empty() {
                    match Url::parse(&value.async_room1) {
                        Ok(room) => if let Some(host) = room.host_str() {
                            if host == "racetime.gg" {
                                valid_room_urls.insert("async_room1", room);
                            } else {
                                form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("async_room1"));
                            }
                        } else {
                            form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("async_room1"));
                        }
                        Err(e) => form.context.push_error(form::Error::validation(format!("Failed to parse race room URL: {e}")).with_name("async_room1")),
                    }
                }
                if !value.async_room2.is_empty() {
                    match Url::parse(&value.async_room2) {
                        Ok(room) => if let Some(host) = room.host_str() {
                            if host == "racetime.gg" {
                                valid_room_urls.insert("async_room2", room);
                            } else {
                                form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("async_room2"));
                            }
                        } else {
                            form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("async_room2"));
                        }
                        Err(e) => form.context.push_error(form::Error::validation(format!("Failed to parse race room URL: {e}")).with_name("async_room2")),
                    }
                }
                if !value.async_room3.is_empty() {
                    match Url::parse(&value.async_room3) {
                        Ok(room) => if let Some(host) = room.host_str() {
                            if host == "racetime.gg" {
                                valid_room_urls.insert("async_room3", room);
                            } else {
                                form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("async_room3"));
                            }
                        } else {
                            form.context.push_error(form::Error::validation("Race room must be a racetime.gg URL.").with_name("async_room3"));
                        }
                        Err(e) => form.context.push_error(form::Error::validation(format!("Failed to parse race room URL: {e}")).with_name("async_room3")),
                    }
                }
            }
        }
        let mut file_hash = None;
        let mut web_id = None;
        let mut web_gen_time = None;
        let mut file_stem = None;
        for (field_name, room) in valid_room_urls {
            if let Some(row) = sqlx::query!(r#"SELECT
                file_stem,
                web_id,
                web_gen_time,
                hash1 AS "hash1: HashIcon",
                hash2 AS "hash2: HashIcon",
                hash3 AS "hash3: HashIcon",
                hash4 AS "hash4: HashIcon",
                hash5 AS "hash5: HashIcon"
            FROM rsl_seeds WHERE room = $1"#, room.to_string()).fetch_optional(&mut *transaction).await? {
                file_hash = Some([row.hash1, row.hash2, row.hash3, row.hash4, row.hash5]);
                if let Some(new_web_id) = row.web_id {
                    web_id = Some(new_web_id);
                }
                if let Some(new_web_gen_time) = row.web_gen_time {
                    web_gen_time = Some(new_web_gen_time);
                }
                file_stem = Some(row.file_stem);
            } else {
                match http_client.get(format!("{room}/data")).send().await {
                    Ok(response) => match response.detailed_error_for_status().await {
                        Ok(response) => match response.json_with_text_in_error::<RaceData>().await {
                            Ok(race_data) => if let Some(info_bot) = race_data.info_bot {
                                if let Some((_, hash1, hash2, hash3, hash4, hash5, info_file_stem)) = regex_captures!("^(?:.+\n)?([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+)\nhttps://midos\\.house/seed/([0-9A-Za-z_-]+)(?:\\.zpfz?)?$", &info_bot) {
                                    let Some(hash1) = HashIcon::from_racetime_emoji(hash1) else { continue };
                                    let Some(hash2) = HashIcon::from_racetime_emoji(hash2) else { continue };
                                    let Some(hash3) = HashIcon::from_racetime_emoji(hash3) else { continue };
                                    let Some(hash4) = HashIcon::from_racetime_emoji(hash4) else { continue };
                                    let Some(hash5) = HashIcon::from_racetime_emoji(hash5) else { continue };
                                    file_hash = Some([hash1, hash2, hash3, hash4, hash5]);
                                    file_stem = Some(info_file_stem.to_owned());
                                    break
                                } else if let Some((_, hash1, hash2, hash3, hash4, hash5, web_id_str)) = regex_captures!("^(?:.+\n)?([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+)\nhttps://ootrandomizer\\.com/seed/get\\?id=([0-9]+)$", &info_bot) {
                                    let Some(hash1) = HashIcon::from_racetime_emoji(hash1) else { continue };
                                    let Some(hash2) = HashIcon::from_racetime_emoji(hash2) else { continue };
                                    let Some(hash3) = HashIcon::from_racetime_emoji(hash3) else { continue };
                                    let Some(hash4) = HashIcon::from_racetime_emoji(hash4) else { continue };
                                    let Some(hash5) = HashIcon::from_racetime_emoji(hash5) else { continue };
                                    file_hash = Some([hash1, hash2, hash3, hash4, hash5]);
                                    web_id = Some(web_id_str.parse().expect("found race room linking to out-of-range web seed ID"));
                                    match http_client.get("https://ootrandomizer.com/patch/get").query(&[("id", web_id)]).send().await {
                                        Ok(patch_response) => match patch_response.detailed_error_for_status().await {
                                            Ok(patch_response) => if let Some(content_disposition) = patch_response.headers().get(reqwest::header::CONTENT_DISPOSITION) {
                                                match content_disposition.to_str() {
                                                    Ok(content_disposition) => if let Some((_, patch_file_name)) = regex_captures!("^attachment; filename=(.+)$", content_disposition) {
                                                        let patch_file_name = patch_file_name.to_owned();
                                                        if let Some((_, patch_file_stem)) = regex_captures!(r"^(.+)\.zpfz?$", &patch_file_name) {
                                                            file_stem = Some(patch_file_stem.to_owned());
                                                            match File::create(Path::new(seed::DIR).join(&patch_file_name)).await {
                                                                Ok(mut file) => if let Err(e) = io::copy_buf(&mut StreamReader::new(patch_response.bytes_stream().map_err(io_error_from_reqwest)), &mut file).await {
                                                                    form.context.push_error(form::Error::validation(format!("Error saving patch file from room data: {e}")).with_name(field_name))
                                                                },
                                                                Err(e) => form.context.push_error(form::Error::validation(format!("Error saving patch file from room data: {e}")).with_name(field_name)),
                                                            }
                                                        } else {
                                                            form.context.push_error(form::Error::validation("Couldn't parse patch file name from room data").with_name(field_name));
                                                        }
                                                    } else {
                                                        form.context.push_error(form::Error::validation("Couldn't parse patch file name from room data").with_name(field_name));
                                                    },
                                                    Err(e) => form.context.push_error(form::Error::validation(format!("Couldn't parse patch file name from room data: {e}")).with_name(field_name)),
                                                }
                                            } else {
                                                form.context.push_error(form::Error::validation("Couldn't parse patch file name from room data").with_name(field_name));
                                            }
                                            Err(wheel::Error::ResponseStatus { inner, text, .. }) if inner.status() == Some(StatusCode::NOT_FOUND) && text.as_ref().is_ok_and(|text| text == "The indicated id is either invalid or has expired") => continue,
                                            Err(e) => form.context.push_error(form::Error::validation(format!("Error getting patch file from room data: {e}")).with_name(field_name)),
                                        },
                                        Err(e) => form.context.push_error(form::Error::validation(format!("Error getting patch file from room data: {e}")).with_name(field_name)),
                                    }
                                    if let Some(ref file_stem) = file_stem {
                                        match http_client.get("https://ootrandomizer.com/spoilers/get").query(&[("id", web_id)]).send().await {
                                            Ok(spoiler_response) => if spoiler_response.status() != StatusCode::BAD_REQUEST { // returns error 400 if no spoiler log has been generated
                                                match spoiler_response.detailed_error_for_status().await {
                                                    Ok(spoiler_response) => {
                                                        let spoiler_filename = format!("{file_stem}_Spoiler.json");
                                                        let spoiler_path = Path::new(seed::DIR).join(&spoiler_filename);
                                                        match File::create(&spoiler_path).await {
                                                            Ok(mut file) => match io::copy_buf(&mut StreamReader::new(spoiler_response.bytes_stream().map_err(io_error_from_reqwest)), &mut file).await {
                                                                Ok(_) => if file_hash.is_none() {
                                                                    match fs::read(spoiler_path).await {
                                                                        Ok(buf) => match serde_json::from_slice::<SpoilerLog>(&buf) {
                                                                            Ok(spoiler_log) => file_hash = Some(spoiler_log.file_hash),
                                                                            Err(e) => form.context.push_error(form::Error::validation(format!("Error reading spoiler log from room data: {e}")).with_name(field_name)),
                                                                        },
                                                                        Err(e) => form.context.push_error(form::Error::validation(format!("Error reading spoiler log from room data: {e}")).with_name(field_name)),
                                                                    }
                                                                },
                                                                Err(e) => form.context.push_error(form::Error::validation(format!("Error saving spoiler log from room data: {e}")).with_name(field_name)),
                                                            },
                                                            Err(e) => form.context.push_error(form::Error::validation(format!("Error saving spoiler log from room data: {e}")).with_name(field_name)),
                                                        }
                                                    }
                                                    Err(e) => form.context.push_error(form::Error::validation(format!("Error getting spoiler log from room data: {e}")).with_name(field_name)),
                                                }
                                            },
                                            Err(e) => form.context.push_error(form::Error::validation(format!("Error getting spoiler log from room data: {e}")).with_name(field_name)),
                                        }
                                    }
                                    break
                                }
                            },
                            Err(e) => form.context.push_error(form::Error::validation(format!("Error getting room data: {e}")).with_name(field_name)),
                        },
                        Err(e) => form.context.push_error(form::Error::validation(format!("Error getting room data: {e}")).with_name(field_name)),
                    },
                    Err(e) => form.context.push_error(form::Error::validation(format!("Error getting room data: {e}")).with_name(field_name)),
                }
            }
        }
        let mut restreamers = HashMap::new();
        for language in all() {
            if let Some(video_url) = value.video_urls.get(&language) {
                if !video_url.is_empty() {
                    if let Err(e) = Url::parse(video_url) {
                        form.context.push_error(form::Error::validation(format!("Failed to parse URL: {e}")).with_name(format!("video_urls.{}", language.short_code())));
                    }
                    if let Some(restreamer) = value.restreamers.get(&language) {
                        if !restreamer.is_empty() {
                            if restreamer == "me" {
                                if let Some(ref racetime) = me.racetime {
                                    restreamers.insert(language, racetime.id.clone());
                                } else {
                                    form.context.push_error(form::Error::validation("A racetime.gg account is required to restream races. Go to your profile and select “Connect a racetime.gg account”.").with_name(format!("restreamers.{}", language.short_code()))); //TODO direct link
                                }
                            } else {
                                match racetime_bot::parse_user(&mut transaction, http_client, env.racetime_host(), restreamer).await {
                                    Ok(racetime_id) => { restreamers.insert(language, racetime_id); }
                                    Err(e @ (racetime_bot::ParseUserError::Format | racetime_bot::ParseUserError::IdNotFound | racetime_bot::ParseUserError::InvalidUrl | racetime_bot::ParseUserError::MidosHouseId | racetime_bot::ParseUserError::MidosHouseUserNoRacetime | racetime_bot::ParseUserError::UrlNotFound)) => {
                                        form.context.push_error(form::Error::validation(e.to_string()).with_name(format!("restreamers.{}", language.short_code())));
                                    }
                                    Err(racetime_bot::ParseUserError::Reqwest(e)) => return Err(e.into()),
                                    Err(racetime_bot::ParseUserError::Sql(e)) => return Err(e.into()),
                                    Err(racetime_bot::ParseUserError::Wheel(e)) => return Err(e.into()),
                                }
                            }
                        }
                    }
                } else {
                    if value.restreamers.get(&language).map_or(false, |restreamer| !restreamer.is_empty()) {
                        form.context.push_error(form::Error::validation("Please either add a restream URL or remove the restreamer.").with_name(format!("restreamers.{}", language.short_code())));
                    }
                }
            }
        }
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(edit_race_form(transaction, &*discord_ctx.read().await, **env, Some(me), uri, csrf.as_ref(), event, race, Some(form.context)).await?)
        } else {
            sqlx::query!(
                "UPDATE races SET
                    room = $1,
                    async_room1 = $2,
                    async_room2 = $3,
                    async_room3 = $4,
                    video_url = $5,
                    restreamer = $6,
                    video_url_fr = $7,
                    restreamer_fr = $8,
                    video_url_de = $9,
                    restreamer_de = $10,
                    video_url_pt = $11,
                    restreamer_pt = $12,
                    last_edited_by = $13,
                    last_edited_at = NOW()
                WHERE id = $14",
                (!value.room.is_empty()).then(|| &value.room),
                (!value.async_room1.is_empty()).then(|| &value.async_room1),
                (!value.async_room2.is_empty()).then(|| &value.async_room2),
                (!value.async_room3.is_empty()).then(|| &value.async_room3),
                value.video_urls.get(&English).filter(|video_url| !video_url.is_empty()),
                restreamers.get(&English),
                value.video_urls.get(&French).filter(|video_url| !video_url.is_empty()),
                restreamers.get(&French),
                value.video_urls.get(&German).filter(|video_url| !video_url.is_empty()),
                restreamers.get(&German),
                value.video_urls.get(&Portuguese).filter(|video_url| !video_url.is_empty()),
                restreamers.get(&Portuguese),
                me.id as _,
                id as _,
            ).execute(&mut *transaction).await?;
            if let Some([hash1, hash2, hash3, hash4, hash5]) = file_hash {
                sqlx::query!(
                    "UPDATE races SET hash1 = $1, hash2 = $2, hash3 = $3, hash4 = $4, hash5 = $5 WHERE id = $6",
                    hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _, id as _,
                ).execute(&mut *transaction).await?;
            }
            if let Some(web_id) = web_id {
                sqlx::query!("UPDATE races SET web_id = $1 WHERE id = $2", web_id as i64, id as _).execute(&mut *transaction).await?;
            }
            if let Some(web_gen_time) = web_gen_time {
                sqlx::query!("UPDATE races SET web_gen_time = $1 WHERE id = $2", web_gen_time, id as _).execute(&mut *transaction).await?;
            }
            if let Some(file_stem) = file_stem {
                sqlx::query!("UPDATE races SET file_stem = $1 WHERE id = $2", file_stem, id as _).execute(&mut *transaction).await?;
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        RedirectOrContent::Content(edit_race_form(transaction, &*discord_ctx.read().await, **env, Some(me), uri, csrf.as_ref(), event, race, Some(form.context)).await?)
    })
}

pub(crate) async fn add_file_hash_form(mut transaction: Transaction<'_, Postgres>, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, race: Race, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, env, me.as_ref(), Tab::Races, true).await?;
    let form = if me.is_some() {
        let mut errors = ctx.errors().collect();
        full_form(uri!(add_file_hash_post(event.series, &*event.event, race.id)), csrf, html! {
            //TODO preview selected icons using CSS/JS?
            : form_field("hash1", &mut errors, html! {
                label(for = "hash1") : "Hash Icon 1:";
                select(name = "hash1") {
                    @for icon in all::<HashIcon>() {
                        option(value = icon.to_string(), selected? = ctx.field_value("hash1") == Some(&icon.to_string())) : icon.to_string();
                    }
                }
            });
            : form_field("hash2", &mut errors, html! {
                label(for = "hash2") : "Hash Icon 2:";
                select(name = "hash2") {
                    @for icon in all::<HashIcon>() {
                        option(value = icon.to_string(), selected? = ctx.field_value("hash2") == Some(&icon.to_string())) : icon.to_string();
                    }
                }
            });
            : form_field("hash3", &mut errors, html! {
                label(for = "hash3") : "Hash Icon 3:";
                select(name = "hash3") {
                    @for icon in all::<HashIcon>() {
                        option(value = icon.to_string(), selected? = ctx.field_value("hash3") == Some(&icon.to_string())) : icon.to_string();
                    }
                }
            });
            : form_field("hash4", &mut errors, html! {
                label(for = "hash4") : "Hash Icon 4:";
                select(name = "hash4") {
                    @for icon in all::<HashIcon>() {
                        option(value = icon.to_string(), selected? = ctx.field_value("hash4") == Some(&icon.to_string())) : icon.to_string();
                    }
                }
            });
            : form_field("hash5", &mut errors, html! {
                label(for = "hash5") : "Hash Icon 5:";
                select(name = "hash5") {
                    @for icon in all::<HashIcon>() {
                        option(value = icon.to_string(), selected? = ctx.field_value("hash5") == Some(&icon.to_string())) : icon.to_string();
                    }
                }
            });
        }, errors, "Save")
    } else {
        html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(edit_race(event.series, &*event.event, race.id))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to edit this race.";
                }
            }
        }
    };
    let content = html! {
        : header;
        h2 : "Add file hash";
        @match race.schedule {
            RaceSchedule::Unscheduled => p : "Not yet scheduled";
            RaceSchedule::Live { room, .. } => @if let Some(room) = room {
                p {
                    a(href = room.to_string()) : "Race room";
                }
            } else {
                p : "Race room not yet assigned";
            }
            RaceSchedule::Async { room1, room2, room3, .. } => {
                @if let Some(room1) = room1 {
                    p {
                        a(href = room1.to_string()) : "Race room 1";
                    }
                } else {
                    p : "Race room 1 not yet assigned";
                }
                @if let Some(room2) = room2 {
                    p {
                        a(href = room2.to_string()) : "Race room 2";
                    }
                } else {
                    p : "Race room 2 not yet assigned";
                }
                @if let Entrants::Three(_) = race.entrants {
                    @if let Some(room3) = room3 {
                        p {
                            a(href = room3.to_string()) : "Race room 3";
                        }
                    } else {
                        p : "Race room 3 not yet assigned";
                    }
                }
            }
        }
        : form;
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await?, ..PageStyle::default() }, &format!("Edit Race — {}", event.display_name), content).await?)
}

#[rocket::get("/event/<series>/<event>/races/<id>/edit-hash")]
pub(crate) async fn add_file_hash(env: &State<Environment>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id<Races>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, id).await?;
    if race.series != event.series || race.event != event.event {
        return Ok(RedirectOrContent::Redirect(Redirect::permanent(uri!(add_file_hash(race.series, race.event, id)))))
    }
    Ok(RedirectOrContent::Content(add_file_hash_form(transaction, **env, me, uri, csrf.as_ref(), event, race, Context::default()).await?))
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct AddFileHashForm {
    #[field(default = String::new())]
    csrf: String,
    hash1: String,
    hash2: String,
    hash3: String,
    hash4: String,
    hash5: String,
}

#[rocket::post("/event/<series>/<event>/races/<id>/edit-hash", data = "<form>")]
pub(crate) async fn add_file_hash_post(env: &State<Environment>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id<Races>, form: Form<Contextual<'_, AddFileHashForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, id).await?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if race.series != event.series || race.event != event.event {
        form.context.push_error(form::Error::validation("This race is not part of this event."));
    }
    if !me.is_archivist && !event.organizers(&mut transaction).await?.contains(&me) {
        form.context.push_error(form::Error::validation("You must be an archivist to edit this race. If you would like to become an archivist, please contact Fenhl on Discord."));
    }
    Ok(if let Some(ref value) = form.value {
        let hash1 = if let Ok(hash1) = value.hash1.parse::<HashIcon>() {
            Some(hash1)
        } else {
            form.context.push_error(form::Error::validation("No such hash icon.").with_name("hash1"));
            None
        };
        let hash2 = if let Ok(hash2) = value.hash2.parse::<HashIcon>() {
            Some(hash2)
        } else {
            form.context.push_error(form::Error::validation("No such hash icon.").with_name("hash2"));
            None
        };
        let hash3 = if let Ok(hash3) = value.hash3.parse::<HashIcon>() {
            Some(hash3)
        } else {
            form.context.push_error(form::Error::validation("No such hash icon.").with_name("hash3"));
            None
        };
        let hash4 = if let Ok(hash4) = value.hash4.parse::<HashIcon>() {
            Some(hash4)
        } else {
            form.context.push_error(form::Error::validation("No such hash icon.").with_name("hash4"));
            None
        };
        let hash5 = if let Ok(hash5) = value.hash5.parse::<HashIcon>() {
            Some(hash5)
        } else {
            form.context.push_error(form::Error::validation("No such hash icon.").with_name("hash5"));
            None
        };
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(add_file_hash_form(transaction, **env, Some(me), uri, csrf.as_ref(), event, race, form.context).await?)
        } else {
            sqlx::query!(
                "UPDATE races SET hash1 = $1, hash2 = $2, hash3 = $3, hash4 = $4, hash5 = $5 WHERE id = $6",
                hash1.unwrap() as _, hash2.unwrap() as _, hash3.unwrap() as _, hash4.unwrap() as _, hash5.unwrap() as _, id as _,
            ).execute(&mut *transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        RedirectOrContent::Content(add_file_hash_form(transaction, **env, Some(me), uri, csrf.as_ref(), event, race, form.context).await?)
    })
}
