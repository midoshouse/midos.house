use crate::prelude::*;

pub(crate) struct GlobalState {
    pub(crate) config: Config,
    pub(crate) db_pool: PgPool,
    /// Should not be accessed directly, use the [`discord_ctx!`] macro
    pub(crate) discord_ctx_: RwFuture<DiscordCtx>,
    pub(crate) http_client: reqwest::Client,
    pub(crate) seed_metadata: Arc<RwLock<HashMap<String, SeedMetadata>>>,
    pub(crate) ootr_api_client: Arc<ootr_web::ApiClient>,
}

impl GlobalState {
    pub(crate) fn new(config: Config, db_pool: PgPool, discord_ctx_: RwFuture<DiscordCtx>, http_client: reqwest::Client, seed_metadata: Arc<RwLock<HashMap<String, SeedMetadata>>>, ootr_api_client: Arc<ootr_web::ApiClient>) -> Arc<Self> {
        Arc::new(Self { config, db_pool, discord_ctx_, http_client, seed_metadata, ootr_api_client })
    }
}

#[async_trait]
impl<'r> FromRequest<'r> for &'r GlobalState {
    type Error = <&'r State<Arc<GlobalState>> as FromRequest<'r>>::Error;

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        request.guard::<&State<Arc<GlobalState>>>().await.map(|global| &***global)
    }
}

macro_rules! discord_ctx {
    ($global:expr) => {
        &*$global.discord_ctx_.read().await
    };
}

pub(crate) use discord_ctx;
