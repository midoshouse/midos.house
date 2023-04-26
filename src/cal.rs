use {
    std::{
        borrow::Cow,
        cmp::Ordering,
        collections::hash_map::{
            self,
            HashMap,
        },
        convert::identity,
        iter,
        path::Path,
        time::{
            Duration as UDuration,
            Instant,
        },
    },
    chrono::{
        Duration,
        prelude::*,
    },
    chrono_tz::America,
    enum_iterator::all,
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
    once_cell::sync::Lazy,
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
    sheets::{
        Sheets,
        ValueRange,
    },
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
        racetime_bot,
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
            full_form,
            io_error_from_reqwest,
            natjoin_str,
            sync::{
                Mutex,
                lock,
            },
        },
    },
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Entrant {
    MidosHouseTeam(Team),
    Named(String),
}

impl Entrant {
    pub(crate) async fn name(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Option<Cow<'_, str>>> {
        Ok(match self {
            Self::MidosHouseTeam(team) => team.name(transaction).await?,
            Self::Named(name) => Some(Cow::Borrowed(name)),
        })
    }

    pub(crate) async fn to_html(&self, transaction: &mut Transaction<'_, Postgres>, running_text: bool) -> sqlx::Result<RawHtml<String>> {
        Ok(match self {
            Self::MidosHouseTeam(team) => team.to_html(transaction, running_text).await?,
            Self::Named(name) => name.to_html(),
        })
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
    pub(crate) scheduling_thread: Option<ChannelId>,
    pub(crate) schedule: RaceSchedule,
    pub(crate) draft: Option<Draft>,
    pub(crate) seed: Option<seed::Data>,
    pub(crate) video_url: Option<Url>,
    pub(crate) restreamer: Option<String>,
    pub(crate) video_url_fr: Option<Url>,
    pub(crate) restreamer_fr: Option<String>,
    pub(crate) ignored: bool,
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
            total,
            finished,
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
            file_stem,
            locked_spoiler_log_path,
            web_id AS "web_id: Id",
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
        } else if let (Some(total), Some(finished)) = (row.total, row.finished) {
            Entrants::Count {
                total: total as u32,
                finished: finished as u32,
            }
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
                    Entrants::Open
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
        let seed_files = match (row.file_stem, row.locked_spoiler_log_path, row.web_id, row.web_gen_time, row.tfb_uuid) {
            (_, _, _, _, Some(uuid)) => Some(seed::Files::TriforceBlitz { uuid }),
            (Some(file_stem), _, Some(Id(id)), Some(gen_time), None) => Some(seed::Files::OotrWeb { id, gen_time, file_stem: Cow::Owned(file_stem) }),
            (Some(file_stem), locked_spoiler_log_path, Some(Id(id)), None, None) => Some(match (row.start, row.async_start1, row.async_start2) {
                (Some(start), None, None) | (None, Some(start), None) | (None, None, Some(start)) => seed::Files::OotrWeb { id, gen_time: start - Duration::days(1), file_stem: Cow::Owned(file_stem) },
                (None, Some(async_start1), Some(async_start2)) => seed::Files::OotrWeb { id, gen_time: async_start1.min(async_start2) - Duration::days(1), file_stem: Cow::Owned(file_stem) },
                (_, _, _) => seed::Files::MidosHouse { file_stem: Cow::Owned(file_stem), locked_spoiler_log_path },
            }),
            (Some(file_stem), locked_spoiler_log_path, None, _, None) => Some(seed::Files::MidosHouse { file_stem: Cow::Owned(file_stem), locked_spoiler_log_path }),
            (None, _, _, _, None) => None,
        };
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
            seed: seed_files.map(|files| seed::Data {
                file_hash: match (row.hash1, row.hash2, row.hash3, row.hash4, row.hash5) {
                    (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                    (None, None, None, None, None) => None,
                    _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
                },
                files,
            }),
            video_url: row.video_url.map(|url| url.parse()).transpose()?,
            restreamer: row.restreamer,
            video_url_fr: row.video_url_fr.map(|url| url.parse()).transpose()?,
            restreamer_fr: row.restreamer_fr,
            ignored: row.ignored,
            startgg_event, startgg_set, entrants, phase, round,
        })
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: &Environment, config: &Config, event: &event::Data<'_>) -> Result<Vec<Self>, Error> {
        async fn add_or_update_race(transaction: &mut Transaction<'_, Postgres>, races: &mut Vec<Race>, mut race: Race) -> sqlx::Result<()> {
            if let Some(found_race) = races.iter_mut().find(|iter_race|
                iter_race.series == race.series
                && iter_race.event == race.event
                && iter_race.phase == race.phase
                && iter_race.round == race.round
                && iter_race.game == race.game
                && iter_race.entrants == race.entrants
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
            Series::League => {} //TODO get from League website somehow
            Series::Multiworld => {} // added to database
            Series::NineDaysOfSaws | Series::Pictionary => {
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
                    draft: None,
                    seed: None, //TODO
                    video_url: event.video_url.clone(), //TODO sync between event and race?
                    restreamer: None,
                    video_url_fr: None, //TODO video_url_fr field on event::Data?
                    restreamer_fr: None,
                    ignored: false,
                    schedule,
                }).await?;
            }
            Series::Rsl => match &*event.event {
                "1" => {} // no match data available
                "2" | "3" | "4" | "5" => {} // added to database
                _ => unimplemented!(),
            },
            Series::Standard => match &*event.event {
                "6" => {
                    // bracket matches
                    for row in sheet_values(&config.zsr_volunteer_signups, format!("Scheduled Races!B2:D")).await? {
                        if let [datetime_et, matchup, round] = &*row {
                            let start = America::New_York.datetime_from_str(&datetime_et, "%d/%m/%Y %H:%M:%S").expect(&format!("failed to parse {datetime_et:?}"));
                            if start < America::New_York.with_ymd_and_hms(2022, 12, 28, 0, 0, 0).single().expect("wrong hardcoded datetime") { continue } //TODO also add an upper bound
                            add_or_update_race(&mut *transaction, &mut races, Self {
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
                                restreamer: None,
                                video_url_fr: None,
                                restreamer_fr: None,
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
                            add_or_update_race(&mut *transaction, &mut races, Self {
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
                                restreamer: None,
                                video_url_fr: None,
                                restreamer_fr: None,
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

    pub(crate) async fn for_scheduling_channel(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, channel_id: ChannelId, game: Option<i16>) -> Result<Vec<Self>, Error> {
        let mut races = Vec::default();
        let rows = if let Some(game) = game {
            sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE scheduling_thread = $1 AND (start IS NULL OR start > NOW()) AND game = $2"#, i64::from(channel_id), game).fetch_all(&mut *transaction).await?
        } else {
            sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE scheduling_thread = $1 AND (start IS NULL OR start > NOW())"#, i64::from(channel_id)).fetch_all(&mut *transaction).await?
        };
        for id in rows {
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

    pub(crate) fn teams_opt(&self) -> Option<impl Iterator<Item = &Team> + Send> {
        match self.entrants {
            Entrants::Two([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2)]) => Some(Box::new([team1, team2].into_iter()) as Box<dyn Iterator<Item = &Team> + Send>),
            Entrants::Three([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2), Entrant::MidosHouseTeam(ref team3)]) => Some(Box::new([team1, team2, team3].into_iter())),
            Entrants::Open | Entrants::Count { .. } | Entrants::Named(_) | Entrants::Two(_) | Entrants::Three(_) => None,
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

    pub(crate) async fn save(&mut self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<()> {
        let id = if self.id.is_some() {
            unimplemented!("updating existing races not yet implemented") //TODO
        } else {
            let id = Id::new(&mut *transaction, IdTable::Races).await?;
            self.id = Some(id);
            id
        };
        let ([team1, team2], [p1, p2, p3], [total, finished]) = match self.entrants {
            Entrants::Open => ([None; 2], [None; 3], [None; 2]),
            Entrants::Count { total, finished } => ([None; 2], [None; 3], [Some(total), Some(finished)]),
            Entrants::Named(ref entrants) => ([None; 2], [Some(entrants), None, None], [None; 2]),
            Entrants::Two([ref p1, ref p2]) => {
                let (team1, p1) = match p1 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None),
                    Entrant::Named(name) => (None, Some(name)),
                };
                let (team2, p2) = match p2 {
                    Entrant::MidosHouseTeam(team) => (Some(team.id), None),
                    Entrant::Named(name) => (None, Some(name)),
                };
                ([team1, team2], [p1, p2, None], [None; 2])
            }
            Entrants::Three([ref p1, ref p2, ref p3]) => {
                (
                    [None; 2],
                    [Some(match p1 {
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
                    })],
                    [None; 2],
                )
            }
        };
        let (start, async_start1, async_start2, end, async_end1, async_end2, room, async_room1, async_room2) = match self.schedule {
            RaceSchedule::Unscheduled => (None, None, None, None, None, None, None, None, None),
            RaceSchedule::Live { start, end, ref room } => (Some(start), None, None, end, None, None, room.as_ref(), None, None),
            RaceSchedule::Async { start1, start2, end1, end2, ref room1, ref room2 } => (None, start1, start2, None, end1, end2, None, room1.as_ref(), room2.as_ref()),
        };
        let (web_id, web_gen_time, file_stem, locked_spoiler_log_path, tfb_uuid) = match self.seed.as_ref().map(|seed| &seed.files) {
            Some(seed::Files::MidosHouse { file_stem, locked_spoiler_log_path }) => (None, None, Some(file_stem), locked_spoiler_log_path.as_ref(), None),
            Some(seed::Files::OotrWeb { id, gen_time, file_stem }) => (Some(*id), Some(*gen_time), Some(file_stem), None, None),
            Some(seed::Files::TriforceBlitz { uuid }) => (None, None, None, None, Some(uuid)),
            None => (None, None, None, None, None),
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
            video_url_fr,
            phase,
            round,
            p3,
            startgg_event,
            scheduling_thread,
            total,
            finished,
            tfb_uuid,
            restreamer,
            restreamer_fr,
            locked_spoiler_log_path
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37, $38, $39, $40)",
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
            web_id.map(|web_id| web_id as i64),
            web_gen_time,
            file_stem.map(|file_stem| &**file_stem),
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
            self.video_url_fr.as_ref().map(|url| url.to_string()),
            self.phase,
            self.round,
            p3,
            self.startgg_event,
            self.scheduling_thread.map(|id| i64::from(id)),
            total.map(|total| total as i32),
            finished.map(|finished| finished as i32),
            tfb_uuid,
            self.restreamer,
            self.restreamer_fr,
            locked_spoiler_log_path,
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
    pub(crate) kind: EventKind,
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

    pub(crate) async fn rooms_to_open(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str) -> Result<Vec<Self>, Error> {
        let mut events = Vec::default();
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE room IS NULL AND start IS NOT NULL AND start > NOW() AND start <= NOW() + TIME '00:30:00'"#).fetch_all(&mut *transaction).await? {
            events.push(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Normal,
            })
        }
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE room IS NULL AND async_start1 IS NOT NULL AND async_start1 > NOW() AND async_start1 <= NOW() + TIME '00:30:00'"#).fetch_all(&mut *transaction).await? {
            let event = Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async1,
            };
            if !matches!(event.race.event(&mut *transaction).await?.team_config(), TeamConfig::Solo | TeamConfig::Pictionary) { // racetime.gg doesn't support single-entrant races
                events.push(event);
            }
        }
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE room IS NULL AND async_start2 IS NOT NULL AND async_start2 > NOW() AND async_start2 <= NOW() + TIME '00:30:00'"#).fetch_all(&mut *transaction).await? {
            let event = Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async2,
            };
            if !matches!(event.race.event(&mut *transaction).await?.team_config(), TeamConfig::Solo | TeamConfig::Pictionary) { // racetime.gg doesn't support single-entrant races
                events.push(event);
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

    pub(crate) fn is_first_async_half(&self) -> bool {
        match self.race.schedule {
            RaceSchedule::Unscheduled | RaceSchedule::Live { .. } => false,
            RaceSchedule::Async { start1, start2, .. } => match self.kind {
                EventKind::Normal => unreachable!(),
                EventKind::Async1 => start1.map_or(false, |start1| start2.map_or(true, |start2| start1 < start2)),
                EventKind::Async2 => start2.map_or(false, |start2| start1.map_or(true, |start1| start2 < start1)),
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
    #[error("wrong number of teams in start.gg set {startgg_set}")]
    StartggTeams {
        startgg_set: String,
    },
    #[error("this start.gg team ID is not associated with a Mido's House team")]
    UnknownTeam,
}

impl<E: Into<Error>> From<E> for StatusOrError<Error> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

static SHEETS_CACHE: Lazy<Mutex<HashMap<(String, String), (Instant, ValueRange)>>> = Lazy::new(|| Mutex::default());

#[derive(Debug, thiserror::Error)]
pub(crate) enum SheetsError {
    #[error(transparent)] Api(#[from] sheets::APIError), //TODO adjust status codes, e.g. 502 Bad Gateway for 503 Service Unavailable
    #[error(transparent)] Io(#[from] tokio::io::Error),
    #[error(transparent)] OAuth(#[from] yup_oauth2::Error),
    #[error("empty token is not valid")]
    EmptyToken,
    #[error("no values in sheet range")]
    NoValues,
}

async fn sheet_values(sheet_id: &str, range: String) -> Result<Vec<Vec<String>>, SheetsError> {
    let key = (sheet_id.to_owned(), range.clone());
    let mut cache = lock!(SHEETS_CACHE);
    cache.retain(|_, (retrieved, _)| retrieved.elapsed() < UDuration::from_secs(5 * 60));
    match cache.entry(key) {
        hash_map::Entry::Occupied(entry) => entry.get().1.values.clone(),
        hash_map::Entry::Vacant(entry) => {
            let gsuite_secret = read_service_account_key("assets/google-client-secret.json").await?;
            let auth = ServiceAccountAuthenticator::builder(gsuite_secret)
                .build()
                .await?;
            let token = auth.token(&["https://www.googleapis.com/auth/spreadsheets"]).await?;
            if token.as_str().is_empty() { return Err(SheetsError::EmptyToken) }
            let sheets_client = Sheets::new(token);
            entry.insert((Instant::now(), sheets_client.get_values(sheet_id, range).await?)).1.values.clone()
        }
    }.ok_or(SheetsError::NoValues)
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
                        EventKind::Normal => format!(
                            "{summary_prefix}: {} vs {}",
                            team1.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async1 => format!(
                            "{summary_prefix} (async): {} vs {}",
                            team1.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async2 => format!(
                            "{summary_prefix} (async): {} vs {}",
                            team2.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team1.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                    },
                    Entrants::Three([ref team1, ref team2, ref team3]) => match race_event.kind {
                        EventKind::Normal => format!(
                            "{summary_prefix}: {} vs {} vs {}",
                            team1.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team3.name(&mut *transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        ),
                        EventKind::Async1 | EventKind::Async2 => unimplemented!(), //TODO
                    },
                };
                cal_event.push(Summary::new(if let Some(game) = race.game {
                    format!("{summary_prefix}, game {game}")
                } else {
                    summary_prefix
                }));
                cal_event.push(DtStart::new(ics_datetime(start)));
                cal_event.push(DtEnd::new(ics_datetime(race_event.end().unwrap_or_else(|| start + match event.series {
                    Series::TriforceBlitz => Duration::hours(2),
                    Series::League | Series::NineDaysOfSaws | Series::Standard => Duration::hours(3) + Duration::minutes(30),
                    Series::Multiworld | Series::Pictionary => Duration::hours(4),
                    Series::Rsl => Duration::hours(4) + Duration::minutes(30),
                })))); //TODO better fallback duration estimates depending on participants
                cal_event.push(URL::new(if let Some(ref video_url) = race.video_url {
                    video_url.to_string()
                } else if let Some(ref video_url_fr) = race.video_url_fr {
                    video_url_fr.to_string()
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
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    add_event_races(&mut transaction, http_client, env, config, &mut cal, &event).await?;
    transaction.commit().await?;
    Ok(Response(cal))
}

pub(crate) async fn create_race_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, me.as_ref(), Tab::Races, true).await?;
    let form = if me.is_some() {
        let teams = Team::for_event(&mut transaction, event.series, &event.event).await?;
        let mut team_data = Vec::with_capacity(teams.len());
        for team in teams {
            let name = if let Some(name) = team.name(&mut transaction).await? {
                name.into_owned()
            } else {
                format!("unnamed team ({})", natjoin_str(team.members(&mut transaction).await?).unwrap_or_else(|| format!("no members")))
            };
            team_data.push((team.id.to_string(), name));
        }
        let phase_options = sqlx::query_scalar!("SELECT option FROM phase_options WHERE series = $1 AND event = $2", event.series as _, &event.event).fetch_all(&mut transaction).await?;
        let round_options = sqlx::query_scalar!("SELECT option FROM round_options WHERE series = $1 AND event = $2", event.series as _, &event.event).fetch_all(&mut transaction).await?;
        let mut errors = ctx.errors().collect_vec();
        full_form(uri!(create_race_post(event.series, &*event.event)), csrf, html! {
            : form_field("team1", &mut errors, html! {
                label(for = "team1") {
                    @if let TeamConfig::Solo = event.team_config() {
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
                    @if let TeamConfig::Solo = event.team_config() {
                        : "Player B:";
                    } else {
                        : "Team B:";
                    }
                }
                select(name = "team2") {
                    @for (id, name) in team_data {
                        option(value = id, selected? = ctx.field_value("team2") == Some(&id)) : name;
                    }
                }
            });
            : form_field("phase", &mut errors, html! {
                label(for = "phase") : "Phase:";
                @if phase_options.is_empty() {
                    input(type = "text", name = "phase", value? = ctx.field_value("phase"));
                } else {
                    select(name = "phase") {
                        @for option in phase_options {
                            option(value = option, selected? = ctx.field_value("phase") == Some(&option)) : option;
                        }
                    }
                }
            });
            : form_field("round", &mut errors, html! {
                label(for = "round") : "Round:";
                @if round_options.is_empty() {
                    input(type = "text", name = "round", value? = ctx.field_value("round"));
                } else {
                    select(name = "round") {
                        @for option in round_options {
                            option(value = option, selected? = ctx.field_value("round") == Some(&option)) : option;
                        }
                    }
                }
            });
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
    Ok(RedirectOrContent::Content(create_race_form(transaction, me, uri, csrf.as_ref(), event, Context::default()).await?))
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
    game_count: i16,
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
            RedirectOrContent::Content(create_race_form(transaction, Some(me), uri, csrf.as_ref(), event, form.context).await?)
        } else {
            let [team1, team2] = [team1, team2].map(|team| team.expect("validated"));
            let scheduling_thread = if let (Some(guild_id), Some(scheduling_channel)) = (event.discord_guild, event.discord_scheduling_channel) {
                let command_ids = discord_ctx.read().await.data.read().await.get::<discord_bot::CommandIds>().and_then(|command_ids| command_ids.get(&guild_id).copied());
                if let Some(command_ids) = command_ids {
                    if let Some(ChannelType::Forum) = scheduling_channel.to_channel(&*discord_ctx.read().await).await?.guild().map(|c| c.kind) {
                        let info_prefix = format!("{}{}{}",
                            value.phase,
                            if value.phase.is_empty() && value.round.is_empty() { "" } else { " " },
                            value.round,
                        );
                        let title = format!("{}{} vs {}",
                            if info_prefix.is_empty() { String::default() } else { format!("{info_prefix}: ") },
                            team1.name(&mut transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut transaction).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                        );
                        let mut content = MessageBuilder::default();
                        {
                            content.mention_team(&mut transaction, Some(guild_id), &team1).await?;
                            content.push(' ');
                            content.mention_team(&mut transaction, Some(guild_id), &team2).await?;
                            content.push(' ');
                        }
                        content.push("Welcome to your ");
                        if !value.phase.is_empty() {
                            content.push_safe(value.phase.clone());
                            content.push(' ');
                        }
                        if !value.round.is_empty() {
                            content.push_safe(value.round.clone());
                            content.push(' ');
                        }
                        content.push("match. Use ");
                        content.mention_command(command_ids.schedule, "schedule");
                        content.push(" to schedule as a live race or ");
                        content.mention_command(command_ids.schedule_async, "schedule-async");
                        content.push(" to schedule as an async."); //TODO adjust message if asyncing is not allowed
                        if value.game_count > 1 {
                            content.push(" You can use the ");
                            content.push_mono("game:");
                            content.push(" parameter with these commands to schedule subsequent games ahead of time.");
                        }
                        match event.draft_kind() {
                            DraftKind::MultiworldS3 => unimplemented!(), //TODO
                            DraftKind::None => {}
                        }
                        Some(scheduling_channel.create_forum_post(&*discord_ctx.read().await, CreateForumPost::new(
                            title,
                            CreateMessage::new().content(content.build()),
                        ).auto_archive_duration(10080)).await?.id)
                    } else {
                        unimplemented!() //TODO create scheduling thread
                    }
                } else {
                    None //TODO still create scheduling thread, just without posting command info?
                }
            } else {
                None
            };
            for game in 1..=value.game_count {
                Race {
                    id: None,
                    series: event.series,
                    event: event.event.to_string(),
                    startgg_event: None,
                    startgg_set: None,
                    entrants: Entrants::Two([
                        Entrant::MidosHouseTeam(team1.clone()),
                        Entrant::MidosHouseTeam(team2.clone()),
                    ]),
                    phase: (!value.phase.is_empty()).then(|| value.phase.clone()),
                    round: (!value.round.is_empty()).then(|| value.round.clone()),
                    game: (value.game_count > 1).then_some(game),
                    schedule: RaceSchedule::Unscheduled,
                    draft: match event.draft_kind() {
                        DraftKind::MultiworldS3 => unimplemented!(), //TODO
                        DraftKind::None => None,
                    },
                    seed: None,
                    video_url: None,
                    restreamer: None,
                    video_url_fr: None,
                    restreamer_fr: None,
                    ignored: false,
                    scheduling_thread,
                }.save(&mut transaction).await?;
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        RedirectOrContent::Content(create_race_form(transaction, Some(me), uri, csrf.as_ref(), event, form.context).await?)
    })
}

pub(crate) async fn edit_race_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, race: Race, ctx: Option<Context<'_>>) -> Result<RawHtml<String>, event::Error> {
    let id = race.id.expect("race being edited must have an ID");
    let header = event.header(&mut transaction, me.as_ref(), Tab::Races, true).await?;
    let fenhl = User::from_id(&mut transaction, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
    let form = if me.is_some() {
        let mut errors = ctx.as_ref().map(|ctx| ctx.errors().collect()).unwrap_or_default();
        full_form(uri!(edit_race_post(event.series, &*event.event, id)), csrf, html! {
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
                RaceSchedule::Async { ref room1, ref room2, .. } => {
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
                }
            }
            : form_field("video_url", &mut errors, html! {
                label(for = "video_url") : "English restream URL:";
                input(type = "text", name = "video_url", value? = if let Some(ref ctx) = ctx {
                    ctx.field_value("video_url").map(|room| room.to_string())
                } else {
                    race.video_url.map(|video_url| video_url.to_string())
                });
                label(class = "help") : "Please use the first available out of the following: Permanent Twitch highlight, YouTube or other video, Twitch past broadcast, Twitch channel.";
            });
            : form_field("restreamer", &mut errors, html! {
                label(for = "restreamer") : "English restreamer:";
                input(type = "text", name = "restreamer", value? = if let Some(ref ctx) = ctx {
                    ctx.field_value("restreamer")
                } else {
                    race.restreamer.as_deref() //TODO display as racetime.gg profile URL
                });
                label(class = "help") : "(racetime.gg profile URL, racetime.gg user ID, or Mido's House user ID. Leave this field blank to assign yourself.)";
            });
            : form_field("video_url_fr", &mut errors, html! {
                label(for = "video_url_fr") : "French restream URL:";
                input(type = "text", name = "video_url_fr", value? = if let Some(ref ctx) = ctx {
                    ctx.field_value("video_url_fr").map(|room| room.to_string())
                } else {
                    race.video_url_fr.map(|video_url_fr| video_url_fr.to_string())
                });
                label(class = "help") : "Please use the first available out of the following: Permanent Twitch highlight, YouTube or other video, Twitch past broadcast, Twitch channel.";
            });
            : form_field("restreamer_fr", &mut errors, html! {
                label(for = "restreamer_fr") : "French restreamer:";
                input(type = "text", name = "restreamer_fr", value? = if let Some(ref ctx) = ctx {
                    ctx.field_value("restreamer_fr")
                } else {
                    race.restreamer_fr.as_deref() //TODO display as racetime.gg profile URL
                });
                label(class = "help") : "(racetime.gg profile URL, racetime.gg user ID, or Mido's House user ID. Leave this field blank to assign yourself.)";
            });
        }, errors, "Save")
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
    let content = html! {
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
                    li : p1.to_html(&mut transaction, false).await?;
                    li : p2.to_html(&mut transaction, false).await?;
                }
            }
            Entrants::Three([p1, p2, p3]) => {
                p : "Entrants:";
                ol {
                    li : p1.to_html(&mut transaction, false).await?;
                    li : p2.to_html(&mut transaction, false).await?;
                    li : p3.to_html(&mut transaction, false).await?;
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
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests(), ..PageStyle::default() }, &format!("Edit Race — {}", event.display_name), content).await?)
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
    Ok(RedirectOrContent::Content(edit_race_form(transaction, me, uri, csrf.as_ref(), event, race, None).await?))
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
    #[field(default = String::new())]
    restreamer: String,
    #[field(default = String::new())]
    video_url_fr: String,
    #[field(default = String::new())]
    restreamer_fr: String,
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
        let restreamer = if !value.video_url.is_empty() {
            if let Err(e) = Url::parse(&value.video_url) {
                form.context.push_error(form::Error::validation(format!("Failed to parse URL: {e}")).with_name("video_url"));
            }
            if value.restreamer.is_empty() {
                if let Some(ref racetime_id) = me.racetime_id {
                    Some(racetime_id.clone())
                } else {
                    form.context.push_error(form::Error::validation("A racetime.gg account is required to restream races. Go to your profile and select “Connect a racetime.gg account”.").with_name("restreamer"));
                    None
                }
            } else {
                match racetime_bot::parse_user(&mut transaction, http_client, env.racetime_host(), &value.restreamer).await {
                    Ok(racetime_id) => Some(racetime_id),
                    Err(e @ (racetime_bot::ParseUserError::Format | racetime_bot::ParseUserError::IdNotFound | racetime_bot::ParseUserError::InvalidUrl | racetime_bot::ParseUserError::MidosHouseId | racetime_bot::ParseUserError::MidosHouseUserNoRacetime | racetime_bot::ParseUserError::UrlNotFound)) => {
                        form.context.push_error(form::Error::validation(e.to_string()).with_name("restreamer"));
                        None
                    }
                    Err(racetime_bot::ParseUserError::Reqwest(e)) => return Err(e.into()),
                    Err(racetime_bot::ParseUserError::Sql(e)) => return Err(e.into()),
                    Err(racetime_bot::ParseUserError::Wheel(e)) => return Err(e.into()),
                }
            }
        } else {
            if !value.restreamer.is_empty() {
                form.context.push_error(form::Error::validation("Please either add a restream URL or remove the restreamer.").with_name("restreamer"));
            }
            None
        };
        let restreamer_fr = if !value.video_url_fr.is_empty() {
            if let Err(e) = Url::parse(&value.video_url_fr) {
                form.context.push_error(form::Error::validation(format!("Failed to parse URL: {e}")).with_name("video_url_fr"));
            }
            if value.restreamer_fr.is_empty() {
                if let Some(ref racetime_id) = me.racetime_id {
                    Some(racetime_id.clone())
                } else {
                    form.context.push_error(form::Error::validation("A racetime.gg account is required to restream races. Go to your profile and select “Connect a racetime.gg account”.").with_name("restreamer_fr"));
                    None
                }
            } else {
                match racetime_bot::parse_user(&mut transaction, http_client, env.racetime_host(), &value.restreamer_fr).await {
                    Ok(racetime_id) => Some(racetime_id),
                    Err(e @ (racetime_bot::ParseUserError::Format | racetime_bot::ParseUserError::IdNotFound | racetime_bot::ParseUserError::InvalidUrl | racetime_bot::ParseUserError::MidosHouseId | racetime_bot::ParseUserError::MidosHouseUserNoRacetime | racetime_bot::ParseUserError::UrlNotFound)) => {
                        form.context.push_error(form::Error::validation(e.to_string()).with_name("restreamer_fr"));
                        None
                    }
                    Err(racetime_bot::ParseUserError::Reqwest(e)) => return Err(e.into()),
                    Err(racetime_bot::ParseUserError::Sql(e)) => return Err(e.into()),
                    Err(racetime_bot::ParseUserError::Wheel(e)) => return Err(e.into()),
                }
            }
        } else {
            if !value.restreamer_fr.is_empty() {
                form.context.push_error(form::Error::validation("Please either add a restream URL or remove the restreamer.").with_name("restreamer_fr"));
            }
            None
        };
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(edit_race_form(transaction, Some(me), uri, csrf.as_ref(), event, race, Some(form.context)).await?)
        } else {
            sqlx::query!(
                "UPDATE races SET room = $1, async_room1 = $2, async_room2 = $3, video_url = $4, restreamer = $5, video_url_fr = $6, restreamer_fr = $7, last_edited_by = $8, last_edited_at = NOW() WHERE id = $9",
                (!value.room.is_empty()).then(|| &value.room),
                (!value.async_room1.is_empty()).then(|| &value.async_room1),
                (!value.async_room2.is_empty()).then(|| &value.async_room2),
                (!value.video_url.is_empty()).then(|| &value.video_url),
                restreamer,
                (!value.video_url_fr.is_empty()).then(|| &value.video_url_fr),
                restreamer_fr,
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
        RedirectOrContent::Content(edit_race_form(transaction, Some(me), uri, csrf.as_ref(), event, race, Some(form.context)).await?)
    })
}

pub(crate) async fn add_file_hash_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, race: Race, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let id = race.id.expect("race being edited must have an ID");
    let header = event.header(&mut transaction, me.as_ref(), Tab::Races, true).await?;
    let form = if me.is_some() {
        let mut errors = ctx.errors().collect();
        full_form(uri!(add_file_hash_post(event.series, &*event.event, id)), csrf, html! {
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
                    a(href = uri!(auth::login(Some(uri!(edit_race(event.series, &*event.event, id))))).to_string()) : "Sign in or create a Mido's House account";
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
            RaceSchedule::Async { room1, room2, .. } => {
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
            }
        }
        : form;
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests(), ..PageStyle::default() }, &format!("Edit Race — {}", event.display_name), content).await?)
}

#[rocket::get("/event/<series>/<event>/races/<id>/edit-hash")]
pub(crate) async fn add_file_hash(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, startgg_token, id).await?;
    if race.series != event.series || race.event != event.event {
        return Ok(RedirectOrContent::Redirect(Redirect::permanent(uri!(add_file_hash(race.series, race.event, id)))))
    }
    Ok(RedirectOrContent::Content(add_file_hash_form(transaction, me, uri, csrf.as_ref(), event, race, Context::default()).await?))
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
pub(crate) async fn add_file_hash_post(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id, form: Form<Contextual<'_, AddFileHashForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
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
            RedirectOrContent::Content(add_file_hash_form(transaction, Some(me), uri, csrf.as_ref(), event, race, form.context).await?)
        } else {
            sqlx::query!(
                "UPDATE races SET hash1 = $1, hash2 = $2, hash3 = $3, hash4 = $4, hash5 = $5 WHERE id = $6",
                hash1.unwrap() as _, hash2.unwrap() as _, hash3.unwrap() as _, hash4.unwrap() as _, hash5.unwrap() as _, i64::from(id),
            ).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        RedirectOrContent::Content(add_file_hash_form(transaction, Some(me), uri, csrf.as_ref(), event, race, form.context).await?)
    })
}
