use {
    std::{
        borrow::Cow,
        cmp::Ordering,
        collections::HashMap,
        convert::identity,
        fmt,
        iter,
        path::Path,
    },
    chrono::{
        Duration,
        prelude::*,
    },
    chrono_tz::America,
    futures::stream::TryStreamExt as _,
    ics::{
        ICalendar,
        properties::{
            DtEnd,
            DtStart,
            Summary,
            URL,
        },
    },
    itertools::Itertools as _,
    lazy_regex::regex_captures,
    ootr_utils::spoiler::{
        HashIcon,
        SpoilerLog,
    },
    racetime::model::RaceData,
    reqwest::StatusCode,
    rocket::{
        FromForm,
        State,
        form::{
            self,
            Context,
            Contextual,
            Form,
        },
        http::Status,
        response::{
            Redirect,
            content::RawHtml,
        },
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        ContextualExt as _,
        CsrfForm,
        Origin,
        Response,
        ToHtml as _,
        html,
    },
    serenity::{
        all::{
            CreateForumPost,
            CreateMessage,
            Context as DiscordCtx,
            MessageBuilder,
        },
        model::prelude::*,
    },
    serenity_utils::RwFuture,
    sheets::Sheets,
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
        types::Json,
    },
    tokio::io,
    tokio_util::io::StreamReader,
    url::Url,
    wheel::{
        fs::{
            self,
            File,
        },
        traits::ReqwestResponseExt as _,
    },
    yup_oauth2::{
        ServiceAccountAuthenticator,
        read_service_account_key,
    },
    crate::{
        Environment,
        auth,
        config::Config,
        discord_bot::{
            self,
            Draft,
            DraftKind,
        },
        event::{
            self,
            MatchSource,
            Series,
            Tab,
            TeamConfig,
        },
        http::{
            PageError,
            PageStyle,
            page,
        },
        seed,
        startgg,
        team::Team,
        user::User,
        util::{
            DateTimeFormat,
            Id,
            IdTable,
            MessageBuilderExt as _,
            RedirectOrContent,
            StatusOrError,
            as_variant,
            form_field,
            format_datetime,
            io_error_from_reqwest,
            utc,
        },
    },
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Entrant {
    MidosHouseTeam(Team),
    Named(String),
}

impl Entrant {
    pub(crate) fn to_html(&self, running_text: bool) -> RawHtml<String> {
        match self {
            Self::MidosHouseTeam(team) => team.to_html(running_text),
            Self::Named(name) => name.to_html(),
        }
    }
}

impl fmt::Display for Entrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MidosHouseTeam(team) => team.fmt(f),
            Self::Named(name) => name.fmt(f),
        }
    }
}

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
        end1: Option<DateTime<Utc>>,
        end2: Option<DateTime<Utc>>,
        room1: Option<Url>,
        room2: Option<Url>,
    },
}

impl RaceSchedule {
    fn new(
        live_start: Option<DateTime<Utc>>, async_start1: Option<DateTime<Utc>>, async_start2: Option<DateTime<Utc>>,
        live_end: Option<DateTime<Utc>>, async_end1: Option<DateTime<Utc>>, async_end2: Option<DateTime<Utc>>,
        live_room: Option<Url>, async_room1: Option<Url>, async_room2: Option<Url>,
    ) -> Self {
        match (live_start, async_start1, async_start2) {
            (None, None, None) => Self::Unscheduled,
            (Some(start), None, None) => Self::Live {
                end: live_end,
                room: live_room,
                start,
            },
            (None, start1, start2) => Self::Async {
                end1: async_end1,
                end2: async_end2,
                room1: async_room1,
                room2: async_room2,
                start1, start2,
            },
            (Some(_), _, _) => unreachable!("both live and async starts included, should be prevented by SQL constraint"),
        }
    }

    pub(crate) fn is_ended(&self) -> bool {
        // Since the end time of a race isn't known in advance, we assume that if a race has an end time, that end time is in the past.
        match *self {
            Self::Unscheduled => false,
            Self::Live { end, .. } => end.is_some(),
            Self::Async { end1, end2, .. } => end1.is_some() && end2.is_some(),
        }
    }

    fn start_matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unscheduled, Self::Unscheduled) => true,
            (Self::Live { start: start_a, .. }, Self::Live { start: start_b, .. }) => start_a == start_b,
            (Self::Async { start1: start_a1, start2: start_a2, .. }, Self::Async { start1: start_b1, start2: start_b2, .. }) => start_a1 == start_b1 && start_a2 == start_b2,
            (Self::Unscheduled, _) | (Self::Live { .. }, _) | (Self::Async { .. }, _) => false, // ensure compile error on missing variants by listing each left-hand side individually
        }
    }
}

