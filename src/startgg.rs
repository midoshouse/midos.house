use {
    std::{
        collections::hash_map::{
            self,
            HashMap,
        },
        hash::Hash,
        marker::PhantomData,
    },
    graphql_client::GraphQLQuery,
    once_cell::sync::Lazy,
    serde::{
        Deserialize,
        Serialize,
    },
    tokio::sync::Mutex,
    typemap_rev::{
        TypeMap,
        TypeMapKey,
    },
    wheel::traits::ReqwestResponseExt as _,
};

static CACHE: Lazy<Mutex<TypeMap>> = Lazy::new(Mutex::default);

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
#[derive(Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
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
    response_derives = "Clone",
)]
pub(crate) struct SetQuery;

pub(crate) async fn query<T: GraphQLQuery + 'static>(client: &reqwest::Client, auth_token: &str, variables: T::Variables) -> Result<T::ResponseData, Error>
where T::Variables: Clone + Eq + Hash + Send + Sync, T::ResponseData: Clone + Send + Sync {
    Ok(match CACHE.lock().await.entry::<QueryCache<T>>().or_default().entry(variables.clone()) {
        hash_map::Entry::Occupied(entry) => entry.get().clone(), //TODO expire cache after some amount of time?
        hash_map::Entry::Vacant(entry) => {
            let graphql_client::Response { data, errors, extensions: _ } = client.post("https://api.start.gg/gql/alpha")
                .bearer_auth(auth_token)
                .json(&T::build_query(variables))
                .send().await?
                .detailed_error_for_status().await?
                .json::<graphql_client::Response<T::ResponseData>>().await?;
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
