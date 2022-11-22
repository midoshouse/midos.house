use {
    std::{
        borrow::Cow,
        cmp::Ordering,
        iter,
    },
    chrono::{
        Duration,
        prelude::*,
    },
    chrono_tz::America,
    ics::{
        ICalendar,
        properties::{
            DtEnd,
            DtStart,
            Summary,
            URL,
        },
    },
    once_cell::sync::Lazy,
    racetime::model::RaceData,
    rocket::{
        State,
        http::Status,
        uri,
    },
    rocket_util::Response,
    serde::Deserialize,
    sheets::Sheets,
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
        types::Json,
    },
    url::Url,
    wheel::traits::ReqwestResponseExt as _,
    yup_oauth2::{
        ServiceAccountAuthenticator,
        read_service_account_key,
    },
    crate::{
        Environment,
        config::Config,
        discord_bot::Draft,
        event::{
            self,
            Series,
        },
        seed::{
            self,
            HashIcon,
        },
        startgg,
        team::Team,
        util::{
            Id,
            StatusOrError,
        },
    },
};

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
    series: Series,
    event: String,
    pub(crate) startgg_event: String,
    pub(crate) startgg_set: String,
    pub(crate) team1: Team,
    pub(crate) team2: Team,
    pub(crate) phase: String,
    pub(crate) round: String,
    pub(crate) schedule: RaceSchedule,
    pub(crate) draft: Option<Draft>,
    pub(crate) seed: Option<seed::Data>,
}

