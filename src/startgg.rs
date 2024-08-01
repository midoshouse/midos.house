use {
    graphql_client::GraphQLQuery,
    typemap_rev::TypeMap,
    crate::prelude::*,
};

static CACHE: LazyLock<Mutex<(Instant, TypeMap)>> = LazyLock::new(|| Mutex::new((Instant::now(), TypeMap::default())));

struct QueryCache<T: GraphQLQuery> {
    _phantom: PhantomData<T>,
}

impl<T: GraphQLQuery + 'static> TypeMapKey for QueryCache<T>
where T::Variables: Send + Sync, T::ResponseData: Send + Sync {
    type Value = HashMap<T::Variables, (Instant, T::ResponseData)>;
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("{} GraphQL errors", .0.len())]
    GraphQL(Vec<graphql_client::Error>),
    #[error("GraphQL response returned neither `data` nor `errors`")]
    NoDataNoErrors,
}

impl IsNetworkError for Error {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Reqwest(e) => e.is_network_error(),
            Self::Wheel(e) => e.is_network_error(),
            Self::GraphQL(_) => false,
            Self::NoDataNoErrors => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IdInner {
    Number(serde_json::Number),
    String(String),
}

impl From<IdInner> for ID {
    fn from(inner: IdInner) -> Self {
        Self(match inner {
            IdInner::Number(n) => n.to_string(),
            IdInner::String(s) => s,
        })
    }
}

/// Workaround for <https://github.com/smashgg/developer-portal/issues/171>
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, sqlx::Type)]
#[serde(from = "IdInner", into = "String")]
#[sqlx(transparent)]
pub struct ID(pub(crate) String);

impl fmt::Display for ID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<ID> for String {
    fn from(ID(s): ID) -> Self {
        s
    }
}

type Int = i64;
type String = std::string::String;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "assets/graphql/startgg-schema.json",
    query_path = "assets/graphql/startgg-current-user-query.graphql",
    skip_default_scalars, // workaround for https://github.com/smashgg/developer-portal/issues/171
    variables_derives = "Clone, PartialEq, Eq, Hash",
    response_derives = "Debug, Clone",
)]
pub(crate) struct CurrentUserQuery;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "assets/graphql/startgg-schema.json",
    query_path = "assets/graphql/startgg-solo-event-sets-query.graphql",
    skip_default_scalars, // workaround for https://github.com/smashgg/developer-portal/issues/171
    variables_derives = "Clone, PartialEq, Eq, Hash",
    response_derives = "Debug, Clone",
)]
pub(crate) struct SoloEventSetsQuery;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "assets/graphql/startgg-schema.json",
    query_path = "assets/graphql/startgg-team-event-sets-query.graphql",
    skip_default_scalars, // workaround for https://github.com/smashgg/developer-portal/issues/171
    variables_derives = "Clone, PartialEq, Eq, Hash",
    response_derives = "Debug, Clone",
)]
pub(crate) struct TeamEventSetsQuery;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "assets/graphql/startgg-schema.json",
    query_path = "assets/graphql/startgg-report-one-game-result-mutation.graphql",
    skip_default_scalars, // workaround for https://github.com/smashgg/developer-portal/issues/171
    variables_derives = "Clone, PartialEq, Eq, Hash",
    response_derives = "Debug, Clone",
)]
pub(crate) struct ReportOneGameResultMutation;

async fn query_inner<T: GraphQLQuery + 'static>(client: &reqwest::Client, auth_token: &str, variables: T::Variables, next_request: &mut Instant) -> Result<T::ResponseData, Error>
where T::Variables: Clone + Eq + Hash + Send + Sync, T::ResponseData: Clone + Send + Sync {
    sleep_until(*next_request).await;
    let graphql_client::Response { data, errors, extensions: _ } = client.post("https://api.start.gg/gql/alpha")
        .bearer_auth(auth_token)
        .json(&T::build_query(variables))
        .send().await?
        .detailed_error_for_status().await?
        .json_with_text_in_error::<graphql_client::Response<T::ResponseData>>().await?;
    // from https://dev.start.gg/docs/rate-limits
    // “You may not average more than 80 requests per 60 seconds.”
    *next_request = Instant::now() + Duration::from_millis(60_000 / 80);
    match (data, errors) {
        (Some(_), Some(errors)) if !errors.is_empty() => Err(Error::GraphQL(errors)),
        (Some(data), _) => Ok(data),
        (None, Some(errors)) => Err(Error::GraphQL(errors)),
        (None, None) => Err(Error::NoDataNoErrors),
    }
}

pub(crate) async fn query_uncached<T: GraphQLQuery + 'static>(client: &reqwest::Client, auth_token: &str, variables: T::Variables) -> Result<T::ResponseData, Error>
where T::Variables: Clone + Eq + Hash + Send + Sync, T::ResponseData: Clone + Send + Sync {
    lock!(cache = CACHE; {
        let (ref mut next_request, _) = *cache;
        query_inner::<T>(client, auth_token, variables, next_request).await
    })
}

