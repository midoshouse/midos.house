use {
    std::{
        cmp::Ordering::{
            self,
            *,
        },
        collections::HashMap,
        hash::{
            Hash,
            Hasher,
        },
        time::Duration,
    },
    itertools::Itertools as _,
    noisy_float::prelude::*,
    racetime::model::{
        RaceData,
        RaceStatusValue,
    },
    rocket::{
        State,
        http::Status,
        response::content::RawHtml,
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        Origin,
        html,
    },
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
    url::Url,
    wheel::traits::ReqwestResponseExt as _,
    crate::{
        Environment,
        cal::{
            self,
            Race,
        },
        config::Config,
        draft,
        event::{
            Data,
            DataError,
            Role,
            Series,
            SignupStatus,
            Tab,
            TeamConfig,
        },
        http::{
            PageError,
            PageStyle,
            page,
        },
        lang::Language::*,
        series::*,
        team::Team,
        user::User,
        util::{
            Id,
            StatusOrError,
            decode_pginterval,
        },
    },
};

#[derive(Clone, Copy)]
pub(crate) enum QualifierKind {
    None,
    Single {
        show_times: bool,
    },
    Multiple,
}

pub(crate) enum MemberUser {
    MidosHouse(User),
    RaceTime {
        id: String,
        url: String,
        name: String,
    },
}

impl PartialEq for MemberUser {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MidosHouse(user1), Self::MidosHouse(user2)) => user1.id == user2.id,
            (Self::MidosHouse(_), Self::RaceTime { .. }) | (Self::RaceTime { .. }, Self::MidosHouse(_)) => false,
            (Self::RaceTime { id: id1, .. }, Self::RaceTime { id: id2, .. }) => id1 == id2,
        }
    }
}

impl Eq for MemberUser {}

impl Hash for MemberUser {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::MidosHouse(user) => {
                false.hash(state);
                user.id.hash(state);
            }
            Self::RaceTime { id, .. } => {
                true.hash(state);
                id.hash(state);
            }
        }
    }
}

impl PartialOrd for MemberUser {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(match (self, other) {
            (Self::MidosHouse(user1), Self::MidosHouse(user2)) => user1.id.cmp(&user2.id),
            (Self::MidosHouse(_), Self::RaceTime { .. }) => Less,
            (Self::RaceTime { .. }, Self::MidosHouse(_)) => Greater,
            (Self::RaceTime { id: id1, .. }, Self::RaceTime { id: id2, .. }) => id1.cmp(id2),
        })
    }
}

impl Ord for MemberUser {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}

