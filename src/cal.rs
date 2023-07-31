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
    collect_mac::collect,
    enum_iterator::all,
    futures::stream::TryStreamExt as _,
    ics::{
        ICalendar,
        properties::{
            Description,
            DtEnd,
            DtStart,
            Summary,
            URL,
        },
    },
    itertools::Itertools as _,
    lazy_regex::regex_captures,
    log_lock::{
        Mutex,
        lock,
    },
    once_cell::sync::Lazy,
    ootr_utils::spoiler::{
        HashIcon,
        SpoilerLog,
    },
    racetime::model::RaceData,
    rand::prelude::*,
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
        ToHtml,
        html,
    },
    serde::Deserialize,
    serenity::{
        all::Context as DiscordCtx,
        model::prelude::*,
    },
    serenity_utils::RwFuture,
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
        traits::{
            IoResultExt as _,
            IsNetworkError,
            ReqwestResponseExt as _,
        },
    },
    yup_oauth2::{
        ServiceAccountAuthenticator,
        read_service_account_key,
    },
    crate::{
        Environment,
        auth,
        config::Config,
        discord_bot,
        draft::{
            self,
            Draft,
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
        lang::Language::{
            self,
            *,
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
            RedirectOrContent,
            StatusOrError,
            as_variant,
            form_field,
            format_datetime,
            full_form,
            io_error_from_reqwest,
        },
    },
};

#[derive(Clone)]
pub(crate) enum Entrant {
    MidosHouseTeam(Team),
    Discord(UserId),
    DiscordTwitch(UserId, String),
    Named(String),
    NamedWithTwitch(String, String),
}

impl Entrant {
    pub(crate) async fn name(&self, transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx) -> Result<Option<Cow<'_, str>>, discord_bot::Error> {
        Ok(match self {
            Self::MidosHouseTeam(team) => team.name(transaction).await?,
            Self::Discord(user_id) | Self::DiscordTwitch(user_id, _) => if let Some(user) = User::from_discord(&mut **transaction, *user_id).await? {
                Some(Cow::Owned(user.discord.unwrap().display_name))
            } else {
                let user = user_id.to_user(discord_ctx).await?;
                Some(Cow::Owned(user.global_name.unwrap_or(user.name)))
            },
            Self::Named(name) | Self::NamedWithTwitch(name, _) => Some(Cow::Borrowed(name)),
        })
    }

    pub(crate) async fn to_html(&self, transaction: &mut Transaction<'_, Postgres>, env: Environment, discord_ctx: &DiscordCtx, running_text: bool) -> Result<RawHtml<String>, discord_bot::Error> {
        Ok(match self {
            Self::MidosHouseTeam(team) => team.to_html(transaction, env, running_text).await?,
            Self::Discord(user_id) | Self::DiscordTwitch(user_id, _) => if let Some(user) = User::from_discord(&mut **transaction, *user_id).await? {
                html! {
                    a(href = format!("https://discord.com/users/{user_id}")) {
                        : user.discord.unwrap().display_name;
                    }
                }
            } else {
                let user = user_id.to_user(discord_ctx).await?;
                html! {
                    a(href = format!("https://discord.com/users/{user_id}")) {
                        : user.global_name.unwrap_or(user.name);
                    }
                }
            },
            Self::Named(name) | Self::NamedWithTwitch(name, _) => name.to_html(),
        })
    }
}

