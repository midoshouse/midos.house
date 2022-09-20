use {
    chrono::{
        Duration,
        prelude::*,
    },
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
    },
    url::Url,
    crate::{
        Environment,
        config::Config,
        event::{
            self,
            Series,
        },
        startgg,
        util::StatusOrError,
    },
};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RaceKind {
    Normal,
    Async1,
    Async2,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Race {
    pub(crate) start: DateTime<Utc>,
    pub(crate) end: Option<DateTime<Utc>>,
    pub(crate) startgg_event: String,
    pub(crate) startgg_set: String,
    pub(crate) room: Option<Url>,
    pub(crate) team1: String,
    pub(crate) team2: String,
    pub(crate) phase: String,
    pub(crate) round: String,
    pub(crate) kind: RaceKind,
}

impl Race {
    pub(crate) async fn new(http_client: &reqwest::Client, startgg_token: &str, startgg_set: String, start: DateTime<Utc>, room: Option<Url>, kind: RaceKind) -> Result<Self, Error> {
        let end = if let Some(ref room) = room {
            http_client.get(format!("{room}/data"))
                .send().await?
                .error_for_status()?
                .json::<RaceData>().await?
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
        } = startgg::query::<startgg::SetQuery>(http_client, startgg_token, startgg::set_query::Variables { set_id: startgg_set.clone() }).await? {
            if let [
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { name: Some(ref team1) }) }),
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { name: Some(ref team2) }) }),
            ] = *slots {
                Ok(Self {
                    team1: team1.clone(),
                    team2: team2.clone(),
                    start, end, startgg_event, startgg_set, room, phase, round, kind,
                })
            } else {
                Err(Error::Teams)
            }
        } else {
            Err(Error::Teams)
        }
    }

    pub(crate) fn startgg_set_url(&self) -> Result<Url, url::ParseError> {
        format!("https://start.gg/{}/set/{}", self.startgg_event, self.startgg_set).parse()
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] StartGG(#[from] startgg::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error("wrong number of teams or missing data")]
    Teams,
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
                let mut rows = sqlx::query!(r#"SELECT startgg_set, start, async_start1, async_start2, room, async_room1, async_room2 FROM races WHERE series = 'mw' AND event = $1 AND (start IS NOT NULL OR async_start1 IS NOT NULL OR async_start2 IS NOT NULL)"#, &event.event).fetch(transaction);
                let mut races = Vec::default();
                while let Some(row) = rows.try_next().await? {
                    if let Some(start) = row.start {
                        races.push(Race::new(http_client, startgg_token, row.startgg_set.clone(), start, row.room.as_deref().map(Url::parse).transpose()?, RaceKind::Normal).await?);
                    }
                    if let Some(start) = row.async_start1 {
                        races.push(Race::new(http_client, startgg_token, row.startgg_set.clone(), start, row.async_room1.as_deref().map(Url::parse).transpose()?, RaceKind::Async1).await?);
                    }
                    if let Some(start) = row.async_start2 {
                        races.push(Race::new(http_client, startgg_token, row.startgg_set.clone(), start, row.async_room2.as_deref().map(Url::parse).transpose()?, RaceKind::Async2).await?);
                    }
                }
                races.sort_unstable();
                for race in races {
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
