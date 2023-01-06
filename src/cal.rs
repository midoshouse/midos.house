use {
    std::{
        borrow::Cow,
        cmp::Ordering,
        convert::identity,
        fmt,
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
    lazy_regex::regex_captures,
    once_cell::sync::Lazy,
    racetime::model::RaceData,
    rocket::{
        State,
        http::Status,
        response::content::RawHtml,
        uri,
    },
    rocket_util::{
        Response,
        ToHtml as _,
    },
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
            as_variant,
        },
    },
};

#[derive(Clone)]
pub(crate) enum RaceTeam {
    MidosHouse(Team),
    Named(String),
}

impl RaceTeam {
    pub(crate) fn to_html(&self, running_text: bool) -> RawHtml<String> {
        match self {
            Self::MidosHouse(team) => team.to_html(running_text),
            Self::Named(name) => name.to_html(),
        }
    }
}

impl fmt::Display for RaceTeam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MidosHouse(team) => team.fmt(f),
            Self::Named(name) => name.fmt(f),
        }
    }
}

#[derive(Clone)]
pub(crate) enum Participants {
    Open,
    Named(String),
    Two([RaceTeam; 2]),
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
    pub(crate) startgg_event: Option<String>,
    pub(crate) startgg_set: Option<String>,
    pub(crate) participants: Participants,
    pub(crate) phase: Option<String>,
    pub(crate) round: Option<String>,
    pub(crate) game: Option<i16>,
    pub(crate) schedule: RaceSchedule,
    pub(crate) draft: Option<Draft>,
    pub(crate) seed: Option<seed::Data>,
    pub(crate) video_url: Option<Url>,
}

