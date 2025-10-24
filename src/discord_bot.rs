use {
    serenity::all::{
        CacheHttp,
        Content,
        CreateAllowedMentions,
        CreateButton,
        CreateCommand,
        CreateCommandOption,
        CreateForumPost,
        CreateInteractionResponse,
        CreateInteractionResponseMessage,
        CreateMessage,
        CreateThread,
        EditRole,
    },
    serenity_utils::{
        builder::ErrorNotifier,
        handler::HandlerMethods as _,
    },
    sqlx::{
        Database,
        Decode,
        Encode,
        types::Json,
    },
    crate::{
        event::SchedulingBackend,
        prelude::*,
    },
};

pub(crate) const FENHL: UserId = UserId::new(86841168427495424);
const BUTTONS_PER_PAGE: usize = 25;

/// A wrapper around serenity's Discord snowflake types that can be stored in a PostgreSQL database as a BIGINT.
#[derive(Debug)]
pub(crate) struct PgSnowflake<T>(pub(crate) T);

impl<'r, T: From<NonZero<u64>>, DB: Database> Decode<'r, DB> for PgSnowflake<T>
where i64: Decode<'r, DB> {
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let id = i64::decode(value)?;
        let id = NonZero::try_from(id as u64)?;
        Ok(Self(id.into()))
    }
}

impl<'q, T: Copy + Into<i64>, DB: Database> Encode<'q, DB> for PgSnowflake<T>
where i64: Encode<'q, DB> {
    fn encode_by_ref(&self, buf: &mut <DB as Database>::ArgumentBuffer<'q>) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>> {
        self.0.into().encode(buf)
    }

    fn encode(self, buf: &mut <DB as Database>::ArgumentBuffer<'q>) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>> {
        self.0.into().encode(buf)
    }

    fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
        self.0.into().produces()
    }

    fn size_hint(&self) -> usize {
        Encode::size_hint(&self.0.into())
    }
}

impl<T, DB: Database> sqlx::Type<DB> for PgSnowflake<T>
where i64: sqlx::Type<DB> {
    fn type_info() -> <DB as Database>::TypeInfo {
        i64::type_info()
    }

    fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
        i64::compatible(ty)
    }
}

#[async_trait]
pub(crate) trait MessageBuilderExt {
    async fn mention_entrant(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, entrant: &Entrant) -> sqlx::Result<&mut Self>;
    async fn mention_team(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, team: &Team) -> sqlx::Result<&mut Self>;
    fn mention_user(&mut self, user: &User) -> &mut Self;
    fn push_emoji(&mut self, emoji: &ReactionType) -> &mut Self;
    fn push_named_link_no_preview(&mut self, name: impl Into<Content>, url: impl Into<Content>) -> &mut Self;
    fn push_named_link_safe_no_preview(&mut self, name: impl Into<Content>, url: impl Into<Content>) -> &mut Self;
}

#[async_trait]
impl MessageBuilderExt for MessageBuilder {
    async fn mention_entrant(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, entrant: &Entrant) -> sqlx::Result<&mut Self> {
        match entrant {
            Entrant::MidosHouseTeam(team) => { self.mention_team(transaction, guild, team).await?; }
            Entrant::Discord { id,  .. } => { self.mention(id); }
            Entrant::Named { name, .. } => { self.push_safe(name); }
        }
        Ok(self)
    }

    async fn mention_team(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, team: &Team) -> sqlx::Result<&mut Self> {
        if let Ok(member) = team.members(&mut *transaction).await?.into_iter().exactly_one() {
            self.mention_user(&member);
        } else {
            let team_role = if let (Some(guild), Some(racetime_slug)) = (guild, &team.racetime_slug) {
                sqlx::query_scalar!(r#"SELECT id AS "id: PgSnowflake<RoleId>" FROM discord_roles WHERE guild = $1 AND racetime_team = $2"#, PgSnowflake(guild) as _, racetime_slug).fetch_optional(&mut **transaction).await?
            } else {
                None
            };
            if let Some(PgSnowflake(team_role)) = team_role {
                self.role(team_role);
            } else if let Some(team_name) = team.name(transaction).await? {
                if let Some(ref racetime_slug) = team.racetime_slug {
                    self.push_named_link_safe_no_preview(team_name, format!("https://{}/team/{racetime_slug}", racetime_host()));
                } else {
                    self.push_italic_safe(team_name);
                }
            } else {
                if let Some(ref racetime_slug) = team.racetime_slug {
                    self.push_named_link_safe_no_preview("an unnamed team", format!("https://{}/team/{racetime_slug}", racetime_host()));
                } else {
                    self.push("an unnamed team");
                }
            }
        }
        Ok(self)
    }

    fn mention_user(&mut self, user: &User) -> &mut Self {
        if let Some(ref discord) = user.discord {
            self.mention(&discord.id)
        } else {
            self.push_safe(user.display_name())
        }
    }

    fn push_emoji(&mut self, emoji: &ReactionType) -> &mut Self {
        self.push(emoji.to_string())
    }

    fn push_named_link_no_preview(&mut self, name: impl Into<Content>, url: impl Into<Content>) -> &mut Self {
        self.push_named_link(name, format!("<{}>", url.into()))
    }

    fn push_named_link_safe_no_preview(&mut self, name: impl Into<Content>, url: impl Into<Content>) -> &mut Self {
        self.push_named_link_safe(name, format!("<{}>", url.into()))
    }
}

enum DbPool {}

impl TypeMapKey for DbPool {
    type Value = PgPool;
}

pub(crate) enum HttpClient {}

impl TypeMapKey for HttpClient {
    type Value = reqwest::Client;
}

pub(crate) enum RacetimeHost {}

impl TypeMapKey for RacetimeHost {
    type Value = racetime::HostInfo;
}

enum StartggToken {}

impl TypeMapKey for StartggToken {
    type Value = String;
}

pub(crate) enum NewRoomLock {}

impl TypeMapKey for NewRoomLock {
    type Value = Arc<Mutex<()>>;
}

pub(crate) enum ExtraRoomTx {}

impl TypeMapKey for ExtraRoomTx {
    type Value = Arc<RwLock<mpsc::Sender<String>>>;
}

#[derive(Clone, Copy)]
pub(crate) struct CommandIds {
    pub(crate) ban: Option<CommandId>,
    delete_after: CommandId,
    draft: Option<CommandId>,
    pub(crate) first: Option<CommandId>,
    pub(crate) no: Option<CommandId>,
    pub(crate) pick: Option<CommandId>,
    post_status: CommandId,
    pronoun_roles: CommandId,
    racing_role: CommandId,
    reset_race: CommandId,
    pub(crate) schedule: CommandId,
    pub(crate) schedule_async: CommandId,
    pub(crate) schedule_remove: CommandId,
    pub(crate) second: Option<CommandId>,
    pub(crate) skip: Option<CommandId>,
    status: CommandId,
    watch_roles: CommandId,
    pub(crate) yes: Option<CommandId>,
}

impl TypeMapKey for CommandIds {
    type Value = HashMap<GuildId, Option<CommandIds>>;
}

pub(crate) const MULTIWORLD_GUILD: GuildId = GuildId::new(826935332867276820);
#[cfg(unix)] pub(crate) const COMMUNITY_MULTIWORLD_ROLE: RoleId = RoleId::new(1086960717749559306);

#[cfg_attr(not(unix), allow(unused))] // only constructed in UNIX socket handler
#[derive(Clone, Copy, PartialEq, Eq, Sequence)]
pub(crate) enum Element {
    Light,
    Forest,
    Fire,
    Water,
    Shadow,
    Spirit,
}

impl Element {
    pub(crate) fn voice_channel(&self) -> ChannelId {
        match self {
            Self::Light => ChannelId::new(1096152882962768032),
            Self::Forest => ChannelId::new(1096153269933441064),
            Self::Fire => ChannelId::new(1096153203508260884),
            Self::Water => ChannelId::new(1096153240049025024),
            Self::Shadow => ChannelId::new(1242773533600387143),
            Self::Spirit => ChannelId::new(1242774260682985573),
        }
    }
}

impl TypeMapKey for Element {
    type Value = HashMap<UserId, Element>;
}

#[async_trait]
trait GenericInteraction {
    fn channel_id(&self) -> ChannelId;
    fn guild_id(&self) -> Option<GuildId>;
    fn user_id(&self) -> UserId;
    async fn create_response(&self, cache_http: impl CacheHttp, builder: CreateInteractionResponse) -> serenity::Result<()>;
}

#[async_trait]
impl GenericInteraction for CommandInteraction {
    fn channel_id(&self) -> ChannelId { self.channel_id }
    fn guild_id(&self) -> Option<GuildId> { self.guild_id }
    fn user_id(&self) -> UserId { self.user.id }

    async fn create_response(&self, cache_http: impl CacheHttp, builder: CreateInteractionResponse) -> serenity::Result<()> {
        self.create_response(cache_http, builder).await
    }
}

#[async_trait]
impl GenericInteraction for ComponentInteraction {
    fn channel_id(&self) -> ChannelId { self.channel_id }
    fn guild_id(&self) -> Option<GuildId> { self.guild_id }
    fn user_id(&self) -> UserId { self.user.id }

    async fn create_response(&self, cache_http: impl CacheHttp, builder: CreateInteractionResponse) -> serenity::Result<()> {
        self.create_response(cache_http, builder).await
    }
}

//TODO refactor (MH admins should have permissions, room already being open should not remove permissions but only remove the team from return)
async fn check_scheduling_thread_permissions<'a>(ctx: &'a DiscordCtx, interaction: &impl GenericInteraction, game: Option<i16>, allow_rooms_for_other_teams: bool, alternative_instructions: Option<&str>) -> Result<Option<(Transaction<'a, Postgres>, Race, Option<Team>)>, Box<dyn std::error::Error + Send + Sync>> {
    let (mut transaction, http_client) = {
        let data = ctx.data.read().await;
        (
            data.get::<DbPool>().expect("database connection pool missing from Discord context").begin().await?,
            data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone(),
        )
    };
    let mut applicable_races = Race::for_scheduling_channel(&mut transaction, &http_client, interaction.channel_id(), game, false).await?;
    if let Some(Some(min_game)) = applicable_races.iter().map(|race| race.game).min() {
        // None < Some(_) so this code only runs if all applicable races are best-of-N
        applicable_races.retain(|race| race.game == Some(min_game));
    }
    Ok(match applicable_races.into_iter().at_most_one() {
        Ok(None) => {
            let command_ids = ctx.data.read().await.get::<CommandIds>().and_then(|command_ids| command_ids.get(&interaction.guild_id()?))
                .expect("interaction called from outside registered guild")
                .expect("interaction called from guild with conflicting draft kinds");
            let mut content = MessageBuilder::default();
            match (Race::for_scheduling_channel(&mut transaction, &http_client, interaction.channel_id(), game, true).await?.is_empty(), game.is_some()) {
                (false, false) => {
                    content.push("Sorry, this thread is not associated with any upcoming races. ");
                    if let Some(alternative_instructions) = alternative_instructions {
                        content.push(alternative_instructions);
                        content.push(", or tournament organizers can use ");
                    } else {
                        content.push("Tournament organizers can use ");
                    }
                    content.mention_command(command_ids.reset_race, "reset-race");
                    content.push(" if necessary.");
                }
                (false, true) => {
                    content.push("Sorry, there don't seem to be any upcoming races with that game number associated with this thread. ");
                    if let Some(alternative_instructions) = alternative_instructions {
                        content.push(alternative_instructions);
                        content.push(", or tournament organizers can use ");
                    } else {
                        content.push("Tournament organizers can use ");
                    }
                    content.mention_command(command_ids.reset_race, "reset-race");
                    content.push(" if necessary.");
                }
                (true, false) => { content.push("Sorry, this thread is not associated with any upcoming races. Please contact a tournament organizer to fix this."); }
                (true, true) => { content.push("Sorry, there don't seem to be any upcoming races with that game number associated with this thread. If this seems wrong, please contact a tournament organizer to fix this."); }
            }
            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                .ephemeral(true)
                .content(content.build())
            )).await?;
            transaction.rollback().await?;
            None
        }
        Ok(Some(race)) => {
            let mut team = None;
            for iter_team in race.teams() {
                if iter_team.members(&mut transaction).await?.into_iter().any(|member| member.discord.is_some_and(|discord| discord.id == interaction.user_id())) {
                    team = Some(iter_team.clone());
                    break
                }
            }
            if let Some(ref team) = team {
                let blocked = if allow_rooms_for_other_teams {
                    race.has_room_for(team)
                } else {
                    race.has_any_room()
                };
                if blocked {
                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                        .ephemeral(true)
                        .content("Sorry, this command can't be used since a race room is already open. Please contact a tournament organizer if necessary.")
                    )).await?;
                    transaction.rollback().await?;
                    return Ok(None)
                }
            }
            Some((transaction, race, team))
        }
        Err(_) => {
            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                .ephemeral(true)
                .content("Sorry, this thread is associated with multiple upcoming races. Please contact a tournament organizer to fix this.")
            )).await?;
            transaction.rollback().await?;
            None
        }
    })
}