pub(crate) async fn query_cached<T: GraphQLQuery + 'static>(client: &reqwest::Client, auth_token: &str, variables: T::Variables) -> Result<T::ResponseData, Error>
where T::Variables: Clone + Eq + Hash + Send + Sync, T::ResponseData: Clone + Send + Sync {
    lock!(cache = CACHE; {
        let (ref mut next_request, ref mut cache) = *cache;
        Ok(match cache.entry::<QueryCache<T>>().or_default().entry(variables.clone()) {
            hash_map::Entry::Occupied(mut entry) => {
                let (retrieved, entry) = entry.get_mut();
                if retrieved.elapsed() >= Duration::from_secs(5 * 60) {
                    *entry = query_inner::<T>(client, auth_token, variables, next_request).await?;
                    *retrieved = Instant::now();
                }
                entry.clone()
            }
            hash_map::Entry::Vacant(entry) => {
                let data = query_inner::<T>(client, auth_token, variables, next_request).await?;
                entry.insert((Instant::now(), data.clone()));
                data
            }
        })
    })
}

pub(crate) enum ImportSkipReason {
    Exists,
    Preview,
    Slots,
    Participants,
    SetGamesType,
}

impl fmt::Display for ImportSkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists => write!(f, "already exists"),
            Self::Preview => write!(f, "is a preview"),
            Self::Slots => write!(f, "no match on slots"),
            Self::Participants => write!(f, "no match on participants"),
            Self::SetGamesType => write!(f, "unknown games type"),
        }
    }
}