impl Race {
    pub(crate) async fn from_startgg(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, startgg_set: String, game: Option<i16>) -> Result<Self, Error> {
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
            FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, &startgg_set, game).fetch_one(&mut *transaction).await?;
            let teams = if let [Some(team1), Some(team2)] = [row.team1, row.team2] {
                [
                    Team::from_id(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?,
                    Team::from_id(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?,
                ]
            } else if let [
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team1)), on: _ }) }) }),
                Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team2)), on: _ }) }) }),
            ] = **slots {
                let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?;
                let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?;
                sqlx::query!("UPDATE races SET team1 = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", team1.id as _, &startgg_set, game).execute(&mut *transaction).await?;
                sqlx::query!("UPDATE races SET team2 = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", team2.id as _, &startgg_set, game).execute(&mut *transaction).await?;
                [team1, team2]
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
                            sqlx::query!($query, end, &startgg_set, game).execute(&mut *transaction).await?;
                        }
                        end
                    } else {
                        None
                    };
                };
            }

            update_end!(end_time, room, "UPDATE races SET end_time = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3");
            update_end!(async_end1, async_room1, "UPDATE races SET async_end1 = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3");
            update_end!(async_end2, async_room2, "UPDATE races SET async_end2 = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3");
            Ok(Self {
                series: row.series,
                event: row.event,
                startgg_event: Some(startgg_event.clone()),
                startgg_set: Some(startgg_set),
                participants: Participants::Two(teams.map(RaceTeam::MidosHouse)),
                phase: Some(phase.clone()),
                round: Some(round.clone()),
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
                video_url: None, //TODO
                game,
            })
        } else {
            Err(Error::Teams { startgg_set, response_data })
        }
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: &Environment, config: &Config, event: &event::Data<'_>) -> Result<Vec<Self>, Error> {
        let mut races = Vec::default();
        match event.series {
            Series::Multiworld => match &*event.event {
                "2" => {
                    #[derive(Deserialize)]
                    struct MwS2Race {
                        start: DateTime<Utc>,
                        end: DateTime<Utc>,
                        team1: String,
                        team2: String,
                        phase: String,
                        round: String,
                        game: Option<i16>,
                        #[serde(rename = "async")]
                        is_async: bool,
                        room: Option<Url>,
                        restream: Option<Url>,
                    }

                    static RACES: Lazy<Vec<Race>> = Lazy::new(||
                        serde_json::from_str::<Vec<MwS2Race>>(include_str!("../assets/event/mw/2.json")) //TODO merge async halves
                            .expect("failed to parse mw/2 race list")
                            .into_iter()
                            .map(|race| Race {
                                series: Series::Multiworld,
                                event: format!("2"),
                                startgg_event: None,
                                startgg_set: None,
                                participants: Participants::Two([
                                    RaceTeam::Named(race.team1),
                                    RaceTeam::Named(race.team2),
                                ]),
                                phase: Some(race.phase),
                                round: Some(race.round),
                                game: race.game,
                                schedule: if race.is_async {
                                    RaceSchedule::Async {
                                        start1: Some(race.start),
                                        start2: None,
                                        end1: Some(race.end),
                                        end2: None,
                                        room1: race.room,
                                        room2: None,
                                    }
                                } else {
                                    RaceSchedule::Live {
                                        start: race.start,
                                        end: Some(race.end),
                                        room: race.room,
                                    }
                                },
                                draft: None,
                                seed: None,
                                video_url: race.restream,
                            })
                            .collect()
                    );

                    for race in &*RACES {
                        races.push(race.clone());
                    }
                }
                _ => {
                    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
                    for row in sqlx::query!("SELECT startgg_set, game FROM races WHERE series = $1 AND event = $2", event.series as _, &event.event).fetch_all(&mut *transaction).await? {
                        races.push(Self::from_startgg(&mut *transaction, http_client, startgg_token, row.startgg_set, row.game).await?);
                    }
                }
            },
            Series::NineDaysOfSaws | Series::Pictionary => {
                races.push(Self {
                    series: event.series,
                    event: event.event.to_string(),
                    startgg_event: None,
                    startgg_set: None,
                    participants: Participants::Open,
                    phase: None,
                    round: None,
                    game: None,
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
                });
            }
            Series::Rsl => match &*event.event {
                "2" => for row in sheet_values("1TEb48hIarEXnsnGxJbq1Y4YiZxNSM1t1oBsXh_bM4LM", format!("Raw form data!B2:F")).await? {
                    if !row.is_empty() {
                        let mut row = row.into_iter().fuse();
                        let p1 = row.next().unwrap();
                        let p2 = row.next().unwrap();
                        let date_et = row.next().unwrap();
                        let time_et = row.next().unwrap();
                        let stream = row.next();
                        assert!(row.next().is_none());
                        let start = America::New_York.datetime_from_str(&format!("{date_et} at {time_et}"), "%-m/%-d/%-Y at %-I:%M:%S %p").expect(&format!("failed to parse {date_et:?} at {time_et:?}"));
                        races.push(Self {
                            series: event.series,
                            event: event.event.to_string(),
                            startgg_event: None,
                            startgg_set: None,
                            participants: Participants::Two([
                                RaceTeam::Named(p1),
                                RaceTeam::Named(p2),
                            ]),
                            //TODO add phases and round numbers from https://challonge.com/ymq48xum
                            phase: None,
                            round: None,
                            game: None,
                            schedule: RaceSchedule::Live {
                                start: start.with_timezone(&Utc),
                                end: None, //TODO get from RSLBot seed archive
                                room: None, //TODO get from RSLBot seed archive
                            },
                            draft: None,
                            seed: None, //TODO get from RSLBot seed archive
                            video_url: stream.map(|stream| Url::parse(&format!("https://{stream}"))).transpose()?,
                        });
                    }
                },
                "3" => for row in sheet_values("1475TTqezcSt-okMfQaG6Rf7AlJsqBx8c_rGDKs4oBYk", format!("Sign Ups!B2:I")).await? {
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
                        races.push(Self {
                            series: event.series,
                            event: event.event.to_string(),
                            startgg_event: None,
                            startgg_set: None,
                            participants: Participants::Two([
                                RaceTeam::Named(p1),
                                RaceTeam::Named(p2),
                            ]),
                            //TODO add phases and round numbers from https://challonge.com/RSL_S3
                            phase: None,
                            round: None,
                            game: None,
                            schedule: RaceSchedule::Live {
                                start: start.with_timezone(&Utc),
                                end: None, //TODO get from RSLBot seed archive
                                room: None, //TODO get from RSLBot seed archive
                            },
                            draft: None,
                            seed: None, //TODO get from RSLBot seed archive
                            video_url: stream.map(|stream| Url::parse(&format!("https://{stream}"))).transpose()?,
                        });
                    }
                },
                "4" => for row in sheet_values("1LRJ3oo_2AWGq8KpNNclRXOq4OW8O7LrHra7uY7oQQlA", format!("Form responses 1!B2:H")).await? {
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
                        let (phase, round, game) = match &*round {
                            "Quarter Final" => ("Top 8", format!("Quarterfinal"), None),
                            "Semi Final" => ("Top 8", format!("Semifinal"), None),
                            "Grand Final (game 1)" => ("Top 8", format!("Finals"), Some(1)),
                            "Grand Final (game 2)" => ("Top 8", format!("Finals"), Some(2)),
                            "Grand Final (game 3)" => ("Top 8", format!("Finals"), Some(3)),
                            _ => ("Swiss", format!("Round {round}"), None),
                        };
                        races.push(Self {
                            series: event.series,
                            event: event.event.to_string(),
                            startgg_event: None,
                            startgg_set: None,
                            participants: Participants::Two([
                                RaceTeam::Named(p1),
                                RaceTeam::Named(p2),
                            ]),
                            phase: Some(phase.to_owned()),
                            round: Some(round),
                            schedule: RaceSchedule::Live {
                                start: start.with_timezone(&Utc),
                                end: None, //TODO get from RSLBot seed archive
                                room: None, //TODO get from RSLBot seed archive
                            },
                            draft: None,
                            seed: None, //TODO get from RSLBot seed archive
                            video_url: stream.map(|stream| Url::parse(&format!("https://{stream}"))).transpose()?,
                            game,
                        });
                    }
                },
                "5" => for row in sheet_values("1nz7XtNxKFTq_6bfjlUmIq0fCXKkQfDC848YmVcbaoQw", format!("Form responses 1!B2:H")).await? {
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
                        races.push(Self {
                            series: event.series,
                            event: event.event.to_string(),
                            startgg_event: None,
                            startgg_set: None,
                            participants: Participants::Two([
                                RaceTeam::Named(p1),
                                RaceTeam::Named(p2),
                            ]),
                            phase: Some(format!("Swiss")), //TODO top 8 support
                            round: Some(format!("Round {round}")),
                            game: None,
                            schedule: RaceSchedule::Live {
                                start: start.with_timezone(&Utc),
                                end: None, //TODO get from RSLBot seed archive
                                room: None, //TODO get from RSLBot seed archive
                            },
                            draft: None,
                            seed: None, //TODO get from RSLBot seed archive
                            video_url: stream.map(|stream| Url::parse(&format!("https://twitch.tv/{stream}"))).transpose()?, //TODO vod links
                        });
                    }
                },
                _ => unimplemented!(),
            },
            Series::Standard => match &*event.event {
                "6" => {
                    // qualifiers
                    for (i, (start, weekly, room, vod)) in [
                        // source: https://docs.google.com/document/d/1fyNO82G2D0Z7J9wobxEbjDjGnomTaIRdKgETGV_ufmc/edit
                        (Utc.with_ymd_and_hms(2022, 11, 19, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), Some("https://racetime.gg/ootr/neutral-bongobongo-4042"), "https://twitch.tv/videos/1657562512"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 11, 20, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), Some("https://racetime.gg/ootr/trusty-volvagia-2022"), "https://twitch.tv/videos/1658095931"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 11, 23, 3, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://racetime.gg/ootr/chaotic-wolfos-5287"), "https://www.twitch.tv/videos/1660399051"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 11, 26, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), Some("https://racetime.gg/ootr/smart-darunia-4679"), "https://www.twitch.tv/videos/1663582442"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 11, 27, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), Some("https://racetime.gg/ootr/comic-sheik-2973"), "https://www.twitch.tv/videos/1664085065"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 11, 29, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, None, "https://twitch.tv/videos/1666092237"), //TODO room URL //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 2, 1, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://racetime.gg/ootr/dazzling-bigocto-7483"), "https://www.twitch.tv/videos/1667839721"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 3, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), Some("https://racetime.gg/ootr/secret-dampe-4738"), "https://www.twitch.tv/videos/1669607104"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 4, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), Some("https://racetime.gg/ootr/clumsy-mido-8938"), "https://www.twitch.tv/videos/1670131046"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 6, 1, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://racetime.gg/ootr/good-bigocto-9887"), "https://www.twitch.tv/videos/1671439689"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 8, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://racetime.gg/ootr/artful-barinade-9952"), "https://www.twitch.tv/videos/1673751509"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 10, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), Some("https://racetime.gg/ootr/trusty-ingo-2577"), "https://www.twitch.tv/videos/1675739280"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 11, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), None, "https://www.twitch.tv/videos/1676628321"), //TODO room URL //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 12, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://racetime.gg/ootr/sleepy-talon-9258"), "https://www.twitch.tv/videos/1677277961"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 15, 1, 0, 0).single().expect("wrong hardcoded datetime"), None, None, "https://www.twitch.tv/videos/1679558638"), //TODO room URL //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 17, 23, 0, 0).single().expect("wrong hardcoded datetime"), Some("NA"), Some("https://racetime.gg/ootr/trusty-wolfos-6723"), "https://www.twitch.tv/videos/1681883072"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 18, 14, 0, 0).single().expect("wrong hardcoded datetime"), Some("EU"), Some("https://racetime.gg/ootr/banzai-medigoron-2895"), "https://www.twitch.tv/videos/1682377804"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 21, 19, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://racetime.gg/ootr/overpowered-zora-1013"), "https://www.twitch.tv/videos/1685210852"), //TODO permanent highlight/YouTube upload //TODO seed info
                        (Utc.with_ymd_and_hms(2022, 12, 23, 3, 0, 0).single().expect("wrong hardcoded datetime"), None, Some("https://racetime.gg/ootr/sleepy-stalfos-1734"), "https://www.twitch.tv/videos/1686484000"), //TODO permanent highlight/YouTube upload //TODO seed info
                    ].into_iter().enumerate() {
                        races.push(Self {
                            series: event.series,
                            event: event.event.to_string(),
                            //TODO keep race IDs? (qN, cc)
                            startgg_event: None,
                            startgg_set: None,
                            participants: Participants::Open,
                            phase: Some(format!("Qualifier")),
                            round: Some(format!("{}{}", i + 1, if let Some(weekly) = weekly { format!(" ({weekly} Weekly)") } else { String::default() })),
                            game: None,
                            schedule: RaceSchedule::Live {
                                end: None, //TODO get from room
                                room: room.map(|room| Url::parse(room)).transpose()?,
                                start,
                            },
                            draft: None,
                            seed: None, //TODO
                            video_url: Some(Url::parse(vod)?),
                        });
                    }
                    // bracket matches
                    for row in sheet_values(&config.zsr_volunteer_signups, format!("Scheduled Races!B2:D")).await? {
                        if !row.is_empty() {
                            let mut row = row.into_iter().fuse();
                            let datetime_et = row.next().unwrap();
                            let matchup = row.next().unwrap();
                            let round = row.next().unwrap();
                            assert!(row.next().is_none());
                            let start = America::New_York.datetime_from_str(&datetime_et, "%d/%m/%Y %H:%M:%S").expect(&format!("failed to parse {datetime_et:?}"));
                            if start < America::New_York.with_ymd_and_hms(2022, 12, 28, 0, 0, 0).single().expect("wrong hardcoded datetime") { continue } //TODO also add an upper bound
                            races.push(Self {
                                series: event.series,
                                event: event.event.to_string(),
                                startgg_event: None,
                                startgg_set: None,
                                participants: if let Some((_, p1, p2)) = regex_captures!("^(.+) +vs?.? +(.+)$", &matchup) {
                                    Participants::Two([
                                        RaceTeam::Named(p1.to_owned()),
                                        RaceTeam::Named(p2.to_owned()),
                                    ])
                                } else {
                                    Participants::Named(matchup)
                                },
                                phase: None, // main bracket
                                round: Some(round),
                                game: None,
                                schedule: RaceSchedule::Live {
                                    start: start.with_timezone(&Utc),
                                    end: None,
                                    room: None,
                                },
                                draft: None,
                                seed: None,
                                video_url: None, //TODO
                            });
                        }
                    }
                    // Challenge Cup bracket matches
                    for row in sheet_values("1Hp0rg_bV1Ja6oPdFLomTWQmwNy7ivmLMZ1rrVC3gx0Q", format!("Submitted Matches!C2:K")).await? {
                        if !row.is_empty() {
                            let mut row = row.into_iter().fuse();
                            let group_round = row.next().unwrap();
                            let p1 = row.next().unwrap();
                            let p2 = row.next().unwrap();
                            let _p3 = row.next().unwrap();
                            let date_et = row.next().unwrap();
                            let time_et = row.next().unwrap();
                            let is_async = row.next().unwrap() == "Yes";
                            let _restream_ok = row.next().unwrap() == "Yes";
                            let is_cancelled = row.next().unwrap() == "TRUE";
                            if is_cancelled { continue }
                            let start = America::New_York.datetime_from_str(&format!("{date_et} at {time_et}"), "%-m/%-d/%-Y at %I:%M %p").expect(&format!("failed to parse {date_et:?} at {time_et:?}"));
                            races.push(Self {
                                series: event.series,
                                event: event.event.to_string(),
                                startgg_event: None,
                                startgg_set: None,
                                participants: Participants::Two([
                                    RaceTeam::Named(p1),
                                    RaceTeam::Named(p2),
                                ]),
                                phase: Some(format!("Challenge Cup")),
                                round: Some(group_round),
                                game: None,
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
                                video_url: None, //TODO
                            });
                        }
                    }
                }
                _ => unimplemented!(),
            },
        }
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
            .then_with(|| self.game.cmp(&other.game))
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
        if let Some(row) = sqlx::query!(r#"SELECT startgg_set, game FROM races WHERE room = $1 AND start IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_startgg(&mut *transaction, http_client, startgg_token, row.startgg_set, row.game).await?,
                kind: EventKind::Normal,
            }))
        }
        if let Some(row) = sqlx::query!(r#"SELECT startgg_set, game FROM races WHERE async_room1 = $1 AND async_start1 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_startgg(&mut *transaction, http_client, startgg_token, row.startgg_set, row.game).await?,
                kind: EventKind::Async1,
            }))
        }
        if let Some(row) = sqlx::query!(r#"SELECT startgg_set, game FROM races WHERE async_room2 = $1 AND async_start2 IS NOT NULL"#, room.to_string()).fetch_optional(&mut *transaction).await? {
            return Ok(Some(Self {
                race: Race::from_startgg(&mut *transaction, http_client, startgg_token, row.startgg_set, row.game).await?,
                kind: EventKind::Async2,
            }))
        }
        Ok(None)
    }

    pub(crate) fn active_teams(&self) -> impl Iterator<Item = &Team> + Send {
        match self.race.participants {
            Participants::Named(_) | Participants::Open => Box::new(iter::empty()) as Box<dyn Iterator<Item = &Team> + Send>,
            Participants::Two([ref team1, ref team2]) => Box::new([
                matches!(self.kind, EventKind::Normal | EventKind::Async1).then_some(team1),
                matches!(self.kind, EventKind::Normal | EventKind::Async2).then_some(team2),
            ].into_iter().filter_map(identity).filter_map(as_variant!(RaceTeam::MidosHouse))),
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

async fn add_event_races(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: &Environment, config: &Config, cal: &mut ICalendar<'_>, event: &event::Data<'_>) -> Result<(), Error> {
    for (i, race) in Race::for_event(transaction, http_client, env, config, event).await?.into_iter().enumerate() {
        for race_event in race.cal_events() {
            if let Some(start) = race_event.start() {
                let mut cal_event = ics::Event::new(format!("{}-{}-{}{}@midos.house",
                    event.series,
                    event.event,
                    race.startgg_set.clone().unwrap_or_else(|| i.to_string()), //TODO use ID systems for other events
                    match race_event.kind {
                        EventKind::Normal => "",
                        EventKind::Async1 => "-1",
                        EventKind::Async2 => "-2",
                    },
                ), ics_datetime(Utc::now()));
                let summary_prefix = match (&race.phase, &race.round) {
                    (Some(phase), Some(round)) => format!("{} {phase} {round}", event.short_name()),
                    (Some(phase), None) => format!("{} {phase}", event.short_name()),
                    (None, Some(round)) => format!("{} {round}", event.short_name()),
                    (None, None) => event.display_name.clone(),
                };
                cal_event.push(Summary::new(match race.participants {
                    Participants::Open => summary_prefix,
                    Participants::Named(ref participants) => match race_event.kind {
                        EventKind::Normal => format!("{summary_prefix}: {participants}"),
                        EventKind::Async1 | EventKind::Async2 => format!("{summary_prefix} (async): {participants}"),
                    },
                    Participants::Two([ref team1, ref team2]) => match race_event.kind {
                        EventKind::Normal => format!("{summary_prefix}: {team1} vs {team2}"),
                        EventKind::Async1 => format!("{summary_prefix} (async): {team1} vs {team2}"),
                        EventKind::Async2 => format!("{summary_prefix} (async): {team2} vs {team1}"),
                    },
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
