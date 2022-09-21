use {
    graphql_client::GraphQLQuery,
    serde::{
        Deserialize,
        Serialize,
    },
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
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
#[derive(Clone, Deserialize, Serialize)]
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
)]
pub(crate) struct SetQuery;

pub(crate) async fn query<T: GraphQLQuery>(client: &reqwest::Client, auth_token: &str, variables: T::Variables) -> Result<T::ResponseData, Error> {
    let graphql_client::Response { data, errors, extensions: _ } = client.post("https://api.start.gg/gql/alpha")
        .bearer_auth(auth_token)
        .json(&T::build_query(variables))
        .send().await?
        .json().await?;
    match (data, errors) {
        (Some(_), Some(errors)) if !errors.is_empty() => Err(Error::GraphQL(errors)),
        (Some(data), _) => Ok(data),
        (None, Some(errors)) => Err(Error::GraphQL(errors)),
        (None, None) => Err(Error::NoDataNoErrors),
    }
}