impl Race {
    pub(crate) async fn from_startgg(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, startgg_set: String) -> Result<Self, Error> {
        let response_data = startgg::query::<startgg::SetQuery>(http_client, startgg_token, startgg::set_query::Variables { set_id: startgg::ID(startgg_set.clone()) }).await?;
        if let startgg::set_query::ResponseData {
            set: Some(startgg::set_query::SetQuerySet {
                full_round_text: Some(ref round),
                phase_group: Some(startgg::set_query::SetQuerySetPhaseGroup {
                    phase: Some(startgg::set_query::SetQuerySetPhaseGroupPhase {
                        event: Some(startgg::set_query::SetQuerySetPhaseGroupPhaseEvent {
                            slug: Some(ref startgg_event),
                        }),
                        name: Some(ref phase),
                    }),
                }),
                slots: Some(ref slots),
            }),
        } = response_data {
            let row = sqlx::query!(r#"SELECT
                series AS "series: Series",
                event,
                team1 AS "team1: Id",
                team2 AS "team2: Id",
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
                hash5 AS "hash5: HashIcon"
            FROM races WHERE startgg_set = $1"#, &startgg_set).fetch_one(&mut *transaction).await?;
            let (team1, team2) = if let (Some(team1), Some(team2)) = (row.team1, row.team2) {
                (
                    Team::from_id(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?,
                    Team::from_id(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?,
                )
            } else if let [
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team1)), on: _ }) }) }),
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team2)), on: _ }) }) }),
            ] = **slots {
                let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?;
                let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?;
                sqlx::query!("UPDATE races SET team1 = $1 WHERE startgg_set = $2", team1.id as _, &startgg_set).execute(&mut *transaction).await?;
                sqlx::query!("UPDATE races SET team2 = $1 WHERE startgg_set = $2", team2.id as _, &startgg_set).execute(&mut *transaction).await?;
                (team1, team2)
            } else {
                return Err(Error::Teams { startgg_set, response_data })
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
                            sqlx::query!($query, end, &startgg_set).execute(&mut *transaction).await?;
                        }
                        end
                    } else {
                        None
                    };
                };
            }

            update_end!(end_time, room, "UPDATE races SET end_time = $1 WHERE startgg_set = $2");
            update_end!(async_end1, async_room1, "UPDATE races SET async_end1 = $1 WHERE startgg_set = $2");
            update_end!(async_end2, async_room2, "UPDATE races SET async_end2 = $1 WHERE startgg_set = $2");
            Ok(Self {
                series: row.series,
                event: row.event,
                startgg_event: startgg_event.clone(),
                phase: phase.clone(),
                round: round.clone(),
                schedule: RaceSchedule::new(
                    row.start, row.async_start1, row.async_start2,
                    end_time, async_end1, async_end2,
                    row.room.map(|room| room.parse()).transpose()?, row.async_room1.map(|room| room.parse()).transpose()?, row.async_room2.map(|room| room.parse()).transpose()?,
                ),
                draft: row.draft_state.map(|Json(draft)| draft),
                seed: row.file_stem.map(|file_stem| seed::Data {
                    web: match (row.web_id, row.web_gen_time) {
                        (Some(Id(id)), Some(gen_time)) => Some(seed::OotrWebData { id, gen_time }),
                        (None, None) => None,
                        _ => unreachable!("only some web data present, should be prevented by SQL constraint"),
                    },
                    file_hash: match (row.hash1, row.hash2, row.hash3, row.hash4, row.hash5) {
                        (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                        (None, None, None, None, None) => None,
                        _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
                    },
                    file_stem: Cow::Owned(file_stem),
                }),
                startgg_set, team1, team2,
            })
        } else {
            Err(Error::Teams { startgg_set, response_data })
        }
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, series: Series, event: &str) -> Result<Vec<Self>, Error> {
        //TODO unify with add_event_races
        let mut races = Vec::default();
        for startgg_set in sqlx::query_scalar!("SELECT startgg_set FROM races WHERE series = $1 AND event = $2", series as _, event).fetch_all(&mut *transaction).await? {
            races.push(Self::from_startgg(&mut *transaction, http_client, startgg_token, startgg_set).await?);
        }
        races.sort_unstable();
        Ok(races)
    }

    pub(crate) async fn event(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<event::Data<'static>, event::DataError> {
        event::Data::new(transaction, self.series, self.event.clone()).await?.ok_or(event::DataError::Missing)
    }

    pub(crate) fn startgg_set_url(&self) -> Result<Url, url::ParseError> {
        format!("https://start.gg/{}/set/{}", self.startgg_event, self.startgg_set).parse()
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
            .then_with(|| self.startgg_event.cmp(&other.startgg_event))
            .then_with(|| self.startgg_set.cmp(&other.startgg_set))
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
        if let Some(startgg_set) = sqlx::query_scalar!(r#"SELECT startgg_set FROM races WHERE room = $1 AND start IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_startgg(&mut *transaction, http_client, startgg_token, startgg_set).await?,
                kind: EventKind::Normal,
            }))
        }
        if let Some(startgg_set) = sqlx::query_scalar!(r#"SELECT startgg_set FROM races WHERE async_room1 = $1 AND async_start1 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_startgg(&mut *transaction, http_client, startgg_token, startgg_set).await?,
                kind: EventKind::Async1,
            }))
        }
        if let Some(startgg_set) = sqlx::query_scalar!(r#"SELECT startgg_set FROM races WHERE async_room2 = $1 AND async_start2 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_startgg(&mut *transaction, http_client, startgg_token, startgg_set).await?,
                kind: EventKind::Async2,
            }))
        }
        Ok(None)
    }

    pub(crate) fn active_teams(&self) -> impl Iterator<Item = &Team> + Send {
        match self.kind {
            EventKind::Normal => Box::new([&self.race.team1, &self.race.team2].into_iter()) as Box<dyn Iterator<Item = &Team> + Send>,
            EventKind::Async1 => Box::new(iter::once(&self.race.team1)),
            EventKind::Async2 => Box::new(iter::once(&self.race.team2)),
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
    #[error("wrong number of teams or missing data in start.gg set {startgg_set}")]
    Teams {
        startgg_set: String,
        response_data: <startgg::SetQuery as graphql_client::GraphQLQuery>::ResponseData,
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

async fn add_event_races(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, cal: &mut ICalendar<'_>, event: &event::Data<'_>) -> Result<(), Error> {
    match event.series {
        Series::Multiworld => match &*event.event {
            "2" => {
                #[derive(Deserialize)]
                struct Race {
                    start: DateTime<Utc>,
                    end: DateTime<Utc>,
                    team1: String,
                    team2: String,
                    round: String,
                    #[serde(rename = "async")]
                    is_async: bool,
                    room: Option<String>,
                    restream: Option<Url>,
                }

                static RACES: Lazy<Vec<ics::Event<'static>>> = Lazy::new(||
                    serde_json::from_str::<Vec<Race>>(include_str!("../assets/event/mw/2.json"))
                        .expect("failed to parse mw/2 race list")
                        .into_iter()
                        .enumerate()
                        .map(|(i, race)| {
                            let mut cal_event = ics::Event::new(format!("mw-2-{i}@midos.house"), ics_datetime(Utc::now()));
                            cal_event.push(Summary::new(format!("MW S2 {}{}: {} vs {}", race.round, if race.is_async { " (async)" } else { "" }, race.team1, race.team2)));
                            cal_event.push(DtStart::new(ics_datetime(race.start)));
                            cal_event.push(DtEnd::new(ics_datetime(race.end)));
                            if let Some(restream_url) = race.restream {
                                cal_event.push(URL::new(restream_url.to_string()));
                            } else if let Some(room_slug) = race.room {
                                cal_event.push(URL::new(room_slug));
                            }
                            cal_event
                        })
                        .collect()
                );

                for race in &*RACES {
                    cal.add_event(race.clone());
                }
            }
            _ => {
                for race in Race::for_event(transaction, http_client, startgg_token, event.series, &event.event).await? {
                    for race_event in race.cal_events() {
                        if let Some(start) = race_event.start() {
                            let mut cal_event = ics::Event::new(format!("{}-{}-{}{}@midos.house",
                                event.series,
                                event.event,
                                race.startgg_set,
                                match race_event.kind {
                                    EventKind::Normal => "",
                                    EventKind::Async1 => "-1",
                                    EventKind::Async2 => "-2",
                                },
                            ), ics_datetime(Utc::now()));
                            cal_event.push(Summary::new(match race_event.kind {
                                EventKind::Normal => format!("MW S{} {} {}: {} vs {}", event.event, race.phase, race.round, race.team1, race.team2),
                                EventKind::Async1 => format!("MW S{} {} {} (async): {} vs {}", event.event, race.phase, race.round, race.team1, race.team2),
                                EventKind::Async2 => format!("MW S{} {} {} (async): {} vs {}", event.event, race.phase, race.round, race.team2, race.team1),
                            }));
                            cal_event.push(DtStart::new(ics_datetime(start)));
                            cal_event.push(DtEnd::new(ics_datetime(race_event.end().unwrap_or_else(|| start + Duration::hours(4))))); //TODO better fallback duration estimates depending on participants
                            cal_event.push(URL::new(if let Some(room) = race_event.room() {
                                room.to_string()
                            } else {
                                race.startgg_set_url()?.to_string()
                            })); //TODO prefer restream URL if one exists
                            cal.add_event(cal_event);
                        }
                    }
                }
            }
        },
        Series::NineDaysOfSaws | Series::Pictionary => {
            let mut cal_event = ics::Event::new(format!("{}-{}@midos.house", event.series, event.event), ics_datetime(Utc::now()));
            cal_event.push(Summary::new(event.display_name.clone()));
            if let Some(start) = event.start(transaction).await? {
                cal_event.push(DtStart::new(ics_datetime(start)));
                let end = event.end.unwrap_or_else(|| start + Duration::hours(4)); //TODO better duration estimates depending on format & participants
                cal_event.push(DtEnd::new(ics_datetime(end)));
            }
            cal_event.push(URL::new(uri!("https://midos.house", event::info(event.series, &*event.event)).to_string()));
            cal.add_event(cal_event);
        }
        Series::Rsl => match &*event.event {
            "2" => for (i, row) in sheet_values("1TEb48hIarEXnsnGxJbq1Y4YiZxNSM1t1oBsXh_bM4LM", format!("Raw form data!B2:F")).await?.into_iter().enumerate() {
                if !row.is_empty() {
                    let mut row = row.into_iter().fuse();
                    let p1 = row.next().unwrap();
                    let p2 = row.next().unwrap();
                    let date_et = row.next().unwrap();
                    let time_et = row.next().unwrap();
                    let stream = row.next();
                    assert!(row.next().is_none());
                    let start = America::New_York.datetime_from_str(&format!("{date_et} at {time_et}"), "%-m/%-d/%-Y at %-I:%M:%S %p").expect(&format!("failed to parse {date_et:?} at {time_et:?}"));
                    let duration = Duration::hours(4) + Duration::minutes(30); //TODO better duration estimate
                    let mut event = ics::Event::new(format!("rsl-2-{i}@midos.house"), ics_datetime(Utc::now()));
                    event.push(Summary::new(format!("RSL S2: {p1} vs {p2}"))); //TODO add round numbers from https://challonge.com/ymq48xum
                    event.push(DtStart::new(ics_datetime(start)));
                    event.push(DtEnd::new(ics_datetime(start + duration)));
                    if let Some(stream) = stream {
                        event.push(URL::new(format!("https://{stream}")));
                    }
                    cal.add_event(event);
                }
            },
            "3" => for (i, row) in sheet_values("1475TTqezcSt-okMfQaG6Rf7AlJsqBx8c_rGDKs4oBYk", format!("Sign Ups!B2:I")).await?.into_iter().enumerate() {
                if !row.is_empty() {
                    let mut row = row.into_iter().fuse();
                    let p1 = row.next().unwrap();
                    if p1.is_empty() { continue }
                    let p2 = row.next().unwrap();
                    let _round = row.next().unwrap();
                    let date_et = row.next().unwrap();
                    let time_et = row.next().unwrap();
                    if row.next().map_or(false, |cancel| cancel == "TRUE") { continue }
                    let _monitor = row.next();
                    let stream = row.next();
                    assert!(row.next().is_none());
                    let start = America::New_York.datetime_from_str(&format!("{date_et} at {time_et}"), "%-m/%-d/%-Y at %-I:%M:%S %p").expect(&format!("failed to parse {date_et:?} at {time_et:?}"));
                    let duration = Duration::hours(4) + Duration::minutes(30); //TODO better duration estimate
                    let mut event = ics::Event::new(format!("rsl-3-{i}@midos.house"), ics_datetime(Utc::now()));
                    event.push(Summary::new(format!("RSL S3: {p1} vs {p2}"))); //TODO add round numbers from https://challonge.com/RSL_S3
                    event.push(DtStart::new(ics_datetime(start)));
                    event.push(DtEnd::new(ics_datetime(start + duration)));
                    if let Some(stream) = stream {
                        event.push(URL::new(format!("https://{stream}")));
                    }
                    cal.add_event(event);
                }
            },
            "4" => for (i, row) in sheet_values("1LRJ3oo_2AWGq8KpNNclRXOq4OW8O7LrHra7uY7oQQlA", format!("Form responses 1!B2:H")).await?.into_iter().enumerate() {
                if !row.is_empty() {
                    let mut row = row.into_iter().fuse();
                    let p1 = row.next().unwrap();
                    let p2 = row.next().unwrap();
                    let round = row.next().unwrap();
                    let date_et = row.next().unwrap();
                    let time_et = row.next().unwrap();
                    let _monitor = row.next();
                    let stream = row.next();
                    assert!(row.next().is_none());
                    let start = America::New_York.datetime_from_str(&format!("{date_et} at {time_et}"), "%d/%m/%Y at %H:%M:%S").expect(&format!("failed to parse {date_et:?} at {time_et:?}"));
                    let duration = Duration::hours(4) + Duration::minutes(30); //TODO better duration estimate
                    let mut event = ics::Event::new(format!("rsl-4-{i}@midos.house"), ics_datetime(Utc::now()));
                    event.push(Summary::new(format!("RSL S4 {}: {p1} vs {p2}", if start >= Utc.with_ymd_and_hms(2022, 5, 8, 23, 51, 35).single().expect("wrong hardcoded datetime") { round } else { format!("Swiss Round {round}") })));
                    event.push(DtStart::new(ics_datetime(start)));
                    event.push(DtEnd::new(ics_datetime(start + duration)));
                    if let Some(stream) = stream {
                        event.push(URL::new(format!("https://{stream}")));
                    }
                    cal.add_event(event);
                }
            },
            "5" => for (i, row) in sheet_values("1nz7XtNxKFTq_6bfjlUmIq0fCXKkQfDC848YmVcbaoQw", format!("Form responses 1!B2:H")).await?.into_iter().enumerate() {
                if !row.is_empty() {
                    let mut row = row.into_iter().fuse();
                    let p1 = row.next().unwrap();
                    let p2 = row.next().unwrap();
                    let round = row.next().unwrap();
                    let date_utc = row.next().unwrap();
                    let time_utc = row.next().unwrap();
                    let _monitor = row.next();
                    let stream = row.next();
                    assert!(row.next().is_none());
                    let start = Utc.datetime_from_str(&format!("{date_utc} at {time_utc}"), "%d/%m/%Y at %H:%M:%S").expect(&format!("failed to parse {date_utc:?} at {time_utc:?}"));
                    let duration = Duration::hours(4) + Duration::minutes(30); //TODO better duration estimate
                    let mut event = ics::Event::new(format!("rsl-5-{i}@midos.house"), ics_datetime(Utc::now()));
                    event.push(Summary::new(format!("RSL S5 Swiss Round {round}: {p1} vs {p2}")));
                    event.push(DtStart::new(ics_datetime(start)));
                    event.push(DtEnd::new(ics_datetime(start + duration)));
                    if let Some(stream) = stream {
                        event.push(URL::new(format!("https://twitch.tv/{stream}"))); //TODO vod links
                    }
                    cal.add_event(event);
                }
            },
            _ => unimplemented!(),
        },
        Series::Standard => match &*event.event {
            "6" => {
                for (i, (start, weekly, vod)) in [
                    // source: https://docs.google.com/document/d/1fyNO82G2D0Z7J9wobxEbjDjGnomTaIRdKgETGV_ufmc/edit
                    (Utc.with_ymd_and_hms(2022, 11, 19, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), Some("https://twitch.tv/videos/1657562512")), //TODO permanent highlight/YouTube upload //TODO seed info (https://racetime.gg/ootr/neutral-bongobongo-4042)
                    (Utc.with_ymd_and_hms(2022, 11, 20, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), Some("https://twitch.tv/videos/1658095931")), //TODO permanent highlight/YouTube upload //TODO seed info (https://racetime.gg/ootr/trusty-volvagia-2022)
                    (Utc.with_ymd_and_hms(2022, 11, 23, 3, 0, 0).single().expect("wrong hardcoded datetime"), None, None),
                    (Utc.with_ymd_and_hms(2022, 11, 26, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), None),
                    (Utc.with_ymd_and_hms(2022, 11, 27, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), None),
                    (Utc.with_ymd_and_hms(2022, 11, 29, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://twitch.tv/thesilvergauntlets")),
                    (Utc.with_ymd_and_hms(2022, 12, 2, 1, 0, 0).single().expect("wrong hardcoded datetime"), None, None),
                    (Utc.with_ymd_and_hms(2022, 12, 3, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), None),
                    (Utc.with_ymd_and_hms(2022, 12, 4, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), None),
                    (Utc.with_ymd_and_hms(2022, 12, 6, 1, 0, 0).single().expect("wrong hardcoded datetime"), None, None),
                    (Utc.with_ymd_and_hms(2022, 12, 8, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, None),
                    (Utc.with_ymd_and_hms(2022, 12, 10, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), None),
                    (Utc.with_ymd_and_hms(2022, 12, 11, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), Some("https://twitch.tv/thesilvergauntlets")),
                    (Utc.with_ymd_and_hms(2022, 12, 12, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, None),
                    (Utc.with_ymd_and_hms(2022, 12, 15, 1, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://twitch.tv/thesilvergauntlets")),
                    (Utc.with_ymd_and_hms(2022, 12, 17, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), None),
                    (Utc.with_ymd_and_hms(2022, 12, 18, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), None),
                    (Utc.with_ymd_and_hms(2022, 12, 21, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, None),
                    (Utc.with_ymd_and_hms(2022, 12, 23, 3, 0, 0).single().expect("wrong hardcoded datetime"), None, None),
                ].into_iter().enumerate() {
                    let mut cal_event = ics::Event::new(format!("{}-{}-q{}@midos.house", event.series, event.event, i + 1), ics_datetime(Utc::now()));
                    cal_event.push(Summary::new(format!("S6 Qualifier {}{}", i + 1, if let Some(weekly) = weekly { format!(" ({weekly} Weekly)") } else { String::default() })));
                    cal_event.push(DtStart::new(ics_datetime(start)));
                    cal_event.push(DtEnd::new(ics_datetime(start + Duration::hours(4)))); //TODO get from race room; better duration estimate from past seasons
                    cal_event.push(URL::new(vod.unwrap_or("https://docs.google.com/document/d/1fyNO82G2D0Z7J9wobxEbjDjGnomTaIRdKgETGV_ufmc/edit")));
                    cal.add_event(cal_event);
                }
                //TODO bracket matches
            }
            _ => unimplemented!(),
        },
    }
    Ok(())
}

#[rocket::get("/calendar.ics")]
pub(crate) async fn index(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>) -> Result<Response<ICalendar<'static>>, Error> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for row in sqlx::query!(r#"SELECT series AS "series!: Series", event FROM events WHERE listed"#).fetch_all(&mut transaction).await? {
        let event = event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, http_client, startgg_token, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/series/<series>/calendar.ics")]
pub(crate) async fn for_series(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series) -> Result<Response<ICalendar<'static>>, Error> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for event in sqlx::query_scalar!(r#"SELECT event FROM events WHERE listed AND series = $1"#, series as _).fetch_all(&mut transaction).await? {
        let event = event::Data::new(&mut transaction, series, event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, http_client, startgg_token, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/event/<series>/<event>/calendar.ics")]
pub(crate) async fn for_event(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series, event: &str) -> Result<Response<ICalendar<'static>>, StatusOrError<Error>> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await.map_err(Error::Sql)?;
    let event = event::Data::new(&mut transaction, series, event).await.map_err(Error::Event)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    add_event_races(&mut transaction, http_client, startgg_token, &mut cal, &event).await?;
    transaction.commit().await.map_err(Error::Sql)?;
    Ok(Response(cal))
}