impl PartialEq for RaceSchedule {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RaceSchedule {}

impl PartialOrd for RaceSchedule {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RaceSchedule {
    fn cmp(&self, other: &Self) -> Ordering {
        let (start_a1, start_a2, end_a) = match *self {
            Self::Unscheduled => (None, None, None),
            Self::Live { start, end, .. } => (Some(start), Some(start), end),
            Self::Async { start1, start2, end1, end2, .. } => (start1, start2, end1.and_then(|end1| Some(end1.max(end2?)))),
        };
        let (start_b1, start_b2, end_b) = match *other {
            Self::Unscheduled => (None, None, None),
            Self::Live { start, end, .. } => (Some(start), Some(start), end),
            Self::Async { start1, start2, end1, end2, .. } => (start1, start2, end1.and_then(|end1| Some(end1.max(end2?)))),
        };
        end_a.is_none().cmp(&end_b.is_none()) // races that have ended first
            .then_with(|| end_a.cmp(&end_b)) // races that ended earlier first
            .then_with(|| (start_a1.is_none() && start_a2.is_none()).cmp(&(start_b1.is_none() && start_b2.is_none()))) // races that have at least 1 starting time first
            .then_with(||
                start_a1.map_or(start_a2, |start_a1| start_a2.map_or(Some(start_a1), |start_a2| Some(start_a1.min(start_a2))))
                .cmp(&start_b1.map_or(start_b2, |start_b1| start_b2.map_or(Some(start_b1), |start_b2| Some(start_b1.min(start_b2)))))
            ) // races whose first half started earlier first
            .then_with(|| (start_a1.is_none() || start_a2.is_none()).cmp(&(start_b1.is_none() || start_b2.is_none()))) // races that have both starting times first
            .then_with(||
                start_a1.map_or(start_a2, |start_a1| start_a2.map_or(Some(start_a1), |start_a2| Some(start_a1.max(start_a2))))
                .cmp(&start_b1.map_or(start_b2, |start_b1| start_b2.map_or(Some(start_b1), |start_b2| Some(start_b1.max(start_b2)))))
            ) // races whose second half started earlier first
    }
}

#[derive(Clone)]
pub(crate) struct Race {
    pub(crate) id: Option<Id>, //TODO make required?
    pub(crate) series: Series,
    pub(crate) event: String,
    pub(crate) startgg_event: Option<String>,
    pub(crate) startgg_set: Option<String>,
    pub(crate) entrants: Entrants,
    pub(crate) phase: Option<String>,
    pub(crate) round: Option<String>,
    pub(crate) game: Option<i16>,
    scheduling_thread: Option<ChannelId>,
    pub(crate) schedule: RaceSchedule,
    pub(crate) draft: Option<Draft>,
    pub(crate) seed: Option<seed::Data>,
    pub(crate) video_url: Option<Url>,
    ignored: bool,
}

impl Race {
    pub(crate) async fn from_id(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, id: Id) -> Result<Self, Error> {
        let row = sqlx::query!(r#"SELECT
            series AS "series: Series",
            event,
            startgg_event,
            startgg_set,
            game,
            team1 AS "team1: Id",
            team2 AS "team2: Id",
            p1,
            p2,
            p3,
            phase,
            round,
            scheduling_thread AS "scheduling_thread: Id",
            draft_state AS "draft_state: Json<Draft>",
            start,
            async_start1,
            async_start2,
            end_time,
            async_end1,
            async_end2,
            room,
            async_room1,
            async_room2,
            web_id AS "web_id: Id",
            web_gen_time,
            file_stem,
            hash1 AS "hash1: HashIcon",
            hash2 AS "hash2: HashIcon",
            hash3 AS "hash3: HashIcon",
            hash4 AS "hash4: HashIcon",
            hash5 AS "hash5: HashIcon",
            video_url,
            ignored
        FROM races WHERE id = $1"#, i64::from(id)).fetch_one(&mut *transaction).await?;
        let (startgg_event, startgg_set, phase, round, slots) = if let Some(startgg_set) = row.startgg_set {
            if row.startgg_event.is_some() && row.phase.is_some() && row.round.is_some() && row.team1.is_some() && row.team2.is_some() {
                (row.startgg_event, Some(startgg_set), row.phase, row.round, None)
            } else if let startgg::set_query::ResponseData {
                set: Some(startgg::set_query::SetQuerySet {
                    full_round_text: Some(round),
                    phase_group: Some(startgg::set_query::SetQuerySetPhaseGroup {
                        phase: Some(startgg::set_query::SetQuerySetPhaseGroupPhase {
                            event: Some(startgg::set_query::SetQuerySetPhaseGroupPhaseEvent {
                                slug: Some(startgg_event),
                            }),
                            name: Some(phase),
                        }),
                    }),
                    slots: Some(slots),
                }),
            } = startgg::query::<startgg::SetQuery>(http_client, startgg_token, startgg::set_query::Variables { set_id: startgg::ID(startgg_set.clone()) }).await? {
                sqlx::query!("UPDATE races SET
                    startgg_event = $1,
                    phase = $2,
                    round = $3
                WHERE id = $4", startgg_event, phase, round, i64::from(id)).execute(&mut *transaction).await?;
                (Some(startgg_event), Some(startgg_set), Some(phase), Some(round), Some(slots))
            } else {
                (None, None, row.phase, row.round, None)
            }
        } else {
            (None, None, row.phase, row.round, None)
        };
        let entrants = if let [Some(team1), Some(team2)] = [row.team1, row.team2] {
            Entrants::Two([
                Entrant::MidosHouseTeam(Team::from_id(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?),
                Entrant::MidosHouseTeam(Team::from_id(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?),
            ])
        } else {
            match [row.p1, row.p2, row.p3] {
                [Some(p1), Some(p2), Some(p3)] => Entrants::Three([
                    Entrant::Named(p1),
                    Entrant::Named(p2),
                    Entrant::Named(p3),
                ]),
                [Some(p1), Some(p2), None] => Entrants::Two([
                    Entrant::Named(p1),
                    Entrant::Named(p2),
                ]),
                [Some(p1), None, None] => Entrants::Named(p1),
                _ => if let (Some(startgg_set), Some(slots)) = (&startgg_set, slots) {
                    if let [
                        Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team1)), on: _ }) }) }),
                        Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team2)), on: _ }) }) }),
                    ] = *slots {
                        let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?;
                        let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?;
                        sqlx::query!("UPDATE races SET team1 = $1 WHERE id = $2", team1.id as _, i64::from(id)).execute(&mut *transaction).await?;
                        sqlx::query!("UPDATE races SET team2 = $1 WHERE id = $2", team2.id as _, i64::from(id)).execute(&mut *transaction).await?;
                        Entrants::Two([Entrant::MidosHouseTeam(team1), Entrant::MidosHouseTeam(team2)])
                    } else {
                        return Err(Error::StartggTeams { startgg_set: startgg_set.clone() })
                    }
                } else {
                    return Err(Error::MissingTeams)
                },
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
                        sqlx::query!($query, end, i64::from(id)).execute(&mut *transaction).await?;
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
        Ok(Self {
            id: Some(id),
            series: row.series,
            event: row.event,
            game: row.game,
            scheduling_thread: row.scheduling_thread.map(|Id(id)| id.into()),
            schedule: RaceSchedule::new(
                row.start, row.async_start1, row.async_start2,
                end_time, async_end1, async_end2,
                row.room.map(|room| room.parse()).transpose()?, row.async_room1.map(|room| room.parse()).transpose()?, row.async_room2.map(|room| room.parse()).transpose()?,
            ),
            draft: row.draft_state.map(|Json(draft)| draft),
            seed: row.file_stem.map(|file_stem| seed::Data {
                web: match (row.web_id, row.web_gen_time) {
                    (Some(Id(id)), Some(gen_time)) => Some(seed::OotrWebData { id, gen_time }),
                    (Some(Id(id)), None) => match (row.start, row.async_start1, row.async_start2) {
                        (Some(start), None, None) | (None, Some(start), None) | (None, None, Some(start)) => Some(seed::OotrWebData { id, gen_time: start - Duration::days(1) }),
                        (None, Some(async_start1), Some(async_start2)) => Some(seed::OotrWebData { id, gen_time: async_start1.min(async_start2) - Duration::days(1) }),
                        (_, _, _) => None,
                    },
                    (None, _) => None,
                },
                file_hash: match (row.hash1, row.hash2, row.hash3, row.hash4, row.hash5) {
                    (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                    (None, None, None, None, None) => None,
                    _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
                },
                file_stem: Cow::Owned(file_stem),
            }),
            video_url: row.video_url.map(|url| url.parse()).transpose()?,
            ignored: row.ignored,
            startgg_event, startgg_set, entrants, phase, round,
        })
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: &Environment, config: &Config, event: &event::Data<'_>) -> Result<Vec<Self>, Error> {
        async fn add_or_update_race(transaction: &mut Transaction<'_, Postgres>, races: &mut Vec<Race>, require_matching_start_time: bool, mut race: Race) -> sqlx::Result<()> {
            if let Some(found_race) = races.iter_mut().find(|iter_race|
                iter_race.series == race.series
                && iter_race.event == race.event
                && iter_race.phase == race.phase
                && iter_race.round == race.round
                && iter_race.game == race.game
                && iter_race.entrants == race.entrants
                && (!require_matching_start_time || iter_race.schedule.start_matches(&race.schedule))
            ) {
                if let Some(id) = found_race.id {
                    if !found_race.schedule.start_matches(&race.schedule) {
                        match race.schedule {
                            RaceSchedule::Unscheduled => {
                                found_race.schedule = RaceSchedule::Unscheduled;
                                sqlx::query!("UPDATE races SET start = NULL, async_start1 = NULL, async_start2 = NULL WHERE id = $1", i64::from(id)).execute(transaction).await?;
                            }
                            RaceSchedule::Live { start, .. } => {
                                match found_race.schedule {
                                    RaceSchedule::Unscheduled => found_race.schedule = race.schedule,
                                    RaceSchedule::Live { start: ref mut old_start, .. } => *old_start = start,
                                    RaceSchedule::Async { .. } => unimplemented!("race listed as async in database was rescheduled as live"), //TODO
                                }
                                sqlx::query!("UPDATE races SET start = $1, async_start1 = NULL, async_start2 = NULL WHERE id = $2", start, i64::from(id)).execute(transaction).await?;
                            },
                            RaceSchedule::Async { start1, start2, .. } => {
                                match found_race.schedule {
                                    RaceSchedule::Unscheduled => found_race.schedule = race.schedule,
                                    RaceSchedule::Live { .. } => unimplemented!("race listed as live in database was rescheduled as async"), //TODO
                                    RaceSchedule::Async { start1: ref mut old_start1, start2: ref mut old_start2, .. } => {
                                        *old_start1 = start1;
                                        *old_start2 = start2;
                                    }
                                }
                                sqlx::query!("UPDATE races SET start = NULL, async_start1 = $1, async_start2 = $2 WHERE id = $3", start1, start2, i64::from(id)).execute(transaction).await?;
                            }
                        }
                    }
                }
            } else {
                // add race to database to give it an ID
                race.save(transaction).await?;
                races.push(race);
            }
            Ok(())
        }

        let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
        let mut races = Vec::default();
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE series = $1 AND event = $2"#, event.series as _, &event.event).fetch_all(&mut *transaction).await? {
            races.push(Self::from_id(&mut *transaction, http_client, startgg_token, id).await?);
        }
        match event.series {
            Series::Multiworld => {} // added to database
            Series::NineDaysOfSaws | Series::Pictionary => {
                races.push(Self {
                    id: None,
                    series: event.series,
                    event: event.event.to_string(),
                    startgg_event: None,
                    startgg_set: None,
                    entrants: Entrants::Open,
                    phase: None,
                    round: None,
                    game: None,
                    scheduling_thread: None,
                    schedule: if let Some(start) = event.start(transaction).await? {
                        RaceSchedule::Live {
                            end: event.end,
                            room: event.url.clone(),
                            start,
                        }
                    } else {
                        RaceSchedule::Unscheduled
                    },
                    draft: None,
                    seed: None, //TODO
                    video_url: event.video_url.clone(),
                    ignored: false,
                });
            }
            Series::Rsl => match &*event.event {
                "1" => {} // no match data available
                "2" | "3" | "4" | "5" => {} // added to database
                _ => unimplemented!(),
            },
            Series::Standard => match &*event.event {
                "6" => {
                    // qualifiers
                    for (i, (start, end, weekly, room, total, finished, vod, seed)) in [
                        // source: https://docs.google.com/document/d/1fyNO82G2D0Z7J9wobxEbjDjGnomTaIRdKgETGV_ufmc/edit
                        (utc!(2022, 11, 19, 23, 0, 0), utc!(2022, 11, 20, 3, 39, 27, 694), Some("NA"), "https://racetime.gg/ootr/neutral-bongobongo-4042", 127, 105, "https://www.youtube.com/watch?v=uuMVHppi8Uk", None),
                        (utc!(2022, 11, 20, 14, 0, 0), utc!(2022, 11, 20, 17, 55, 28, 500), Some("EU"), "https://racetime.gg/ootr/trusty-volvagia-2022", 109, 89, "https://www.youtube.com/watch?v=7eQB379leVo", None),
                        (utc!(2022, 11, 23, 3, 0, 0), utc!(2022, 11, 23, 9, 53, 31, 278), None, "https://racetime.gg/ootr/chaotic-wolfos-5287", 82, 74, "https://www.youtube.com/watch?v=tQQyq6x76yw", Some(seed::Data { web: Some(seed::OotrWebData { id: 1262335, gen_time: utc!(2022, 11, 23, 2, 46, 48) }), file_hash: Some([HashIcon::BottledFish, HashIcon::MirrorShield, HashIcon::Mushroom, HashIcon::BigMagic, HashIcon::Compass]), file_stem: Cow::Borrowed("OoTR_1262335_BZDNTLI1IE") })),
                        (utc!(2022, 11, 26, 23, 0, 0), utc!(2022, 11, 27, 4, 54, 15, 67), Some("NA"), "https://racetime.gg/ootr/smart-darunia-4679", 113, 96, "https://www.youtube.com/watch?v=jaVoVsw7PqE", Some(seed::Data { web: Some(seed::OotrWebData { id: 1265302, gen_time: utc!(2022, 11, 26, 22, 45, 47) }), file_hash: Some([HashIcon::MegatonHammer, HashIcon::Saw, HashIcon::Longshot, HashIcon::MirrorShield, HashIcon::BottledMilk]), file_stem: Cow::Borrowed("OoTR_1265302_4RNJWBU8GE") })),
                        (utc!(2022, 11, 27, 14, 0, 0), utc!(2022, 11, 27, 18, 41, 28, 265), Some("EU"), "https://racetime.gg/ootr/comic-sheik-2973", 98, 93, "https://www.youtube.com/watch?v=D6SWuCmuYRM", Some(seed::Data { web: Some(seed::OotrWebData { id: 1265745, gen_time: utc!(2022, 11, 27, 13, 45, 36) }), file_hash: Some([HashIcon::MaskOfTruth, HashIcon::Bombchu, HashIcon::SoldOut, HashIcon::DekuStick, HashIcon::SoldOut]), file_stem: Cow::Borrowed("OoTR_1265745_IY1CK1DMJX") })),
                        (utc!(2022, 11, 29, 19, 0, 0), utc!(2022, 11, 29, 23, 20, 27, 920), None, "https://racetime.gg/ootr/eager-jabu-1097", 92, 83, "https://twitch.tv/videos/1666092237", Some(seed::Data { web: Some(seed::OotrWebData { id: 1267442, gen_time: utc!(2022, 11, 29, 18, 46, 39) }), file_hash: Some([HashIcon::MaskOfTruth, HashIcon::Bombchu, HashIcon::Bombchu, HashIcon::Compass, HashIcon::HeartContainer]), file_stem: Cow::Borrowed("OoTR_1267442_KHLKWX7GGO") })),
                        (utc!(2022, 12, 2, 1, 0, 0), utc!(2022, 12, 2, 4, 51, 0, 313), None, "https://racetime.gg/ootr/dazzling-bigocto-7483", 91, 84, "https://www.youtube.com/watch?v=k7_2gAHZOfk", Some(seed::Data { web: Some(seed::OotrWebData { id: 1269079, gen_time: utc!(2022, 12, 2, 0, 45, 11) }), file_hash: Some([HashIcon::BottledFish, HashIcon::Mushroom, HashIcon::Bombchu, HashIcon::SilverGauntlets, HashIcon::FairyOcarina]), file_stem: Cow::Borrowed("OoTR_1269079_UA0S3MDBWJ") })),
                        (utc!(2022, 12, 3, 23, 0, 0), utc!(2022, 12, 4, 4, 55, 29, 700), Some("NA"), "https://racetime.gg/ootr/secret-dampe-4738", 131, 104, "https://www.youtube.com/watch?v=mWNNMG9UIa4", Some(seed::Data { web: Some(seed::OotrWebData { id: 1270728, gen_time: utc!(2022, 12, 3, 22, 46, 18) }), file_hash: Some([HashIcon::Compass, HashIcon::Bow, HashIcon::SkullToken, HashIcon::SkullToken, HashIcon::MaskOfTruth]), file_stem: Cow::Borrowed("OoTR_1270728_N5RSVBP64P") })),
                        (utc!(2022, 12, 4, 14, 0, 0), utc!(2022, 12, 4, 19, 36, 40, 94), Some("EU"), "https://racetime.gg/ootr/clumsy-mido-8938", 105, 99, "https://www.youtube.com/watch?v=yuvVxgYawCk", Some(seed::Data { web: Some(seed::OotrWebData { id: 1271256, gen_time: utc!(2022, 12, 4, 13, 45, 22) }), file_hash: Some([HashIcon::Bombchu, HashIcon::Map, HashIcon::Saw, HashIcon::SoldOut, HashIcon::BottledMilk]), file_stem: Cow::Borrowed("OoTR_1271256_XZYKYQ01Q1") })),
                        (utc!(2022, 12, 6, 1, 0, 0), utc!(2022, 12, 6, 5, 52, 45, 974), None, "https://racetime.gg/ootr/good-bigocto-9887", 86, 69, "https://www.youtube.com/watch?v=yTOC4ArmC6g", Some(seed::Data { web: Some(seed::OotrWebData { id: 1272732, gen_time: utc!(2022, 12, 6, 0, 45, 17) }), file_hash: Some([HashIcon::MegatonHammer, HashIcon::Cucco, HashIcon::SoldOut, HashIcon::BottledMilk, HashIcon::Slingshot]), file_stem: Cow::Borrowed("OoTR_1272732_FUP20778J4") })),
                        (utc!(2022, 12, 8, 19, 0, 0), utc!(2022, 12, 9, 1, 11, 51, 557), None, "https://racetime.gg/ootr/artful-barinade-9952", 80, 65, "https://www.youtube.com/watch?v=PxYEh63lvr4", Some(seed::Data { web: Some(seed::OotrWebData { id: 1274699, gen_time: utc!(2022, 12, 8, 18, 45, 51) }), file_hash: Some([HashIcon::MaskOfTruth, HashIcon::BottledFish, HashIcon::MaskOfTruth, HashIcon::Beans, HashIcon::HeartContainer]), file_stem: Cow::Borrowed("OoTR_1274699_XJA87DO91V") })),
                        (utc!(2022, 12, 10, 23, 0, 0), utc!(2022, 12, 11, 3, 35, 44, 300), Some("NA"), "https://racetime.gg/ootr/trusty-ingo-2577", 113, 80, "https://www.youtube.com/watch?v=zcSfvGAyGh0", Some(seed::Data { web: Some(seed::OotrWebData { id: 1276441, gen_time: utc!(2022, 12, 10, 22, 45, 2) }), file_hash: Some([HashIcon::MirrorShield, HashIcon::DekuStick, HashIcon::MaskOfTruth, HashIcon::Cucco, HashIcon::Map]), file_stem: Cow::Borrowed("OoTR_1276441_OBLJGB7Y83") })),
                        (utc!(2022, 12, 11, 14, 0, 0), utc!(2022, 12, 11, 18, 37, 19, 31), Some("EU"), "https://racetime.gg/ootr/speedy-jiro-3637", 113, 90, "https://www.twitch.tv/videos/1676628321", Some(seed::Data { web: Some(seed::OotrWebData { id: 1276935, gen_time: utc!(2022, 12, 11, 13, 47, 26) }), file_hash: Some([HashIcon::Longshot, HashIcon::Map, HashIcon::SilverGauntlets, HashIcon::MegatonHammer, HashIcon::Map]), file_stem: Cow::Borrowed("OoTR_1276935_0XPIIY36Q7") })),
                        (utc!(2022, 12, 12, 19, 0, 0), utc!(2022, 12, 13, 0, 13, 10, 597), None, "https://racetime.gg/ootr/sleepy-talon-9258", 87, 59, "https://www.youtube.com/watch?v=ZtT4f7w24-4", Some(seed::Data { web: Some(seed::OotrWebData { id: 1277918, gen_time: utc!(2022, 12, 12, 18, 45, 33) }), file_hash: Some([HashIcon::BossKey, HashIcon::KokiriTunic, HashIcon::SkullToken, HashIcon::Frog, HashIcon::Beans]), file_stem: Cow::Borrowed("OoTR_1277918_GFH8F88GIT") })),
                        (utc!(2022, 12, 15, 1, 0, 0), utc!(2022, 12, 15, 6, 3, 30, 579), None, "https://racetime.gg/ootr/hungry-gohma-3413", 69, 55, "https://www.twitch.tv/videos/1679558638", Some(seed::Data { web: Some(seed::OotrWebData { id: 1279608, gen_time: utc!(2022, 12, 15, 0, 45, 3) }), file_hash: Some([HashIcon::Frog, HashIcon::FairyOcarina, HashIcon::MegatonHammer, HashIcon::FairyOcarina, HashIcon::MirrorShield]), file_stem: Cow::Borrowed("OoTR_1279608_HXOIQ25MV3") })),
                        (utc!(2022, 12, 17, 23, 0, 0), utc!(2022, 12, 18, 3, 22, 37, 317), Some("NA"), "https://racetime.gg/ootr/trusty-wolfos-6723", 92, 70, "https://www.youtube.com/watch?v=6BBQ7VGUSZE", Some(seed::Data { web: Some(seed::OotrWebData { id: 1281942, gen_time: utc!(2022, 12, 17, 22, 45, 58) }), file_hash: Some([HashIcon::Longshot, HashIcon::Saw, HashIcon::LensOfTruth, HashIcon::MegatonHammer, HashIcon::MasterSword]), file_stem: Cow::Borrowed("OoTR_1281942_CKYJSQ7YS9") })),
                        (utc!(2022, 12, 18, 14, 0, 0), utc!(2022, 12, 18, 19, 44, 27, 758), Some("EU"), "https://racetime.gg/ootr/banzai-medigoron-2895", 69, 51, "https://www.youtube.com/watch?v=JAxNet4zeuk", Some(seed::Data { web: Some(seed::OotrWebData { id: 1282394, gen_time: utc!(2022, 12, 18, 13, 45, 7) }), file_hash: Some([HashIcon::GoldScale, HashIcon::MegatonHammer, HashIcon::DekuStick, HashIcon::Frog, HashIcon::Map]), file_stem: Cow::Borrowed("OoTR_1282394_K0OJIAVDCX") })),
                        (utc!(2022, 12, 21, 19, 0, 0), utc!(2022, 12, 22, 3, 56, 57, 266), None, "https://racetime.gg/ootr/overpowered-zora-1013", 68, 43, "https://www.youtube.com/watch?v=zUw7vwS96HU", Some(seed::Data { web: Some(seed::OotrWebData { id: 1285036, gen_time: utc!(2022, 12, 21, 18, 45, 29) }), file_hash: Some([HashIcon::Boomerang, HashIcon::BossKey, HashIcon::BottledMilk, HashIcon::MasterSword, HashIcon::LensOfTruth]), file_stem: Cow::Borrowed("OoTR_1285036_5ZGU6QBS9B") })),
                        (utc!(2022, 12, 23, 3, 0, 0), utc!(2022, 12, 23, 7, 41, 05, 441), None, "https://racetime.gg/ootr/sleepy-stalfos-1734", 56, 37, "https://www.youtube.com/watch?v=iALvni6vFoA", Some(seed::Data { web: Some(seed::OotrWebData { id: 1286215, gen_time: utc!(2022, 12, 23, 2, 45, 18) }), file_hash: Some([HashIcon::HeartContainer, HashIcon::StoneOfAgony, HashIcon::MirrorShield, HashIcon::Mushroom, HashIcon::BottledMilk]), file_stem: Cow::Borrowed("OoTR_1286215_LNKWY5APAY") })),
                    ].into_iter().enumerate() {
                        races.push(Self {
                            id: None,
                            series: event.series,
                            event: event.event.to_string(),
                            //TODO keep race IDs? (qN, cc)
                            startgg_event: None,
                            startgg_set: None,
                            entrants: Entrants::Count { total, finished },
                            phase: Some(format!("Qualifier")),
                            round: Some(format!("{}{}", i + 1, if let Some(weekly) = weekly { format!(" ({weekly} Weekly)") } else { String::default() })),
                            game: None,
                            scheduling_thread: None,
                            schedule: RaceSchedule::Live {
                                end: Some(end),
                                room: Some(Url::parse(room)?),
                                start,
                            },
                            draft: None,
                            video_url: Some(Url::parse(vod)?),
                            ignored: false,
                            seed,
                        });
                    }
                    // bracket matches
                    for row in sheet_values(&config.zsr_volunteer_signups, format!("Scheduled Races!B2:D")).await? {
                        if let [datetime_et, matchup, round] = &*row {
                            let start = America::New_York.datetime_from_str(&datetime_et, "%d/%m/%Y %H:%M:%S").expect(&format!("failed to parse {datetime_et:?}"));
                            if start < America::New_York.with_ymd_and_hms(2022, 12, 28, 0, 0, 0).single().expect("wrong hardcoded datetime") { continue } //TODO also add an upper bound
                            add_or_update_race(&mut *transaction, &mut races, false, Self {
                                id: None,
                                series: event.series,
                                event: event.event.to_string(),
                                startgg_event: None,
                                startgg_set: None,
                                entrants: if let Some((_, p1, p2)) = regex_captures!("^(.+) +(?i:vs?\\.?|x) +(.+)$", matchup) {
                                    Entrants::Two([
                                        Entrant::Named(p1.to_owned()),
                                        Entrant::Named(p2.to_owned()),
                                    ])
                                } else {
                                    Entrants::Named(matchup.clone())
                                },
                                phase: None, // main bracket
                                round: Some(round.clone()),
                                game: None,
                                scheduling_thread: None,
                                schedule: RaceSchedule::Live {
                                    start: start.with_timezone(&Utc),
                                    end: None,
                                    room: None,
                                },
                                draft: None,
                                seed: None,
                                video_url: None,
                                ignored: false,
                            }).await?;
                        }
                    }
                    // Challenge Cup bracket matches
                    for row in sheet_values("1Hp0rg_bV1Ja6oPdFLomTWQmwNy7ivmLMZ1rrVC3gx0Q", format!("Submitted Matches!C2:K")).await? {
                        if let [group_round, p1, p2, p3, date_et, time_et, is_async, restream_ok, is_cancelled] = &*row {
                            if group_round.is_empty() { continue }
                            let is_async = is_async == "Yes";
                            let _restream_ok = restream_ok == "Yes";
                            if is_cancelled == "TRUE" { continue }
                            let start = America::New_York.datetime_from_str(&format!("{date_et} at {time_et}"), "%-m/%-d/%-Y at %I:%M %p").expect(&format!("failed to parse {date_et:?} at {time_et:?}"));
                            let (round, entrants) = if p3.is_empty() {
                                (group_round.clone(), Entrants::Two([
                                    Entrant::Named(p1.clone()),
                                    Entrant::Named(p2.clone()),
                                ]))
                            } else {
                                (format!("{group_round} Tiebreaker"), Entrants::Three([
                                    Entrant::Named(p1.clone()),
                                    Entrant::Named(p2.clone()),
                                    Entrant::Named(p3.clone()),
                                ]))
                            };
                            add_or_update_race(&mut *transaction, &mut races, false, Self {
                                id: None,
                                series: event.series,
                                event: event.event.to_string(),
                                startgg_event: None,
                                startgg_set: None,
                                phase: Some(format!("Challenge Cup")),
                                round: Some(round),
                                game: None,
                                scheduling_thread: None,
                                schedule: if is_async {
                                    RaceSchedule::Async {
                                        start1: Some(start.with_timezone(&Utc)),
                                        start2: None,
                                        end1: None, end2: None,
                                        room1: None,
                                        room2: None,
                                    }
                                } else {
                                    RaceSchedule::Live {
                                        start: start.with_timezone(&Utc),
                                        end: None,
                                        room: None,
                                    }
                                },
                                draft: None,
                                seed: None,
                                video_url: None,
                                ignored: false,
                                entrants,
                            }).await?;
                        }
                    }
                }
                _ => unimplemented!(),
            },
            Series::TriforceBlitz => {} // manually added by organizers pending reply from Challonge support
        }
        races.retain(|race| !race.ignored);
        races.sort_unstable();
        Ok(races)
    }

    pub(crate) async fn for_scheduling_channel(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, channel_id: ChannelId) -> Result<Vec<Self>, Error> {
        let mut races = Vec::default();
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE scheduling_thread = $1 AND (start IS NULL OR start > NOW())"#, i64::from(channel_id)).fetch_all(&mut *transaction).await? {
            races.push(Self::from_id(&mut *transaction, http_client, startgg_token, id).await?);
        }
        races.retain(|race| !race.ignored);
        races.sort_unstable();
        Ok(races)
    }

    pub(crate) async fn event(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<event::Data<'static>, event::DataError> {
        event::Data::new(transaction, self.series, self.event.clone()).await?.ok_or(event::DataError::Missing)
    }

    pub(crate) fn startgg_set_url(&self) -> Result<Option<Url>, url::ParseError> {
        Ok(if let Self { startgg_event: Some(event), startgg_set: Some(set), .. } = self {
            Some(format!("https://start.gg/{event}/set/{set}").parse()?)
        } else {
            None
        })
    }

    fn cal_events(&self) -> impl Iterator<Item = Event> + Send {
        match self.schedule {
            RaceSchedule::Unscheduled => Box::new(iter::empty()) as Box<dyn Iterator<Item = Event> + Send>,
            RaceSchedule::Live { .. } => Box::new(iter::once(Event { race: self.clone(), kind: EventKind::Normal })),
            RaceSchedule::Async { .. } => Box::new([Event { race: self.clone(), kind: EventKind::Async1 }, Event { race: self.clone(), kind: EventKind::Async2 }].into_iter()),
        }
    }

    pub(crate) fn rooms(&self) -> impl Iterator<Item = Url> + Send {
        self.cal_events().filter_map(|event| event.room().cloned())
    }

    pub(crate) fn teams(&self) -> impl Iterator<Item = &Team> + Send {
        match self.entrants {
            Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => Box::new(iter::empty()) as Box<dyn Iterator<Item = &Team> + Send>,
            Entrants::Two([ref team1, ref team2]) => Box::new([team1, team2].into_iter().filter_map(as_variant!(Entrant::MidosHouseTeam))),
            Entrants::Three([ref team1, ref team2, ref team3]) => Box::new([team1, team2, team3].into_iter().filter_map(as_variant!(Entrant::MidosHouseTeam))),
        }
    }

    pub(crate) fn has_room_for(&self, team: &Team) -> bool {
        match &self.schedule {
            RaceSchedule::Unscheduled => false,
            RaceSchedule::Live { room, .. } => room.is_some(),
            RaceSchedule::Async { room1, room2, .. } => match &self.entrants {
                Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) | Entrants::Three(_) => panic!("asynced race not with Entrants::Two"),
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
            },
        }
    }

    async fn save(&mut self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<()> {
        let id = if self.id.is_some() {
            unimplemented!("updating existing races not yet implemented") //TODO
        } else {
            let id = Id::new(&mut *transaction, IdTable::Races).await?;
            self.id = Some(id);
            id
        };
        let (team1, team2, p1, p2, p3) = match self.entrants {
            Entrants::Open => (None, None, None, None, None),
            Entrants::Count { .. } => unimplemented!(), //TODO
            Entrants::Named(ref entrants) => (None, None, Some(entrants), None, None),
            Entrants::Two([ref p1, ref p2]) => {
                let (team1, p1) = match p1 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None),
                    Entrant::Named(name) => (None, Some(name)),
                };
                let (team2, p2) = match p2 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None),
                    Entrant::Named(name) => (None, Some(name)),
                };
                (team1, team2, p1, p2, None)
            }
            Entrants::Three([ref p1, ref p2, ref p3]) => {
                (
                    None,
                    None,
                    Some(match p1 {
                        Entrant::MidosHouseTeam(_) => unimplemented!(), //TODO
                        Entrant::Named(name) => name,
                    }),
                    Some(match p2 {
                        Entrant::MidosHouseTeam(_) => unimplemented!(), //TODO
                        Entrant::Named(name) => name,
                    }),
                    Some(match p3 {
                        Entrant::MidosHouseTeam(_) => unimplemented!(), //TODO
                        Entrant::Named(name) => name,
                    }),
                )
            }
        };
        let (start, async_start1, async_start2, end, async_end1, async_end2, room, async_room1, async_room2) = match self.schedule {
            RaceSchedule::Unscheduled => (None, None, None, None, None, None, None, None, None),
            RaceSchedule::Live { start, end, ref room } => (Some(start), None, None, end, None, None, room.as_ref(), None, None),
            RaceSchedule::Async { start1, start2, end1, end2, ref room1, ref room2 } => (None, start1, start2, None, end1, end2, None, room1.as_ref(), room2.as_ref()),
        };
        sqlx::query!("INSERT INTO races (
            startgg_set,
            start,
            series,
            event,
            async_start2,
            async_start1,
            room,
            async_room1,
            async_room2,
            draft_state,
            async_end1,
            async_end2,
            end_time,
            team1,
            team2,
            web_id,
            web_gen_time,
            file_stem,
            hash1,
            hash2,
            hash3,
            hash4,
            hash5,
            game,
            id,
            p1,
            p2,
            video_url,
            phase,
            round,
            p3,
            startgg_event,
            scheduling_thread
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33)",
            self.startgg_set,
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
            team1.map(|id| i64::from(id)),
            team2.map(|id| i64::from(id)),
            self.seed.as_ref().and_then(|seed| seed.web).map(|web| web.id as i64),
            self.seed.as_ref().and_then(|seed| seed.web).map(|web| web.gen_time),
            self.seed.as_ref().map(|seed| &*seed.file_stem),
            self.seed.as_ref().and_then(|seed| seed.file_hash).map(|[hash1, _, _, _, _]| hash1) as _,
            self.seed.as_ref().and_then(|seed| seed.file_hash).map(|[_, hash2, _, _, _]| hash2) as _,
            self.seed.as_ref().and_then(|seed| seed.file_hash).map(|[_, _, hash3, _, _]| hash3) as _,
            self.seed.as_ref().and_then(|seed| seed.file_hash).map(|[_, _, _, hash4, _]| hash4) as _,
            self.seed.as_ref().and_then(|seed| seed.file_hash).map(|[_, _, _, _, hash5]| hash5) as _,
            self.game,
            id as _,
            p1,
            p2,
            self.video_url.as_ref().map(|url| url.to_string()),
            self.phase,
            self.round,
            p3,
            self.startgg_event,
            self.scheduling_thread.map(|id| i64::from(id)),
        ).execute(transaction).await?;
        Ok(())
    }
}

