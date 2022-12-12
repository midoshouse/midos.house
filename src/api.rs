use {
    async_graphql::{
        *,
        http::{
            GraphQLPlaygroundConfig,
            playground_source,
        },
    },
    async_graphql_rocket::{
        GraphQLQuery,
        GraphQLRequest,
        GraphQLResponse,
    },
    enum_iterator::all,
    rocket::{
        State,
        response::content::RawHtml,
    },
    crate::racetime_bot,
};

type MidosHouseSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub(crate) struct Query;

#[Object] impl Query {
    async fn goal_names(&self) -> Vec<&'static str> {
        all::<racetime_bot::Goal>().filter(|goal| goal.is_custom()).map(|goal| goal.as_str()).collect()
    }
}

#[rocket::get("/api/v1/graphql?<query..>")]
pub(crate) async fn graphql_query(schema: &State<MidosHouseSchema>, query: GraphQLQuery) -> GraphQLResponse {
    query.execute(&**schema).await
}

#[rocket::post("/api/v1/graphql", data = "<request>", format = "application/json")]
pub(crate) async fn graphql_request(schema: &State<MidosHouseSchema>, request: GraphQLRequest) -> GraphQLResponse {
    request.execute(&**schema).await
}

#[rocket::get("/api/v1/graphql")]
pub(crate) fn graphql_playground() -> RawHtml<String> {
    RawHtml(playground_source(GraphQLPlaygroundConfig::new("/api/v1/graphql")))
}