pub(crate) struct SignupsMember {
    role: Role,
    pub(crate) user: MemberUser,
    is_confirmed: bool,
    qualifier_time: Option<Duration>,
    qualifier_vod: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum Qualification {
    Single {
        qualified: bool,
    },
    TriforceBlitz {
        qualified: bool,
        pieces: i16,
    },
    Multiple {
        num_qualifiers: usize,
        score: R64,
    },
}

pub(crate) struct SignupsTeam {
    pub(crate) team: Option<Team>,
    pub(crate) members: Vec<SignupsMember>,
    pub(crate) qualification: Qualification,
    hard_settings_ok: bool,
    mq_ok: bool,
}

pub(crate) async fn signups_sorted(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, me: Option<&User>, data: &Data<'_>, qualifier_kind: QualifierKind) -> Result<Vec<SignupsTeam>, cal::Error> {
    let mut signups = if let QualifierKind::Multiple = qualifier_kind {
        let mut scores = HashMap::<_, Vec<_>>::default();
        for race in Race::for_event(transaction, http_client, env, config, data).await? {
            if race.phase.as_ref().map_or(true, |phase| phase != "Qualifier") { continue }
            let Ok(room) = race.rooms().exactly_one() else { continue };
            let room_data = http_client.get(format!("{room}/data"))
                .send().await?
                .detailed_error_for_status().await?
                .json_with_text_in_error::<RaceData>().await?;
            if room_data.status.value != RaceStatusValue::Finished { continue }
            let mut entrants = room_data.entrants;
            entrants.retain(|entrant| entrant.user.id != "yMewn83Vj3405Jv7"); // user was banned
            if race.id == Id(17171498007470059483) {
                entrants.retain(|entrant| entrant.user.id != "JrM6PoY6LQWRdm5v"); // result was annulled
            }
            entrants.sort_by_key(|entrant| (entrant.finish_time.is_none(), entrant.finish_time));
            let par_cutoff = if entrants.len() < 20 { 3 } else { 4 };
            let par_time = entrants[0..par_cutoff].iter().map(|entrant| entrant.finish_time.expect("not enough finishers to calculate par")).sum::<Duration>() / par_cutoff as u32;
            for entrant in entrants {
                scores.entry(MemberUser::RaceTime {
                    id: entrant.user.id,
                    url: entrant.user.url,
                    name: entrant.user.name,
                }).or_default().push(r64(if let Some(finish_time) = entrant.finish_time {
                    (100.0 * (2.0 - (finish_time.as_secs_f64() / par_time.as_secs_f64()))).clamp(10.0, 110.0)
                } else {
                    0.0
                }));
            }
        }
        scores.into_iter()
            .map(|(user, mut scores)| SignupsTeam {
                team: None, //TODO
                members: vec![SignupsMember {
                    role: Role::None,
                    is_confirmed: false, //TODO
                    qualifier_time: None,
                    qualifier_vod: None,
                    user,
                }],
                qualification: {
                    let num_qualifiers = scores.len();
                    scores.truncate(5); // only count the first 5 qualifiers chronologically
                    scores.sort();
                    if num_qualifiers >= 4 {
                        // remove best score
                        scores.pop();
                    }
                    if num_qualifiers >= 5 {
                        // remove worst score
                        scores.swap_remove(0);
                    }
                    Qualification::Multiple {
                        num_qualifiers,
                        score: scores.iter().copied().sum::<R64>() / r64(scores.len().max(3) as f64),
                    }
                },
                hard_settings_ok: false,
                mq_ok: false,
            })
            .collect()
    } else {
        let teams = sqlx::query!(r#"SELECT id AS "id!: Id", name, racetime_slug, plural_name, submitted IS NOT NULL AS "qualified!", pieces, hard_settings_ok, mq_ok, restream_consent, mw_impl AS "mw_impl: mw::Impl" FROM teams LEFT OUTER JOIN async_teams ON (id = team) WHERE
            series = $1
            AND event = $2
            AND NOT resigned
            AND (
                EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                OR NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            )
            AND (kind = 'qualifier' OR kind IS NULL)
        "#, data.series as _, &data.event, me.as_ref().map(|me| i64::from(me.id))).fetch_all(&mut **transaction).await?;
        let roles = data.team_config().roles();
        let mut signups = Vec::with_capacity(teams.len());
        for team in teams {
            let mut members = Vec::with_capacity(roles.len());
            for &(role, _) in roles {
                let row = sqlx::query!(r#"
                    SELECT member AS "id: Id", status AS "status: SignupStatus", time, vod
                    FROM team_members LEFT OUTER JOIN async_players ON (member = player AND series = $1 AND event = $2 AND kind = 'qualifier')
                    WHERE team = $3 AND role = $4
                "#, data.series as _, &data.event, team.id as _, role as _).fetch_one(&mut **transaction).await?;
                let is_confirmed = row.status.is_confirmed();
                let user = User::from_id(&mut **transaction, row.id).await?.ok_or(DataError::NonexistentUser)?;
                members.push(SignupsMember {
                    user: MemberUser::MidosHouse(user),
                    qualifier_time: row.time.map(decode_pginterval).transpose().map_err(DataError::PgInterval)?,
                    qualifier_vod: row.vod,
                    role, is_confirmed,
                });
            }
            signups.push(SignupsTeam {
                team: Some(Team {
                    id: team.id,
                    name: team.name,
                    racetime_slug: team.racetime_slug,
                    plural_name: team.plural_name,
                    restream_consent: team.restream_consent,
                    mw_impl: team.mw_impl,
                }),
                qualification: if let Some(pieces) = team.pieces {
                    Qualification::TriforceBlitz { qualified: team.qualified, pieces }
                } else {
                    Qualification::Single { qualified: team.qualified }
                },
                hard_settings_ok: team.hard_settings_ok,
                mq_ok: team.mq_ok,
                members,
            });
        }
        signups
    };
    signups.sort_unstable_by(|SignupsTeam { team: team1, members: members1, qualification: qualification1, .. }, SignupsTeam { team: team2, members: members2, qualification: qualification2, .. }| {
        match qualifier_kind {
            QualifierKind::None | QualifierKind::Single { show_times: false } => {
                let qualified1 = match qualification1 {
                    Qualification::Single { qualified } | Qualification::TriforceBlitz { qualified, .. } => qualified,
                    Qualification::Multiple { .. } => unreachable!("Qualification::Multiple in QualifierKind::{{None, Single}}"),
                };
                let qualified2 = match qualification2 {
                    Qualification::Single { qualified } | Qualification::TriforceBlitz { qualified, .. } => qualified,
                    Qualification::Multiple { .. } => unreachable!("Qualification::Multiple in QualifierKind::{{None, Single}}"),
                };
                qualified2.cmp(&qualified1) // reversed to list qualified teams first
                .then_with(|| team1.cmp(&team2))
            }
            QualifierKind::Single { show_times: true } => {
                #[derive(PartialEq, Eq, PartialOrd, Ord)]
                enum QualificationOrder {
                    Finished(Option<i16>, Duration),
                    DidNotFinish,
                    NotYetQualified,
                }

                impl QualificationOrder {
                    fn new(qualification: Qualification, members: &[SignupsMember]) -> Self {
                        match qualification {
                            Qualification::Single { qualified: false } | Qualification::TriforceBlitz { qualified: false, .. } => Self::NotYetQualified,
                            Qualification::Single { qualified: true } => if let Some(time) = members.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)) {
                                Self::Finished(None, time)
                            } else {
                                Self::DidNotFinish
                            },
                            Qualification::TriforceBlitz { qualified: true, pieces } => if let Some(time) = members.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)) {
                                Self::Finished(
                                    Some(-pieces), // list teams with more pieces first
                                    time,
                                )
                            } else {
                                Self::DidNotFinish
                            },
                            Qualification::Multiple { .. } => unreachable!("Qualification::Multiple in QualifierKind::Single"),
                        }
                    }
                }

                QualificationOrder::new(*qualification1, members1).cmp(&QualificationOrder::new(*qualification2, members2))
                .then_with(|| team1.cmp(&team2))
            }
            QualifierKind::Multiple => {
                let (num1, score1) = match *qualification1 {
                    Qualification::Multiple { num_qualifiers, score } => (num_qualifiers, score),
                    _ => unreachable!("QualifierKind::Multiple must use Qualification::Multiple"),
                };
                let (num2, score2) = match *qualification2 {
                    Qualification::Multiple { num_qualifiers, score } => (num_qualifiers, score),
                    _ => unreachable!("QualifierKind::Multiple must use Qualification::Multiple"),
                };
                num2.min(3).cmp(&num1.min(3)) // list racers closer to reaching the required number of qualifiers first
                .then_with(|| score2.cmp(&score1)) // list racers with higher scores first
                .then_with(|| members1.iter().map(|member| &member.user).cmp(members2.iter().map(|member| &member.user)))
            }
        }
    });
    Ok(signups)
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Cal(#[from] cal::Error),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Event(#[from] crate::event::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] PgInterval(#[from] crate::util::PgIntervalDecodeError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

impl<E: Into<Error>> From<E> for StatusOrError<Error> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::get("/event/<series>/<event>/teams")]
pub(crate) async fn get(pool: &State<PgPool>, http_client: &State<reqwest::Client>, env: &State<Environment>, config: &State<Config>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, **env, me.as_ref(), Tab::Teams, false).await?;
    let qualifier_kind = if data.series == Series::SpeedGaming && data.event == "2023onl" {
        QualifierKind::Multiple
    } else if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM asyncs WHERE series = $1 AND event = $2 AND kind = 'qualifier') AS "exists!""#, series as _, event).fetch_one(&mut *transaction).await? {
        QualifierKind::Single {
            show_times: data.show_qualifier_times && (
                sqlx::query_scalar!(r#"SELECT submitted IS NOT NULL AS "qualified!" FROM async_teams, team_members WHERE async_teams.team = team_members.team AND member = $1 AND kind = 'qualifier'"#, me.as_ref().map(|me| i64::from(me.id))).fetch_optional(&mut *transaction).await?.unwrap_or(false)
                || data.is_started(&mut transaction).await?
            ),
        }
    } else {
        QualifierKind::None
    };
    let show_restream_consent = if let Some(ref me) = me {
        data.organizers(&mut transaction).await?.contains(me) || data.restreamers(&mut transaction).await?.contains(me)
    } else {
        false
    };
    let roles = data.team_config().roles();
    let signups = signups_sorted(&mut transaction, http_client, **env, config, me.as_ref(), &data, qualifier_kind).await?;
    let mut footnotes = Vec::default();
    let teams_label = if let TeamConfig::Solo = data.team_config() { "Entrants" } else { "Teams" };
    let content = html! {
        : header;
        table {
            thead {
                tr {
                    @if !matches!(data.team_config(), TeamConfig::Solo) {
                        th : "Team Name";
                    }
                    @for &(role, display_name) in roles {
                        th(class = role.css_class()) : display_name;
                    }
                    @match qualifier_kind {
                        QualifierKind::None => {}
                        QualifierKind::Single { show_times: false } => th : "Qualified";
                        QualifierKind::Single { show_times: true } => @if series == Series::TriforceBlitz {
                            th : "Pieces Found";
                        }
                        QualifierKind::Multiple => {
                            th : "# Qualifiers";
                            th : "Qualifier Points";
                        }
                    }
                    @if let Some(draft::Kind::TournoiFrancoS3) = data.draft_kind() {
                        th : "Advanced Settings OK";
                        th : "MQ OK";
                    }
                    @if show_restream_consent {
                        th : "Restream Consent";
                    }
                }
            }
            tbody {
                @if signups.is_empty() {
                    tr {
                        td(colspan =
                            if let TeamConfig::Solo = data.team_config() { 0 } else { 1 } + roles.len()
                            + match qualifier_kind {
                                QualifierKind::None => 0,
                                QualifierKind::Single { show_times: false } => 1,
                                QualifierKind::Single { show_times: true } => if series == Series::TriforceBlitz { 1 } else { 0 },
                                QualifierKind::Multiple => 2,
                            }
                        ) {
                            i : "(no signups yet)";
                        }
                    }
                } else {
                    @for SignupsTeam { team, members, qualification, hard_settings_ok, mq_ok } in signups {
                        tr {
                            @if !matches!(data.team_config(), TeamConfig::Solo) {
                                td {
                                    @if let Some(ref team) = team {
                                        : team.to_html(&mut transaction, **env, false).await?;
                                    }
                                    @if let (QualifierKind::Single { show_times: true }, Qualification::Single { qualified: true } | Qualification::TriforceBlitz { qualified: true, .. }) = (qualifier_kind, qualification) {
                                        br;
                                        small {
                                            @if let Some(time) = members.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)) {
                                                : English.format_duration(time / u32::try_from(members.len()).expect("too many team members"), false);
                                            } else {
                                                : "DNF";
                                            }
                                        }
                                    }
                                }
                            }
                            @for SignupsMember { role, user, is_confirmed, qualifier_time, qualifier_vod } in &members {
                                td(class? = role.css_class()) {
                                    @match user {
                                        MemberUser::MidosHouse(user) => {
                                            : user;
                                            @if let Some(ref team) = team {
                                                @if *is_confirmed {
                                                    @if me.as_ref().map_or(false, |me| me == user) && members.iter().any(|member| !member.is_confirmed) {
                                                        : " ";
                                                        span(class = "button-row") {
                                                            form(action = uri!(crate::event::resign_post(series, event, team.id)).to_string(), method = "post") {
                                                                : csrf;
                                                                input(type = "submit", value = "Retract");
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    : " ";
                                                    @if me.as_ref().map_or(false, |me| me == user) {
                                                        span(class = "button-row") {
                                                            form(action = uri!(crate::event::confirm_signup(series, event, team.id)).to_string(), method = "post") {
                                                                : csrf;
                                                                input(type = "submit", value = "Accept");
                                                            }
                                                            form(action = uri!(crate::event::resign_post(series, event, team.id)).to_string(), method = "post") {
                                                                : csrf;
                                                                input(type = "submit", value = "Decline");
                                                            }
                                                            //TODO options to block sender or event
                                                        }
                                                    } else {
                                                        : "(unconfirmed)";
                                                    }
                                                }
                                            }
                                            @if let (QualifierKind::Single { show_times: true }, Qualification::Single { qualified: true } | Qualification::TriforceBlitz { qualified: true, .. }) = (qualifier_kind, qualification) {
                                                br;
                                                small {
                                                    @let time = if let Some(time) = qualifier_time { English.format_duration(*time, false) } else { format!("DNF") };
                                                    @if let Some(vod) = qualifier_vod {
                                                        @if let Some(Ok(vod_url)) = (!vod.contains(' ')).then(|| Url::parse(vod)) {
                                                            a(href = vod_url.to_string()) : time;
                                                        } else {
                                                            : time;
                                                            sup {
                                                                @let footnote_id = { footnotes.push(vod.clone()); footnotes.len() };
                                                                a(href = format!("#footnote{footnote_id}")) {
                                                                    : "[";
                                                                    : footnote_id;
                                                                    : "]";
                                                                }
                                                            };
                                                        }
                                                    } else {
                                                        : time;
                                                    }
                                                }
                                            }
                                        }
                                        MemberUser::RaceTime { url, name, .. } => a(href = format!("https://{}{url}", env.racetime_host())) : name;
                                    }
                                }
                            }
                            @match (qualifier_kind, qualification) {
                                (QualifierKind::None, _) | (QualifierKind::Single { show_times: true }, Qualification::Single { .. }) => {}
                                (QualifierKind::Single { show_times: false }, Qualification::Single { qualified } | Qualification::TriforceBlitz { qualified, .. }) => td {
                                    @if qualified {
                                        : "✓";
                                    }
                                }
                                (QualifierKind::Single { show_times: true }, Qualification::TriforceBlitz { pieces, .. }) => td : pieces;
                                (QualifierKind::Multiple, Qualification::Multiple { num_qualifiers, score }) => {
                                    td(style = "text-align: right;") : num_qualifiers;
                                    td(style = "text-align: right;") : format!("{score:.2}");
                                }
                                (_, _) => @unreachable
                            }
                            @if let Some(draft::Kind::TournoiFrancoS3) = data.draft_kind() {
                                td {
                                    @if hard_settings_ok {
                                        : "✓";
                                    }
                                }
                                td {
                                    @if mq_ok {
                                        : "✓";
                                    }
                                }
                            }
                            @if show_restream_consent {
                                td {
                                    @if team.map_or(false, |team| team.restream_consent) {
                                        : "✓";
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        @for (i, footnote) in footnotes.into_iter().enumerate() {
            p(id = format!("footnote{}", i + 1)) {
                : "[";
                : i + 1;
                : "]";
                @for word in footnote.split(' ') {
                    : " ";
                    @if let Ok(word_url) = Url::parse(word) {
                        a(href = word_url.to_string()) : word;
                    } else {
                        : word;
                    }
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("{teams_label} — {}", data.display_name), content).await?)
}
