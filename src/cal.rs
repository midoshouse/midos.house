use {
    std::{
        cmp::Ordering,
        iter,
    },
    chrono::{
        Duration,
        prelude::*,
    },
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
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
        types::Json,
    },
    url::Url,
    wheel::traits::ReqwestResponseExt as _,
    crate::{
        Environment,
        config::Config,
        discord_bot::Draft,
        event::{
            self,
            Series,
        },
        startgg,
        team::Team,
        util::StatusOrError,
    },
};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RaceKind {
    Normal,
    Async1,
    Async2,
}

pub(crate) struct Race {
    pub(crate) start: DateTime<Utc>,
    pub(crate) end: Option<DateTime<Utc>>,
    pub(crate) startgg_event: String,
    pub(crate) startgg_set: String,
    pub(crate) room: Option<Url>,
    pub(crate) team1: Team,
    pub(crate) team2: Team,
    pub(crate) phase: String,
    pub(crate) round: String,
    pub(crate) kind: RaceKind,
    pub(crate) draft: Option<Draft>,
}

impl Race {
    pub(crate) async fn new(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, startgg_set: String, draft: Option<Draft>, start: DateTime<Utc>, end: Option<DateTime<Utc>>, room: Option<Url>, kind: RaceKind) -> Result<Self, Error> {
        let end = if let Some(end) = end {
            Some(end)
        } else if let Some(ref room) = room {
            http_client.get(format!("{room}/data"))
                .send().await?
                .detailed_error_for_status().await?
                .json_with_text_in_error::<RaceData>().await?
                .ended_at
        } else {
            None
        };
        if let startgg::set_query::ResponseData {
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
            if let [
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team1)), on: _ }) }) }),
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team2)), on: _ }) }) }),
            ] = *slots {
                Ok(Self {
                    team1: Team::from_startgg(transaction, team1).await?.ok_or(Error::UnknownTeam)?,
                    team2: Team::from_startgg(transaction, team2).await?.ok_or(Error::UnknownTeam)?,
                    draft, start, end, startgg_event, startgg_set, room, phase, round, kind,
                })
            } else {
                Err(Error::Teams)
            }
        } else {
            Err(Error::Teams)
        }
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, series: Series, event: &str) -> Result<Vec<Self>, Error> {
        let mut races = Vec::default();
        for row in sqlx::query!(r#"SELECT startgg_set, draft_state AS "draft_state: Json<Draft>", start, async_start1, async_start2, end_time, async_end1, async_end2, room, async_room1, async_room2 FROM races WHERE series = $1 AND event = $2 AND (start IS NOT NULL OR async_start1 IS NOT NULL OR async_start2 IS NOT NULL)"#, series as _, event).fetch_all(&mut *transaction).await? {
            if let Some(start) = row.start {
                races.push(Self::new(&mut *transaction, http_client, startgg_token, row.startgg_set.clone(), row.draft_state.clone().map(|Json(draft)| draft), start, row.end_time, row.room.as_deref().map(Url::parse).transpose()?, RaceKind::Normal).await?);
            }
            if let Some(start) = row.async_start1 {
                races.push(Self::new(&mut *transaction, http_client, startgg_token, row.startgg_set.clone(), row.draft_state.clone().map(|Json(draft)| draft), start, row.async_end1, row.async_room1.as_deref().map(Url::parse).transpose()?, RaceKind::Async1).await?);
            }
            if let Some(start) = row.async_start2 {
                races.push(Self::new(&mut *transaction, http_client, startgg_token, row.startgg_set.clone(), row.draft_state.clone().map(|Json(draft)| draft), start, row.async_end2, row.async_room2.as_deref().map(Url::parse).transpose()?, RaceKind::Async2).await?);
            }
        }
        races.sort_unstable();
        Ok(races)
    }

    pub(crate) async fn from_room(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, room: Url) -> Result<Option<Self>, Error> {
        if let Some(row) = sqlx::query!(r#"SELECT startgg_set, draft_state AS "draft_state: Json<Draft>", start AS "start!", end_time FROM races WHERE room = $1 AND start IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self::new(&mut *transaction, http_client, startgg_token, row.startgg_set, row.draft_state.map(|Json(draft)| draft), row.start, row.end_time, Some(room), RaceKind::Normal).await?))
        }
        if let Some(row) = sqlx::query!(r#"SELECT startgg_set, draft_state AS "draft_state: Json<Draft>", async_start1 AS "async_start1!", async_end1 FROM races WHERE async_room1 = $1 AND async_start1 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self::new(&mut *transaction, http_client, startgg_token, row.startgg_set, row.draft_state.map(|Json(draft)| draft), row.async_start1, row.async_end1, Some(room), RaceKind::Async1).await?))
        }
        if let Some(row) = sqlx::query!(r#"SELECT startgg_set, draft_state AS "draft_state: Json<Draft>", async_start2 AS "async_start2!", async_end2 FROM races WHERE async_room2 = $1 AND async_start2 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self::new(&mut *transaction, http_client, startgg_token, row.startgg_set, row.draft_state.map(|Json(draft)| draft), row.async_start2, row.async_end2, Some(room), RaceKind::Async2).await?))
        }
        Ok(None)
    }

    pub(crate) fn startgg_set_url(&self) -> Result<Url, url::ParseError> {
        format!("https://start.gg/{}/set/{}", self.startgg_event, self.startgg_set).parse()
    }

    pub(crate) fn active_teams(&self) -> impl Iterator<Item = &Team> + Send {
        match self.kind {
            RaceKind::Normal => Box::new([&self.team1, &self.team2].into_iter()) as Box<dyn Iterator<Item = &Team> + Send>,
            RaceKind::Async1 => Box::new(iter::once(&self.team1)),
            RaceKind::Async2 => Box::new(iter::once(&self.team2)),
        }
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
        self.start.cmp(&other.start)
            .then_with(|| self.end.cmp(&other.end))
            .then_with(|| self.startgg_event.cmp(&other.startgg_event))
            .then_with(|| self.startgg_set.cmp(&other.startgg_set))
            .then_with(|| self.kind.cmp(&other.kind))
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] StartGG(#[from] startgg::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("wrong number of teams or missing data")]
    Teams,
    #[error("this start.gg team ID is not associated with a Mido's House team")]
    UnknownTeam,
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
                            } else if let Some(ref room_slug) = race.room {
                                cal_event.push(URL::new(format!("https://racetime.gg/ootr/{room_slug}"))); //TODO support misc category rooms
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
                    let mut cal_event = ics::Event::new(format!("{}-{}-{}{}@midos.house",
                        event.series,
                        event.event,
                        race.startgg_set,
                        match race.kind {
                            RaceKind::Normal => "",
                            RaceKind::Async1 => "-1",
                            RaceKind::Async2 => "-2",
                        },
                    ), ics_datetime(Utc::now()));
                    cal_event.push(Summary::new(match race.kind {
                        RaceKind::Normal => format!("MW S{} {} {}: {} vs {}", event.event, race.phase, race.round, race.team1, race.team2),
                        RaceKind::Async1 => format!("MW S{} {} {} (async): {} vs {}", event.event, race.phase, race.round, race.team1, race.team2),
                        RaceKind::Async2 => format!("MW S{} {} {} (async): {} vs {}", event.event, race.phase, race.round, race.team2, race.team1),
                    }));
                    cal_event.push(DtStart::new(ics_datetime(race.start)));
                    cal_event.push(DtEnd::new(ics_datetime(race.end.unwrap_or_else(|| race.start + Duration::hours(4))))); //TODO better fallback duration estimates depending on participants
                    cal_event.push(URL::new(if let Some(room) = race.room {
                        room.to_string()
                    } else {
                        race.startgg_set_url()?.to_string()
                    })); //TODO prefer restream URL if one exists
                    cal.add_event(cal_event);
                }
            }
        },
        Series::Pictionary => {
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
    Ok(Response(cal))
}

#[rocket::get("/event/<series>/<event>/calendar.ics")]
pub(crate) async fn for_event(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series, event: &str) -> Result<Response<ICalendar<'static>>, StatusOrError<Error>> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await.map_err(Error::Sql)?;
    let event = event::Data::new(&mut transaction, series, event).await.map_err(Error::Event)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    add_event_races(&mut transaction, http_client, startgg_token, &mut cal, &event).await?;
    Ok(Response(cal))
}
