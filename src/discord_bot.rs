use {
    std::collections::HashMap,
    chrono::{
        Duration,
        prelude::*,
    },
    enum_iterator::all,
    itertools::Itertools as _,
    lazy_regex::regex_captures,
    serde::{
        Deserialize,
        Serialize,
    },
    serenity::{
        all::{
            CreateButton,
            CreateCommand,
            CreateCommandOption,
            CreateInteractionResponse,
            CreateInteractionResponseMessage,
            EditRole,
            MessageBuilder,
        },
        model::prelude::*,
        prelude::*,
    },
    serenity_utils::{
        builder::ErrorNotifier,
        handler::HandlerMethods as _,
    },
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
        types::Json,
    },
    crate::{
        Environment,
        cal::{
            Entrant,
            Entrants,
            Race,
            RaceSchedule,
        },
        config::Config,
        event::{
            self,
            MatchSource,
            Series,
            mw,
        },
        team::Team,
        util::{
            Id,
            IdTable,
            MessageBuilderExt as _,
        },
    },
};

const FENHL: UserId = UserId::new(86841168427495424);

enum DbPool {}

impl TypeMapKey for DbPool {
    type Value = PgPool;
}

enum HttpClient {}

impl TypeMapKey for HttpClient {
    type Value = reqwest::Client;
}

enum StartggToken {}

impl TypeMapKey for StartggToken {
    type Value = String;
}

#[derive(Clone, Copy)]
pub(crate) struct CommandIds {
    assign: CommandId,
    ban: Option<CommandId>,
    draft: Option<CommandId>,
    first: Option<CommandId>,
    post_status: CommandId,
    pronoun_roles: Option<CommandId>, //TODO control via event config
    pub(crate) schedule: CommandId,
    pub(crate) schedule_async: CommandId,
    pub(crate) schedule_remove: CommandId,
    second: Option<CommandId>,
    skip: Option<CommandId>,
    status: CommandId,
    watch_roles: Option<CommandId>, //TODO control via event config
}

impl TypeMapKey for CommandIds {
    type Value = HashMap<GuildId, CommandIds>;
}

pub(crate) enum DraftKind {
    None,
    MultiworldS3,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Draft { //TODO move to cal.rs?
    pub(crate) high_seed: Id,
    pub(crate) state: mw::S3Draft,
}

impl Draft {
    /// Assumes that the caller has checked that the team is part of the race in the first place.
    fn is_active_team(&self, team: Id) -> bool {
        match self.state.active_team() {
            Some(mw::Team::HighSeed) => team == self.high_seed,
            Some(mw::Team::LowSeed) => team != self.high_seed,
            None => false,
        }
    }

    async fn next_step(&self, transaction: &mut Transaction<'_, Postgres>, guild: GuildId, command_ids: &CommandIds, teams: impl Iterator<Item = &Team>) -> sqlx::Result<String> {
        let (mut high_seed, mut low_seed) = teams.partition::<Vec<_>, _>(|team| team.id == self.high_seed);
        let high_seed = high_seed.remove(0);
        let low_seed = low_seed.remove(0);
        Ok(match self.state.next_step() {
            mw::DraftStep::GoFirst => MessageBuilder::default()
                .mention_team(transaction, guild, high_seed).await?
                .push(": you have the higher seed. Choose whether you want to go ")
                .mention_command(command_ids.first.unwrap(), "first")
                .push(" or ")
                .mention_command(command_ids.second.unwrap(), "second")
                .push(" in the settings draft.")
                .build(),
            mw::DraftStep::Ban { team, .. } => MessageBuilder::default()
                .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                .push(": lock a setting to its default using ")
                .mention_command(command_ids.ban.unwrap(), "ban")
                .push(", or use ")
                .mention_command(command_ids.skip.unwrap(), "skip")
                .push(" if you don't want to ban anything.")
                .build(),
            mw::DraftStep::Pick { prev_picks, team } => match prev_picks {
                0 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick a setting using ")
                    .mention_command(command_ids.draft.unwrap(), "draft")
                    .push('.')
                    .build(),
                1 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick a setting using ")
                    .mention_command(command_ids.draft.unwrap(), "draft")
                    .push(". You will have another pick after this.")
                    .build(),
                2 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick your second setting using ")
                    .mention_command(command_ids.draft.unwrap(), "draft")
                    .push('.')
                    .build(),
                3 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick a setting using ")
                    .mention_command(command_ids.draft.unwrap(), "draft")
                    .push(". You can also use ")
                    .mention_command(command_ids.skip.unwrap(), "skip")
                    .push(" if you want to leave the settings as they are.")
                    .build(),
                _ => unreachable!(),
            },
            mw::DraftStep::Done(settings) => format!("Settings draft completed. You will be playing with {settings}."),
        })
    }
}

async fn check_scheduling_thread_permissions<'a>(ctx: &'a Context, interaction: &CommandInteraction) -> Result<Option<(Transaction<'a, Postgres>, Race, Option<Team>)>, Box<dyn std::error::Error + Send + Sync>> {
    let (mut transaction, http_client, startgg_token) = {
        let data = ctx.data.read().await;
        (
            data.get::<DbPool>().expect("database connection pool missing from Discord context").begin().await?,
            data.get::<HttpClient>().expect("HTTP client missing from Discord context").clone(),
            data.get::<StartggToken>().expect("start.gg auth token missing from Discord context").clone(),
        )
    };
    Ok(match Race::for_scheduling_channel(&mut transaction, &http_client, &startgg_token, interaction.channel_id).await?.into_iter().at_most_one() {
        Ok(None) => {
            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                .ephemeral(true)
                .content("Sorry, this thread is not associated with any upcoming races. Please contact a tournament organizer to fix this.")
            )).await?;
            transaction.rollback().await?;
            None
        }
        Ok(Some(race)) => {
            let mut team = None;
            for iter_team in race.teams() {
                if iter_team.members(&mut transaction).await?.into_iter().any(|member| member.discord_id == Some(interaction.user.id)) {
                    team = Some(iter_team.clone());
                }
            }
            if let Some(ref team) = team {
                if race.has_room_for(team) {
                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                        .ephemeral(true)
                        .content("Sorry, this command can't be used since a race room is already open.")
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
                .content("Sorry, this thread is associated with multiple upcoming races. Please contact a tournament organizer to fix this.") //TODO allow selecting race via optional parameter
            )).await?;
            transaction.rollback().await?;
            None
        }
    })
}

