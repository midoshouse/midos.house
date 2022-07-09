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
    rocket::{
        State,
        http::Status,
        uri,
    },
    rocket_util::Response,
    serde::Deserialize,
    sqlx::PgPool,
    url::Url,
    crate::{
        event::{
            self,
            Series,
        },
        util::StatusOrError,
    },
};

fn ics_datetime<Tz: TimeZone>(datetime: DateTime<Tz>) -> String {
    datetime.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string()
}

fn add_event_races(cal: &mut ICalendar<'_>, event: &event::Data<'_>) {
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
            _ => {} //TODO add races from event
        },
        Series::Pictionary => {
            let mut cal_event = ics::Event::new(format!("{}-{}@midos.house", event.series, event.event), ics_datetime(Utc::now()));
            cal_event.push(Summary::new(event.display_name.clone()));
            if let Some(start) = event.start {
                cal_event.push(DtStart::new(ics_datetime(start)));
                let end = event.end.unwrap_or_else(|| start + Duration::hours(4)); //TODO better duration estimates depending on format & participants
                cal_event.push(DtEnd::new(ics_datetime(end)));
            }
            cal_event.push(URL::new(uri!("https://midos.house", event::info(event.series, &*event.event)).to_string()));
            cal.add_event(cal_event);
        }
    }
}

#[rocket::get("/calendar.ics")]
pub(crate) async fn index(pool: &State<PgPool>) -> Result<Response<ICalendar<'static>>, event::DataError> {
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    let mut listed_events = sqlx::query!(r#"SELECT series AS "series!: Series", event FROM events WHERE listed"#).fetch(&**pool);
    while let Some(row) = listed_events.try_next().await? {
        let event = event::Data::new((**pool).clone(), row.series, row.event).await?.expect("event deleted during calendar load"); //TODO use a transaction to enforce consistency?
        add_event_races(&mut cal, &event);
    }
    Ok(Response(cal))
}

#[rocket::get("/event/<series>/<event>/calendar.ics")]
pub(crate) async fn for_event(pool: &State<PgPool>, series: Series, event: &str) -> Result<Response<ICalendar<'static>>, StatusOrError<event::DataError>> {
    let event = event::Data::new((**pool).clone(), series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    add_event_races(&mut cal, &event);
    Ok(Response(cal))
}