impl PartialEq for Entrant {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MidosHouseTeam(lhs), Self::MidosHouseTeam(rhs)) => lhs == rhs,
            (Self::Discord(lhs) | Self::DiscordTwitch(lhs, _), Self::Discord(rhs) | Self::DiscordTwitch(rhs, _)) => lhs == rhs,
            (Self::Named(lhs) | Self::NamedWithTwitch(lhs, _), Self::Named(rhs) | Self::NamedWithTwitch(rhs, _)) => lhs == rhs,
            (Self::MidosHouseTeam(_), Self::Discord(_) | Self::DiscordTwitch(_, _) | Self::Named(_) | Self::NamedWithTwitch(_, _)) |
            (Self::Discord(_) | Self::DiscordTwitch(_, _), Self::MidosHouseTeam(_) | Self::Named(_) | Self::NamedWithTwitch(_, _)) |
            (Self::Named(_) | Self::NamedWithTwitch(_, _), Self::MidosHouseTeam(_) | Self::Discord(_) | Self::DiscordTwitch(_, _)) => false,
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
    pub(crate) id: Id,
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
    pub(crate) seed: seed::Data,
    pub(crate) video_urls: HashMap<Language, Url>,
    pub(crate) restreamers: HashMap<Language, String>,
    pub(crate) ignored: bool,
    pub(crate) schedule_locked: bool,
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
            p1_discord AS "p1_discord: Id",
            p2_discord AS "p2_discord: Id",
            p1_twitch,
            p2_twitch,
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
            video_url_pt,
            restreamer_pt,
            ignored,
            schedule_locked
        FROM races WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
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
                WHERE id = $4", startgg_event, phase, round, id as _).execute(&mut **transaction).await?;
                (Some(startgg_event), Some(startgg_set), Some(phase), Some(round), Some(slots))
            } else {
                (None, None, row.phase, row.round, None)
            }
        } else {
            (None, None, row.phase, row.round, None)
        };
        let entrants = {
            let p1 = if let Some(team1) = row.team1 {
                Some(Entrant::MidosHouseTeam(Team::from_id(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?))
            } else if let Some(Id(p1_discord)) = row.p1_discord {
                Some(if let Some(p1_twitch) = row.p1_twitch { Entrant::DiscordTwitch(UserId::new(p1_discord), p1_twitch) } else { Entrant::Discord(UserId::new(p1_discord)) })
            } else if let Some(p1) = row.p1 {
                Some(if let Some(p1_twitch) = row.p1_twitch { Entrant::NamedWithTwitch(p1, p1_twitch) } else { Entrant::Named(p1) })
            } else {
                None
            };
            let p2 = if let Some(team2) = row.team2 {
                Some(Entrant::MidosHouseTeam(Team::from_id(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?))
            } else if let Some(Id(p2_discord)) = row.p2_discord {
                Some(if let Some(p2_twitch) = row.p2_twitch { Entrant::DiscordTwitch(UserId::new(p2_discord), p2_twitch) } else { Entrant::Discord(UserId::new(p2_discord)) })
            } else if let Some(p2) = row.p2 {
                Some(if let Some(p2_twitch) = row.p2_twitch { Entrant::NamedWithTwitch(p2, p2_twitch) } else { Entrant::Named(p2) })
            } else {
                None
            };
            let p3 = if let Some(p3) = row.p3 {
                Some(Entrant::Named(p3))
            } else {
                None
            };
            match [p1, p2, p3] {
                [Some(p1), Some(p2), Some(p3)] => Entrants::Three([p1, p2, p3]),
                [Some(p1), Some(p2), None] => Entrants::Two([p1, p2]),
                [Some(Entrant::Named(p1)), None, None] => Entrants::Named(p1),
                [None, None, None] => if let (Some(startgg_set), Some(slots)) = (&startgg_set, slots) {
                    if let [
                        Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team1)), on: _ }) }) }),
                        Some(startgg::set_query::SetQuerySetSlots { entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant { team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam { id: Some(startgg::ID(ref team2)), on: _ }) }) }),
                    ] = *slots {
                        let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or(Error::UnknownTeam)?;
                        let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or(Error::UnknownTeam)?;
                        sqlx::query!("UPDATE races SET team1 = $1 WHERE id = $2", team1.id as _, id as _).execute(&mut **transaction).await?;
                        sqlx::query!("UPDATE races SET team2 = $1 WHERE id = $2", team2.id as _, id as _).execute(&mut **transaction).await?;
                        Entrants::Two([Entrant::MidosHouseTeam(team1), Entrant::MidosHouseTeam(team2)])
                    } else {
                        return Err(Error::StartggTeams { startgg_set: startgg_set.clone() })
                    }
                } else if let (Some(total), Some(finished)) = (row.total, row.finished) {
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
            seed: seed::Data {
                file_hash: match (row.hash1, row.hash2, row.hash3, row.hash4, row.hash5) {
                    (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                    (None, None, None, None, None) => None,
                    _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
                },
                files: seed_files,
            },
            video_urls: {
                let mut video_urls = HashMap::default();
                if let Some(video_url_en) = row.video_url {
                    video_urls.insert(English, video_url_en.parse()?);
                }
                if let Some(video_url_fr) = row.video_url_fr {
                    video_urls.insert(French, video_url_fr.parse()?);
                }
                if let Some(video_url_pt) = row.video_url_pt {
                    video_urls.insert(Portuguese, video_url_pt.parse()?);
                }
                video_urls
            },
            restreamers: {
                let mut restreamers = HashMap::default();
                if let Some(restreamer_en) = row.restreamer {
                    restreamers.insert(English, restreamer_en);
                }
                if let Some(restreamer_fr) = row.restreamer_fr {
                    restreamers.insert(French, restreamer_fr);
                }
                if let Some(restreamer_pt) = row.restreamer_pt {
                    restreamers.insert(Portuguese, restreamer_pt);
                }
                restreamers
            },
            ignored: row.ignored,
            schedule_locked: row.schedule_locked,
            id, startgg_event, startgg_set, entrants, phase, round,
        })
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, event: &event::Data<'_>) -> Result<Vec<Self>, Error> {
        async fn add_or_update_race(transaction: &mut Transaction<'_, Postgres>, races: &mut Vec<Race>, mut race: Race) -> sqlx::Result<()> {
            if let Some(found_race) = races.iter_mut().find(|iter_race|
                iter_race.series == race.series
                && iter_race.event == race.event
                && iter_race.phase == race.phase
                && iter_race.round == race.round
                && iter_race.game == race.game
                && iter_race.entrants == race.entrants
                && !iter_race.schedule_locked
            ) {
                if !found_race.schedule.start_matches(&race.schedule) {
                    match race.schedule {
                        RaceSchedule::Unscheduled => {
                            found_race.schedule = RaceSchedule::Unscheduled;
                            sqlx::query!("UPDATE races SET start = NULL, async_start1 = NULL, async_start2 = NULL WHERE id = $1", found_race.id as _).execute(&mut **transaction).await?;
                        }
                        RaceSchedule::Live { start, .. } => {
                            match found_race.schedule {
                                RaceSchedule::Unscheduled => found_race.schedule = race.schedule,
                                RaceSchedule::Live { start: ref mut old_start, .. } => *old_start = start,
                                RaceSchedule::Async { .. } => unimplemented!("race listed as async in database was rescheduled as live"), //TODO
                            }
                            sqlx::query!("UPDATE races SET start = $1, async_start1 = NULL, async_start2 = NULL WHERE id = $2", start, found_race.id as _).execute(&mut **transaction).await?;
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
                            sqlx::query!("UPDATE races SET start = NULL, async_start1 = $1, async_start2 = $2 WHERE id = $3", start1, start2, found_race.id as _).execute(&mut **transaction).await?;
                        }
                    }
                }
            } else {
                // add race to database
                race.save(transaction).await?;
                races.push(race);
            }
            Ok(())
        }

        let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
        let mut races = Vec::default();
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE series = $1 AND event = $2"#, event.series as _, &event.event).fetch_all(&mut **transaction).await? {
            races.push(Self::from_id(&mut *transaction, http_client, startgg_token, id).await?);
        }
        match event.series {
            Series::CopaDoBrasil => match &*event.event {
                "1" => {} //TODO automate Challonge match source for top 8
                _ => unimplemented!(),
            },
            Series::League => match &*event.event {
                "4" => {} // added to database
                _ => unimplemented!(),
            },
            Series::MixedPools => match &*event.event {
                "1" => {} // added to database
                "2" => for row in sheet_values(http_client.clone(), "1nz43jWsDrTgsnMzdLdXI13l9J6b8xHx9Ycpp8PAv9E8", "Schedule!B2:F").await? {
                    if let [p1, p2, round, date_et, time_et] = &*row {
                        let id = Id::new(&mut *transaction, IdTable::Races).await?;
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
                            startgg_event: None,
                            startgg_set: None,
                            entrants: Entrants::Two([
                                Entrant::Named(p1.clone()),
                                Entrant::Named(p2.clone()),
                            ]),
                            phase: Some(phase),
                            round: Some(round),
                            game: None,
                            scheduling_thread: None,
                            schedule: RaceSchedule::Live {
                                start: America::New_York.datetime_from_str(&format!("{date_et} at {time_et}"), "%d.%m.%Y at %H:%M:%S").expect(&format!("failed to parse {date_et:?} at {time_et:?}")).with_timezone(&Utc),
                                end: None,
                                room: None,
                            },
                            draft: None,
                            seed: seed::Data::default(),
                            video_urls: HashMap::default(),
                            restreamers: HashMap::default(),
                            ignored: false,
                            schedule_locked: false,
                            id,
                        }).await?;
                    }
                },
                _ => unimplemented!(),
            },
            Series::Multiworld => match &*event.event {
                "1" => {} // no match data available
                "2" | "3" => {} // added to database
                _ => unimplemented!(),
            },
            Series::NineDaysOfSaws | Series::Pictionary => {
                let id = if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE series = $1 AND event = $2"#, event.series as _, &event.event).fetch_optional(&mut **transaction).await? {
                    id
                } else {
                    Id::new(&mut *transaction, IdTable::Races).await?
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
                    startgg_event: None,
                    startgg_set: None,
                    entrants: Entrants::Open,
                    phase: None,
                    round: None,
                    game: None,
                    scheduling_thread: None,
                    draft: None,
                    seed: seed::Data::default(), //TODO
                    video_urls: event.video_url.iter().map(|video_url| (English, video_url.clone())).collect(), //TODO sync between event and race? Video URL fields for other languages on event::Data?
                    restreamers: HashMap::default(),
                    ignored: false,
                    schedule_locked: false,
                    id, schedule,
                }).await?;
            }
            Series::Rsl => match &*event.event {
                "1" => {} // no match data available
                "2" | "3" | "4" | "5" => {} // added to database
                _ => unimplemented!(),
            },
            Series::SpeedGaming => match &*event.event {
                "2023onl" | "2023live" => {} //TODO sync with SGL restream schedule website, automate Challonge match source
                _ => unimplemented!(),
            },
            Series::Standard => match &*event.event {
                "6" => {} // added to database
                _ => unimplemented!(),
            },
            Series::TournoiFrancophone => match &*event.event {
                "3" => {} // added to database
                _ => unimplemented!(),
            },
            Series::TriforceBlitz => match &*event.event {
                "2" => {} // added to database
                _ => unimplemented!(),
            },
        }
        races.retain(|race| !race.ignored);
        races.sort_unstable();
        Ok(races)
    }

    pub(crate) async fn for_scheduling_channel(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, channel_id: ChannelId, game: Option<i16>) -> Result<Vec<Self>, Error> {
        let mut races = Vec::default();
        let rows = if let Some(game) = game {
            sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE scheduling_thread = $1 AND (start IS NULL OR start > NOW()) AND game = $2"#, i64::from(channel_id), game).fetch_all(&mut **transaction).await?
        } else {
            sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE scheduling_thread = $1 AND (start IS NULL OR start > NOW())"#, i64::from(channel_id)).fetch_all(&mut **transaction).await?
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

    pub(crate) async fn multistream_url(&self, transaction: &mut Transaction<'_, Postgres>, env: Environment, http_client: &reqwest::Client, event: &event::Data<'_>) -> Result<Option<Url>, Error> {
        async fn entrant_twitch_names<'a>(transaction: &mut Transaction<'_, Postgres>, env: Environment, http_client: &reqwest::Client, event: &event::Data<'_>, entrant: &'a Entrant) -> Result<Option<Vec<Cow<'a, str>>>, Error> {
            let mut channels = Vec::default();
            match entrant {
                Entrant::MidosHouseTeam(team) => for (member, role) in team.members_roles(&mut *transaction).await? {
                    if event.team_config().role_is_racing(role) {
                        if let Some(twitch_name) = member.racetime_user_data(env, http_client).await?.and_then(|racetime_user_data| racetime_user_data.twitch_name) {
                            channels.push(Cow::Owned(twitch_name));
                        } else {
                            return Ok(None)
                        }
                    }
                },
                Entrant::Named(_) => return Ok(None),
                Entrant::Discord(_) => return Ok(None), //TODO if this Discord account is associated with a Mido's House account, check racetime.gg for a Twitch username
                Entrant::NamedWithTwitch(_, twitch_name) | Entrant::DiscordTwitch(_, twitch_name) => channels.push(Cow::Borrowed(&**twitch_name)),
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
        let rows = sqlx::query!(r#"SELECT player AS "player: Id", video FROM race_player_videos WHERE race = $1"#, self.id as _).fetch_all(&mut **transaction).await?;
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
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM races WHERE id = $1) AS "exists!""#, self.id as _).fetch_one(&mut **transaction).await? {
            unimplemented!("updating existing races not yet implemented") //TODO
        } else {
            let ([team1, team2], [p1, p2, p3], [p1_discord, p2_discord], [p1_twitch, p2_twitch], [total, finished]) = match self.entrants {
                Entrants::Open => ([None; 2], [None; 3], [None; 2], [None; 2], [None; 2]),
                Entrants::Count { total, finished } => ([None; 2], [None; 3], [None; 2], [None; 2], [Some(total), Some(finished)]),
                Entrants::Named(ref entrants) => ([None; 2], [Some(entrants), None, None], [None; 2], [None; 2], [None; 2]),
                Entrants::Two([ref p1, ref p2]) => {
                    let (team1, p1, p1_discord, p1_twitch) = match p1 {
                        Entrant::MidosHouseTeam(team) => (Some(team.id), None, None, None),
                        Entrant::Discord(discord_id) => (None, None, Some(*discord_id), None),
                        Entrant::DiscordTwitch(discord_id, twitch) => (None, None, Some(*discord_id), Some(twitch)),
                        Entrant::Named(name) => (None, Some(name), None, None),
                        Entrant::NamedWithTwitch(name, twitch) => (None, Some(name), None, Some(twitch)),
                    };
                    let (team2, p2, p2_discord, p2_twitch) = match p2 {
                        Entrant::MidosHouseTeam(team) => (Some(team.id), None, None, None),
                        Entrant::Discord(discord_id) => (None, None, Some(*discord_id), None),
                        Entrant::DiscordTwitch(discord_id, twitch) => (None, None, Some(*discord_id), Some(twitch)),
                        Entrant::Named(name) => (None, Some(name), None, None),
                        Entrant::NamedWithTwitch(name, twitch) => (None, Some(name), None, Some(twitch)),
                    };
                    ([team1, team2], [p1, p2, None], [p1_discord, p2_discord], [p1_twitch, p2_twitch], [None; 2])
                }
                Entrants::Three([ref p1, ref p2, ref p3]) => {
                    (
                        [None; 2],
                        [Some(match p1 {
                            Entrant::Named(name) => name,
                            _ => unimplemented!(), //TODO
                        }),
                        Some(match p2 {
                            Entrant::Named(name) => name,
                            _ => unimplemented!(), //TODO
                        }),
                        Some(match p3 {
                            Entrant::Named(name) => name,
                            _ => unimplemented!(), //TODO
                        })],
                        [None; 2],
                        [None; 2],
                        [None; 2],
                    )
                }
            };
            let (start, async_start1, async_start2, end, async_end1, async_end2, room, async_room1, async_room2) = match self.schedule {
                RaceSchedule::Unscheduled => (None, None, None, None, None, None, None, None, None),
                RaceSchedule::Live { start, end, ref room } => (Some(start), None, None, end, None, None, room.as_ref(), None, None),
                RaceSchedule::Async { start1, start2, end1, end2, ref room1, ref room2 } => (None, start1, start2, None, end1, end2, None, room1.as_ref(), room2.as_ref()),
            };
            let (web_id, web_gen_time, file_stem, locked_spoiler_log_path, tfb_uuid) = match self.seed.files {
                Some(seed::Files::MidosHouse { ref file_stem, ref locked_spoiler_log_path }) => (None, None, Some(file_stem), locked_spoiler_log_path.as_ref(), None),
                Some(seed::Files::OotrWeb { id, gen_time, ref file_stem }) => (Some(id), Some(gen_time), Some(file_stem), None, None),
                Some(seed::Files::TriforceBlitz { uuid }) => (None, None, None, None, Some(uuid)),
                Some(seed::Files::TfbSotd { .. }) => unimplemented!("Triforce Blitz seed of the day not supported for official races"),
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
                phase,
                round,
                p3,
                startgg_event,
                scheduling_thread,
                total,
                finished,
                tfb_uuid,
                video_url_fr,
                restreamer,
                restreamer_fr,
                locked_spoiler_log_path,
                video_url_pt,
                restreamer_pt,
                p1_twitch,
                p2_twitch,
                p1_discord,
                p2_discord
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37, $38, $39, $40, $41, $42, $43, $44, $45, $46)",
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
                self.seed.file_hash.map(|[hash1, _, _, _, _]| hash1) as _,
                self.seed.file_hash.map(|[_, hash2, _, _, _]| hash2) as _,
                self.seed.file_hash.map(|[_, _, hash3, _, _]| hash3) as _,
                self.seed.file_hash.map(|[_, _, _, hash4, _]| hash4) as _,
                self.seed.file_hash.map(|[_, _, _, _, hash5]| hash5) as _,
                self.game,
                self.id as _,
                p1,
                p2,
                self.video_urls.get(&English).map(|url| url.to_string()),
                self.phase,
                self.round,
                p3,
                self.startgg_event,
                self.scheduling_thread.map(|id| i64::from(id)),
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
                p1_discord.map(|id| i64::from(id)),
                p2_discord.map(|id| i64::from(id)),
            ).execute(&mut **transaction).await?;
        }
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EventKind {
    Normal,
    Async1,
    Async2,
}

#[derive(Clone)]
pub(crate) struct Event {
    pub(crate) race: Race,
    pub(crate) kind: EventKind,
}

impl Event {
    pub(crate) async fn from_room(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str, room: Url) -> Result<Option<Self>, Error> {
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE room = $1 AND start IS NOT NULL"#, room.to_string()).fetch_optional(&mut **transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Normal,
            }))
        }
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE async_room1 = $1 AND async_start1 IS NOT NULL"#, room.to_string()).fetch_optional(&mut **transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async1,
            }))
        }
        if let Some(id) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE async_room2 = $1 AND async_start2 IS NOT NULL"#, room.to_string()).fetch_optional(&mut **transaction).await? {
            return Ok(Some(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async2,
            }))
        }
        Ok(None)
    }

    pub(crate) async fn rooms_to_open(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, startgg_token: &str) -> Result<Vec<Self>, Error> {
        let mut events = Vec::default();
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE room IS NULL AND start IS NOT NULL AND start > NOW() AND (start <= NOW() + TIME '00:30:00' OR (team1 IS NULL AND p1_discord IS NULL AND p1 IS NULL AND start <= NOW() + TIME '01:00:00'))"#).fetch_all(&mut **transaction).await? {
            events.push(Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Normal,
            })
        }
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE async_room1 IS NULL AND async_start1 IS NOT NULL AND async_start1 > NOW() AND (async_start1 <= NOW() + TIME '00:30:00' OR (team1 IS NULL AND p1_discord IS NULL AND p1 IS NULL AND async_start1 <= NOW() + TIME '01:00:00'))"#).fetch_all(&mut **transaction).await? {
            let event = Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async1,
            };
            if event.race.event(&mut *transaction).await?.team_config().is_racetime_team_format() { // racetime.gg doesn't support single-entrant races
                events.push(event);
            }
        }
        for id in sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM races WHERE async_room2 IS NULL AND async_start2 IS NOT NULL AND async_start2 > NOW() AND (async_start2 <= NOW() + TIME '00:30:00' OR (team1 IS NULL AND p1_discord IS NULL AND p1 IS NULL AND async_start2 <= NOW() + TIME '01:00:00'))"#).fetch_all(&mut **transaction).await? {
            let event = Self {
                race: Race::from_id(&mut *transaction, http_client, startgg_token, id).await?,
                kind: EventKind::Async2,
            };
            if event.race.event(&mut *transaction).await?.team_config().is_racetime_team_format() { // racetime.gg doesn't support single-entrant races
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
    #[error(transparent)] Discord(#[from] discord_bot::Error),
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

static SHEETS_CACHE: Lazy<Mutex<HashMap<(String, String), (Instant, Vec<Vec<String>>)>>> = Lazy::new(|| Mutex::default());

#[derive(Debug, thiserror::Error)]
pub(crate) enum SheetsError {
    #[error(transparent)] OAuth(#[from] yup_oauth2::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("empty token is not valid")]
    EmptyToken,
    #[error("OAuth token is expired")]
    TokenExpired,
}

impl IsNetworkError for SheetsError {
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

async fn sheet_values(http_client: reqwest::Client, sheet_id: &str, range: &str) -> Result<Vec<Vec<String>>, SheetsError> {
    #[derive(Deserialize)]
    struct ValueRange {
        values: Vec<Vec<String>>,
    }

    async fn sheet_values_uncached(http_client: &reqwest::Client, sheet_id: &str, range: &str) -> Result<Vec<Vec<String>>, SheetsError> {
        let gsuite_secret = read_service_account_key("assets/google-client-secret.json").await.at("assets/google-client-secret.json")?;
        let auth = ServiceAccountAuthenticator::builder(gsuite_secret)
            .build().await.at_unknown()?;
        let token = auth.token(&["https://www.googleapis.com/auth/spreadsheets"]).await?;
        if token.is_expired() { return Err(SheetsError::TokenExpired) }
        let Some(token) = token.token() else { return Err(SheetsError::EmptyToken) };
        if token.is_empty() { return Err(SheetsError::EmptyToken) }
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
    let mut cache = lock!(SHEETS_CACHE);
    Ok(match cache.entry(key) {
        hash_map::Entry::Occupied(mut entry) => {
            let (retrieved, values) = entry.get();
            if retrieved.elapsed() < UDuration::from_secs(5 * 60) {
                values.clone()
            } else {
                match sheet_values_uncached(&http_client, sheet_id, range).await {
                    Ok(values) => {
                        entry.insert((Instant::now(), values.clone()));
                        values
                    }
                    Err(e) if e.is_network_error() && retrieved.elapsed() < UDuration::from_secs(60 * 60) => values.clone(),
                    Err(e) => return Err(e),
                }
            }
        }
        hash_map::Entry::Vacant(entry) => {
            let values = sheet_values_uncached(&http_client, sheet_id, range).await?;
            entry.insert((Instant::now(), values.clone()));
            values
        }
    })
}

fn ics_datetime<Z: TimeZone>(datetime: DateTime<Z>) -> String {
    datetime.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string()
}

async fn add_event_races(transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, http_client: &reqwest::Client, env: Environment, config: &Config, cal: &mut ICalendar<'_>, event: &event::Data<'_>) -> Result<(), Error> {
    for race in Race::for_event(transaction, http_client, env, config, event).await?.into_iter() {
        for race_event in race.cal_events() {
            if let Some(start) = race_event.start() {
                let mut cal_event = ics::Event::new(format!("{}{}@midos.house",
                    race.id,
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
                let summary_prefix = match race.entrants {
                    Entrants::Open | Entrants::Count { .. } => summary_prefix,
                    Entrants::Named(ref entrants) => match race_event.kind {
                        EventKind::Normal => format!("{summary_prefix}: {entrants}"),
                        EventKind::Async1 | EventKind::Async2 => format!("{summary_prefix} (async): {entrants}"),
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
                    },
                    Entrants::Three([ref team1, ref team2, ref team3]) => match race_event.kind {
                        EventKind::Normal => format!(
                            "{summary_prefix}: {} vs {} vs {}",
                            team1.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team2.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                            team3.name(&mut *transaction, discord_ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
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
                    Series::MixedPools | Series::SpeedGaming => Duration::hours(3),
                    Series::CopaDoBrasil | Series::League | Series::NineDaysOfSaws | Series::Standard | Series::TournoiFrancophone => Duration::hours(3) + Duration::minutes(30),
                    Series::Multiworld | Series::Pictionary => Duration::hours(4),
                    Series::Rsl => Duration::hours(4) + Duration::minutes(30),
                })))); //TODO better fallback duration estimates depending on participants
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
            }
        }
    }
    Ok(())
}

#[rocket::get("/calendar.ics")]
pub(crate) async fn index(env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>) -> Result<Response<ICalendar<'static>>, Error> {
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for row in sqlx::query!(r#"SELECT series AS "series!: Series", event FROM events WHERE listed"#).fetch_all(&mut *transaction).await? {
        let event = event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, &*discord_ctx.read().await, http_client, **env, config, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/series/<series>/calendar.ics")]
pub(crate) async fn for_series(env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series) -> Result<Response<ICalendar<'static>>, Error> {
    let mut transaction = pool.begin().await?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    for event in sqlx::query_scalar!(r#"SELECT event FROM events WHERE listed AND series = $1"#, series as _).fetch_all(&mut *transaction).await? {
        let event = event::Data::new(&mut transaction, series, event).await?.expect("event deleted during calendar load");
        add_event_races(&mut transaction, &*discord_ctx.read().await, http_client, **env, config, &mut cal, &event).await?;
    }
    transaction.commit().await?;
    Ok(Response(cal))
}

#[rocket::get("/event/<series>/<event>/calendar.ics")]
pub(crate) async fn for_event(env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, series: Series, event: &str) -> Result<Response<ICalendar<'static>>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut cal = ICalendar::new("2.0", concat!("midos.house/", env!("CARGO_PKG_VERSION")));
    add_event_races(&mut transaction, &*discord_ctx.read().await, http_client, **env, config, &mut cal, &event).await?;
    transaction.commit().await?;
    Ok(Response(cal))
}

pub(crate) async fn create_race_form(mut transaction: Transaction<'_, Postgres>, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
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
                    a(href = uri!(auth::login(Some(uri!(create_race(event.series, &*event.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to create a race.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await, ..PageStyle::default() }, &format!("New Race — {}", event.display_name), html! {
        : header;
        h2 : "Create race";
        : form;
    }).await?)
}

#[rocket::get("/event/<series>/<event>/races/new")]
pub(crate) async fn create_race(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: String) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(create_race_form(transaction, **env, me, uri, csrf.as_ref(), event, Context::default()).await?))
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
            RedirectOrContent::Content(create_race_form(transaction, **env, Some(me), uri, csrf.as_ref(), event, form.context).await?)
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
            let draft = match event.draft_kind() {
                Some(draft::Kind::MultiworldS3) => unimplemented!(), //TODO
                Some(draft::Kind::TournoiFrancoS3) => {
                    let high_seed = *[team1.id, team2.id].choose(&mut thread_rng()).unwrap();
                    Some(Draft {
                        went_first: None,
                        skipped_bans: 0,
                        settings: {
                            let team_rows = sqlx::query!("SELECT hard_settings_ok, mq_ok FROM teams WHERE id = $1 OR id = $2", team1.id as _, team2.id as _).fetch_all(&mut *transaction).await?;
                            let hard_settings_ok = team_rows.iter().all(|row| row.hard_settings_ok);
                            let mq_ok = team_rows.iter().all(|row| row.mq_ok);
                            collect![as HashMap<_, _>:
                                Cow::Borrowed("hard_settings_ok") => Cow::Borrowed(if hard_settings_ok { "ok" } else { "no" }),
                                Cow::Borrowed("mq_ok") => Cow::Borrowed(if mq_ok { "ok" } else { "no" }),
                            ]
                        },
                        high_seed,
                    })
                }
                None => None,
            };
            for game in 1..=value.game_count {
                let mut race = Race {
                    id: Id::new(&mut transaction, IdTable::Races).await?,
                    series: event.series,
                    event: event.event.to_string(),
                    startgg_event: None,
                    startgg_set: None,
                    entrants: Entrants::Two([
                        Entrant::MidosHouseTeam(team1.clone()),
                        Entrant::MidosHouseTeam(team2.clone()),
                    ]),
                    phase: phase.clone(),
                    round: round.clone(),
                    game: (value.game_count > 1).then_some(game),
                    scheduling_thread: None,
                    schedule: RaceSchedule::Unscheduled,
                    draft: draft.clone(),
                    seed: seed::Data::default(),
                    video_urls: HashMap::default(),
                    restreamers: HashMap::default(),
                    ignored: false,
                    schedule_locked: false,
                };
                if game == 1 {
                    discord_bot::create_scheduling_thread(&*discord_ctx.read().await, &mut transaction, &mut race, value.game_count).await?;
                }
                race.save(&mut transaction).await?;
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(event::races(event.series, &*event.event))))
        }
    } else {
        RedirectOrContent::Content(create_race_form(transaction, **env, Some(me), uri, csrf.as_ref(), event, form.context).await?)
    })
}