impl PartialEq for Race {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
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
        self.schedule.cmp(&other.schedule)
            .then_with(|| self.series.to_str().cmp(other.series.to_str()))
            .then_with(|| self.event.cmp(&other.event))
            .then_with(|| self.phase.cmp(&other.phase))
            .then_with(|| self.round.cmp(&other.round))
            .then_with(|| self.startgg_set.cmp(&other.startgg_set))
            .then_with(|| self.game.cmp(&other.game))
            .then_with(|| self.id.cmp(&other.id))
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EventKind {
    Normal,
    Async1,
    Async2,
}

pub(crate) struct Event {
    pub(crate) race: Race,
    kind: EventKind,
}

impl Event {
    pub(crate) async fn from_room(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, room: Url) -> Result<Option<Self>, Error> {
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE room = $1 AND start IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Normal,
            }))
        }
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE async_room1 = $1 AND async_start1 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async1,
            }))
        }
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE async_room2 = $1 AND async_start2 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async2,
            }))
        }
        Ok(None)
    }

    pub(crate) fn active_teams(&self) -> impl Iterator<Item = &Team> + Send {
        match self.race.entrants {
            Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) => Box::new(iter::empty()) as Box<dyn Iterator<Item = &Team> + Send>,
            Entrants::Two([ref team1, ref team2]) => Box::new([
                matches!(self.kind, EventKind::Normal | EventKind::Async1).then_some(team1),
                matches!(self.kind, EventKind::Normal | EventKind::Async2).then_some(team2),
            ].into_iter().filter_map(identity).filter_map(as_variant!(Entrant::MidosHouseTeam))),
            Entrants::Three([ref team1, ref team2, ref team3]) => match self.kind {
                EventKind::Normal => Box::new([team1, team2, team3].into_iter().filter_map(as_variant!(Entrant::MidosHouseTeam))),
                EventKind::Async1 | EventKind::Async2 => unimplemented!(), //TODO
            },
        }
    }

    fn room(&self) -> Option<&Url> {
        match self.race.schedule {
            RaceSchedule::Unscheduled => unreachable!(),
            RaceSchedule::Live { ref room, .. } => room.as_ref(),
            RaceSchedule::Async { ref room1, ref room2, .. } => match self.kind {
                EventKind::Normal => unreachable!(),
                EventKind::Async1 => room1.as_ref(),
                EventKind::Async2 => room2.as_ref(),
            },
        }
    }

    pub(crate) fn start(&self) -> Option<DateTime<Utc>> {
        match self.race.schedule {
            RaceSchedule::Unscheduled => unreachable!(),
            RaceSchedule::Live { start, .. } => Some(start),
            RaceSchedule::Async { start1, start2, .. } => match self.kind {
                EventKind::Normal => unreachable!(),
                EventKind::Async1 => start1,
                EventKind::Async2 => start2,
            },
        }
    }

    pub(crate) fn end(&self) -> Option<DateTime<Utc>> {
        match self.race.schedule {
            RaceSchedule::Unscheduled => unreachable!(),
            RaceSchedule::Live { end, .. } => end,
            RaceSchedule::Async { end1, end2, .. } => match self.kind {
                EventKind::Normal => unreachable!(),
                EventKind::Async1 => end1,
                EventKind::Async2 => end2,
            },
        }
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sheets(#[from] SheetsError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] StartGG(#[from] startgg::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("missing teams data in race")]
    MissingTeams,
    #[error("wrong number of teams in start.gg set {startgg_set}")]
    StartggTeams {
        startgg_set: String,
    },
    #[error("this start.gg team ID is not associated with a Mido's House team")]
    UnknownTeam,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SheetsError {
    #[error(transparent)] Api(#[from] sheets::APIError),
    #[error(transparent)] Io(#[from] tokio::io::Error),
    #[error(transparent)] OAuth(#[from] yup_oauth2::Error),
    #[error("empty token is not valid")]
    EmptyToken,
    #[error("no values in sheet range")]
    NoValues,
}

async fn sheet_values(sheet_id: &str, range: String) -> Result<Vec<Vec<String>>, SheetsError> {
    let gsuite_secret = read_service_account_key("assets/google-client-secret.json").await?;
    let auth = ServiceAccountAuthenticator::builder(gsuite_secret)
        .build()
        .await?;
    let token = auth.token(&["https://www.googleapis.com/auth/spreadsheets"]).await?;
    if token.as_str().is_empty() { return Err(SheetsError::EmptyToken) }
    let sheets_client = Sheets::new(token);
    let sheet_values = sheets_client.get_values(sheet_id, range).await?;
    sheet_values.values.ok_or(SheetsError::NoValues)
}

fn ics_datetime<Tz: TimeZone>(datetime: DateTime<Tz>) -> String {
    datetime.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string()
}

async fn add_event_races(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: &Environment, config: &Config, cal: &mut ICalendar<'_>, event: &event::Data<'_>) -> Result<(), Error> {
    for (i, race) in Race::for_event(transaction, http_client, env, config, event).await?.into_iter().enumerate() {
        for race_event in race.cal_events() {
            if let Some(start) = race_event.start() {
                let mut cal_event = ics::Event::new(if let Some(id) = race.id {
                    format!("{id}{}@midos.house",
                        match race_event.kind {
                            EventKind::Normal => "",
                            EventKind::Async1 => "-1",
                            EventKind::Async2 => "-2",
                        },
                    )
                } else {
                    format!("{}-{}-{}{}@midos.house",
                        event.series,
                        event.event,
                        race.startgg_set.clone().unwrap_or_else(|| i.to_string()),
                        match race_event.kind {
                            EventKind::Normal => "",
                            EventKind::Async1 => "-1",
                            EventKind::Async2 => "-2",
                        },
                    )
                }, ics_datetime(Utc::now()));
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
                        EventKind::Async1 | EventKind::Async2 => format!("{summary_prefix} (async): {entrants}"),
                    },
                    Entrants::Two([ref team1, ref team2]) => match race_event.kind {
                        EventKind::Normal => format!("{summary_prefix}: {team1} vs {team2}"),
                        EventKind::Async1 => format!("{summary_prefix} (async): {team1} vs {team2}"),
                        EventKind::Async2 => format!("{summary_prefix} (async): {team2} vs {team1}"),
                    },
                    Entrants::Three([ref team1, ref team2, ref team3]) => match race_event.kind {
                        EventKind::Normal => format!("{summary_prefix}: {team1} vs {team2} vs {team3}"),
                        EventKind::Async1 | EventKind::Async2 => unimplemented!(), //TODO
                    },
                };
                cal_event.push(Summary::new(if let Some(game) = race.game {
                    format!("{summary_prefix}, game {game}")
                } else {
                    summary_prefix
                }));
                cal_event.push(DtStart::new(ics_datetime(start)));
                cal_event.push(DtEnd::new(ics_datetime(race_event.end().unwrap_or_else(|| start + Duration::hours(4))))); //TODO better fallback duration estimates depending on format and participants
                cal_event.push(URL::new(if let Some(ref video_url) = race.video_url {
                    video_url.to_string()
                } else if let Some(room) = race_event.room() {
                    room.to_string()
                } else if let Some(set_url) = race.startgg_set_url()? {
                    set_url.to_string()
                } else {
                    uri!(event::info(event.series, &*event.event)).to_string()
                }));
                cal.add_event(cal_event);
            }
        }
    }
    Ok(())
}