/// Returns:
///
/// * A list of races to import. Only one race for each match is imported, with the `game` field specifying the total number of games in the match.
///   The caller is expected to duplicate this race to get the different games of the match, and create a single scheduling thread for the match.
///   A `game` value of `None` should be treated like `Some(1)`.
/// * A list of start.gg set IDs that were not imported, along with the reasons they were skipped.
pub(crate) async fn races_to_import(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, event: &event::Data<'_>, event_slug: &str) -> Result<(Vec<Race>, Vec<(ID, ImportSkipReason)>), cal::Error> {
    async fn process_set(
        transaction: &mut Transaction<'_, Postgres>,
        http_client: &reqwest::Client,
        event: &event::Data<'_>,
        races: &mut Vec<Race>,
        startgg_event: &str,
        set: ID,
        phase: Option<String>,
        round: Option<String>,
        team1: Team,
        team2: Team,
        set_games_type: Option<i64>,
        total_games: Option<i64>,
        best_of: Option<i64>,
    ) -> Result<Option<ImportSkipReason>, cal::Error> {
        races.push(Race {
            id: Id::new(&mut *transaction).await?,
            series: event.series,
            event: event.event.to_string(),
            source: cal::Source::StartGG {
                event: startgg_event.to_owned(),
                set,
            },
            entrants: Entrants::Two([
                Entrant::MidosHouseTeam(team1.clone()),
                Entrant::MidosHouseTeam(team2.clone()),
            ]),
            game: match set_games_type {
                Some(1) => best_of.map(|best_of| best_of.try_into().expect("too many games")),
                Some(2) => total_games.map(|total_games| total_games.try_into().expect("too many games")),
                _ => return Ok(Some(ImportSkipReason::SetGamesType)),
            },
            scheduling_thread: None,
            schedule: RaceSchedule::Unscheduled,
            schedule_updated_at: None,
            draft: if let Some(draft_kind) = event.draft_kind() {
                Some(Draft::for_game1(&mut *transaction, http_client, draft_kind, event, phase.as_deref(), [&team1, &team2]).await?)
            } else {
                None
            },
            seed: seed::Data::default(),
            video_urls: HashMap::default(),
            restreamers: HashMap::default(),
            last_edited_by: None,
            last_edited_at: None,
            ignored: false,
            schedule_locked: false,
            phase, round,
        });
        Ok(None)
    }

    async fn process_page(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, event: &event::Data<'_>, event_slug: &str, page: i64, races: &mut Vec<Race>, skips: &mut Vec<(ID, ImportSkipReason)>) -> Result<i64, cal::Error> {
        let startgg_token = if env.is_dev() { &config.startgg_dev } else { &config.startgg_production };
        if let TeamConfig::Solo = event.team_config {
            let solo_event_sets_query::ResponseData {
                event: Some(solo_event_sets_query::SoloEventSetsQueryEvent {
                    sets: Some(solo_event_sets_query::SoloEventSetsQueryEventSets {
                        page_info: Some(solo_event_sets_query::SoloEventSetsQueryEventSetsPageInfo { total_pages: Some(total_pages) }),
                        nodes: Some(sets),
                    }),
                }),
            } = query_cached::<SoloEventSetsQuery>(http_client, startgg_token, solo_event_sets_query::Variables { event_slug: event_slug.to_owned(), page }).await? else { panic!("no match on query") };
            for set in sets.into_iter().filter_map(identity) {
                let solo_event_sets_query::SoloEventSetsQueryEventSetsNodes { id: Some(id), phase_group, full_round_text, slots: Some(slots), set_games_type, total_games, round } = set else { panic!("unexpected set format") };
                if id.0.starts_with("preview") {
                    skips.push((id, ImportSkipReason::Preview));
                } else if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM races WHERE startgg_set = $1) AS "exists!""#, id as _).fetch_one(&mut **transaction).await? {
                    skips.push((id, ImportSkipReason::Exists));
                } else if let [
                    Some(solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlots { entrant: Some(solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrant { participants: Some(ref p1) }) }),
                    Some(solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlots { entrant: Some(solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrant { participants: Some(ref p2) }) }),
                ] = *slots {
                    if let [Some(solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrantParticipants { id: Some(ref team1) })] = **p1 {
                        if let [Some(solo_event_sets_query::SoloEventSetsQueryEventSetsNodesSlotsEntrantParticipants { id: Some(ref team2) })] = **p2 {
                            let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or_else(|| cal::Error::UnknownTeamStartGG(team1.clone()))?;
                            let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or_else(|| cal::Error::UnknownTeamStartGG(team2.clone()))?;
                            let best_of = phase_group.as_ref()
                                .and_then(|solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroup { rounds, .. }| rounds.as_ref())
                                .and_then(|rounds| rounds.iter().filter_map(Option::as_ref).find(|solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroupRounds { number, .. }| *number == round))
                                .and_then(|solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroupRounds { best_of, .. }| *best_of);
                            let phase = phase_group
                                .and_then(|solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroup { phase, .. }| phase)
                                .and_then(|solo_event_sets_query::SoloEventSetsQueryEventSetsNodesPhaseGroupPhase { name }| name);
                            if let Some(reason) = process_set(&mut *transaction, http_client, event, races, event_slug, id.clone(), phase, full_round_text, team1, team2, set_games_type, total_games, best_of).await? {
                                skips.push((id, reason));
                            }
                        } else {
                            skips.push((id, ImportSkipReason::Participants));
                        }
                    } else {
                        skips.push((id, ImportSkipReason::Participants));
                    }
                } else {
                    skips.push((id, ImportSkipReason::Slots));
                }
            }
            Ok(total_pages)
        } else {
            let team_event_sets_query::ResponseData {
                event: Some(team_event_sets_query::TeamEventSetsQueryEvent {
                    sets: Some(team_event_sets_query::TeamEventSetsQueryEventSets {
                        page_info: Some(team_event_sets_query::TeamEventSetsQueryEventSetsPageInfo { total_pages: Some(total_pages) }),
                        nodes: Some(sets),
                    }),
                }),
            } = query_cached::<TeamEventSetsQuery>(http_client, startgg_token, team_event_sets_query::Variables { event_slug: event_slug.to_owned(), page }).await? else { panic!("no match on query") };
            for set in sets.into_iter().filter_map(identity) {
                let team_event_sets_query::TeamEventSetsQueryEventSetsNodes { id: Some(id), phase_group, full_round_text, slots: Some(slots), set_games_type, total_games, round } = set else { panic!("unexpected set format") };
                if id.0.starts_with("preview") {
                    skips.push((id, ImportSkipReason::Preview));
                } else if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM races WHERE startgg_set = $1) AS "exists!""#, id as _).fetch_one(&mut **transaction).await? {
                    skips.push((id, ImportSkipReason::Exists));
                } else if let [
                    Some(team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlots { entrant: Some(team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlotsEntrant { id: Some(ref team1) }) }),
                    Some(team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlots { entrant: Some(team_event_sets_query::TeamEventSetsQueryEventSetsNodesSlotsEntrant { id: Some(ref team2) }) }),
                ] = *slots {
                    let team1 = Team::from_startgg(&mut *transaction, team1).await?.ok_or_else(|| cal::Error::UnknownTeamStartGG(team1.clone()))?;
                    let team2 = Team::from_startgg(&mut *transaction, team2).await?.ok_or_else(|| cal::Error::UnknownTeamStartGG(team2.clone()))?;
                    let best_of = phase_group.as_ref()
                        .and_then(|team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroup { rounds, .. }| rounds.as_ref())
                        .and_then(|rounds| rounds.iter().filter_map(Option::as_ref).find(|team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroupRounds { number, .. }| *number == round))
                        .and_then(|team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroupRounds { best_of, .. }| *best_of);
                    let phase = phase_group
                        .and_then(|team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroup { phase, .. }| phase)
                        .and_then(|team_event_sets_query::TeamEventSetsQueryEventSetsNodesPhaseGroupPhase { name }| name);
                    if let Some(reason) = process_set(&mut *transaction, http_client, event, races, event_slug, id.clone(), phase, full_round_text, team1, team2, set_games_type, total_games, best_of).await? {
                        skips.push((id, reason));
                    }
                } else {
                    skips.push((id, ImportSkipReason::Slots));
                }
            }
            Ok(total_pages)
        }
    }

    let mut races = Vec::default();
    let mut skips = Vec::default();
    let total_pages = process_page(&mut *transaction, http_client, env, config, event, event_slug, 1, &mut races, &mut skips).await?;
    for page in 2..=total_pages {
        process_page(&mut *transaction, http_client, env, config, event, event_slug, page, &mut races, &mut skips).await?;
    }
    Ok((races, skips))
}
