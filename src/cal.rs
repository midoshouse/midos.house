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
    rocket::{
        State,
        uri,
    },
    rocket_util::Response,
    sqlx::PgPool,
    crate::event::{
        self,
        Series,
    },
};

fn ics_datetime<Tz: TimeZone>(datetime: DateTime<Tz>) -> String {
    datetime.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string()
}

#[rocket::get("/calendar.ics")]
pub(crate) async fn index(pool: &State<PgPool>) -> Result<Response<ICalendar<'static>>, event::DataError> {
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    let mut listed_events = sqlx::query!(r#"SELECT series AS "series!: Series", event FROM events WHERE listed"#).fetch(&**pool);
    while let Some(row) = listed_events.try_next().await? {
        let event = event::Data::new((**pool).clone(), row.series, row.event).await?.expect("event deleted during calendar load"); //TODO use a transaction to enforce consistency?
        match event.series {
            Series::Multiworld => {} //TODO add races from event
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
    Ok(Response(cal))
}
