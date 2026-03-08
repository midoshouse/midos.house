use crate::prelude::*;

pub(crate) struct GlobalState {
    pub(crate) clean_shutdown: Arc<Mutex<CleanShutdown>>,
    pub(crate) config: Config,
    pub(crate) db_pool: PgPool,
    /// Should not be accessed directly, use the [`discord_ctx!`] macro
    pub(crate) discord_ctx_: RwFuture<DiscordCtx>,
    pub(crate) extra_room_tx: Arc<RwLock<mpsc::Sender<String>>>,
    pub(crate) http_client: reqwest::Client,
    /// Locked while event rooms are being created. Wait with handling new rooms while it's held.
    pub(crate) new_room_lock: Arc<Mutex<()>>,
    pub(crate) ootr_api_client: Arc<ootr_web::ApiClient>,
    pub(crate) seed_cache_tx: watch::Sender<()>,
    pub(crate) seed_metadata: Arc<RwLock<HashMap<String, SeedMetadata>>>,
}

impl GlobalState {
    pub(crate) fn new(
        config: Config,
        db_pool: PgPool,
        discord_ctx_: RwFuture<DiscordCtx>,
        http_client: reqwest::Client,
        ootr_api_client: Arc<ootr_web::ApiClient>,
        seed_cache_tx: watch::Sender<()>,
        seed_metadata: Arc<RwLock<HashMap<String, SeedMetadata>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            clean_shutdown: Arc::default(),
            extra_room_tx: Arc::new(RwLock::new(mpsc::channel(1).0)),
            new_room_lock: Arc::default(),
            config, db_pool, discord_ctx_, http_client, seed_cache_tx, seed_metadata, ootr_api_client,
        })
    }

    #[cfg(test)]
    pub(crate) async fn dummy() -> sqlx::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            clean_shutdown: Arc::default(),
            config: Config::dummy(),
            db_pool: PgPool::connect_with(PgConnectOptions::default()
                .username("mido")
                .database("fados_house")
                .application_name("midos-house")
            ).await?,
            discord_ctx_: RwFuture::new(future::pending()),
            extra_room_tx: Arc::new(RwLock::new(mpsc::channel(1).0)),
            http_client: reqwest::Client::new(),
            new_room_lock: Arc::default(),
            ootr_api_client: Arc::new(ootr_web::ApiClient::new(reqwest::Client::new(), String::default(), String::default())),
            seed_cache_tx: watch::Sender::default(),
            seed_metadata: Arc::default(),
        }))
    }
}

#[async_trait]
impl<'r> FromRequest<'r> for &'r GlobalState {
    type Error = <&'r State<Arc<GlobalState>> as FromRequest<'r>>::Error;

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        request.guard::<&State<Arc<GlobalState>>>().await.map(|global| &***global)
    }
}

impl TypeMapKey for GlobalState {
    type Value = Arc<Self>;
}

macro_rules! discord_ctx {
    ($global:expr) => {
        &*$global.discord_ctx_.read().await
    };
}

pub(crate) use discord_ctx;