#[rocket::get("/calendar.ics")]
pub(crate) async fn index(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>) -> Result<Response<ICalendar<'static>>, Error> {
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for row in sqlx::query!(r#"SELECT series AS "series!: Series", event FROM events WHERE listed"#).fetch_all(&mut transaction).await? {
        let event = event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, http_client, env, config, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/series/<series>/calendar.ics")]
pub(crate) async fn for_series(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series) -> Result<Response<ICalendar<'static>>, Error> {
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for event in sqlx::query_scalar!(r#"SELECT event FROM events WHERE listed AND series = $1"#, series as _).fetch_all(&mut transaction).await? {
        let event = event::Data::new(&mut transaction, series, event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, http_client, env, config, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/event/<series>/<event>/calendar.ics")]
pub(crate) async fn for_event(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series, event: &str) -> Result<Response<ICalendar<'static>>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await.map_err(Error::Sql)?;
    let event = event::Data::new(&mut transaction, series, event).await.map_err(Error::Event)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    add_event_races(&mut transaction, http_client, env, config, &mut cal, &event).await?;
    transaction.commit().await.map_err(Error::Sql)?;
    Ok(Response(cal))
}

pub(crate) async fn create_race_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, event: event::Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, me.as_ref(), Tab::Races, true).await?;
    let form = if me.is_some() {
        let teams = html! {
            @for team in Team::for_event(&mut transaction, event.series, &event.event).await? {
                option(value = team.id.0) : team.name;
            }
        };
        let mut errors = ctx.errors().collect_vec();
        html! {
            form(action = uri!(create_race_post(event.series, &*event.event)).to_string(), method = "post") {
                : csrf;
                : form_field("team1", &mut errors, html! {
                    label(for = "team1") {
                        @if let TeamConfig::Solo = event.team_config() {
                            : "Player A:";
                        } else {
                            : "Team A:";
                        }
                    }
                    select(name = "team1") : teams;
                });
                : form_field("team2", &mut errors, html! {
                    label(for = "team2") {
                        @if let TeamConfig::Solo = event.team_config() {
                            : "Player B:";
                        } else {
                            : "Team B:";
                        }
                    }
                    select(name = "team2") : teams;
                });
                : form_field("phase", &mut errors, html! {
                    label(for = "phase") : "Phase:";
                    input(type = "text", name = "phase", value? = ctx.field_value("phase"));
                });
                : form_field("round", &mut errors, html! {
                    label(for = "round") : "Round:";
                    input(type = "text", name = "round", value? = ctx.field_value("round"));
                });
                : form_field("multiple_games", &mut errors, html! {
                    input(type = "checkbox", id = "multiple_games", name = "multiple_games", checked? = ctx.field_value("multiple_games") == Some("on"));
                    label(for = "multiple_games") {
                        : "This is a multi-game match. (Create follow-up games using ";
                        code : "/assign"; //TODO manual mode for /assign that only takes a game ID
                        : " in the scheduling thread once game 1 has been played.)";
                    }
                });
                fieldset {
                    input(type = "submit", value = "Create");
                }
            }
        }
    } else {
        html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(create_race(event.series, &*event.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to create a race.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests(), ..PageStyle::default() }, &format!("New Race — {}", event.display_name), html! {
        : header;
        h2 : "Create race";
        : form;
    }).await?)
}