pub(crate) async fn edit_race_form(mut transaction: Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: event::Data<'_>, race: Race, ctx: Option<Context<'_>>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, env, me.as_ref(), Tab::Races, true).await?;
    let fenhl = User::from_id(&mut *transaction, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
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
            @for language in all::<Language>() {
                @let field_name = format!("video_urls.{}", language.short_code());
                : form_field(&field_name, &mut errors, html! {
                    label(for = &field_name) {
                        : language;
                        : " restream URL:";
                    }
                    input(type = "text", name = &field_name, value? = if let Some(ref ctx) = ctx {
                        ctx.field_value(&*field_name).map(|room| room.to_string())
                    } else {
                        race.video_urls.get(&language).map(|video_url| video_url.to_string())
                    });
                    label(class = "help") : "Please use the first available out of the following: Permanent Twitch highlight, YouTube or other video, Twitch past broadcast, Twitch channel.";
                });
                @let field_name = format!("restreamers.{}", language.short_code());
                : form_field(&field_name, &mut errors, html! {
                    label(for = &field_name) {
                        : language;
                        : " restreamer:";
                    }
                    input(type = "text", name = &field_name, value? = if let Some(ref ctx) = ctx {
                        ctx.field_value(&*field_name)
                    } else if me.as_ref().and_then(|me| me.racetime.as_ref()).map_or(false, |racetime| race.restreamers.get(&language).map_or(false, |restreamer| *restreamer == racetime.id)) {
                        Some("me")
                    } else {
                        race.restreamers.get(&language).map(|restreamer| restreamer.as_str()) //TODO display as racetime.gg profile URL
                    });
                    label(class = "help") : "(racetime.gg profile URL, racetime.gg user ID, or Mido's House user ID. Enter “me” to assign yourself.)";
                });
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
                    li : p1.to_html(&mut transaction, env, discord_ctx, false).await?;
                    li : p2.to_html(&mut transaction, env, discord_ctx, false).await?;
                }
            }
            Entrants::Three([p1, p2, p3]) => {
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
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await, ..PageStyle::default() }, &format!("Edit Race — {}", event.display_name), content).await?)
}

