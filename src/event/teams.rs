use {
    std::time::Duration,
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
    crate::{
        Environment,
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
        team::Team,
        user::User,
        util::{
            Id,
            StatusOrError,
            decode_pginterval,
        },
    },
};

struct SignupsMember {
    role: Role,
    user: User,
    is_confirmed: bool,
    qualifier_time: Option<Duration>,
    qualifier_vod: Option<String>,
}

pub(crate) struct SignupsTeam {
    pub(crate) team: Team,
    members: Vec<SignupsMember>,
    qualified: bool,
    pieces: Option<i16>,
    hard_settings_ok: bool,
    mq_ok: bool,
}

pub(crate) async fn signups_sorted(data: &Data<'_>, transaction: &mut Transaction<'_, Postgres>, me: Option<&User>, show_qualifier_times: bool) -> Result<Vec<SignupsTeam>, DataError> {
    let teams = sqlx::query!(r#"SELECT id AS "id!: Id", name, racetime_slug, plural_name, submitted IS NOT NULL AS "qualified!", pieces, hard_settings_ok, mq_ok, restream_consent FROM teams LEFT OUTER JOIN async_teams ON (id = team) WHERE
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
                qualifier_time: row.time.map(decode_pginterval).transpose()?,
                qualifier_vod: row.vod,
                role, user, is_confirmed,
            });
        }
        signups.push(SignupsTeam {
            team: Team { id: team.id, name: team.name, racetime_slug: team.racetime_slug, plural_name: team.plural_name, restream_consent: team.restream_consent },
            qualified: team.qualified,
            pieces: team.pieces,
            hard_settings_ok: team.hard_settings_ok,
            mq_ok: team.mq_ok,
            members,
        });
    }
    if show_qualifier_times {
        signups.sort_unstable_by(|SignupsTeam { team: team1, members: members1, qualified: qualified1, pieces: pieces1, .. }, SignupsTeam { team: team2, members: members2, qualified: qualified2, pieces: pieces2, .. }| {
            #[derive(PartialEq, Eq, PartialOrd, Ord)]
            enum Qualification {
                Finished(Option<i16>, Duration),
                DidNotFinish,
                NotYetQualified,
            }

            impl Qualification {
                fn new(qualified: bool, pieces: Option<i16>, members: &[SignupsMember]) -> Self {
                    if qualified {
                        if let Some(time) = members.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)) {
                            Self::Finished(
                                pieces.map(|pieces| -pieces), // list teams with more pieces first
                                time,
                            )
                        } else {
                            Self::DidNotFinish
                        }
                    } else {
                        Self::NotYetQualified
                    }
                }
            }

            Qualification::new(*qualified1, *pieces1, members1).cmp(&Qualification::new(*qualified2, *pieces2, members2))
            .then_with(|| team1.cmp(team2))
        });
    } else {
        signups.sort_unstable_by(|SignupsTeam { team: team1, qualified: qualified1, .. }, SignupsTeam { team: team2, qualified: qualified2, .. }|
            qualified2.cmp(qualified1) // reversed to list qualified teams first
            .then_with(|| team1.cmp(team2))
        );
    }
    Ok(signups)
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
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
pub(crate) async fn get(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, **env, me.as_ref(), Tab::Teams, false).await?;
    let has_qualifier = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM asyncs WHERE series = $1 AND event = $2 AND kind = 'qualifier') AS "exists!""#, series as _, event).fetch_one(&mut *transaction).await?;
    let show_qualifier_times = data.show_qualifier_times && (
        sqlx::query_scalar!(r#"SELECT submitted IS NOT NULL AS "qualified!" FROM async_teams, team_members WHERE async_teams.team = team_members.team AND member = $1 AND kind = 'qualifier'"#, me.as_ref().map(|me| i64::from(me.id))).fetch_optional(&mut *transaction).await?.unwrap_or(false)
        || data.is_started(&mut transaction).await?
    );
    let show_restream_consent = if let Some(ref me) = me {
        data.organizers(&mut transaction).await?.contains(me) || data.restreamers(&mut transaction).await?.contains(me)
    } else {
        false
    };
    let roles = data.team_config().roles();
    let signups = signups_sorted(&data, &mut transaction, me.as_ref(), show_qualifier_times).await?;
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
                    @if has_qualifier {
                        @if show_qualifier_times {
                            @if series == Series::TriforceBlitz {
                                th : "Pieces Found";
                            }
                        } else {
                            th : "Qualified";
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
                            + if has_qualifier { if show_qualifier_times { if series == Series::TriforceBlitz { 1 } else { 0 } } else { 1 } } else { 0 }
                        ) {
                            i : "(no signups yet)";
                        }
                    }
                } else {
                    @for SignupsTeam { team, members, qualified, pieces, hard_settings_ok, mq_ok } in signups {
                        tr {
                            @if !matches!(data.team_config(), TeamConfig::Solo) {
                                td {
                                    : team.to_html(&mut transaction, **env, false).await?;
                                    @if show_qualifier_times && qualified {
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
                                    : user;
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
                                    @if show_qualifier_times && qualified {
                                        br;
                                        small {
                                            @let time = if let Some(time) = qualifier_time { English.format_duration(*time, false) } else { format!("DNF") }; //TODO include number of pieces found in Triforce Blitz
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
                            }
                            @if has_qualifier {
                                @if show_qualifier_times {
                                    @if series == Series::TriforceBlitz {
                                        td : pieces;
                                    }
                                } else {
                                    td {
                                        @if qualified {
                                            : "✓";
                                        }
                                    }
                                }
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
                                    @if team.restream_consent {
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