#[rocket::get("/event/<series>/<event>/races/new")]
pub(crate) async fn create_race(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: String) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(create_race_form(transaction, me, uri, csrf, event, Context::default()).await?))
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct CreateRaceForm {
    #[field(default = String::new())]
    csrf: String,
    team1: Id,
    team2: Id,
    #[field(default = String::new())]
    phase: String,
    #[field(default = String::new())]
    round: String,
    multiple_games: bool,
}

#[rocket::post("/event/<series>/<event>/races/new", data = "<form>")]
pub(crate) async fn create_race_post(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, CreateRaceForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if !event.organizers(&mut transaction).await?.contains(&me) {
        form.context.push_error(form::Error::validation("You must be an organizer of this event to add a race."));
    }
    match event.match_source() {
        MatchSource::Manual => {}
        MatchSource::StartGG => form.context.push_error(form::Error::validation("This event's races are generated automatically from start.gg and cannot be edited manually. Please contact Fenhl if a race needs to be added that's not represented by a start.gg match.")),
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
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(create_race_form(transaction, Some(me), uri, csrf, event, form.context).await?)
        } else {
            let mut race = Race {
                id: None,
                series: event.series,
                event: event.event.to_string(),
                startgg_event: None,
                startgg_set: None,
                entrants: Entrants::Two([
                    Entrant::MidosHouseTeam(team1.expect("validated")),
                    Entrant::MidosHouseTeam(team2.expect("validated")),
                ]),
                phase: (!value.phase.is_empty()).then(|| value.phase.clone()),
                round: (!value.round.is_empty()).then(|| value.round.clone()),
                game: value.multiple_games.then_some(1),
                scheduling_thread: None,
                schedule: RaceSchedule::Unscheduled,
                draft: match event.draft_kind() {
                    DraftKind::MultiworldS3 => unimplemented!(), //TODO
                    DraftKind::None => None,
                },
                seed: None,
                video_url: None,
                ignored: false,
            };
            if let (Some(guild_id), Some(scheduling_channel)) = (event.discord_guild, event.discord_scheduling_channel) {
                let ctx = discord_ctx.read().await;
                if let Some(command_ids) = ctx.data.read().await.get::<discord_bot::CommandIds>().and_then(|command_ids| command_ids.get(&guild_id)) {
                    if let Some(ChannelType::Forum) = scheduling_channel.to_channel(&*ctx).await?.guild().map(|c| c.kind) {
                        let info_prefix = match (&race.phase, &race.round) {
                            (Some(phase), Some(round)) => Some(format!("{phase} {round}")),
                            (Some(phase), None) => Some(phase.to_owned()),
                            (None, Some(round)) => Some(round.to_owned()),
                            (None, None) => None,
                        };
                        let title = match race.entrants {
                            Entrants::Open | Entrants::Count { .. } => info_prefix.unwrap_or_default(),
                            Entrants::Named(ref entrants) => format!("{}{}",
                                info_prefix.map(|prefix| format!("{prefix}: ")).unwrap_or_default(),
                                entrants,
                            ),
                            Entrants::Two([ref team1, ref team2]) => format!("{}{team1} vs {team2}",
                                info_prefix.map(|prefix| format!("{prefix}: ")).unwrap_or_default(),
                            ),
                            Entrants::Three([ref team1, ref team2, ref team3]) => format!("{}{team1} vs {team2} vs {team3}",
                                info_prefix.map(|prefix| format!("{prefix}: ")).unwrap_or_default(),
                            ),
                        };
                        let mut content = MessageBuilder::default();
                        //TODO ping participants
                        content.push("Welcome to ");
                        if let Some(game) = race.game {
                            content.push("game ");
                            content.push(game.to_string());
                            content.push(" of ");
                        }
                        content.push("your ");
                        if let Some(ref phase) = race.phase {
                            content.push_safe(phase);
                            content.push(' ');
                        }
                        if let Some(ref round) = race.round {
                            content.push_safe(round);
                            content.push(' ');
                        }
                        content.push("match. Use ");
                        content.mention_command(command_ids.schedule, "schedule");
                        content.push(" to schedule as a live race or ");
                        content.mention_command(command_ids.schedule_async, "schedule-async");
                        content.push(" to schedule as an async."); //TODO adjust message if asyncing is not allowed
                        match event.draft_kind() {
                            DraftKind::MultiworldS3 => unimplemented!(), //TODO
                            DraftKind::None => {}
                        }
                        race.scheduling_thread = Some(scheduling_channel.create_forum_post(&*ctx, CreateForumPost::new(
                            title,
                            CreateMessage::new().content(content.build()),
                        ).auto_archive_duration(10080)).await?.id);
                    } else {
                        unimplemented!() //TODO create scheduling thread
                    }
                };
            }
            race.save(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        RedirectOrContent::Content(create_race_form(transaction, Some(me), uri, csrf, event, form.context).await?)
    })
}

