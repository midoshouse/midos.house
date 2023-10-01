use {
    std::time::Duration,
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
    type Value = HashMap<T::Variables, T::ResponseData>;
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(from = "IdInner", into = "String")]
pub struct ID(pub(crate) String);

impl From<ID> for String {
    fn from(ID(s): ID) -> Self {
        s
    }
}

type String = std::string::String;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "assets/graphql/startgg-schema.json",
    query_path = "assets/graphql/startgg-set-query.graphql",
    skip_default_scalars, // workaround for https://github.com/smashgg/developer-portal/issues/171
    variables_derives = "Clone, PartialEq, Eq, Hash",
    response_derives = "Debug, Clone",
)]
pub(crate) struct SetQuery;

pub(crate) async fn query<T: GraphQLQuery + 'static>(client: &reqwest::Client, auth_token: &str, variables: T::Variables) -> Result<T::ResponseData, Error>
where T::Variables: Clone + Eq + Hash + Send + Sync, T::ResponseData: Clone + Send + Sync {
    let (ref mut next_request, ref mut cache) = *lock!(CACHE);
    Ok(match cache.entry::<QueryCache<T>>().or_default().entry(variables.clone()) {
        hash_map::Entry::Occupied(entry) => entry.get().clone(), //TODO expire cache after some amount of time?
        hash_map::Entry::Vacant(entry) => {
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
            let data = match (data, errors) {
                (Some(_), Some(errors)) if !errors.is_empty() => Err(Error::GraphQL(errors)),
                (Some(data), _) => Ok(data),
                (None, Some(errors)) => Err(Error::GraphQL(errors)),
                (None, None) => Err(Error::NoDataNoErrors),
            }?;
            entry.insert(data.clone());
            data
        }
    })
}