async fn check_draft_permissions<'a>(ctx: &'a DiscordCtx, interaction: &impl GenericInteraction) -> Result<Option<(event::Data<'static>, Race, draft::Kind, draft::MessageContext<'a>)>, Box<dyn std::error::Error + Send + Sync>> {
    let Some((mut transaction, race, team)) = check_scheduling_thread_permissions(ctx, interaction, None, false, Some("You can continue the draft in the race room")).await? else { return Ok(None) };
    let guild_id = interaction.guild_id().expect("Received interaction from outside of a guild");
    let event = race.event(&mut transaction).await?;
    Ok(if let Some(team) = team {
        if let Some(draft_kind) = event.draft_kind() {
            if let Some(ref draft) = race.draft {
                if draft.is_active_team(draft_kind, race.game, team.id).await? {
                    let msg_ctx = draft::MessageContext::Discord {
                        command_ids: ctx.data.read().await.get::<CommandIds>().and_then(|command_ids| command_ids.get(&guild_id))
                            .expect("draft action called from outside registered guild")
                            .expect("interaction called from guild with conflicting draft kinds"),
                        teams: race.teams().cloned().collect(),
                        transaction, guild_id, team,
                    };
                    Some((event, race, draft_kind, msg_ctx))
                } else {
                    let response_content = if let French = event.language {
                        format!("Désolé, mais ce n'est pas votre tour.")
                    } else {
                        format!("Sorry, it's not {} turn in the settings draft.", if let TeamConfig::Solo = event.team_config { "your" } else { "your team's" })
                    };
                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                        .ephemeral(true)
                        .content(response_content)
                    )).await?;
                    transaction.rollback().await?;
                    None
                }
            } else {
                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                )).await?;
                transaction.rollback().await?;
                None
            }
        } else {
            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                .ephemeral(true)
                .content("Sorry, there is no settings draft for this event.")
            )).await?;
            transaction.rollback().await?;
            None
        }
    } else {
        let response_content = if let French = event.language {
            "Désolé, seuls les participants de la race peuvent utiliser cette commande."
        } else {
            "Sorry, only participants in this race can use this command."
        };
        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
            .ephemeral(true)
            .content(response_content)
        )).await?;
        transaction.rollback().await?;
        None
    })
}

async fn send_draft_settings_page(ctx: &DiscordCtx, interaction: &impl GenericInteraction, action: &str, page: usize) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some((event, mut race, draft_kind, mut msg_ctx)) = check_draft_permissions(ctx, interaction).await? else { return Ok(()) };
    match race.draft.as_ref().unwrap().next_step(draft_kind, race.game, &mut msg_ctx).await?.kind {
        draft::StepKind::GoFirst | draft::StepKind::BooleanChoice { .. } | draft::StepKind::Done(_) | draft::StepKind::DoneRsl { .. } => match race.draft.as_mut().unwrap().apply(draft_kind, race.game, &mut msg_ctx, draft::Action::Pick { setting: format!("@placeholder"), value: format!("@placeholder") }).await? {
            Ok(_) => unreachable!(),
            Err(error_msg) => {
                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .content(error_msg)
                )).await?;
                msg_ctx.into_transaction().rollback().await?;
                return Ok(())
            }
        },
        draft::StepKind::Ban { available_settings, rsl, .. } => {
            let response_content = if_chain! {
                if let French = event.language;
                if let Some(action) = match action {
                    "ban" => Some("ban"),
                    "draft" => Some("pick"),
                    _ => None,
                };
                then {
                    format!("Sélectionnez le setting à {action} :")
                } else {
                    format!("Select the setting to {}:", if rsl { "block" } else { action })
                }
            };
            let mut response_msg = CreateInteractionResponseMessage::new()
                .ephemeral(true)
                .content(response_content);
            if available_settings.num_settings() <= BUTTONS_PER_PAGE {
                for draft::BanSetting { name, display, .. } in available_settings.all() {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_setting_{name}")).label(display));
                }
            } else {
                if let Some((page_name, _)) = page.checked_sub(1).and_then(|prev_page| available_settings.page(prev_page)) {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_page_{}", page - 1)).label(page_name).style(ButtonStyle::Secondary));
                }
                for draft::BanSetting { name, display, .. } in available_settings.page(page).unwrap().1 {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_setting_{name}")).label(*display));
                }
                if let Some((page_name, _)) = page.checked_add(1).and_then(|next_page| available_settings.page(next_page)) {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_page_{}", page + 1)).label(page_name).style(ButtonStyle::Secondary));
                }
            }
            interaction.create_response(ctx, CreateInteractionResponse::Message(response_msg)).await?;
        }
        draft::StepKind::Pick { available_choices, rsl, .. } => {
            let response_content = if_chain! {
                if let French = event.language;
                if let Some(action) = match action {
                    "ban" => Some("ban"),
                    "draft" => Some("pick"),
                    _ => None,
                };
                then {
                    format!("Sélectionnez le setting à {action} :")
                } else {
                    format!("Select the setting to {}:", if rsl { "ban" } else { action })
                }
            };
            let mut response_msg = CreateInteractionResponseMessage::new()
                .ephemeral(true)
                .content(response_content);
            if available_choices.num_settings() <= BUTTONS_PER_PAGE {
                for draft::DraftSetting { name, display, .. } in available_choices.all() {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_setting_{name}")).label(display));
                }
            } else {
                if let Some((page_name, _)) = page.checked_sub(1).and_then(|prev_page| available_choices.page(prev_page)) {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_page_{}", page - 1)).label(page_name).style(ButtonStyle::Secondary));
                }
                for draft::DraftSetting { name, display, .. } in available_choices.page(page).unwrap().1 {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_setting_{name}")).label(*display));
                }
                if let Some((page_name, _)) = page.checked_add(1).and_then(|next_page| available_choices.page(next_page)) {
                    response_msg = response_msg.button(CreateButton::new(format!("{action}_page_{}", page + 1)).label(page_name).style(ButtonStyle::Secondary));
                }
            }
            interaction.create_response(ctx, CreateInteractionResponse::Message(response_msg)).await?;
        }
    }
    msg_ctx.into_transaction().commit().await?;
    Ok(())
}

async fn draft_action(ctx: &DiscordCtx, interaction: &impl GenericInteraction, action: draft::Action) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some((event, mut race, draft_kind, mut msg_ctx)) = check_draft_permissions(ctx, interaction).await? else { return Ok(()) };
    match race.draft.as_mut().unwrap().apply(draft_kind, race.game, &mut msg_ctx, action).await? {
        Ok(apply_response) => {
            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                .ephemeral(false)
                .content(apply_response)
            )).await?;
            if let Some(draft_kind) = event.draft_kind() {
                interaction.channel_id()
                    .say(ctx, race.draft.as_ref().unwrap().next_step(draft_kind, race.game, &mut msg_ctx).await?.message).await?;
            }
            let mut transaction = msg_ctx.into_transaction();
            sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(race.draft.as_ref().unwrap()) as _, race.id as _).execute(&mut *transaction).await?;
            transaction.commit().await?;
        }
        Err(error_msg) => {
            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                .ephemeral(true)
                .content(error_msg)
            )).await?;
            msg_ctx.into_transaction().rollback().await?;
        }
    }
    Ok(())
}

fn parse_timestamp(timestamp: &str) -> Option<DateTime<Utc>> {
    regex_captures!("^<t:(-?[0-9]+)(?::[tTdDfFR])?>$", timestamp)
        .and_then(|(_, timestamp)| timestamp.parse().ok())
        .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
}