pub(crate) async fn edit_race_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, event: event::Data<'_>, race: Race, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let id = race.id.expect("race being edited must have an ID");
    let header = event.header(&mut transaction, me.as_ref(), Tab::Races, true).await?;
    let fenhl = User::from_id(&mut transaction, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
    let form = if me.is_some() {
        let mut errors = ctx.errors().collect_vec();
        html! {
            form(action = uri!(edit_race_post(event.series, &*event.event, id)).to_string(), method = "post") {
                : csrf;
                @match race.schedule {
                    RaceSchedule::Unscheduled => {}
                    RaceSchedule::Live { ref room, .. } => : form_field("room", &mut errors, html! {
                        label(for = "room") : "racetime.gg room:";
                        input(type = "text", name = "room", value? = room.as_ref().map(|room| room.as_ref().to_string())); //TODO get from form context, fall back to current race data
                    });
                    RaceSchedule::Async { ref room1, ref room2, .. } => {
                        : form_field("async_room1", &mut errors, html! {
                            label(for = "async_room1") : "racetime.gg room (team A):";
                            input(type = "text", name = "async_room1", value? = room1.as_ref().map(|room1| room1.to_string())); //TODO get from form context, fall back to current race data
                        });
                        : form_field("async_room2", &mut errors, html! {
                            label(for = "async_room2") : "racetime.gg room (team B):";
                            input(type = "text", name = "async_room2", value? = room2.as_ref().map(|room2| room2.to_string())); //TODO get from form context, fall back to current race data
                        });
                    }
                }
                //TODO allow editing seed
                : form_field("video_url", &mut errors, html! {
                    label(for = "video_url") : "Restream URL:";
                    input(type = "text", name = "video_url", value? = race.video_url.map(|video_url| video_url.to_string())); //TODO get from form context, fall back to current race data
                    label(class = "help") : "Please use the first available out of the following: Permanent Twitch highlight, YouTube or other video, Twitch past broadcast, Twitch channel.";
                });
                fieldset {
                    input(type = "submit", value = "Save");
                }
            }
        }
    } else {
        html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(edit_race(event.series, &*event.event, id))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to edit this race.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests(), ..PageStyle::default() }, &format!("Edit Race — {}", event.display_name), html! {
        : header;
        h2 : "Edit race";
        @if let Some(startgg_event) = race.startgg_event {
            p {
                : "start.gg event: ";
                : startgg_event;
            }
        }
        @if let Some(startgg_set) = race.startgg_set {
            p {
                : "start.gg match: ";
                : startgg_set;
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
            Entrants::Named(entrants) => p {
                : "Entrants: ";
                : entrants;
            }
            Entrants::Two([p1, p2]) => {
                p : "Entrants:";
                ol {
                    li {
                        @match p1 {
                            Entrant::MidosHouseTeam(team) => : team.to_html(false);
                            Entrant::Named(name) => : name;
                        }
                    }
                    li {
                        @match p2 {
                            Entrant::MidosHouseTeam(team) => : team.to_html(false);
                            Entrant::Named(name) => : name;
                        }
                    }
                }
            }
            Entrants::Three([p1, p2, p3]) => {
                p : "Entrants:";
                ol {
                    li {
                        @match p1 {
                            Entrant::MidosHouseTeam(team) => : team.to_html(false);
                            Entrant::Named(name) => : name;
                        }
                    }
                    li {
                        @match p2 {
                            Entrant::MidosHouseTeam(team) => : team.to_html(false);
                            Entrant::Named(name) => : name;
                        }
                    }
                    li {
                        @match p3 {
                            Entrant::MidosHouseTeam(team) => : team.to_html(false);
                            Entrant::Named(name) => : name;
                        }
                    }
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
            RaceSchedule::Async { start1, start2, end1, end2, room1: _, room2: _ } => {
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
            }
        }
        p {
            : "The data above is currently not editable for technical reasons. Please contact ";
            : fenhl;
            : " if you've spotted an error in it.";
        }
        : form;
    }).await?)
}

#[rocket::get("/event/<series>/<event>/races/<id>/edit")]
pub(crate) async fn edit_race(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, startgg_token, id).await?;
    if race.series != event.series || race.event != event.event {
        return Ok(RedirectOrContent::Redirect(Redirect::permanent(uri!(edit_race(race.series, race.event, id)))))
    }
    Ok(RedirectOrContent::Content(edit_race_form(transaction, me, uri, csrf, event, race, Context::default()).await?))
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
    video_url: String,
}

