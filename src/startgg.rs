use {
    graphql_client::GraphQLQuery,
    typemap_rev::TypeMap,
    crate::prelude::*,
};

static CACHE: Lazy<Mutex<(Instant, TypeMap)>> = Lazy::new(|| Mutex::new((Instant::now(), TypeMap::default())));

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
    query_path = "assets/graphql/startgg-set-query.graphql",
    skip_default_scalars, // workaround for https://github.com/smashgg/developer-portal/issues/171
    variables_derives = "Clone, PartialEq, Eq, Hash",
    response_derives = "Debug, Clone",
)]
pub(crate) struct SetQuery;

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