pub(crate) fn configure_builder(discord_builder: serenity_utils::Builder, db_pool: PgPool, http_client: reqwest::Client, config: Config, new_room_lock: Arc<Mutex<()>>, extra_room_tx: Arc<RwLock<mpsc::Sender<String>>>, clean_shutdown: Arc<Mutex<CleanShutdown>>, shutdown: rocket::Shutdown) -> serenity_utils::Builder {
    discord_builder
        .error_notifier(ErrorNotifier::User(FENHL)) //TODO also print to stderr and/or report to night
        .data::<DbPool>(db_pool)
        .data::<HttpClient>(http_client)
        .data::<RacetimeHost>(racetime::HostInfo {
            hostname: Cow::Borrowed(racetime_host()),
            ..racetime::HostInfo::default()
        })
        .data::<ConfigRaceTime>(config.racetime_bot.clone())
        .data::<StartggToken>(config.startgg)
        .data::<NewRoomLock>(new_room_lock)
        .data::<ExtraRoomTx>(extra_room_tx)
        .data::<CleanShutdown>(clean_shutdown)
        .on_guild_create(false, |ctx, guild, _| Box::pin(async move {
            let mut transaction = ctx.data.read().await.get::<DbPool>().expect("database connection pool missing from Discord context").begin().await?;
            let guild_event_rows = sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE discord_guild = $1 AND (end_time IS NULL OR end_time > NOW())"#, PgSnowflake(guild.id) as _).fetch_all(&mut *transaction).await?;
            let mut guild_events = Vec::with_capacity(guild_event_rows.len());
            for row in guild_event_rows {
                guild_events.push(event::Data::new(&mut transaction, row.series, row.event).await?.expect("just received from database"));
            }
            let mut commands = Vec::default();
            let mut draft_kind = None;
            for event in &guild_events {
                if let Some(new_kind) = event.draft_kind() {
                    if draft_kind.is_some_and(|prev_kind| prev_kind != new_kind) {
                        #[derive(Debug, thiserror::Error)]
                        #[error("multiple conflicting draft kinds in the same Discord guild")]
                        struct DraftKindsError;

                        ctx.data.write().await.entry::<CommandIds>().or_default().insert(guild.id, None);
                        return Err(Box::new(DraftKindsError) as Box<dyn std::error::Error + Send + Sync>)
                    }
                    draft_kind = Some(new_kind);
                }
            }
            let ban = draft_kind.map(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => CreateCommand::new("ban")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Locks a setting for this race to its default value."),
                    draft::Kind::RslS7 => CreateCommand::new("block")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Blocks the weights of a setting from being changed."),
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("ban")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Verrouille un setting à sa valeur par défaut.")
                        .description_localized("en-GB", "Locks a setting for this race to its default value.")
                        .description_localized("en-US", "Locks a setting for this race to its default value."),
                });
                idx
            });
            let delete_after = {
                let idx = commands.len();
                commands.push(CreateCommand::new("delete-after")
                    .kind(CommandType::ChatInput)
                    .add_context(InteractionContext::Guild)
                    .description("Deletes games of the match that are not required.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "game",
                        "The last game number within the match that should be kept.",
                    )
                        .min_int_value(1)
                        .max_int_value(255)
                        .required(true)
                    )
                );
                idx
            };
            let draft = draft_kind.and_then(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => CreateCommand::new("draft")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Chooses a setting for this race (same as /pick)."),
                    draft::Kind::RslS7 => return None, // command is called /ban, no alias necessary
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("draft")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Choisit un setting pour la race (identique à /pick).")
                        .description_localized("en-GB", "Chooses a setting for this race (same as /pick).")
                        .description_localized("en-US", "Chooses a setting for this race (same as /pick)."),
                });
                Some(idx)
            });
            let first = draft_kind.map(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => CreateCommand::new("first")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Go first in the settings draft."),
                    draft::Kind::RslS7 => CreateCommand::new("first")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Go first in the weights draft.")
                        .add_option(CreateCommandOption::new(
                            CommandOptionType::Boolean,
                            "lite",
                            "Use RSL-Lite weights",
                        )
                            .required(false)
                        ),
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("first")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Partir premier dans la phase de pick&ban.")
                        .description_localized("en-GB", "Go first in the settings draft.")
                        .description_localized("en-US", "Go first in the settings draft.")
                        .add_option(CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "mq",
                            "Nombre de donjons MQ",
                        )
                            .description_localized("en-GB", "Number of MQ dungeons")
                            .description_localized("en-US", "Number of MQ dungeons")
                            .min_int_value(0)
                            .max_int_value(12)
                            .required(false)
                        ),
                });
                idx
            });
            let no = draft_kind.and_then(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 | draft::Kind::RslS7 => return None,
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("no")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Répond à la négative dans une question fermée.")
                        .description_localized("en-GB", "Answers no to a yes/no question in the settings draft.")
                        .description_localized("en-US", "Answers no to a yes/no question in the settings draft."),
                });
                Some(idx)
            });
            let pick = draft_kind.map(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => CreateCommand::new("pick")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Chooses a setting for this race."),
                    draft::Kind::RslS7 => CreateCommand::new("ban")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Sets a weight of a setting to 0."),
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("pick")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Choisit un setting pour la race.")
                        .description_localized("en-GB", "Chooses a setting for this race.")
                        .description_localized("en-US", "Chooses a setting for this race."),
                });
                idx
            });
            let post_status = {
                let idx = commands.len();
                commands.push(CreateCommand::new("post-status")
                    .kind(CommandType::ChatInput)
                    .add_context(InteractionContext::Guild)
                    .description("Posts this race's status to the thread, pinging the team whose turn it is in the settings draft.")
                );
                idx
            };
            let pronoun_roles = {
                let idx = commands.len();
                commands.push(CreateCommand::new("pronoun-roles")
                    .kind(CommandType::ChatInput)
                    .default_member_permissions(Permissions::ADMINISTRATOR)
                    .add_context(InteractionContext::Guild)
                    .description("Creates gender pronoun roles and posts a message here that allows members to self-assign them.")
                );
                idx
            };
            let racing_role = {
                let idx = commands.len();
                commands.push(CreateCommand::new("racing-role")
                    .kind(CommandType::ChatInput)
                    .default_member_permissions(Permissions::ADMINISTRATOR)
                    .add_context(InteractionContext::Guild)
                    .description("Creates a racing role and posts a message here that allows members to self-assign it.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Channel,
                        "race-planning-channel",
                        "Will be linked to from the description message.",
                    )
                        .required(true)
                        .channel_types(vec![ChannelType::Text, ChannelType::News])
                    )
                );
                idx
            };
            let reset_race = {
                let idx = commands.len();
                let mut command = CreateCommand::new("reset-race")
                    .kind(CommandType::ChatInput)
                    .add_context(InteractionContext::Guild)
                    .description("Deletes selected data from a race.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "game",
                        "The game number within the match.",
                    )
                        .min_int_value(1)
                        .max_int_value(255)
                        .required(false)
                    );
                if draft_kind.is_some() {
                    command = command.add_option(CreateCommandOption::new(
                        CommandOptionType::Boolean,
                        "draft",
                        "Reset the settings draft.",
                    )
                        .required(false)
                    );
                }
                command = command.add_option(CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "schedule",
                    "Reset the schedule, race room, and seed.",
                )
                    .required(false)
                );
                commands.push(command);
                idx
            };
            let schedule = {
                let idx = commands.len();
                commands.push(CreateCommand::new("schedule")
                    .kind(CommandType::ChatInput)
                    .add_context(InteractionContext::Guild)
                    .description("Submits a starting time for this race.")
                    .description_localized("fr", "Planifie une date/heure pour une race.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::String,
                        "start",
                        "The starting time as a Discord timestamp",
                    )
                        .description_localized("fr", "La date de début comme timestamp de Discord")
                        .required(true)
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "game",
                        "The game number within the match. Defaults to the next upcoming game.",
                    )
                        .min_int_value(1)
                        .max_int_value(255)
                        .required(false)
                    )
                );
                idx
            };
            let schedule_async = {
                let idx = commands.len();
                commands.push(CreateCommand::new("schedule-async")
                    .kind(CommandType::ChatInput)
                    .add_context(InteractionContext::Guild)
                    .description("Submits a starting time for your half of this race.")
                    .description_localized("fr", "Planifie votre partie de l'async.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::String,
                        "start",
                        "The starting time as a Discord timestamp",
                    )
                        .description_localized("fr", "La date de début comme timestamp de Discord")
                        .required(true)
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "game",
                        "The game number within the match. Defaults to the next upcoming game.",
                    )
                        .min_int_value(1)
                        .max_int_value(255)
                        .required(false)
                    )
                );
                idx
            };
            let schedule_remove = {
                let idx = commands.len();
                commands.push(CreateCommand::new("schedule-remove")
                    .kind(CommandType::ChatInput)
                    .add_context(InteractionContext::Guild)
                    .description("Removes the starting time(s) for this race from the schedule.")
                    .description_localized("fr", "Supprime le(s) date(s) de début sur le document des races planifiées.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "game",
                        "The game number within the match. Defaults to the next upcoming game.",
                    )
                        .min_int_value(1)
                        .max_int_value(255)
                        .required(false)
                    )
                );
                idx
            };
            let second = draft_kind.map(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => CreateCommand::new("second")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Go second in the settings draft."),
                    draft::Kind::RslS7 => CreateCommand::new("second")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Go second in the weights draft.")
                        .add_option(CreateCommandOption::new(
                            CommandOptionType::Boolean,
                            "lite",
                            "Use RSL-Lite weights",
                        )
                            .required(false)
                        ),
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("second")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Partir second dans la phase de pick&ban.")
                        .description_localized("en-GB", "Go second in the settings draft.")
                        .description_localized("en-US", "Go second in the settings draft.")
                        .add_option(CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "mq",
                            "Nombre de donjons MQ",
                        )
                            .description_localized("en-GB", "Number of MQ dungeons")
                            .description_localized("en-US", "Number of MQ dungeons")
                            .min_int_value(0)
                            .max_int_value(12)
                            .required(false)
                        ),
                });
                idx
            });
            let skip = draft_kind.map(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => CreateCommand::new("skip")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Skips your current turn of the settings draft."),
                    draft::Kind::RslS7 => CreateCommand::new("skip")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Skips your current turn of the weights draft."),
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("skip")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Skip le dernier pick du draft.")
                        .description_localized("en-GB", "Skips the final pick of the settings draft.")
                        .description_localized("en-US", "Skips the final pick of the settings draft."),
                });
                idx
            });
            let status = {
                let idx = commands.len();
                commands.push(CreateCommand::new("status")
                    .kind(CommandType::ChatInput)
                    .add_context(InteractionContext::Guild)
                    .description("Shows you this race's current scheduling and settings draft status.")
                    .description_localized("fr", "Montre l'avancement de la planification de votre race, avec les détails.")
                );
                idx
            };
            let watch_roles = {
                let idx = commands.len();
                commands.push(CreateCommand::new("watch-roles")
                    .kind(CommandType::ChatInput)
                    .default_member_permissions(Permissions::ADMINISTRATOR)
                    .add_context(InteractionContext::Guild)
                    .description("Creates watch notification roles and posts a message here that allows members to self-assign them.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Channel,
                        "watch-party-channel",
                        "Will be linked to from the description message.",
                    )
                        .required(true)
                        .channel_types(vec![ChannelType::Voice, ChannelType::Stage])
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Channel,
                        "race-rooms-channel",
                        "Will be linked to from the description message.",
                    )
                        .required(true)
                        .channel_types(vec![ChannelType::Text, ChannelType::News])
                    )
                );
                idx
            };
            let yes = draft_kind.and_then(|draft_kind| {
                let idx = commands.len();
                commands.push(match draft_kind {
                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 | draft::Kind::RslS7 => return None,
                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => CreateCommand::new("yes")
                        .kind(CommandType::ChatInput)
                        .add_context(InteractionContext::Guild)
                        .description("Répond à l'affirmative dans une question fermée.")
                        .description_localized("en-GB", "Answers yes to a yes/no question in the settings draft.")
                        .description_localized("en-US", "Answers yes to a yes/no question in the settings draft."),
                });
                Some(idx)
            });
            let commands = guild.set_commands(ctx, commands).await?;
            ctx.data.write().await.entry::<CommandIds>().or_default().insert(guild.id, Some(CommandIds {
                ban: ban.map(|idx| commands[idx].id),
                delete_after: commands[delete_after].id,
                draft: draft.map(|idx| commands[idx].id),
                first: first.map(|idx| commands[idx].id),
                no: no.map(|idx| commands[idx].id),
                pick: pick.map(|idx| commands[idx].id),
                post_status: commands[post_status].id,
                pronoun_roles: commands[pronoun_roles].id,
                racing_role: commands[racing_role].id,
                reset_race: commands[reset_race].id,
                schedule: commands[schedule].id,
                schedule_async: commands[schedule_async].id,
                schedule_remove: commands[schedule_remove].id,
                second: second.map(|idx| commands[idx].id),
                skip: skip.map(|idx| commands[idx].id),
                status: commands[status].id,
                watch_roles: commands[watch_roles].id,
                yes: yes.map(|idx| commands[idx].id),
            }));
            transaction.commit().await?;
            Ok(())
        }))
        .on_interaction_create(|ctx, interaction| Box::pin(async move {
            match interaction {
                Interaction::Command(interaction) => {
                    let guild_id = interaction.guild_id.expect("Discord slash command called outside of a guild");
                    if let Some(&Some(command_ids)) = ctx.data.read().await.get::<CommandIds>().and_then(|command_ids| command_ids.get(&guild_id)) {
                        if Some(interaction.data.id) == command_ids.ban {
                            send_draft_settings_page(ctx, interaction, "ban", 0).await?;
                        } else if interaction.data.id == command_ids.delete_after {
                            let Some(parent_channel) = interaction.channel.as_ref().and_then(|thread| thread.parent_id) else {
                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content("Sorry, this command can only be used inside threads and forum posts.")
                                )).await?;
                                return Ok(())
                            };
                            let mut transaction = ctx.data.read().await.get::<DbPool>().as_ref().expect("database connection pool missing from Discord context").begin().await?;
                            if let Some(event_row) = sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE discord_scheduling_channel = $1 AND end_time IS NULL"#, PgSnowflake(parent_channel) as _).fetch_optional(&mut *transaction).await? {
                                let event = event::Data::new(&mut transaction, event_row.series, event_row.event).await?.expect("just received from database");
                                match event.match_source() {
                                    MatchSource::Manual | MatchSource::Challonge { .. } => {}
                                    MatchSource::StartGG(_) => {} //TODO automate
                                    MatchSource::League => {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Sorry, this command is not available for events sourcing their match schedule from league.ootrandomizer.com")
                                        )).await?;
                                        return Ok(())
                                    }
                                };
                                if !event.organizers(&mut transaction).await?.into_iter().any(|organizer| organizer.discord.is_some_and(|discord| discord.id == interaction.user.id)) {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, only event organizers can use this command.")
                                    )).await?;
                                    return Ok(())
                                }
                                let after_game = match interaction.data.options[0].value {
                                    CommandDataOptionValue::Integer(game) => i16::try_from(game).expect("game number out of range"),
                                    _ => panic!("unexpected slash command option type"),
                                };
                                let races_deleted = sqlx::query_scalar!(r#"DELETE FROM races WHERE scheduling_thread = $1 AND NOT ignored AND GAME > $2"#, PgSnowflake(interaction.channel_id) as _, after_game).execute(&mut *transaction).await?
                                    .rows_affected();
                                transaction.commit().await?;
                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content(if races_deleted == 0 {
                                        format!("Sorry, looks like that didn't delete any races.")
                                    } else {
                                        format!("{races_deleted} race{} deleted from the schedule.", if races_deleted == 1 { "" } else { "s" })
                                    })
                                )).await?;
                            } else {
                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content("Sorry, this channel is not configured as the scheduling channel for any ongoing Mido's House events.")
                                )).await?;
                            }
                        } else if Some(interaction.data.id) == command_ids.draft || Some(interaction.data.id) == command_ids.pick {
                            send_draft_settings_page(ctx, interaction, "draft", 0).await?;
                        } else if Some(interaction.data.id) == command_ids.first {
                            if let Some((_, mut race, draft_kind, msg_ctx)) = check_draft_permissions(ctx, interaction).await? {
                                match draft_kind {
                                    draft::Kind::RslS7 => {
                                        let settings = &mut race.draft.as_mut().unwrap().settings;
                                        let lite = interaction.data.options.get(0).map(|option| match option.value {
                                            CommandDataOptionValue::Boolean(lite) => lite,
                                            _ => panic!("unexpected slash command option type"),
                                        });
                                        if settings.get("lite_ok").map(|lite_ok| &**lite_ok).unwrap_or("no") == "ok" {
                                            let mut transaction = msg_ctx.into_transaction();
                                            if let Some(lite) = lite {
                                                settings.insert(Cow::Borrowed("preset"), Cow::Borrowed(if lite { "lite" } else { "league" }));
                                                sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(race.draft.as_ref().unwrap()) as _, race.id as _).execute(&mut *transaction).await?;
                                                transaction.commit().await?;
                                            } else {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content(MessageBuilder::default().push("Sorry, please specify the ").push_mono("lite").push(" parameter.").build())
                                                )).await?;
                                                transaction.rollback().await?;
                                                return Ok(())
                                            }
                                        } else {
                                            if lite.is_some_and(identity) {
                                                //TODO different error messages depending on which player(s) didn't opt into RSL-Lite
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content("Sorry, either you or your opponent didn't opt into RSL-Lite.")
                                                )).await?;
                                                return Ok(())
                                            }
                                        }
                                    }
                                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => {
                                        let settings = &mut race.draft.as_mut().unwrap().settings;
                                        let mq = interaction.data.options.get(0).map(|option| match option.value {
                                            CommandDataOptionValue::Integer(mq) => u8::try_from(mq).expect("MQ count out of range"),
                                            _ => panic!("unexpected slash command option type"),
                                        });
                                        if settings.get("mq_ok").map(|mq_ok| &**mq_ok).unwrap_or("no") == "ok" {
                                            let mut transaction = msg_ctx.into_transaction();
                                            if let Some(mq) = mq {
                                                settings.insert(Cow::Borrowed("mq_dungeons_count"), Cow::Owned(mq.to_string()));
                                                sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(race.draft.as_ref().unwrap()) as _, race.id as _).execute(&mut *transaction).await?;
                                                transaction.commit().await?;
                                            } else {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content("Désolé, veuillez entrer le nombre de donjons MQ d'abord.")
                                                )).await?;
                                                transaction.rollback().await?;
                                                return Ok(())
                                            }
                                        } else {
                                            if mq.is_some_and(|mq| mq != 0) {
                                                //TODO different error messages depending on which player(s) didn't opt into MQ
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content("Désolé, mais l'un d'entre vous n'a pas choisi les donjons MQ.")
                                                )).await?;
                                                return Ok(())
                                            }
                                        }
                                    }
                                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => {}
                                }
                                draft_action(ctx, interaction, draft::Action::GoFirst(true)).await?;
                            }
                        } else if Some(interaction.data.id) == command_ids.no {
                            draft_action(ctx, interaction, draft::Action::BooleanChoice(false)).await?;
                        } else if interaction.data.id == command_ids.post_status {
                            if let Some((mut transaction, race, team)) = check_scheduling_thread_permissions(ctx, interaction, None, true, None).await? {
                                let event = race.event(&mut transaction).await?;
                                if event.organizers(&mut transaction).await?.into_iter().any(|organizer| organizer.discord.is_some_and(|discord| discord.id == interaction.user.id)) {
                                    if let Some(draft_kind) = event.draft_kind() {
                                        if let Some(ref draft) = race.draft {
                                            let mut msg_ctx = draft::MessageContext::Discord {
                                                teams: race.teams().cloned().collect(),
                                                team: team.unwrap_or_else(Team::dummy),
                                                transaction, guild_id, command_ids,
                                            };
                                            let message_content = MessageBuilder::default()
                                                //TODO include scheduling status, both for regular races and for asyncs
                                                .push(draft.next_step(draft_kind, race.game, &mut msg_ctx).await?.message)
                                                .build();
                                            interaction.channel.as_ref().expect("received draft action outside channel")
                                                .id
                                                .say(ctx, message_content).await?;
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("done")
                                            )).await?;
                                            msg_ctx.into_transaction().commit().await?;
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
                                    } else {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Sorry, this command is currently only available for events with settings drafts.") //TODO
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content(if let French = event.language {
                                            "Désolé, seuls les organisateurs du tournoi peuvent utiliser cette commande."
                                        } else {
                                            "Sorry, only organizers can use this command."
                                        })
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if interaction.data.id == command_ids.pronoun_roles {
                            guild_id.create_role(ctx, EditRole::new()
                                .hoist(false)
                                .mentionable(false)
                                .name("he/him")
                                .permissions(Permissions::empty())
                            ).await?;
                            guild_id.create_role(ctx, EditRole::new()
                                .hoist(false)
                                .mentionable(false)
                                .name("she/her")
                                .permissions(Permissions::empty())
                            ).await?;
                            guild_id.create_role(ctx, EditRole::new()
                                .hoist(false)
                                .mentionable(false)
                                .name("they/them")
                                .permissions(Permissions::empty())
                            ).await?;
                            guild_id.create_role(ctx, EditRole::new()
                                .hoist(false)
                                .mentionable(false)
                                .name("other pronouns")
                                .permissions(Permissions::empty())
                            ).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(false)
                                .content("Click a button below to get a gender pronoun role. Click again to remove it. Multiple selections allowed.")
                                .button(CreateButton::new("pronouns_he").label("he/him"))
                                .button(CreateButton::new("pronouns_she").label("she/her"))
                                .button(CreateButton::new("pronouns_they").label("they/them"))
                                .button(CreateButton::new("pronouns_other").label("other"))
                            )).await?;
                        } else if interaction.data.id == command_ids.racing_role {
                            let race_planning_channel = match interaction.data.options[0].value {
                                CommandDataOptionValue::Channel(channel) => channel,
                                _ => panic!("unexpected slash command option type"),
                            };
                            guild_id.create_role(ctx, EditRole::new()
                                .hoist(false)
                                .mentionable(true)
                                .name("racing")
                                .permissions(Permissions::empty())
                            ).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(false)
                                .content(MessageBuilder::default()
                                    .push("Click the button below to get notified when a race is being planned. Click again to remove it. Ping this role in ")
                                    .mention(&race_planning_channel)
                                    .push(" when planning a race.")
                                    .build()
                                )
                                .button(CreateButton::new("racingrole").label("racing"))
                            )).await?;
                        } else if interaction.data.id == command_ids.reset_race {
                            let Some(parent_channel) = interaction.channel.as_ref().and_then(|thread| thread.parent_id) else {
                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content("Sorry, this command can only be used inside threads and forum posts.")
                                )).await?;
                                return Ok(())
                            };
                            let mut transaction = ctx.data.read().await.get::<DbPool>().as_ref().expect("database connection pool missing from Discord context").begin().await?;
                            if let Some(event_row) = sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE discord_scheduling_channel = $1 AND end_time IS NULL"#, PgSnowflake(parent_channel) as _).fetch_optional(&mut *transaction).await? {
                                let event = event::Data::new(&mut transaction, event_row.series, event_row.event).await?.expect("just received from database");
                                if !event.organizers(&mut transaction).await?.into_iter().any(|organizer| organizer.discord.is_some_and(|discord| discord.id == interaction.user.id)) {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, only event organizers can use this command.")
                                    )).await?;
                                    return Ok(())
                                }
                                let mut game = None;
                                let mut reset_draft = false;
                                let mut reset_schedule = false;
                                for option in &interaction.data.options {
                                    match &*option.name {
                                        "draft" => match option.value {
                                            CommandDataOptionValue::Boolean(value) => reset_draft = value,
                                            _ => panic!("unexpected slash command option type"),
                                        },
                                        "game" => match option.value {
                                            CommandDataOptionValue::Integer(value) => game = Some(i16::try_from(value).expect("game number out of range")),
                                            _ => panic!("unexpected slash command option type"),
                                        },
                                        "schedule" => match option.value {
                                            CommandDataOptionValue::Boolean(value) => reset_schedule = value,
                                            _ => panic!("unexpected slash command option type"),
                                        },
                                        name => panic!("unexpected option for /reset-race: {name}"),
                                    }
                                }
                                let Some(aspects_reset) = NEVec::try_from_vec(reset_draft.then_some("draft").into_iter().chain(reset_schedule.then_some("schedule")).collect_vec()) else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Please specify at least one thing to delete using the slash command's options.")
                                    )).await?;
                                    return Ok(())
                                };
                                let http_client = {
                                    let data = ctx.data.read().await;
                                    data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone()
                                };
                                match Race::for_scheduling_channel(&mut transaction, &http_client, interaction.channel_id(), game, true).await?.into_iter().at_most_one() {
                                    Ok(None) => {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(if game.is_some() {
                                                "Sorry, there don't seem to be any races with that game number associated with this thread."
                                            } else {
                                                "Sorry, this thread is not associated with any races."
                                            })
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                    Ok(Some(race)) => {
                                        let race = Race {
                                            source: if let cal::Source::SpeedGamingOnline { id: _ } | cal::Source::SpeedGamingInPerson { id: _ } = race.source {
                                                if reset_schedule { cal::Source::Manual } else { race.source }
                                            } else {
                                                race.source
                                            },
                                            schedule: if reset_schedule { RaceSchedule::Unscheduled } else { race.schedule },
                                            schedule_updated_at: if reset_schedule { Some(Utc::now()) } else { race.schedule_updated_at },
                                            fpa_invoked: if reset_schedule { false } else { race.fpa_invoked },
                                            breaks_used: if reset_schedule { false } else { race.breaks_used },
                                            draft: if reset_draft {
                                                if_chain! {
                                                    if let Some(draft_kind) = event.draft_kind();
                                                    if let Some(draft) = race.draft;
                                                    if let Entrants::Two(entrants) = &race.entrants;
                                                    if let Ok(low_seed) = entrants.iter()
                                                        .filter_map(as_variant!(Entrant::MidosHouseTeam))
                                                        .filter(|team| team.id != draft.high_seed)
                                                        .exactly_one();
                                                    then {
                                                        Some(Draft::for_next_game(&mut transaction, draft_kind, draft.high_seed, low_seed.id).await?)
                                                    } else {
                                                        None
                                                    }
                                                }
                                            } else {
                                                race.draft
                                            },
                                            seed: if reset_schedule { seed::Data::default() } else { race.seed },
                                            notified: race.notified && !reset_schedule,
                                            // explicitly listing remaining fields here instead of using `..race` so if the fields change they're kept/reset correctly
                                            id: race.id,
                                            series: race.series,
                                            event: race.event,
                                            entrants: race.entrants,
                                            phase: race.phase,
                                            round: race.round,
                                            game: race.game,
                                            scheduling_thread: race.scheduling_thread,
                                            video_urls: race.video_urls,
                                            restreamers: race.restreamers,
                                            last_edited_by: race.last_edited_by,
                                            last_edited_at: race.last_edited_at,
                                            ignored: race.ignored,
                                            schedule_locked: race.schedule_locked,
                                        };
                                        race.save(&mut transaction).await?;
                                        transaction.commit().await?;
                                        let verb = if aspects_reset.len() == NonZero::<usize>::MIN { " has" } else { " have" };
                                        let response_content = MessageBuilder::default()
                                            .push(English.join_str(aspects_reset))
                                            .push(verb)
                                            .push(" been reset.")
                                            .build();
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(false)
                                            .content(response_content)
                                        )).await?;
                                    }
                                    Err(_) => {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Sorry, this thread is associated with multiple races. Please specify the game number.")
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                }
                            } else {
                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content("Sorry, this thread is not associated with an ongoing Mido's House event.")
                                )).await?;
                            }
                        } else if interaction.data.id == command_ids.schedule {
                            let game = interaction.data.options.get(1).map(|option| match option.value {
                                CommandDataOptionValue::Integer(game) => i16::try_from(game).expect("game number out of range"),
                                _ => panic!("unexpected slash command option type"),
                            });
                            if let Some((mut transaction, mut race, team)) = check_scheduling_thread_permissions(ctx, interaction, game, false, None).await? {
                                let event = race.event(&mut transaction).await?;
                                let is_organizer = event.organizers(&mut transaction).await?.into_iter().any(|organizer| organizer.discord.is_some_and(|discord| discord.id == interaction.user.id));
                                let was_scheduled = !matches!(race.schedule, RaceSchedule::Unscheduled);
                                match event.scheduling_backend(&mut transaction).await? {
                                    SchedulingBackend::MidosHouse => if team.is_some() || is_organizer {
                                        let start = match interaction.data.options[0].value {
                                            CommandDataOptionValue::String(ref start) => start,
                                            _ => panic!("unexpected slash command option type"),
                                        };
                                        if let Some(start) = parse_timestamp(start) {
                                            if (start - Utc::now()).to_std().map_or(true, |schedule_notice| schedule_notice < event.min_schedule_notice) {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content(if event.min_schedule_notice <= Duration::default() {
                                                        if let French = event.language {
                                                            format!("Désolé mais cette date est dans le passé.")
                                                        } else {
                                                            format!("Sorry, that timestamp is in the past.")
                                                        }
                                                    } else {
                                                        if let French = event.language {
                                                            format!("Désolé, les races doivent être planifiées au moins {} en avance.", French.format_duration(event.min_schedule_notice, true))
                                                        } else {
                                                            format!("Sorry, races must be scheduled at least {} in advance.", English.format_duration(event.min_schedule_notice, true))
                                                        }
                                                    })
                                                )).await?;
                                                transaction.rollback().await?;
                                            } else {
                                                race.schedule.set_live_start(start);
                                                race.schedule_updated_at = Some(Utc::now());
                                                let mut cal_event = cal::Event { kind: cal::EventKind::Normal, race };
                                                if start - Utc::now() < TimeDelta::minutes(30) {
                                                    let (http_client, new_room_lock, racetime_host, racetime_config, extra_room_tx, clean_shutdown) = {
                                                        let data = ctx.data.read().await;
                                                        (
                                                            data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone(),
                                                            data.get::<NewRoomLock>().expect("new room lock missing from Discord context").clone(),
                                                            data.get::<RacetimeHost>().expect("racetime.gg host missing from Discord context").clone(),
                                                            data.get::<ConfigRaceTime>().expect("racetime.gg config missing from Discord context").clone(),
                                                            data.get::<ExtraRoomTx>().expect("extra room sender missing from Discord context").clone(),
                                                            data.get::<CleanShutdown>().expect("clean shutdown state missing from Discord context").clone(),
                                                        )
                                                    };
                                                    lock!(new_room_lock = new_room_lock; {
                                                        if let Some((_, msg)) = racetime_bot::create_room(&mut transaction, ctx, &racetime_host, &racetime_config.client_id, &racetime_config.client_secret, &extra_room_tx, &http_client, clean_shutdown, &mut cal_event, &event).await? {
                                                            if let Some(channel) = event.discord_race_room_channel {
                                                                channel.say(ctx, &msg).await?;
                                                            }
                                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                                .ephemeral(false)
                                                                .content(msg)
                                                            )).await?;
                                                        } else {
                                                            let mut response_content = MessageBuilder::default();
                                                            response_content.push(if let Some(game) = cal_event.race.game { format!("Game {game}") } else { format!("This race") });
                                                            response_content.push(if was_scheduled { " has been rescheduled for " } else { " is now scheduled for " });
                                                            response_content.push_timestamp(start, serenity_utils::message::TimestampStyle::LongDateTime);
                                                            let response_content = response_content
                                                                .push(". The race room will be opened momentarily.")
                                                                .build();
                                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                                .ephemeral(false)
                                                                .content(response_content)
                                                            )).await?;
                                                        }
                                                        cal_event.race.save(&mut transaction).await?;
                                                        transaction.commit().await?;
                                                    })
                                                } else {
                                                    cal_event.race.save(&mut transaction).await?;
                                                    let overlapping_maintenance_windows = if let RaceHandleMode::RaceTime = cal_event.should_create_room(&mut transaction, &event).await? {
                                                        sqlx::query_as!(Range::<DateTime<Utc>>, r#"SELECT start, end_time AS "end" FROM racetime_maintenance WHERE start < $1 AND end_time > $2"#, start + event.series.default_race_duration(), start - TimeDelta::minutes(30)).fetch_all(&mut *transaction).await?
                                                    } else {
                                                        Vec::default()
                                                    };
                                                    transaction.commit().await?;
                                                    let response_content = if_chain! {
                                                        if let French = event.language;
                                                        if cal_event.race.game.is_none();
                                                        if overlapping_maintenance_windows.is_empty();
                                                        then {
                                                            MessageBuilder::default()
                                                                .push("Votre race a été planifiée pour le ")
                                                                .push_timestamp(start, serenity_utils::message::TimestampStyle::LongDateTime)
                                                                .push('.')
                                                                .build()
                                                        } else {
                                                            let mut response_content = MessageBuilder::default();
                                                            response_content.push(if let Some(game) = cal_event.race.game { format!("Game {game}") } else { format!("This race") });
                                                            response_content.push(if was_scheduled { " has been rescheduled for " } else { " is now scheduled for " });
                                                            response_content.push_timestamp(start, serenity_utils::message::TimestampStyle::LongDateTime);
                                                            response_content.push('.');
                                                            for window in overlapping_maintenance_windows {
                                                                response_content.push_line("");
                                                                response_content.push_bold("Warning:");
                                                                response_content.push(" this race may overlap with racetime.gg maintenance planned for ");
                                                                response_content.push_timestamp(window.start, serenity_utils::message::TimestampStyle::ShortDateTime);
                                                                response_content.push(" until ");
                                                                response_content.push_timestamp(window.end, serenity_utils::message::TimestampStyle::ShortDateTime);
                                                                response_content.push('.');
                                                            }
                                                            response_content.build()
                                                        }
                                                    };
                                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                        .ephemeral(false)
                                                        .content(response_content)
                                                    )).await?;
                                                }
                                            }
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content(if let French = event.language {
                                                    "Désolé, cela n'est pas un timestamp au format de Discord. Vous pouvez utiliser <https://hammertime.cyou/> pour en générer un."
                                                } else {
                                                    "Sorry, that doesn't look like a Discord timestamp. You can use <https://hammertime.cyou/> to generate one."
                                                })
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
                                    } else {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(if let French = event.language {
                                                "Désolé, seuls les participants de cette race et les organisateurs peuvent utiliser cette commande."
                                            } else {
                                                "Sorry, only participants in this race and organizers can use this command."
                                            })
                                        )).await?;
                                        transaction.rollback().await?;
                                    },
                                    SchedulingBackend::SpeedGamingOnline(speedgaming_slug) => {
                                        let response_content = if was_scheduled {
                                            format!("Please contact a tournament organizer to reschedule this race.")
                                        } else {
                                            MessageBuilder::default()
                                                .push("Please use <https://speedgaming.org/")
                                                .push(speedgaming_slug)
                                                .push("/submit> to schedule races for this event.")
                                                .build()
                                        };
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(response_content)
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                    SchedulingBackend::SpeedGamingInPerson => {
                                        let response_content = if was_scheduled {
                                            format!("Please contact a tournament organizer to reschedule this race.")
                                        } else {
                                            format!("Please use <https://onsite.speedgaming.org/?tab=Player> to schedule races for this event.")
                                        };
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(response_content)
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                }
                            }
                        } else if interaction.data.id == command_ids.schedule_async {
                            let game = interaction.data.options.get(1).map(|option| match option.value {
                                CommandDataOptionValue::Integer(game) => i16::try_from(game).expect("game number out of range"),
                                _ => panic!("unexpected slash command option type"),
                            });
                            if let Some((mut transaction, mut race, team)) = check_scheduling_thread_permissions(ctx, interaction, game, true, None).await? {
                                let event = race.event(&mut transaction).await?;
                                let is_organizer = event.organizers(&mut transaction).await?.into_iter().any(|organizer| organizer.discord.is_some_and(|discord| discord.id == interaction.user.id));
                                let was_scheduled = !matches!(race.schedule, RaceSchedule::Unscheduled);
                                match event.scheduling_backend(&mut transaction).await? {
                                    SchedulingBackend::MidosHouse => if team.is_some() && event.asyncs_allowed() || is_organizer {
                                        let start = match interaction.data.options[0].value {
                                            CommandDataOptionValue::String(ref start) => start,
                                            _ => panic!("unexpected slash command option type"),
                                        };
                                        if let Some(start) = parse_timestamp(start) {
                                            if (start - Utc::now()).to_std().map_or(true, |schedule_notice| schedule_notice < event.min_schedule_notice) {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content(if event.min_schedule_notice <= Duration::default() {
                                                        if let French = event.language {
                                                            format!("Désolé mais cette date est dans le passé.")
                                                        } else {
                                                            format!("Sorry, that timestamp is in the past.")
                                                        }
                                                    } else {
                                                        if let French = event.language {
                                                            format!("Désolé, les races doivent être planifiées au moins {} en avance.", French.format_duration(event.min_schedule_notice, true))
                                                        } else {
                                                            format!("Sorry, races must be scheduled at least {} in advance.", English.format_duration(event.min_schedule_notice, true))
                                                        }
                                                    })
                                                )).await?;
                                                transaction.rollback().await?;
                                            } else {
                                                let (kind, was_scheduled) = match race.entrants {
                                                    Entrants::Two([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2)]) => {
                                                        if team.as_ref().is_some_and(|team| team1 == team) {
                                                            let was_scheduled = race.schedule.set_async_start1(start).is_some();
                                                            race.schedule_updated_at = Some(Utc::now());
                                                            (cal::EventKind::Async1, was_scheduled)
                                                        } else if team.as_ref().is_some_and(|team| team2 == team) {
                                                            let was_scheduled = race.schedule.set_async_start2(start).is_some();
                                                            race.schedule_updated_at = Some(Utc::now());
                                                            (cal::EventKind::Async2, was_scheduled)
                                                        } else {
                                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                                .ephemeral(true)
                                                                .content("Sorry, only participants in this race can use this command for now. Please contact Fenhl to edit the schedule.") //TODO allow TOs to schedule as async (with team parameter)
                                                            )).await?;
                                                            transaction.rollback().await?;
                                                            return Ok(())
                                                        }
                                                    }
                                                    Entrants::Three([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2), Entrant::MidosHouseTeam(ref team3)]) => {
                                                        if team.as_ref().is_some_and(|team| team1 == team) {
                                                            let was_scheduled = race.schedule.set_async_start1(start).is_some();
                                                            race.schedule_updated_at = Some(Utc::now());
                                                            (cal::EventKind::Async1, was_scheduled)
                                                        } else if team.as_ref().is_some_and(|team| team2 == team) {
                                                            let was_scheduled = race.schedule.set_async_start2(start).is_some();
                                                            race.schedule_updated_at = Some(Utc::now());
                                                            (cal::EventKind::Async2, was_scheduled)
                                                        } else if team.as_ref().is_some_and(|team| team3 == team) {
                                                            let was_scheduled = race.schedule.set_async_start3(start).is_some();
                                                            race.schedule_updated_at = Some(Utc::now());
                                                            (cal::EventKind::Async3, was_scheduled)
                                                        } else {
                                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                                .ephemeral(true)
                                                                .content("Sorry, only participants in this race can use this command for now. Please contact Fenhl to edit the schedule.") //TODO allow TOs to schedule as async (with team parameter)
                                                            )).await?;
                                                            transaction.rollback().await?;
                                                            return Ok(())
                                                        }
                                                    }
                                                    _ => panic!("tried to schedule race with not 2 or 3 MH teams as async"),
                                                };
                                                let mut cal_event = cal::Event { race, kind };
                                                if start - Utc::now() < TimeDelta::minutes(30) {
                                                    let (http_client, new_room_lock, racetime_host, racetime_config, extra_room_tx, clean_shutdown) = {
                                                        let data = ctx.data.read().await;
                                                        (
                                                            data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone(),
                                                            data.get::<NewRoomLock>().expect("new room lock missing from Discord context").clone(),
                                                            data.get::<RacetimeHost>().expect("racetime.gg host missing from Discord context").clone(),
                                                            data.get::<ConfigRaceTime>().expect("racetime.gg config missing from Discord context").clone(),
                                                            data.get::<ExtraRoomTx>().expect("extra room sender missing from Discord context").clone(),
                                                            data.get::<CleanShutdown>().expect("clean shutdown state missing from Discord context").clone(),
                                                        )
                                                    };
                                                    lock!(new_room_lock = new_room_lock; {
                                                        let should_post_regular_response = if let Some((is_room_url, mut msg)) = racetime_bot::create_room(&mut transaction, ctx, &racetime_host, &racetime_config.client_id, &racetime_config.client_secret, &extra_room_tx, &http_client, clean_shutdown, &mut cal_event, &event).await? {
                                                            if is_room_url && cal_event.is_private_async_part() {
                                                                msg = match cal_event.race.entrants {
                                                                    Entrants::Two(_) => format!("unlisted room for first async half: {msg}"),
                                                                    Entrants::Three(_) => format!("unlisted room for first/second async part: {msg}"),
                                                                    _ => format!("unlisted room for async part: {msg}"),
                                                                };
                                                                if let Some(channel) = event.discord_organizer_channel {
                                                                    channel.say(ctx, &msg).await?;
                                                                } else {
                                                                    FENHL.create_dm_channel(ctx).await?.say(ctx, &msg).await?;
                                                                }
                                                            } else {
                                                                if let Some(channel) = event.discord_race_room_channel {
                                                                    channel.send_message(ctx, CreateMessage::default().content(&msg).allowed_mentions(CreateAllowedMentions::default())).await?;
                                                                }
                                                            }
                                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                                .ephemeral(cal_event.is_private_async_part()) //TODO create public response without room link
                                                                .content(msg)
                                                            )).await?;
                                                            cal_event.is_private_async_part()
                                                        } else {
                                                            true
                                                        };
                                                        if should_post_regular_response {
                                                            let mut response_content = MessageBuilder::default();
                                                            response_content.push(if let Entrants::Two(_) = cal_event.race.entrants { "Your half of " } else { "Your part of " });
                                                            response_content.push(if let Some(game) = cal_event.race.game { format!("game {game}") } else { format!("this race") });
                                                            response_content.push(if was_scheduled { " has been rescheduled for " } else { " is now scheduled for " });
                                                            response_content.push_timestamp(start, serenity_utils::message::TimestampStyle::LongDateTime);
                                                            let response_content = response_content
                                                                .push(". The race room will be opened momentarily.")
                                                                .build();
                                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                                .ephemeral(false)
                                                                .content(response_content)
                                                            )).await?;
                                                        }
                                                        cal_event.race.save(&mut transaction).await?;
                                                        transaction.commit().await?;
                                                    });
                                                } else {
                                                    cal_event.race.save(&mut transaction).await?;
                                                    let overlapping_maintenance_windows = if let RaceHandleMode::RaceTime = cal_event.should_create_room(&mut transaction, &event).await? {
                                                        sqlx::query_as!(Range::<DateTime<Utc>>, r#"SELECT start, end_time AS "end" FROM racetime_maintenance WHERE start < $1 AND end_time > $2"#, start + event.series.default_race_duration(), start - TimeDelta::minutes(30)).fetch_all(&mut *transaction).await?
                                                    } else {
                                                        Vec::default()
                                                    };
                                                    transaction.commit().await?;
                                                    let response_content = if_chain! {
                                                        if let French = event.language;
                                                        if cal_event.race.game.is_none();
                                                        if overlapping_maintenance_windows.is_empty();
                                                        then {
                                                            MessageBuilder::default()
                                                                .push("La partie de votre async a été planifiée pour le ")
                                                                .push_timestamp(start, serenity_utils::message::TimestampStyle::LongDateTime)
                                                                .push('.')
                                                                .build()
                                                        } else {
                                                            let mut response_content = MessageBuilder::default();
                                                            response_content.push(if let Entrants::Two(_) = cal_event.race.entrants { "Your half of " } else { "Your part of " });
                                                            response_content.push(if let Some(game) = cal_event.race.game { format!("game {game}") } else { format!("this race") });
                                                            response_content.push(if was_scheduled { " has been rescheduled for " } else { " is now scheduled for " });
                                                            response_content.push_timestamp(start, serenity_utils::message::TimestampStyle::LongDateTime);
                                                            response_content.push('.');
                                                            for window in overlapping_maintenance_windows {
                                                                response_content.push_line("");
                                                                response_content.push_bold("Warning:");
                                                                if let Entrants::Two(_) = cal_event.race.entrants {
                                                                    response_content.push(" this async half may overlap with racetime.gg maintenance planned for ");
                                                                } else {
                                                                    response_content.push(" this async part may overlap with racetime.gg maintenance planned for ");
                                                                }
                                                                response_content.push_timestamp(window.start, serenity_utils::message::TimestampStyle::ShortDateTime);
                                                                response_content.push(" until ");
                                                                response_content.push_timestamp(window.end, serenity_utils::message::TimestampStyle::ShortDateTime);
                                                                response_content.push('.');
                                                            }
                                                            response_content.build()
                                                        }
                                                    };
                                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                        .ephemeral(false)
                                                        .content(response_content)
                                                    )).await?;
                                                }
                                            }
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content(if let French = event.language {
                                                    "Désolé, cela n'est pas un timestamp au format de Discord. Vous pouvez utiliser <https://hammertime.cyou/> pour en générer un."
                                                } else {
                                                    "Sorry, that doesn't look like a Discord timestamp. You can use <https://hammertime.cyou/> to generate one."
                                                })
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
                                    } else {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(if event.asyncs_allowed() {
                                                if let French = event.language {
                                                    "Désolé, seuls les participants de cette race et les organisateurs peuvent utiliser cette commande."
                                                } else {
                                                    "Sorry, only participants in this race and organizers can use this command."
                                                }
                                            } else {
                                                "Sorry, asyncing races is not allowed for this event."
                                            })
                                        )).await?;
                                        transaction.rollback().await?;
                                    },
                                    SchedulingBackend::SpeedGamingOnline(speedgaming_slug) => {
                                        let response_content = if was_scheduled {
                                            format!("Please contact a tournament organizer to reschedule this race.")
                                        } else {
                                            MessageBuilder::default()
                                                .push("Please use <https://speedgaming.org/")
                                                .push(speedgaming_slug)
                                                .push("/submit> to schedule races for this event.")
                                                .build()
                                        };
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(response_content)
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                    SchedulingBackend::SpeedGamingInPerson => {
                                        let response_content = if was_scheduled {
                                            format!("Please contact a tournament organizer to reschedule this race.")
                                        } else {
                                            format!("Please use <https://onsite.speedgaming.org/?tab=Player> to schedule races for this event.")
                                        };
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(response_content)
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                }
                            }
                        } else if interaction.data.id == command_ids.schedule_remove {
                            let game = interaction.data.options.get(0).map(|option| match option.value {
                                CommandDataOptionValue::Integer(game) => i16::try_from(game).expect("game number out of range"),
                                _ => panic!("unexpected slash command option type"),
                            });
                            if let Some((mut transaction, race, team)) = check_scheduling_thread_permissions(ctx, interaction, game, true, None).await? {
                                let event = race.event(&mut transaction).await?;
                                let is_organizer = event.organizers(&mut transaction).await?.into_iter().any(|organizer| organizer.discord.is_some_and(|discord| discord.id == interaction.user.id));
                                match event.scheduling_backend(&mut transaction).await? {
                                    SchedulingBackend::MidosHouse => if team.is_some() || is_organizer {
                                        match race.schedule {
                                            RaceSchedule::Unscheduled => {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content(if let French = event.language {
                                                        "Désolé, cette race n'a pas de date de début prévue."
                                                    } else {
                                                        "Sorry, this race already doesn't have a starting time."
                                                    })
                                                )).await?;
                                                transaction.rollback().await?;
                                            }
                                            RaceSchedule::Live { .. } => {
                                                sqlx::query!("UPDATE races SET start = NULL, schedule_updated_at = NOW() WHERE id = $1", race.id as _).execute(&mut *transaction).await?;
                                                transaction.commit().await?;
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(false)
                                                    .content(if let Some(game) = race.game {
                                                        format!("Game {game}'s starting time has been removed from the schedule.")
                                                    } else {
                                                        if let French = event.language {
                                                            format!("L'horaire pour cette race ou cette async a été correctement retirée.")
                                                        } else {
                                                            format!("This race's starting time has been removed from the schedule.")
                                                        }
                                                    })
                                                )).await?;
                                            }
                                            RaceSchedule::Async { .. } => match race.entrants {
                                                Entrants::Two([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2)]) => {
                                                    if team.as_ref().is_some_and(|team| team1 == team) {
                                                        sqlx::query!("UPDATE races SET async_start1 = NULL, schedule_updated_at = NOW() WHERE id = $1", race.id as _).execute(&mut *transaction).await?;
                                                    } else if team.as_ref().is_some_and(|team| team2 == team) {
                                                        sqlx::query!("UPDATE races SET async_start2 = NULL, schedule_updated_at = NOW() WHERE id = $1", race.id as _).execute(&mut *transaction).await?;
                                                    } else {
                                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                            .ephemeral(true)
                                                            .content("Sorry, only participants in this race can use this command for now. Please contact Fenhl to edit the schedule.") //TODO allow TOs to edit asynced schedules (with team parameter)
                                                        )).await?;
                                                        transaction.rollback().await?;
                                                        return Ok(())
                                                    }
                                                    transaction.commit().await?;
                                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                        .ephemeral(false)
                                                        .content(if let Some(game) = race.game {
                                                            format!("The starting time for your half of game {game} has been removed from the schedule.")
                                                        } else {
                                                            format!("The starting time for your half of this race has been removed from the schedule.")
                                                        })
                                                    )).await?;
                                                }
                                                Entrants::Three([Entrant::MidosHouseTeam(ref team1), Entrant::MidosHouseTeam(ref team2), Entrant::MidosHouseTeam(ref team3)]) => {
                                                    if team.as_ref().is_some_and(|team| team1 == team) {
                                                        sqlx::query!("UPDATE races SET async_start1 = NULL, schedule_updated_at = NOW() WHERE id = $1", race.id as _).execute(&mut *transaction).await?;
                                                    } else if team.as_ref().is_some_and(|team| team2 == team) {
                                                        sqlx::query!("UPDATE races SET async_start2 = NULL, schedule_updated_at = NOW() WHERE id = $1", race.id as _).execute(&mut *transaction).await?;
                                                    } else if team.as_ref().is_some_and(|team| team3 == team) {
                                                        sqlx::query!("UPDATE races SET async_start3 = NULL, schedule_updated_at = NOW() WHERE id = $1", race.id as _).execute(&mut *transaction).await?;
                                                    } else {
                                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                            .ephemeral(true)
                                                            .content("Sorry, only participants in this race can use this command for now. Please contact Fenhl to edit the schedule.") //TODO allow TOs to edit asynced schedules (with team parameter)
                                                        )).await?;
                                                        transaction.rollback().await?;
                                                        return Ok(())
                                                    }
                                                    transaction.commit().await?;
                                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                        .ephemeral(false)
                                                        .content(if let Some(game) = race.game {
                                                            format!("The starting time for your part of game {game} has been removed from the schedule.")
                                                        } else {
                                                            format!("The starting time for your part of this race has been removed from the schedule.")
                                                        })
                                                    )).await?;
                                                }
                                                _ => panic!("found race with not 2 or 3 MH teams scheduled as async"),
                                            },
                                        }
                                    } else {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(if let French = event.language {
                                                "Désolé, seuls les participants de cette race et les organisateurs peuvent utiliser cette commande."
                                            } else {
                                                "Sorry, only participants in this race and organizers can use this command."
                                            })
                                        )).await?;
                                        transaction.rollback().await?;
                                    },
                                    SchedulingBackend::SpeedGamingOnline(_) | SchedulingBackend::SpeedGamingInPerson => {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Please contact a tournament organizer to reschedule this race.")
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                }
                            }
                        } else if Some(interaction.data.id) == command_ids.second {
                            if let Some((_, mut race, draft_kind, msg_ctx)) = check_draft_permissions(ctx, interaction).await? {
                                match draft_kind {
                                    draft::Kind::RslS7 => {
                                        let settings = &mut race.draft.as_mut().unwrap().settings;
                                        let lite = interaction.data.options.get(0).map(|option| match option.value {
                                            CommandDataOptionValue::Boolean(lite) => lite,
                                            _ => panic!("unexpected slash command option type"),
                                        });
                                        if settings.get("lite_ok").map(|lite_ok| &**lite_ok).unwrap_or("no") == "ok" {
                                            let mut transaction = msg_ctx.into_transaction();
                                            if let Some(lite) = lite {
                                                settings.insert(Cow::Borrowed("preset"), Cow::Borrowed(if lite { "lite" } else { "league" }));
                                                sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(race.draft.as_ref().unwrap()) as _, race.id as _).execute(&mut *transaction).await?;
                                                transaction.commit().await?;
                                            } else {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content(MessageBuilder::default().push("Sorry, please specify the ").push_mono("lite").push(" parameter.").build())
                                                )).await?;
                                                transaction.rollback().await?;
                                                return Ok(())
                                            }
                                        } else {
                                            if lite.is_some_and(identity) {
                                                //TODO different error messages depending on which player(s) didn't opt into RSL-Lite
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content("Sorry, either you or your opponent didn't opt into RSL-Lite.")
                                                )).await?;
                                                return Ok(())
                                            }
                                        }
                                    }
                                    draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4 | draft::Kind::TournoiFrancoS5 => {
                                        let settings = &mut race.draft.as_mut().unwrap().settings;
                                        let mq = interaction.data.options.get(0).map(|option| match option.value {
                                            CommandDataOptionValue::Integer(mq) => u8::try_from(mq).expect("MQ count out of range"),
                                            _ => panic!("unexpected slash command option type"),
                                        });
                                        if settings.get("mq_ok").map(|mq_ok| &**mq_ok).unwrap_or("no") == "ok" {
                                            let mut transaction = msg_ctx.into_transaction();
                                            if let Some(mq) = mq {
                                                settings.insert(Cow::Borrowed("mq_dungeons_count"), Cow::Owned(mq.to_string()));
                                                sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(race.draft.as_ref().unwrap()) as _, race.id as _).execute(&mut *transaction).await?;
                                                transaction.commit().await?;
                                            } else {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content("Désolé, veuillez entrer le nombre de donjons MQ d'abord.")
                                                )).await?;
                                                transaction.rollback().await?;
                                                return Ok(())
                                            }
                                        } else {
                                            if mq.is_some_and(|mq| mq != 0) {
                                                //TODO different error messages depending on which player(s) didn't opt into MQ
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content("Désolé, mais l'un d'entre vous n'a pas choisi les donjons MQ.")
                                                )).await?;
                                                return Ok(())
                                            }
                                        }
                                    }
                                    draft::Kind::S7 | draft::Kind::MultiworldS3 | draft::Kind::MultiworldS4 | draft::Kind::MultiworldS5 => {}
                                }
                                draft_action(ctx, interaction, draft::Action::GoFirst(false)).await?;
                            }
                        } else if Some(interaction.data.id) == command_ids.skip {
                            draft_action(ctx, interaction, draft::Action::Skip).await?;
                        } else if interaction.data.id == command_ids.status {
                            if let Some((mut transaction, race, team)) = check_scheduling_thread_permissions(ctx, interaction, None, true, None).await? {
                                let event = race.event(&mut transaction).await?;
                                if let Some(draft_kind) = event.draft_kind() {
                                    if let Some(ref draft) = race.draft {
                                        let mut msg_ctx = draft::MessageContext::Discord {
                                            teams: race.teams().cloned().collect(),
                                            team: team.unwrap_or_else(Team::dummy),
                                            transaction, guild_id, command_ids,
                                        };
                                        let response_content = MessageBuilder::default()
                                            //TODO include scheduling status, both for regular races and for asyncs
                                            .push(draft.next_step(draft_kind, race.game, &mut msg_ctx).await?.message)
                                            .build();
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(response_content)
                                        )).await?;
                                        msg_ctx.into_transaction().commit().await?;
                                    } else {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, this command is currently only available for events with settings drafts.") //TODO
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if interaction.data.id == command_ids.watch_roles {
                            let watch_party_channel = match interaction.data.options[0].value {
                                CommandDataOptionValue::Channel(channel) => channel,
                                _ => panic!("unexpected slash command option type"),
                            };
                            let race_rooms_channel = match interaction.data.options[1].value {
                                CommandDataOptionValue::Channel(channel) => channel,
                                _ => panic!("unexpected slash command option type"),
                            };
                            guild_id.create_role(ctx, EditRole::new()
                                .hoist(false)
                                .mentionable(false)
                                .name("restream watcher")
                                .permissions(Permissions::empty())
                            ).await?;
                            let watch_party_role = guild_id.create_role(ctx, EditRole::new()
                                .hoist(false)
                                .mentionable(true)
                                .name("watch party watcher")
                                .permissions(Permissions::empty())
                            ).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(false)
                                .content(MessageBuilder::default()
                                    .push("Click a button below to get notified when a restream or Discord watch party is about to start. Click again to remove it. Multiple selections allowed. If you start watching a race in ")
                                    .mention(&watch_party_channel)
                                    .push(", please ping ")
                                    .mention(&watch_party_role)
                                    .push(". To get notified for ")
                                    .push_italic("all")
                                    .push(" matches, set notifications for ")
                                    .mention(&race_rooms_channel)
                                    .push(" to all messages.")
                                    .build()
                                )
                                .button(CreateButton::new("watchrole_restream").label("restream watcher"))
                                .button(CreateButton::new("watchrole_party").label("watch party watcher"))
                            )).await?;
                        } else if Some(interaction.data.id) == command_ids.yes {
                            draft_action(ctx, interaction, draft::Action::BooleanChoice(true)).await?;
                        } else {
                            panic!("unexpected slash command")
                        }
                    }
                }
                Interaction::Component(interaction) => match &*interaction.data.custom_id {
                    "pronouns_he" => {
                        let member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "he/him").expect("missing “he/him” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role removed.")
                            )).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role added.")
                            )).await?;
                        }
                    }
                    "pronouns_she" => {
                        let member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "she/her").expect("missing “she/her” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role removed.")
                            )).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role added.")
                            )).await?;
                        }
                    }
                    "pronouns_they" => {
                        let member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "they/them").expect("missing “they/them” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role removed.")
                            )).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role added.")
                            )).await?;
                        }
                    }
                    "pronouns_other" => {
                        let member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "other pronouns").expect("missing “other pronouns” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role removed.")
                            )).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role added.")
                            )).await?;
                        }
                    }
                    "racingrole" => {
                        let member = interaction.member.clone().expect("/racing-role called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "racing").expect("missing “racing” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role removed.")
                            )).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role added.")
                            )).await?;
                        }
                    }
                    "watchrole_restream" => {
                        let member = interaction.member.clone().expect("/watch-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "restream watcher").expect("missing “restream watcher” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role removed.")
                            )).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role added.")
                            )).await?;
                        }
                    }
                    "watchrole_party" => {
                        let member = interaction.member.clone().expect("/watch-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "watch party watcher").expect("missing “watch party watcher” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role removed.")
                            )).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content("Role added.")
                            )).await?;
                        }
                    }
                    custom_id => if let Some(page) = custom_id.strip_prefix("ban_page_") {
                        send_draft_settings_page(ctx, interaction, "ban", page.parse().unwrap()).await?;
                    } else if let Some(setting) = custom_id.strip_prefix("ban_setting_") {
                        draft_action(ctx, interaction, draft::Action::Ban { setting: setting.to_owned() }).await?;
                    } else if let Some(page) = custom_id.strip_prefix("draft_page_") {
                        send_draft_settings_page(ctx, interaction, "draft", page.parse().unwrap()).await?;
                    } else if let Some(setting) = custom_id.strip_prefix("draft_setting_") {
                        let Some((event, mut race, draft_kind, mut msg_ctx)) = check_draft_permissions(ctx, interaction).await? else { return Ok(()) };
                        match race.draft.as_ref().unwrap().next_step(draft_kind, race.game, &mut msg_ctx).await?.kind {
                            draft::StepKind::Ban { available_settings, .. } if available_settings.get(setting).is_some() => {
                                let setting = available_settings.get(setting).unwrap(); // `if let` guards are experimental
                                msg_ctx.into_transaction().commit().await?;
                                let response_content = if let French = event.language {
                                    format!("Sélectionnez la configuration du setting {} :", setting.display)
                                } else {
                                    format!("Select the value for the {} setting:", setting.display)
                                };
                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content(response_content)
                                    .button(CreateButton::new(format!("draft_option_{}__{}", setting.name, setting.default)).label(setting.default_display))
                                    .button(CreateButton::new("draft_page_0").label(if let French = event.language { "Retour" } else { "Back" }).style(ButtonStyle::Secondary)) //TODO remember page?
                                )).await?;
                            }
                            draft::StepKind::Pick { available_choices, .. } if available_choices.get(setting).is_some() => {
                                let setting = available_choices.get(setting).unwrap(); // `if let` guards are experimental
                                msg_ctx.into_transaction().commit().await?;
                                let response_content = if let French = event.language {
                                    format!("Sélectionnez la configuration du setting {} :", setting.display)
                                } else {
                                    format!("Select the value for the {} setting:", setting.display)
                                };
                                let mut response_msg = CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content(response_content);
                                for option in setting.options {
                                    response_msg = response_msg.button(CreateButton::new(format!("draft_option_{}__{}", setting.name, option.name)).label(option.display));
                                }
                                response_msg = response_msg.button(CreateButton::new("draft_page_0").label(if let French = event.language { "Retour" } else { "Back" }).style(ButtonStyle::Secondary)); //TODO remember page?
                                interaction.create_response(ctx, CreateInteractionResponse::Message(response_msg)).await?;
                            }
                            | draft::StepKind::GoFirst
                            | draft::StepKind::Ban { .. }
                            | draft::StepKind::Pick { .. }
                            | draft::StepKind::BooleanChoice { .. }
                            | draft::StepKind::Done(_)
                            | draft::StepKind::DoneRsl { .. }
                                => match race.draft.as_mut().unwrap().apply(draft_kind, race.game, &mut msg_ctx, draft::Action::Pick { setting: format!("@placeholder"), value: format!("@placeholder") }).await? {
                                    Ok(_) => unreachable!(),
                                    Err(error_msg) => {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content(error_msg)
                                        )).await?;
                                        msg_ctx.into_transaction().rollback().await?;
                                    }
                                },
                        }
                    } else if let Some((setting, value)) = custom_id.strip_prefix("draft_option_").and_then(|setting_value| setting_value.split_once("__")) {
                        draft_action(ctx, interaction, draft::Action::Pick { setting: setting.to_owned(), value: value.to_owned() }).await?;
                    } else if let Some(speedgaming_id) = custom_id.strip_prefix("sgdisambig_") {
                        let (db_pool, http_client) = {
                            let data = ctx.data.read().await;
                            (
                                data.get::<DbPool>().expect("database connection pool missing from Discord context").clone(),
                                data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone(),
                            )
                        };
                        let mut transaction = db_pool.begin().await?;
                        let speedgaming_id = speedgaming_id.parse()?;
                        let ComponentInteractionDataKind::StringSelect { ref values } = interaction.data.kind else { panic!("sgdisambig interaction with unexpected payload") };
                        let race_id = values.iter().exactly_one().expect("sgdisambig interaction with unexpected payload").parse()?;
                        let mut cal_event = cal::Event { race: Race::from_id(&mut transaction, &http_client, race_id).await?, kind: cal::EventKind::Normal };
                        let event = cal_event.race.event(&mut transaction).await?;
                        let Some(ref speedgaming_slug) = event.speedgaming_slug else { panic!("sgdisambig interaction for race from non-SpeedGaming event") };
                        let schedule = sgl::online_schedule(&http_client, speedgaming_slug).await?;
                        let restream = schedule.into_iter().find(|restream| restream.matches().any(|restream_match| restream_match.id == speedgaming_id)).expect("no such SpeedGaming match ID");
                        transaction = restream.update_race(&db_pool, transaction, ctx, &event, &mut cal_event, speedgaming_id).await?;
                        cal_event.race.save(&mut transaction).await?;
                        transaction.commit().await?;
                    } else if let Some(speedgaming_id) = custom_id.strip_prefix("sgonsitedisambig_") {
                        let (db_pool, http_client) = {
                            let data = ctx.data.read().await;
                            (
                                data.get::<DbPool>().expect("database connection pool missing from Discord context").clone(),
                                data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone(),
                            )
                        };
                        let mut transaction = db_pool.begin().await?;
                        let speedgaming_id = speedgaming_id.parse::<i64>()?;
                        let ComponentInteractionDataKind::StringSelect { ref values } = interaction.data.kind else { panic!("sgonsitedisambig interaction with unexpected payload") };
                        let race_id = values.iter().exactly_one().expect("sgonsitedisambig interaction with unexpected payload").parse()?;
                        let mut cal_event = cal::Event { race: Race::from_id(&mut transaction, &http_client, race_id).await?, kind: cal::EventKind::Normal };
                        let event = cal_event.race.event(&mut transaction).await?;
                        let Some(speedgaming_in_person_id) = event.speedgaming_in_person_id else { panic!("sgonsitedisambig interaction for race from non-SpeedGaming-on-site event") };
                        let schedule = sgl::in_person_schedule(&http_client, speedgaming_in_person_id).await?;
                        let schedule_match = schedule.into_iter().find(|schedule_match| schedule_match.id == speedgaming_id).expect("no such SpeedGaming on-site match ID");
                        transaction = schedule_match.update_race(&db_pool, transaction, ctx, &event, &mut cal_event).await?;
                        cal_event.race.save(&mut transaction).await?;
                        transaction.commit().await?;
                    } else {
                        panic!("received message component interaction with unknown custom ID {custom_id:?}")
                    },
                },
                _ => {}
            }
            Ok(())
        }))
        .on_voice_state_update(|ctx, _, new| Box::pin(async move {
            if let Some(source_channel) = new.channel_id {
                if new.guild_id == Some(MULTIWORLD_GUILD) && all::<Element>().any(|region| region.voice_channel() == source_channel) {
                    let target_channel = ctx.data.read().await.get::<Element>().and_then(|regions| regions.get(&new.user_id)).copied().unwrap_or(Element::Light).voice_channel();
                    if source_channel != target_channel {
                        MULTIWORLD_GUILD.move_member(ctx, new.user_id, target_channel).await?;
                    }
                }
            }
            Ok(())
        }))
        .task(|ctx_fut, _| async move {
            shutdown.await;
            serenity_utils::shut_down(&*ctx_fut.read().await).await;
        })
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)] Draft(#[from] draft::Error),
    #[error(transparent)] EventData(#[from] event::DataError),
    #[error(transparent)] Serenity(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("attempted to create scheduling thread in Discord guild that hasn't been initialized yet")]
    UninitializedDiscordGuild(GuildId),
    #[error("attempted to create scheduling thread in Discord guild without command IDs")]
    UnregisteredDiscordGuild(GuildId),
}