#[rocket::post("/event/<series>/<event>/races/<id>/edit", data = "<form>")]
pub(crate) async fn edit_race_post(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id, form: Form<Contextual<'_, EditRaceForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, startgg_token, id).await?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if race.series != event.series || race.event != event.event {
        form.context.push_error(form::Error::validation("This race is not part of this event."));
    }
    if !me.is_archivist && !event.organizers(&mut transaction).await?.contains(&me) {
        form.context.push_error(form::Error::validation("You must be an archivist to edit this race. If you would like to become an archivist, please contact Fenhl on Discord."));
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
            }
        }
        let mut file_hash = None;
        let mut web_id = None;
        let mut web_gen_time = None;
        let mut file_stem = None;
        for (field_name, room) in valid_room_urls {
            if let Some(row) = sqlx::query!(r#"SELECT
                file_stem,
                web_id AS "web_id: Id",
                web_gen_time,
                hash1 AS "hash1: HashIcon",
                hash2 AS "hash2: HashIcon",
                hash3 AS "hash3: HashIcon",
                hash4 AS "hash4: HashIcon",
                hash5 AS "hash5: HashIcon"
            FROM rsl_seeds WHERE room = $1"#, room.to_string()).fetch_optional(&mut transaction).await? {
                file_hash = Some([row.hash1, row.hash2, row.hash3, row.hash4, row.hash5]);
                if let Some(Id(new_web_id)) = row.web_id {
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
                                if let Some((_, hash1, hash2, hash3, hash4, hash5, web_id_str)) = regex_captures!("^([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+)\nhttps://ootrandomizer\\.com/seed/get\\?id=([0-9]+)$", &info_bot) {
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
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(edit_race_form(transaction, Some(me), uri, csrf, event, race, form.context).await?)
        } else {
            sqlx::query!(
                "UPDATE races SET room = $1, async_room1 = $2, async_room2 = $3, video_url = $4, last_edited_by = $5, last_edited_at = NOW() WHERE id = $6",
                (!value.room.is_empty()).then(|| &value.room),
                (!value.async_room1.is_empty()).then(|| &value.async_room1),
                (!value.async_room2.is_empty()).then(|| &value.async_room2),
                (!value.video_url.is_empty()).then(|| &value.video_url),
                me.id as _,
                i64::from(id),
            ).execute(&mut transaction).await?;
            if let Some([hash1, hash2, hash3, hash4, hash5]) = file_hash {
                sqlx::query!(
                    "UPDATE races SET hash1 = $1, hash2 = $2, hash3 = $3, hash4 = $4, hash5 = $5 WHERE id = $6",
                    hash1 as _, hash2 as _, hash3 as _, hash4 as _, hash5 as _, i64::from(id),
                ).execute(&mut transaction).await?;
            }
            if let Some(web_id) = web_id {
                sqlx::query!("UPDATE races SET web_id = $1 WHERE id = $2", web_id as i64, i64::from(id)).execute(&mut transaction).await?;
            }
            if let Some(web_gen_time) = web_gen_time {
                sqlx::query!("UPDATE races SET web_gen_time = $1 WHERE id = $2", web_gen_time, i64::from(id)).execute(&mut transaction).await?;
            }
            if let Some(file_stem) = file_stem {
                sqlx::query!("UPDATE races SET file_stem = $1 WHERE id = $2", file_stem, i64::from(id)).execute(&mut transaction).await?;
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        RedirectOrContent::Content(edit_race_form(transaction, Some(me), uri, csrf, event, race, form.context).await?)
    })
}