fn parse_timestamp(timestamp: &str) -> Option<DateTime<Utc>> {
    regex_captures!("^<t:(-?[0-9]+)(?::[tTdDfFR])?>$", timestamp)
        .and_then(|(_, timestamp)| timestamp.parse().ok())
        .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
}

pub(crate) fn configure_builder(discord_builder: serenity_utils::Builder, db_pool: PgPool, http_client: reqwest::Client, config: Config, env: Environment, shutdown: rocket::Shutdown) -> serenity_utils::Builder {
    discord_builder
        .error_notifier(ErrorNotifier::User(FENHL))
        .data::<DbPool>(db_pool)
        .data::<HttpClient>(http_client)
        .data::<StartggToken>(if env.is_dev() { config.startgg_dev } else { config.startgg_production })
        .on_guild_create(false, |ctx, guild, _| Box::pin(async move {
            let mut transaction = ctx.data.read().await.get::<DbPool>().expect("database connection pool missing from Discord context").begin().await?;
            let guild_event_rows = sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE discord_guild = $1"#, i64::from(guild.id)).fetch_all(&mut transaction).await?; //TODO only use ongoing and upcoming events
            let mut guild_events = Vec::with_capacity(guild_event_rows.len());
            for row in guild_event_rows {
                guild_events.push(event::Data::new(&mut transaction, row.series, row.event).await?.expect("just received from database"));
            }
            // if different kinds of draft are added in the future, this has to be refactored since they'll require different slash command setups
            // (and we'll have to make sure there's no conflicting draft kinds in the same guild)
            let has_draft = guild_events.iter().any(|event| match event.draft_kind() {
                DraftKind::MultiworldS3 => true,
                DraftKind::None => false,
            });
            let match_source = all().find(|match_source| guild_events.iter().all(|event| event.match_source() == *match_source));
            let assign = match match_source {
                Some(MatchSource::Manual) => guild.create_application_command(ctx, CreateCommand::new("assign")
                    .kind(CommandType::ChatInput)
                    .default_member_permissions(Permissions::ADMINISTRATOR) //TODO allow TOs to use this
                    .dm_permission(false)
                    .description("Marks this thread as the scheduling thread for the given game of the match.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "game",
                        "The game number within the match.",
                    )
                        .min_int_value(1)
                        .max_int_value(255)
                        .required(true)
                    )
                    //TODO high-seed option
                ).await?.id,
                Some(MatchSource::StartGG) => guild.create_application_command(ctx, {
                    let mut c = CreateCommand::new("assign")
                        .kind(CommandType::ChatInput)
                        .default_member_permissions(Permissions::ADMINISTRATOR) //TODO allow TOs to use this
                        .dm_permission(false)
                        .description("Marks this thread as the scheduling thread for the given start.gg set.")
                        .add_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "startgg-set", //TODO Challonge support?
                            "The start.gg set (match) ID",
                        ).required(true));
                    if has_draft {
                        c = c.add_option(CreateCommandOption::new(
                            CommandOptionType::Role,
                            "high-seed",
                            "The team that decides which team starts the settings draft. If the teams are tied, flip a coin.",
                        ).required(true));
                    }
                    c.add_option(CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "game",
                        "The game number within the match, if this is a best-of-n-races match.",
                    )
                        .min_int_value(1)
                        .max_int_value(255)
                        .required(false)
                    )
                }).await?.id,
                None => unimplemented!("Discord guilds with mixed match sources not yet supported"),
            };
            let ban = if has_draft {
                Some(guild.create_application_command(ctx, CreateCommand::new("ban")
                    .kind(CommandType::ChatInput)
                    .dm_permission(false)
                    .description("Locks a setting for this match to its default value.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::String,
                        "setting",
                        "The setting to lock in",
                    )
                        .required(true)
                        .add_string_choice("win conditions", "wincon")
                        .add_string_choice("dungeons", "dungeons")
                        .add_string_choice("entrance rando", "er")
                        .add_string_choice("trials", "trials")
                        .add_string_choice("shops", "shops")
                        .add_string_choice("scrubs", "scrubs")
                        .add_string_choice("fountain", "fountain")
                        .add_string_choice("spawns", "spawn")
                    )
                ).await?.id)
            } else {
                None
            };
            let draft = if has_draft {
                Some(guild.create_application_command(ctx, CreateCommand::new("draft")
                    .kind(CommandType::ChatInput)
                    .dm_permission(false)
                    .description("Chooses a setting for this match.")
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "wincon",
                        "win conditions",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for the win condition settings",
                        )
                            .required(true)
                            .add_string_choice("default wincons", "meds")
                            .add_string_choice("Scrubs wincons", "scrubs")
                            .add_string_choice("Triforce Hunt", "th")
                        )
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "dungeons",
                        "dungeons",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for the dungeon item settings",
                        )
                            .required(true)
                            .add_string_choice("tournament dungeons", "tournament")
                            .add_string_choice("dungeon tokens", "skulls")
                            .add_string_choice("keyrings", "keyrings")
                        )
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "er",
                        "entrance rando",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for entrance randomizer",
                        )
                            .required(true)
                            .add_string_choice("no ER", "off")
                            .add_string_choice("dungeon ER", "dungeon")
                        )
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "trials",
                        "trials",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for the Ganon's Trials setting",
                        )
                            .required(true)
                            .add_string_choice("0 trials", "0")
                            .add_string_choice("2 trials", "2")
                        )
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "shops",
                        "shops",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for the Shop Shuffle setting",
                        )
                            .required(true)
                            .add_string_choice("shops 4", "4")
                            .add_string_choice("no shops", "off")
                        )
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "scrubs",
                        "scrubs",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for the Scrub Shuffle setting",
                        )
                            .required(true)
                            .add_string_choice("affordable scrubs", "affordable")
                            .add_string_choice("no scrubs", "off")
                        )
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "fountain",
                        "fountain",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for the Zora's Fountain setting",
                        )
                            .required(true)
                            .add_string_choice("closed fountain", "closed")
                            .add_string_choice("open fountain", "open")
                        )
                    )
                    .add_option(CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "spawn",
                        "spawns",
                    )
                        .add_sub_option(CreateCommandOption::new(
                            CommandOptionType::String,
                            "value",
                            "Your choice for the spawn settings",
                        )
                            .required(true)
                            .add_string_choice("ToT spawns", "tot")
                            .add_string_choice("random spawns & starting age", "random")
                        )
                    )
                ).await?.id)
            } else {
                None
            };
            let first = if has_draft {
                Some(guild.create_application_command(ctx, CreateCommand::new("first")
                    .kind(CommandType::ChatInput)
                    .dm_permission(false)
                    .description("Go first in the settings draft.")
                ).await?.id)
            } else {
                None
            };
            let post_status = guild.create_application_command(ctx, CreateCommand::new("post-status")
                .kind(CommandType::ChatInput)
                .default_member_permissions(Permissions::ADMINISTRATOR)
                .dm_permission(false)
                .description("Posts this race's status to the thread, pinging the team whose turn it is in the settings draft.")
            ).await?.id;
            let pronoun_roles = if guild.id == mw::GUILD_ID {
                Some(guild.create_application_command(ctx, CreateCommand::new("pronoun-roles")
                    .kind(CommandType::ChatInput)
                    .default_member_permissions(Permissions::ADMINISTRATOR)
                    .dm_permission(false)
                    .description("Creates gender pronoun roles and posts a message here that allows members to self-assign them.")
                ).await?.id)
            } else {
                None
            };
            let schedule = guild.create_application_command(ctx, CreateCommand::new("schedule")
                .kind(CommandType::ChatInput)
                .dm_permission(false)
                .description("Submits a starting time for this race.")
                .add_option(CreateCommandOption::new(
                    CommandOptionType::String,
                    "start",
                    "The starting time as a Discord timestamp",
                ).required(true))
            ).await?.id;
            let schedule_async = guild.create_application_command(ctx, CreateCommand::new("schedule-async")
                .kind(CommandType::ChatInput)
                .dm_permission(false)
                .description("Submits a starting time for your half of this race.")
                .add_option(CreateCommandOption::new(
                    CommandOptionType::String,
                    "start",
                    "The starting time as a Discord timestamp",
                ).required(true))
            ).await?.id;
            let schedule_remove = guild.create_application_command(ctx, CreateCommand::new("schedule-remove")
                .kind(CommandType::ChatInput)
                .dm_permission(false)
                .description("Removes the starting time(s) for this race from the schedule.")
            ).await?.id;
            let second = if has_draft {
                Some(guild.create_application_command(ctx, CreateCommand::new("second")
                    .kind(CommandType::ChatInput)
                    .dm_permission(false)
                    .description("Go second in the settings draft.")
                ).await?.id)
            } else {
                None
            };
            let skip = if has_draft {
                Some(guild.create_application_command(ctx, CreateCommand::new("skip")
                    .kind(CommandType::ChatInput)
                    .dm_permission(false)
                    .description("Skip your ban or the final pick of the settings draft.")
                ).await?.id)
            } else {
                None
            };
            let status = guild.create_application_command(ctx, CreateCommand::new("status")
                .kind(CommandType::ChatInput)
                .dm_permission(false)
                .description("Shows you this race's current scheduling and settings draft status.")
            ).await?.id;
            let watch_roles = if guild.id == mw::GUILD_ID {
                Some(guild.create_application_command(ctx, CreateCommand::new("watch-roles")
                    .kind(CommandType::ChatInput)
                    .default_member_permissions(Permissions::ADMINISTRATOR)
                    .dm_permission(false)
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
                ).await?.id)
            } else {
                None
            };
            ctx.data.write().await
                .entry::<CommandIds>()
                .or_default()
                .insert(guild.id, CommandIds { assign, ban, draft, first, post_status, pronoun_roles, schedule, schedule_async, schedule_remove, second, skip, status, watch_roles });
            transaction.commit().await?;
            Ok(())
        }))
        .on_interaction_create(|ctx, interaction| Box::pin(async move {
            match interaction {
                Interaction::Command(interaction) => {
                    let guild_id = interaction.guild_id.expect("/assign called outside of a guild");
                    if let Some(&command_ids) = ctx.data.read().await.get::<CommandIds>().and_then(|command_ids| command_ids.get(&guild_id)) {
                        if interaction.data.id == command_ids.assign {
                            let mut transaction = ctx.data.read().await.get::<DbPool>().as_ref().expect("database connection pool missing from Discord context").begin().await?;
                            if let Some(event_row) = sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE discord_guild = $1 AND end_time IS NULL"#, i64::from(guild_id)).fetch_optional(&mut transaction).await? {
                                let event = event::Data::new(&mut transaction, event_row.series, event_row.event).await?.expect("just received from database");
                                match event.match_source() {
                                    MatchSource::Manual => unimplemented!(), //TODO
                                    MatchSource::StartGG => {
                                        let startgg_set = match interaction.data.options[0].value {
                                            CommandDataOptionValue::String(ref startgg_set) => startgg_set.clone(),
                                            _ => panic!("unexpected slash command option type"),
                                        };
                                        let high_seed = match event.draft_kind() {
                                            DraftKind::MultiworldS3 => Some(match interaction.data.options[1].value {
                                                CommandDataOptionValue::Role(discord_role) => discord_role,
                                                _ => panic!("unexpected slash command option type"),
                                            }),
                                            DraftKind::None => None,
                                        };
                                        let game = interaction.data.options.get(2).map(|option| match option.value {
                                            CommandDataOptionValue::Integer(game) => i16::try_from(game).expect("game number out of range"),
                                            _ => panic!("unexpected slash command option type"),
                                        });
                                        let high_seed = if let Some(high_seed) = high_seed {
                                            if let Some(high_seed) = Team::from_discord(&mut transaction, high_seed).await? {
                                                Some(high_seed)
                                            } else {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content("Sorry, that doesn't seem to be a team role.")
                                                )).await?;
                                                return Ok(())
                                            }
                                        } else {
                                            None
                                        };
                                        let id = Id::new(&mut transaction, IdTable::Races).await?;
                                        match event.draft_kind() {
                                            DraftKind::MultiworldS3 => sqlx::query!("INSERT INTO races
                                                (id, startgg_set, game, series, event, scheduling_thread, draft_state) VALUES ($1, $2, $3, $4, $5, $6, $7)
                                                ON CONFLICT (startgg_set, game) DO UPDATE SET scheduling_thread = EXCLUDED.scheduling_thread, draft_state = EXCLUDED.draft_state
                                            ", i64::from(id), &startgg_set, game, event.series as _, &event.event, i64::from(interaction.channel_id), Json(Draft {
                                                high_seed: high_seed.as_ref().unwrap().id,
                                                state: mw::S3Draft::default(),
                                            }) as _).execute(&mut transaction).await?,
                                            DraftKind::None => sqlx::query!("INSERT INTO races
                                                (id, startgg_set, game, series, event, scheduling_thread) VALUES ($1, $2, $3, $4, $5, $6)
                                                ON CONFLICT (startgg_set, game) DO UPDATE SET scheduling_thread = EXCLUDED.scheduling_thread
                                            ", i64::from(id), &startgg_set, game, event.series as _, &event.event, i64::from(interaction.channel_id)).execute(&mut transaction).await?,
                                        };
                                        let guild_id = interaction.guild_id.expect("/ban called outside of a guild");
                                        let mut response_content = MessageBuilder::default();
                                        response_content.push("This thread is now assigned to ");
                                        if let Some(game) = game {
                                            response_content.push("game ");
                                            response_content.push(game.to_string());
                                            response_content.push(" of ");
                                        }
                                        response_content.push("set ");
                                        response_content.push_safe(startgg_set); //TODO linkify set page, use phase/round/identifier
                                        response_content.push(". Use ");
                                        response_content.mention_command(command_ids.schedule, "schedule");
                                        response_content.push(" to schedule as a live race or ");
                                        response_content.mention_command(command_ids.schedule_async, "schedule-async");
                                        response_content.push(" to schedule as an async."); //TODO adjust message if asyncing is not allowed
                                        if let (Some(high_seed), Some(first), Some(second)) = (high_seed, command_ids.first, command_ids.second) {
                                            response_content.push_line("");
                                            response_content.mention_team(&mut transaction, guild_id, &high_seed).await?;
                                            response_content.push(": you have the higher seed. Choose whether you want to go ");
                                            response_content.mention_command(first, "first");
                                            response_content.push(" or ");
                                            response_content.mention_command(second, "second");
                                            response_content.push(" in the settings draft.");
                                        }
                                        let response_content = response_content.build();
                                        transaction.commit().await?;
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(false)
                                            .content(response_content)
                                        )).await?;
                                    }
                                }
                            } else {
                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                    .ephemeral(true)
                                    .content("Sorry, this Discord server is not associated with an ongoing Mido's House event.")
                                )).await?;
                            }
                        } else if Some(interaction.data.id) == command_ids.ban {
                            if let Some((mut transaction, mut race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if let Some(team) = team {
                                    if let Some(mut draft) = race.draft.take() {
                                        if draft.state.went_first.is_none() {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, first pick hasn't been chosen yet, use ")
                                                    .mention_command(command_ids.first.unwrap(), "first")
                                                    .push(" or ")
                                                    .mention_command(command_ids.second.unwrap(), "second")
                                                    .push('.')
                                                    .build()
                                                )
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if draft.state.pick_count() >= 2 {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, bans have already been chosen.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if draft.is_active_team(team.id) {
                                            let setting = match interaction.data.options[0].value {
                                                CommandDataOptionValue::String(ref setting) => setting.parse().expect("unknown setting in /ban"),
                                                _ => panic!("unexpected slash command option type"),
                                            };
                                            if draft.state.available_settings().contains(&setting) {
                                                match setting {
                                                    mw::S3Setting::Wincon => draft.state.wincon = Some(mw::Wincon::default()),
                                                    mw::S3Setting::Dungeons => draft.state.dungeons = Some(mw::Dungeons::default()),
                                                    mw::S3Setting::Er => draft.state.er = Some(mw::Er::default()),
                                                    mw::S3Setting::Trials => draft.state.trials = Some(mw::Trials::default()),
                                                    mw::S3Setting::Shops => draft.state.shops = Some(mw::Shops::default()),
                                                    mw::S3Setting::Scrubs => draft.state.scrubs = Some(mw::Scrubs::default()),
                                                    mw::S3Setting::Fountain => draft.state.fountain = Some(mw::Fountain::default()),
                                                    mw::S3Setting::Spawn => draft.state.spawn = Some(mw::Spawn::default()),
                                                }
                                                sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(&draft) as _, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                                let guild_id = interaction.guild_id.expect("/ban called outside of a guild");
                                                let response_content = MessageBuilder::default()
                                                    .mention_team(&mut transaction, guild_id, &team).await?
                                                    .push(if team.name_is_plural() { " have locked in " } else { " has locked in " })
                                                    .push(match setting {
                                                        mw::S3Setting::Wincon => "default wincons",
                                                        mw::S3Setting::Dungeons => "tournament dungeons",
                                                        mw::S3Setting::Er => "no ER",
                                                        mw::S3Setting::Trials => "0 trials",
                                                        mw::S3Setting::Shops => "shops 4",
                                                        mw::S3Setting::Scrubs => "affordable scrubs",
                                                        mw::S3Setting::Fountain => "closed fountain",
                                                        mw::S3Setting::Spawn => "ToT spawns",
                                                    })
                                                    .push_line('.')
                                                    .push(draft.next_step(&mut transaction, guild_id, &command_ids, race.teams()).await?)
                                                    .build();
                                                transaction.commit().await?;
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(false)
                                                    .content(response_content)
                                                )).await?;
                                            } else {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content(MessageBuilder::default()
                                                        .push("Sorry, that setting is already locked in. Use ")
                                                        .mention_command(command_ids.skip.unwrap(), "skip")
                                                        .push(" if you don't want to ban anything.")
                                                        .build()
                                                    )
                                                )).await?;
                                                transaction.rollback().await?;
                                            }
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
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
                                        .content("Sorry, only participants in this race can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if Some(interaction.data.id) == command_ids.draft {
                            if let Some((mut transaction, mut race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if let Some(team) = team {
                                    if let Some(mut draft) = race.draft.take() {
                                        if draft.state.went_first.is_none() {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, first pick hasn't been chosen yet, use ")
                                                    .mention_command(command_ids.first.unwrap(), "first")
                                                    .push(" or ")
                                                    .mention_command(command_ids.second.unwrap(), "second")
                                                    .build()
                                                )
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if draft.state.pick_count() < 2 {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, bans haven't been chosen yet, use ")
                                                    .mention_command(command_ids.ban.unwrap(), "ban")
                                                    .build()
                                                )
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if draft.is_active_team(team.id) {
                                            let setting = interaction.data.options[0].name.parse().expect("unknown setting in /draft");
                                            let value = match interaction.data.options[0].value {
                                                CommandDataOptionValue::SubCommand(ref value) => match value[0].value {
                                                    CommandDataOptionValue::String(ref value) => value,
                                                    _ => panic!("unexpected slash command option type"),
                                                },
                                                _ => panic!("unexpected slash command option type"),
                                            };
                                            if draft.state.available_settings().contains(&setting) {
                                                let value = match setting {
                                                    mw::S3Setting::Wincon => { let value = all::<mw::Wincon>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.wincon = Some(value); value.to_string() }
                                                    mw::S3Setting::Dungeons => { let value = all::<mw::Dungeons>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.dungeons = Some(value); value.to_string() }
                                                    mw::S3Setting::Er => { let value = all::<mw::Er>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.er = Some(value); value.to_string() }
                                                    mw::S3Setting::Trials => { let value = all::<mw::Trials>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.trials = Some(value); value.to_string() }
                                                    mw::S3Setting::Shops => { let value = all::<mw::Shops>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.shops = Some(value); value.to_string() }
                                                    mw::S3Setting::Scrubs => { let value = all::<mw::Scrubs>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.scrubs = Some(value); value.to_string() }
                                                    mw::S3Setting::Fountain => { let value = all::<mw::Fountain>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.fountain = Some(value); value.to_string() }
                                                    mw::S3Setting::Spawn => { let value = all::<mw::Spawn>().find(|option| option.arg() == value).expect("unknown value in /draft"); draft.state.spawn = Some(value); value.to_string() }
                                                };
                                                sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(&draft) as _, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                                let guild_id = interaction.guild_id.expect("/draft called outside of a guild");
                                                let response_content = MessageBuilder::default()
                                                    .mention_team(&mut transaction, guild_id, &team).await?
                                                    .push(if team.name_is_plural() { " have picked " } else { " has picked " })
                                                    .push(value)
                                                    .push_line('.')
                                                    .push(draft.next_step(&mut transaction, guild_id, &command_ids, race.teams()).await?)
                                                    .build();
                                                transaction.commit().await?;
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(false)
                                                    .content(response_content)
                                                )).await?;
                                            } else {
                                                let mut content = MessageBuilder::default();
                                                content.push("Sorry, that setting is already locked in. Use one of the following: ");
                                                for (i, setting) in draft.state.available_settings().into_iter().enumerate() {
                                                    if i > 0 {
                                                        content.push(" or ");
                                                    }
                                                    content.mention_command(command_ids.draft.unwrap(), &format!("draft {setting}"));
                                                }
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(true)
                                                    .content(content.build())
                                                )).await?;
                                                transaction.rollback().await?;
                                            }
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
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
                                        .content("Sorry, only participants in this race can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if Some(interaction.data.id) == command_ids.first {
                            if let Some((mut transaction, mut race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if let Some(team) = team {
                                    if let Some(mut draft) = race.draft.take() {
                                        if draft.state.went_first.is_some() {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, first pick has already been chosen.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if draft.is_active_team(team.id) {
                                            draft.state.went_first = Some(true);
                                            sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(&draft) as _, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                            let guild_id = interaction.guild_id.expect("/first called outside of a guild");
                                            let response_content = MessageBuilder::default()
                                                .mention_team(&mut transaction, guild_id, &team).await?
                                                .push(if team.name_is_plural() { " have" } else { " has" })
                                                .push_line(" chosen to go first in the settings draft.")
                                                .push(draft.next_step(&mut transaction, guild_id, &command_ids, race.teams()).await?)
                                                .build();
                                            transaction.commit().await?;
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(false)
                                                .content(response_content)
                                            )).await?;
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
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
                                        .content("Sorry, only participants in this race can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if interaction.data.id == command_ids.post_status {
                            if let Some((mut transaction, mut race, _)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if let Some(draft) = race.draft.take() {
                                    let guild_id = interaction.guild_id.expect("/post-status called outside of a guild");
                                    let response_content = MessageBuilder::default()
                                        //TODO include scheduling status, both for regular races and for asyncs
                                        .push(draft.next_step(&mut transaction, guild_id, &command_ids, race.teams()).await?)
                                        .build();
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(false)
                                        .content(response_content)
                                    )).await?;
                                    transaction.commit().await?;
                                } else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if Some(interaction.data.id) == command_ids.pronoun_roles {
                            let guild_id = interaction.guild_id.expect("/pronoun-roles called outside of a guild");
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
                        } else if interaction.data.id == command_ids.schedule {
                            if let Some((mut transaction, race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if team.is_some() || interaction.member.as_ref().expect("/schedule called outside of a guild").permissions.expect("permissions should be included in interaction response").administrator() {
                                    let start = match interaction.data.options[0].value {
                                        CommandDataOptionValue::String(ref start) => start,
                                        _ => panic!("unexpected slash command option type"),
                                    };
                                    if let Some(start) = parse_timestamp(start) {
                                        if start < Utc::now() + Duration::minutes(30) {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, races must be scheduled at least 30 minutes in advance.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else {
                                            sqlx::query!("UPDATE races SET start = $1, async_start1 = NULL, async_start2 = NULL WHERE id = $2", start, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                            transaction.commit().await?;
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(false)
                                                .content(format!("This race is now scheduled for <t:{}:F>.", start.timestamp()))
                                            )).await?;
                                        }
                                    } else {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Sorry, that doesn't look like a Discord timestamp. You can use <https://hammertime.cyou/> to generate one.")
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race and administrators can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if interaction.data.id == command_ids.schedule_async {
                            if let Some((mut transaction, race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if team.is_some() || interaction.member.as_ref().expect("/schedule-async called outside of a guild").permissions.expect("permissions should be included in interaction response").administrator() {
                                    let start = match interaction.data.options[0].value {
                                        CommandDataOptionValue::String(ref start) => start,
                                        _ => panic!("unexpected slash command option type"),
                                    };
                                    if let Some(start) = parse_timestamp(start) {
                                        if start < Utc::now() + Duration::minutes(30) {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, races must be scheduled at least 30 minutes in advance.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else {
                                            match race.entrants {
                                                Entrants::Two([Entrant::MidosHouseTeam(team1), Entrant::MidosHouseTeam(team2)]) => {
                                                    if team.as_ref().map_or(false, |team| team1 == *team) {
                                                        sqlx::query!("UPDATE races SET async_start1 = $1, start = NULL WHERE id = $2", start, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                                    } else if team.as_ref().map_or(false, |team| team2 == *team) {
                                                        sqlx::query!("UPDATE races SET async_start2 = $1, start = NULL WHERE id = $2", start, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                                    } else {
                                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                            .ephemeral(true)
                                                            .content("Sorry, only participants in this race can use this command for now. Please contact Fenhl to edit the schedule.") //TODO allow TOs to schedule as async
                                                        )).await?;
                                                        transaction.rollback().await?;
                                                        return Ok(())
                                                    }
                                                }
                                                _ => panic!("tried to schedule race with not two MH teams as async"),
                                            }
                                            transaction.commit().await?;
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(false)
                                                .content(format!("Your half of this race is now scheduled for <t:{}:F>.", start.timestamp()))
                                            )).await?;
                                        }
                                    } else {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Sorry, that doesn't look like a Discord timestamp. You can use <https://hammertime.cyou/> to generate one.")
                                        )).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race and administrators can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if interaction.data.id == command_ids.schedule_remove {
                            if let Some((mut transaction, race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                let is_admin = interaction.member.as_ref().expect("/schedule-remove called outside of a guild").permissions.expect("permissions should be included in interaction response").administrator();
                                if team.is_some() || is_admin {
                                    if !is_admin && race.has_room_for(team.as_ref().expect("checked above")) {
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(true)
                                            .content("Sorry, the room for this race is already open. Please contact a tournament organizer if necessary.")
                                        )).await?;
                                        transaction.rollback().await?;
                                    } else {
                                        let had_multiple_times = match race.schedule {
                                            RaceSchedule::Unscheduled => {
                                                interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                    .ephemeral(false)
                                                    .content("Sorry, this race already doesn't have a starting time.")
                                                )).await?;
                                                transaction.rollback().await?;
                                                return Ok(())
                                            }
                                            RaceSchedule::Live { .. } => false,
                                            RaceSchedule::Async { start1, start2, .. } => start1.is_some() && start2.is_some(),
                                        };
                                        sqlx::query!("UPDATE races SET start = NULL, async_start1 = NULL, async_start2 = NULL WHERE id = $1", i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                        transaction.commit().await?;
                                        interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                            .ephemeral(false)
                                            .content(if had_multiple_times {
                                                "This race's starting times have been removed from the schedule."
                                            } else {
                                                "This race's starting time has been removed from the schedule."
                                            })
                                        )).await?;
                                    }
                                } else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race and administrators can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if Some(interaction.data.id) == command_ids.second {
                            if let Some((mut transaction, mut race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if let Some(team) = team {
                                    if let Some(mut draft) = race.draft.take() {
                                        if draft.state.went_first.is_some() {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, first pick has already been chosen.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if draft.is_active_team(team.id) {
                                            draft.state.went_first = Some(false);
                                            sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(&draft) as _, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                            let guild_id = interaction.guild_id.expect("/second called outside of a guild");
                                            let response_content = MessageBuilder::default()
                                                .mention_team(&mut transaction, guild_id, &team).await?
                                                .push(if team.name_is_plural() { " have" } else { " has" })
                                                .push_line(" chosen to go second in the settings draft.")
                                                .push(draft.next_step(&mut transaction, guild_id, &command_ids, race.teams()).await?)
                                                .build();
                                            transaction.commit().await?;
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(false)
                                                .content(response_content)
                                            )).await?;
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
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
                                        .content("Sorry, only participants in this race can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if Some(interaction.data.id) == command_ids.skip {
                            if let Some((mut transaction, mut race, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if let Some(team) = team {
                                    if let Some(mut draft) = race.draft.take() {
                                        if draft.state.went_first.is_none() {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, first pick hasn't been chosen yet, use ")
                                                    .mention_command(command_ids.first.unwrap(), "first")
                                                    .push(" or ")
                                                    .mention_command(command_ids.second.unwrap(), "second")
                                                    .build()
                                                )
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if !matches!(draft.state.pick_count(), 0 | 1 | 5) {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, this part of the draft can't be skipped.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        } else if draft.is_active_team(team.id) {
                                            let skip_kind = match draft.state.pick_count() {
                                                0 | 1 => "ban",
                                                5 => "final pick",
                                                _ => unreachable!(),
                                            };
                                            draft.state.skipped_bans += 1;
                                            sqlx::query!("UPDATE races SET draft_state = $1 WHERE id = $2", Json(&draft) as _, i64::from(race.id.expect("Race::for_scheduling_channel returned race without ID"))).execute(&mut transaction).await?;
                                            let guild_id = interaction.guild_id.expect("/skip called outside of a guild");
                                            let response_content = MessageBuilder::default()
                                                .mention_team(&mut transaction, guild_id, &team).await?
                                                .push(if team.name_is_plural() { " have skipped their " } else { " has skipped their " })
                                                .push(skip_kind)
                                                .push_line('.')
                                                .push(draft.next_step(&mut transaction, guild_id, &command_ids, race.teams()).await?)
                                                .build();
                                            transaction.commit().await?;
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(false)
                                                .content(response_content)
                                            )).await?;
                                        } else {
                                            interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )).await?;
                                            transaction.rollback().await?;
                                        }
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
                                        .content("Sorry, only participants in this race can use this command.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if interaction.data.id == command_ids.status {
                            if let Some((mut transaction, race, _)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                                if let Some(ref draft) = race.draft {
                                    let guild_id = interaction.guild_id.expect("/status called outside of a guild");
                                    let response_content = MessageBuilder::default()
                                        //TODO include scheduling status, both for regular races and for asyncs
                                        .push(draft.next_step(&mut transaction, guild_id, &command_ids, race.teams()).await?)
                                        .build();
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content(response_content)
                                    )).await?;
                                    transaction.rollback().await?;
                                } else {
                                    interaction.create_response(ctx, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new()
                                        .ephemeral(true)
                                        .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                    )).await?;
                                    transaction.rollback().await?;
                                }
                            }
                        } else if Some(interaction.data.id) == command_ids.watch_roles {
                            let guild_id = interaction.guild_id.expect("/watch-roles called outside of a guild");
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
                        }
                    }
                }
                Interaction::Component(interaction) => match &*interaction.data.custom_id {
                    "pronouns_he" => {
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
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
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
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
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
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
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
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
                    "watchrole_restream" => {
                        let mut member = interaction.member.clone().expect("/watch-roles called outside of a guild");
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
                        let mut member = interaction.member.clone().expect("/watch-roles called outside of a guild");
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
                    custom_id => panic!("received message component interaction with unknown custom ID {custom_id:?}"),
                },
                _ => {}
            }
            Ok(())
        }))
        .task(|ctx_fut, _| async move {
            shutdown.await;
            serenity_utils::shut_down(&*ctx_fut.read().await).await;
        })
}