pub(crate) async fn create_scheduling_thread<'a>(ctx: &DiscordCtx, mut transaction: Transaction<'a, Postgres>, race: &mut Race, game_count: i16) -> Result<Transaction<'a, Postgres>, Error> {
    let event = race.event(&mut transaction).await?;
    let (Some(guild_id), Some(scheduling_channel)) = (event.discord_guild, event.discord_scheduling_channel) else { return Ok(transaction) };
    let command_ids = match ctx.data.read().await.get::<CommandIds>().and_then(|command_ids| command_ids.get(&guild_id).copied()) {
        None => return Err(Error::UninitializedDiscordGuild(guild_id)),
        Some(None) => return Err(Error::UnregisteredDiscordGuild(guild_id)),
        Some(Some(command_ids)) => command_ids,
    };
    let mut title = if_chain! {
        if let French = event.language;
        if let (Some(phase), Some(round)) = (race.phase.as_ref(), race.round.as_ref());
        if let Some(Some(info_prefix)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut *transaction).await?;
        then {
            match race.entrants {
                Entrants::Open | Entrants::Count { .. } => info_prefix,
                Entrants::Named(ref entrants) => format!("{info_prefix} : {entrants}"),
                Entrants::Two([ref team1, ref team2]) => format!(
                    "{info_prefix} : {} vs {}",
                    team1.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team2.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                ),
                Entrants::Three([ref team1, ref team2, ref team3]) => format!(
                    "{info_prefix} : {} vs {} vs {}",
                    team1.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team2.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team3.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                ),
            }
        } else {
            let info_prefix = format!("{}{}{}",
                race.phase.as_deref().unwrap_or(""),
                if race.phase.is_none() || race.round.is_none() { "" } else { " " },
                race.round.as_deref().unwrap_or(""),
            );
            match race.entrants {
                Entrants::Open | Entrants::Count { .. } => if info_prefix.is_empty() { format!("Untitled Race") } else { info_prefix },
                Entrants::Named(ref entrants) => format!("{info_prefix}{}{entrants}", if info_prefix.is_empty() { "" } else { ": " }),
                Entrants::Two([ref team1, ref team2]) => format!(
                    "{info_prefix}{}{} vs {}",
                    if info_prefix.is_empty() { "" } else { ": " },
                    team1.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team2.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                ),
                Entrants::Three([ref team1, ref team2, ref team3]) => format!(
                    "{info_prefix}{}{} vs {} vs {}",
                    if info_prefix.is_empty() { "" } else { ": " },
                    team1.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team2.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                    team3.name(&mut transaction, ctx).await?.unwrap_or(Cow::Borrowed("(unnamed)")),
                ),
            }
        }
    };
    let mut content = MessageBuilder::default();
    if_chain! {
        if let French = event.language;
        if let (Some(phase), Some(round)) = (race.phase.as_ref(), race.round.as_ref());
        if let Some(Some(phase_round)) = sqlx::query_scalar!("SELECT display_fr FROM phase_round_options WHERE series = $1 AND event = $2 AND phase = $3 AND round = $4", event.series as _, &event.event, phase, round).fetch_optional(&mut *transaction).await?;
        if game_count == 1;
        if event.asyncs_allowed();
        if let None | Some(draft::Kind::TournoiFrancoS3 | draft::Kind::TournoiFrancoS4) = event.draft_kind();
        then {
            for team in race.teams() {
                content.mention_team(&mut transaction, Some(guild_id), team).await?;
                content.push(' ');
            }
            content.push("Bienvenue dans votre ");
            content.push_safe(phase_round);
            content.push(". Veuillez utiliser ");
            content.mention_command(command_ids.schedule, "schedule");
            content.push(" pour schedule votre race en live ou ");
            content.mention_command(command_ids.schedule_async, "schedule-async");
            content.push(" pour schedule votre async. Vous devez insérer un timestamp Discord que vous pouvez créer sur <https://hammertime.cyou/>.");
        } else {
            for team in race.teams() {
                content.mention_team(&mut transaction, Some(guild_id), team).await?;
                content.push(' ');
            }
            content.push("Welcome to your ");
            if let Some(ref phase) = race.phase {
                content.push_safe(phase.clone());
                content.push(' ');
            }
            if let Some(ref round) = race.round {
                content.push_safe(round.clone());
                content.push(' ');
            }
            content.push("match. Use ");
            match event.scheduling_backend(&mut transaction).await? {
                SchedulingBackend::MidosHouse => {
                    content.mention_command(command_ids.schedule, "schedule");
                    if event.asyncs_allowed() {
                        content.push(" to schedule as a live race or ");
                        content.mention_command(command_ids.schedule_async, "schedule-async");
                        content.push(" to schedule as an async. These commands take a Discord timestamp, which you can generate at <https://hammertime.cyou/>.");
                    } else {
                        content.push(" to schedule your race. This command takes a Discord timestamp, which you can generate at <https://hammertime.cyou/>.");
                    }
                    if game_count > 1 {
                        content.push(" You can use the ");
                        content.push_mono("game:");
                        content.push(" parameter with these commands to schedule subsequent games ahead of time.");
                    }
                }
                SchedulingBackend::SpeedGamingOnline(speedgaming_slug) =>  {
                    content.push("<https://speedgaming.org/");
                    content.push(speedgaming_slug);
                    if game_count > 1 {
                        content.push("/submit> to schedule your races.");
                    } else {
                        content.push("/submit> to schedule your race.");
                    }
                }
                SchedulingBackend::SpeedGamingInPerson => if game_count > 1 {
                    content.push("<https://onsite.speedgaming.org/?tab=Player> to schedule your races.");
                } else {
                    content.push("<https://onsite.speedgaming.org/?tab=Player> to schedule your race.");
                },
            }
        }
    };
    if title.len() > 100 {
        // Discord thread titles are limited to 100 characters, unclear on specifics, limit to 100 bytes to be safe
        let mut cutoff = 100 - "[…]".len();
        while !title.is_char_boundary(cutoff) { cutoff -= 1 }
        title.truncate(cutoff);
        title.push_str("[…]");
    }
    if let Some(draft_kind) = event.draft_kind() {
        if let Some(ref draft) = race.draft {
            let mut msg_ctx = draft::MessageContext::Discord {
                teams: race.teams().cloned().collect(),
                team: Team::dummy(),
                transaction, guild_id, command_ids,
            };
            content.push_line("");
            content.push_line("");
            content.push(draft.next_step(draft_kind, race.game, &mut msg_ctx).await?.message);
            transaction = msg_ctx.into_transaction();
        }
    }
    race.scheduling_thread = Some(if let Some(ChannelType::Forum) = scheduling_channel.to_channel(ctx).await?.guild().map(|c| c.kind) {
        scheduling_channel.create_forum_post(ctx, CreateForumPost::new(
            title,
            CreateMessage::default().content(content.build()),
        ).auto_archive_duration(AutoArchiveDuration::OneWeek)).await?.id
    } else {
        let thread = scheduling_channel.create_thread(ctx, CreateThread::new(
            title,
        ).kind(ChannelType::PublicThread).auto_archive_duration(AutoArchiveDuration::OneWeek)).await?;
        thread.say(ctx, content.build()).await?;
        thread.id
    });
    Ok(transaction)
}

pub(crate) async fn handle_race() {
    //TODO start Discord race handler (in DMs for private async parts, in scheduling thread for public ones):
    // * post seed 15 minutes before start
    // * reminder to go live for public async parts
    // * Ready button to post password and start countdown
    // * Done/Forfeit/FPA buttons
    // * reminder to send vod to organizers
}