#[rocket::get("/event/<series>/<event>/races/<id>/edit")]
pub(crate) async fn edit_race(env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
    let mut transaction = pool.begin().await?;
    let event = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let race = Race::from_id(&mut transaction, http_client, startgg_token, id).await?;
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
    #[field(default = HashMap::new())]
    video_urls: HashMap<Language, String>,
    #[field(default = HashMap::new())]
    restreamers: HashMap<Language, String>,
}

#[rocket::post("/event/<series>/<event>/races/<id>/edit", data = "<form>")]
pub(crate) async fn edit_race_post(discord_ctx: &State<RwFuture<DiscordCtx>>, env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, id: Id, form: Form<Contextual<'_, EditRaceForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
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
            FROM rsl_seeds WHERE room = $1"#, room.to_string()).fetch_optional(&mut *transaction).await? {
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
                                if let Some((_, hash1, hash2, hash3, hash4, hash5, info_file_stem)) = regex_captures!("^([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+)\nhttps://midos\\.house/seed/([0-9A-Za-z_-]+)(?:\\.zpfz?)?$", &info_bot) {
                                    let Some(hash1) = HashIcon::from_racetime_emoji(hash1) else { continue };
                                    let Some(hash2) = HashIcon::from_racetime_emoji(hash2) else { continue };
                                    let Some(hash3) = HashIcon::from_racetime_emoji(hash3) else { continue };
                                    let Some(hash4) = HashIcon::from_racetime_emoji(hash4) else { continue };
                                    let Some(hash5) = HashIcon::from_racetime_emoji(hash5) else { continue };
                                    file_hash = Some([hash1, hash2, hash3, hash4, hash5]);
                                    file_stem = Some(info_file_stem.to_owned());
                                    break
                                } else if let Some((_, hash1, hash2, hash3, hash4, hash5, web_id_str)) = regex_captures!("^([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+) ([^ ]+)\nhttps://ootrandomizer\\.com/seed/get\\?id=([0-9]+)$", &info_bot) {
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
                    video_url = $4,
                    restreamer = $5,
                    video_url_fr = $6,
                    restreamer_fr = $7,
                    video_url_pt = $8,
                    restreamer_pt = $9,
                    last_edited_by = $10,
                    last_edited_at = NOW()
                WHERE id = $11",
                (!value.room.is_empty()).then(|| &value.room),
                (!value.async_room1.is_empty()).then(|| &value.async_room1),
                (!value.async_room2.is_empty()).then(|| &value.async_room2),
                value.video_urls.get(&English).filter(|video_url| !video_url.is_empty()),
                restreamers.get(&English),
                value.video_urls.get(&French).filter(|video_url| !video_url.is_empty()),
                restreamers.get(&French),
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
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await, ..PageStyle::default() }, &format!("Edit Race — {}", event.display_name), content).await?)
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
