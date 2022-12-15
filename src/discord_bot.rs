use {
    chrono::{
        Duration,
        prelude::*,
    },
    enum_iterator::all,
    lazy_regex::regex_captures,
    serde::{
        Deserialize,
        Serialize,
    },
    serenity::{
        model::{
            application::{
                command::CommandOptionType,
                interaction::{
                    Interaction,
                    application_command::CommandDataOptionValue,
                },
            },
            prelude::*,
        },
        prelude::*,
        utils::MessageBuilder,
    },
    serenity_utils::{
        builder::ErrorNotifier,
        handler::HandlerMethods as _,
        slash::*,
    },
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
        types::Json,
    },
    crate::{
        Environment,
        cal,
        config::Config,
        event::mw,
        startgg,
        team::Team,
        util::{
            Id,
            MessageBuilderExt as _,
        },
    },
};

const FENHL: UserId = UserId(86841168427495424);

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
struct CommandIds {
    assign: CommandId,
    ban: CommandId,
    draft: CommandId,
    first: CommandId,
    post_status: CommandId,
    pronoun_roles: CommandId,
    schedule: CommandId,
    second: CommandId,
    skip: CommandId,
    status: CommandId,
    watch_roles: CommandId,
}

impl TypeMapKey for CommandIds {
    type Value = CommandIds;
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

    async fn next_step(&self, transaction: &mut Transaction<'_, Postgres>, guild: GuildId, command_ids: &CommandIds, teams: &[Team]) -> sqlx::Result<String> {
        let high_seed = teams.iter().find(|team| team.id == self.high_seed).expect("high seed not in teams list");
        let low_seed = teams.iter().find(|team| team.id != self.high_seed).expect("low seed not in teams list");
        Ok(match self.state.next_step() {
            mw::DraftStep::GoFirst => MessageBuilder::default()
                .mention_team(transaction, guild, high_seed).await?
                .push(": you have the higher seed. Choose whether you want to go ")
                .mention_command(command_ids.first, "first")
                .push(" or ")
                .mention_command(command_ids.second, "second")
                .push(" in the settings draft.")
                .build(),
            mw::DraftStep::Ban { team, .. } => MessageBuilder::default()
                .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                .push(": lock a setting to its default using ")
                .mention_command(command_ids.ban, "ban")
                .push(", or use ")
                .mention_command(command_ids.skip, "skip")
                .push(" if you don't want to ban anything.")
                .build(),
            mw::DraftStep::Pick { prev_picks, team } => match prev_picks {
                0 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick a setting using ")
                    .mention_command(command_ids.draft, "draft")
                    .push('.')
                    .build(),
                1 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick a setting using ")
                    .mention_command(command_ids.draft, "draft")
                    .push(". You will have another pick after this.")
                    .build(),
                2 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick your second setting using ")
                    .mention_command(command_ids.draft, "draft")
                    .push('.')
                    .build(),
                3 => MessageBuilder::default()
                    .mention_team(transaction, guild, team.choose(high_seed, low_seed)).await?
                    .push(": pick a setting using ")
                    .mention_command(command_ids.draft, "draft")
                    .push(". You can also use ")
                    .mention_command(command_ids.skip, "skip")
                    .push(" if you want to leave the settings as they are.")
                    .build(),
                _ => unreachable!(),
            },
            mw::DraftStep::Done(settings) => format!("Settings draft completed. You will be playing with {settings}."),
        })
    }
}

async fn check_scheduling_thread_permissions<'a>(ctx: &'a Context, interaction: &ApplicationCommandInteraction) -> Result<Option<(Transaction<'a, Postgres>, String, Option<i16>, Vec<Team>, Option<Team>)>, Box<dyn std::error::Error + Send + Sync>> {
    let mut transaction = ctx.data.read().await.get::<DbPool>().expect("database connection pool missing from Discord context").begin().await?;
    Ok(if let Some(row) = sqlx::query!(r#"SELECT startgg_set, game, room IS NOT NULL OR async_room1 IS NOT NULL OR async_room2 IS NOT NULL AS "has_room!" FROM races WHERE scheduling_thread = $1 ORDER BY game DESC"#, i64::from(interaction.channel_id)).fetch_optional(&mut transaction).await? {
        if row.has_room {
            interaction.create_interaction_response(ctx, |r| r
                .interaction_response_data(|d| d
                    .ephemeral(true)
                    .content("Sorry, this command can't be used since a race room is already open.")
                )
            ).await?;
            transaction.rollback().await?;
            None
        } else {
            let mut teams = Vec::with_capacity(2);
            let mut team = None;
            let response_data = startgg::query::<startgg::SetQuery>(
                ctx.data.read().await.get::<HttpClient>().expect("HTTP client missing from Discord context"),
                ctx.data.read().await.get::<StartggToken>().expect("start.gg auth token missing from Discord context"),
                startgg::set_query::Variables { set_id: startgg::ID(row.startgg_set.clone()) },
            ).await?;
            if let startgg::set_query::ResponseData {
                set: Some(startgg::set_query::SetQuerySet {
                    slots: Some(ref slots),
                    .. //TODO separate query with only the data used?
                }),
            } = response_data {
                for slot in slots {
                    if let Some(startgg::set_query::SetQuerySetSlots {
                        entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant {
                            team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam {
                                id: Some(startgg::ID(ref iter_team)),
                                on: _,
                            }),
                        }),
                    }) = slot {
                        let iter_team = Team::from_startgg(&mut transaction, iter_team).await?.ok_or(cal::Error::UnknownTeam)?;
                        if iter_team.members(&mut transaction).await?.into_iter().any(|member| member.discord_id == Some(interaction.user.id)) {
                            team = Some(iter_team.clone());
                        }
                        teams.push(iter_team);
                    } else {
                        return Err(cal::Error::Teams { startgg_set: row.startgg_set, response_data }.into())
                    }
                }
            } else {
                return Err(cal::Error::Teams { startgg_set: row.startgg_set, response_data }.into())
            }
            Some((transaction, row.startgg_set, row.game, teams, team))
        }
    } else {
        interaction.create_interaction_response(ctx, |r| r
            .interaction_response_data(|d| d
                .ephemeral(true)
                .content("Sorry, this thread is not associated with a match. Please contact a tournament organizer to fix this.")
            )
        ).await?;
        transaction.rollback().await?;
        None
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
            let assign = guild.create_application_command(ctx, |c| c
                .name("assign")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .default_member_permissions(Permissions::ADMINISTRATOR)
                .dm_permission(false)
                .description("Marks this thread as the scheduling thread for the given start.gg set.")
                .create_option(|o| o
                    .kind(CommandOptionType::String)
                    .name("startgg-set")
                    .description("The start.gg set (match) ID")
                    .required(true)
                )
                .create_option(|o| o
                    .kind(CommandOptionType::Role)
                    .name("high-seed")
                    .description("The team that decides which team starts the settings draft. If the teams are tied, flip a coin.")
                    .required(true)
                )
                .create_option(|o| o
                    .kind(CommandOptionType::Integer)
                    .name("game")
                    .description("The game number within the match, if this is a best-of-n-races match.")
                    .min_int_value(1)
                    .max_int_value(255)
                    .required(false)
                )
            ).await?.id;
            let ban = guild.create_application_command(ctx, |c| c
                .name("ban")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .dm_permission(false)
                .description("Locks a setting for this match to its default value.")
                .create_option(|o| o
                    .kind(CommandOptionType::String)
                    .name("setting")
                    .description("The setting to lock in")
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
            ).await?.id;
            let draft = guild.create_application_command(ctx, |c| c
                .name("draft")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .dm_permission(false)
                .description("Chooses a setting for this match.")
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("wincon")
                    .description("win conditions")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for the win condition settings")
                        .required(true)
                        .add_string_choice("default wincons", "meds")
                        .add_string_choice("Scrubs wincons", "scrubs")
                        .add_string_choice("Triforce Hunt", "th")
                    )
                )
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("dungeons")
                    .description("dungeons")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for the dungeon item settings")
                        .required(true)
                        .add_string_choice("tournament dungeons", "tournament")
                        .add_string_choice("dungeon tokens", "skulls")
                        .add_string_choice("keyrings", "keyrings")
                    )
                )
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("er")
                    .description("entrance rando")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for entrance randomizer")
                        .required(true)
                        .add_string_choice("no ER", "off")
                        .add_string_choice("dungeon ER", "dungeon")
                    )
                )
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("trials")
                    .description("trials")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for the Ganon's Trials setting")
                        .required(true)
                        .add_string_choice("0 trials", "0")
                        .add_string_choice("2 trials", "2")
                    )
                )
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("shops")
                    .description("shops")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for the Shop Shuffle setting")
                        .required(true)
                        .add_string_choice("shops 4", "4")
                        .add_string_choice("no shops", "off")
                    )
                )
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("scrubs")
                    .description("scrubs")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for the Scrub Shuffle setting")
                        .required(true)
                        .add_string_choice("affordable scrubs", "affordable")
                        .add_string_choice("no scrubs", "off")
                    )
                )
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("fountain")
                    .description("fountain")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for the Zora's Fountain setting")
                        .required(true)
                        .add_string_choice("closed fountain", "closed")
                        .add_string_choice("open fountain", "open")
                    )
                )
                .create_option(|o| o
                    .kind(CommandOptionType::SubCommand)
                    .name("spawn")
                    .description("spawns")
                    .create_sub_option(|o| o
                        .kind(CommandOptionType::String)
                        .name("value")
                        .description("Your choice for the spawn settings")
                        .required(true)
                        .add_string_choice("ToT spawns", "tot")
                        .add_string_choice("random spawns & starting age", "random")
                    )
                )
            ).await?.id;
            let first = guild.create_application_command(ctx, |c| c
                .name("first")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .dm_permission(false)
                .description("Go first in the settings draft.")
            ).await?.id;
            let post_status = guild.create_application_command(ctx, |c| c
                .name("post-status")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .default_member_permissions(Permissions::ADMINISTRATOR)
                .dm_permission(false)
                .description("Posts this race's status to the thread, pinging the team whose turn it is in the settings draft.")
            ).await?.id;
            let pronoun_roles = guild.create_application_command(ctx, |c| c
                .name("pronoun-roles")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .default_member_permissions(Permissions::ADMINISTRATOR)
                .dm_permission(false)
                .description("Creates gender pronoun roles and posts a message here that allows members to self-assign them.")
            ).await?.id;
            let schedule = guild.create_application_command(ctx, |c| c
                .name("schedule")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .dm_permission(false)
                .description("Submits a starting time for this match.")
                .create_option(|o| o
                    .kind(CommandOptionType::String)
                    .name("start")
                    .description("The starting time as a Discord timestamp")
                    .required(true)
                )
            ).await?.id;
            let second = guild.create_application_command(ctx, |c| c
                .name("second")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .dm_permission(false)
                .description("Go second in the settings draft.")
            ).await?.id;
            let skip = guild.create_application_command(ctx, |c| c
                .name("skip")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .dm_permission(false)
                .description("Skip your ban or the final pick of the settings draft.")
            ).await?.id;
            let status = guild.create_application_command(ctx, |c| c
                .name("status")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .dm_permission(false)
                .description("Shows you this race's current scheduling and settings draft status.")
            ).await?.id;
            let watch_roles = guild.create_application_command(ctx, |c| c
                .name("watch-roles")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .default_member_permissions(Permissions::ADMINISTRATOR)
                .dm_permission(false)
                .description("Creates watch notification roles and posts a message here that allows members to self-assign them.")
                .create_option(|o| o
                    .kind(CommandOptionType::Channel)
                    .name("watch-party-channel")
                    .description("Will be linked to from the description message.")
                    .required(true)
                    .channel_types(&[ChannelType::Voice, ChannelType::Stage])
                )
                .create_option(|o| o
                    .kind(CommandOptionType::Channel)
                    .name("race-rooms-channel")
                    .description("Will be linked to from the description message.")
                    .required(true)
                    .channel_types(&[ChannelType::Text, ChannelType::News])
                )
            ).await?.id;
            ctx.data.write().await.insert::<CommandIds>(CommandIds { assign, ban, draft, first, post_status, pronoun_roles, schedule, second, skip, status, watch_roles });
            Ok(())
        }))
        .on_interaction_create(|ctx, interaction| Box::pin(async move {
            match interaction {
                Interaction::ApplicationCommand(interaction) => if let Some(&command_ids) = ctx.data.read().await.get::<CommandIds>() {
                    if interaction.data.id == command_ids.assign {
                        let mut transaction = ctx.data.read().await.get::<DbPool>().as_ref().expect("database connection pool missing from Discord context").begin().await?;
                        let guild_id = interaction.guild_id.expect("/assign called outside of a guild");
                        if let Some(event_row) = sqlx::query!("SELECT series, event FROM events WHERE discord_guild = $1 AND end_time IS NULL", i64::from(guild_id)).fetch_optional(&mut transaction).await? {
                            let startgg_set = match interaction.data.options[0].resolved.as_ref().expect("missing slash command option") {
                                CommandDataOptionValue::String(startgg_set) => startgg_set,
                                _ => panic!("unexpected slash command option type"),
                            };
                            let high_seed = match interaction.data.options[1].resolved.as_ref().expect("missing slash command option") {
                                CommandDataOptionValue::Role(discord_role) => discord_role.id,
                                _ => panic!("unexpected slash command option type"),
                            };
                            let game = interaction.data.options.get(2).map(|option| match option.resolved.as_ref().expect("missing slash command option") {
                                &CommandDataOptionValue::Integer(game) => i16::try_from(game).expect("game number out of range"),
                                _ => panic!("unexpected slash command option type"),
                            });
                            if let Some(high_seed) = Team::from_discord(&mut transaction, high_seed).await? {
                                sqlx::query!("INSERT INTO races
                                    (startgg_set, game, series, event, scheduling_thread, draft_state) VALUES ($1, $2, $3, $4, $5, $6)
                                    ON CONFLICT (startgg_set, game) DO UPDATE SET scheduling_thread = EXCLUDED.scheduling_thread, draft_state = EXCLUDED.draft_state
                                ", &startgg_set, game, event_row.series, event_row.event, i64::from(interaction.channel_id), Json(Draft {
                                    high_seed: high_seed.id,
                                    state: mw::S3Draft::default(),
                                }) as _).execute(&mut transaction).await?;
                                let guild_id = interaction.guild_id.expect("/ban called outside of a guild");
                                let mut response_content = MessageBuilder::default();
                                response_content.push("This thread is now assigned to ");
                                if let Some(game) = game {
                                    response_content.push("game ");
                                    response_content.push(game);
                                    response_content.push(" of ");
                                }
                                let response_content = response_content.push("set ")
                                    .push_safe(startgg_set) //TODO linkify set page, use phase/round/identifier
                                    .push(". Use ")
                                    .mention_command(command_ids.schedule, "schedule")
                                    .push_line(" to schedule as a regular race, or ping a tournament organizer to schedule as an async.") //TODO adjust message if asyncing is not allowed
                                    .mention_team(&mut transaction, guild_id, &high_seed).await?
                                    .push(": you have the higher seed. Choose whether you want to go ")
                                    .mention_command(command_ids.first, "first")
                                    .push(" or ")
                                    .mention_command(command_ids.second, "second")
                                    .push(" in the settings draft.")
                                    .build();
                                transaction.commit().await?;
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(false)
                                        .content(response_content)
                                    )
                                ).await?;
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, that doesn't seem to be a team role.")
                                    )
                                ).await?;
                            }
                        } else {
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Sorry, this Discord server is not associated with an ongoing Mido's House event.")
                                )
                            ).await?;
                        }
                    } else if interaction.data.id == command_ids.ban {
                        if let Some((mut transaction, startgg_set, game, teams, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(team) = team {
                                if let Some(Json(mut draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, startgg_set, game).fetch_one(&mut transaction).await? {
                                    if draft.state.went_first.is_none() {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, first pick hasn't been chosen yet, use ")
                                                    .mention_command(command_ids.first, "first")
                                                    .push(" or ")
                                                    .mention_command(command_ids.second, "second")
                                                    .push('.')
                                                )
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if draft.state.pick_count() >= 2 {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, bans have already been chosen.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if draft.is_active_team(team.id) {
                                        let setting = match interaction.data.options[0].resolved.as_ref().expect("missing slash command option") {
                                            CommandDataOptionValue::String(setting) => setting.parse().expect("unknown setting in /ban"),
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
                                            sqlx::query!("UPDATE races SET draft_state = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", Json(&draft) as _, startgg_set, game).execute(&mut transaction).await?;
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
                                                .push(draft.next_step(&mut transaction, guild_id, &command_ids, &teams).await?)
                                                .build();
                                            transaction.commit().await?;
                                            interaction.create_interaction_response(ctx, |r| r
                                                .interaction_response_data(|d| d
                                                    .ephemeral(false)
                                                    .content(response_content)
                                                )
                                            ).await?;
                                        } else {
                                            interaction.create_interaction_response(ctx, |r| r
                                                .interaction_response_data(|d| d
                                                    .ephemeral(true)
                                                    .content(MessageBuilder::default()
                                                        .push("Sorry, that setting is already locked in. Use ")
                                                        .mention_command(command_ids.skip, "skip")
                                                        .push(" if you don't want to ban anything.")
                                                    )
                                                )
                                            ).await?;
                                            transaction.rollback().await?;
                                        }
                                    } else {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                        )
                                    ).await?;
                                    transaction.rollback().await?;
                                }
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race can use this command.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.draft {
                        if let Some((mut transaction, startgg_set, game, teams, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(team) = team {
                                if let Some(Json(mut draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, startgg_set, game).fetch_one(&mut transaction).await? {
                                    if draft.state.went_first.is_none() {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, first pick hasn't been chosen yet, use ")
                                                    .mention_command(command_ids.first, "first")
                                                    .push(" or ")
                                                    .mention_command(command_ids.second, "second")
                                                )
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if draft.state.pick_count() < 2 {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, bans haven't been chosen yet, use ")
                                                    .mention_command(command_ids.ban, "ban")
                                                )
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if draft.is_active_team(team.id) {
                                        let setting = interaction.data.options[0].name.parse().expect("unknown setting in /draft");
                                        let value = match interaction.data.options[0].options[0].resolved.as_ref().expect("missing slash command option") {
                                            CommandDataOptionValue::String(value) => value,
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
                                            sqlx::query!("UPDATE races SET draft_state = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", Json(&draft) as _, startgg_set, game).execute(&mut transaction).await?;
                                            let guild_id = interaction.guild_id.expect("/draft called outside of a guild");
                                            let response_content = MessageBuilder::default()
                                                .mention_team(&mut transaction, guild_id, &team).await?
                                                .push(if team.name_is_plural() { " have picked " } else { " has picked " })
                                                .push(value)
                                                .push_line('.')
                                                .push(draft.next_step(&mut transaction, guild_id, &command_ids, &teams).await?)
                                                .build();
                                            transaction.commit().await?;
                                            interaction.create_interaction_response(ctx, |r| r
                                                .interaction_response_data(|d| d
                                                    .ephemeral(false)
                                                    .content(response_content)
                                                )
                                            ).await?;
                                        } else {
                                            let mut content = MessageBuilder::default();
                                            content.push("Sorry, that setting is already locked in. Use one of the following: ");
                                            for (i, setting) in draft.state.available_settings().into_iter().enumerate() {
                                                if i > 0 {
                                                    content.push(" or ");
                                                }
                                                content.mention_command(command_ids.draft, &format!("draft {setting}"));
                                            }
                                            interaction.create_interaction_response(ctx, |r| r
                                                .interaction_response_data(|d| d
                                                    .ephemeral(true)
                                                    .content(content)
                                                )
                                            ).await?;
                                            transaction.rollback().await?;
                                        }
                                    } else {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                        )
                                    ).await?;
                                    transaction.rollback().await?;
                                }
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race can use this command.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.first {
                        if let Some((mut transaction, startgg_set, game, teams, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(team) = team {
                                if let Some(Json(mut draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, startgg_set, game).fetch_one(&mut transaction).await? {
                                    if draft.state.went_first.is_some() {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, first pick has already been chosen.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if draft.is_active_team(team.id) {
                                        draft.state.went_first = Some(true);
                                        sqlx::query!("UPDATE races SET draft_state = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", Json(&draft) as _, startgg_set, game).execute(&mut transaction).await?;
                                        let guild_id = interaction.guild_id.expect("/first called outside of a guild");
                                        let response_content = MessageBuilder::default()
                                            .mention_team(&mut transaction, guild_id, &team).await?
                                            .push(if team.name_is_plural() { " have" } else { " has" })
                                            .push_line(" chosen to go first in the settings draft.")
                                            .push(draft.next_step(&mut transaction, guild_id, &command_ids, &teams).await?)
                                            .build();
                                        transaction.commit().await?;
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(false)
                                                .content(response_content)
                                            )
                                        ).await?;
                                    } else {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                        )
                                    ).await?;
                                    transaction.rollback().await?;
                                }
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race can use this command.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.post_status {
                        if let Some((mut transaction, startgg_set, game, teams, _)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(Json(draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, startgg_set, game).fetch_one(&mut transaction).await? {
                                let guild_id = interaction.guild_id.expect("/post-status called outside of a guild");
                                let response_content = MessageBuilder::default()
                                    //TODO include scheduling status, both for regular races and for asyncs
                                    .push(draft.next_step(&mut transaction, guild_id, &command_ids, &teams).await?)
                                    .build();
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(false)
                                        .content(response_content)
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.pronoun_roles {
                        let guild_id = interaction.guild_id.expect("/pronoun-roles called outside of a guild");
                        guild_id.create_role(ctx, |r| r
                            .hoist(false)
                            .mentionable(false)
                            .name("he/him")
                            .permissions(Permissions::empty())
                        ).await?;
                        guild_id.create_role(ctx, |r| r
                            .hoist(false)
                            .mentionable(false)
                            .name("she/her")
                            .permissions(Permissions::empty())
                        ).await?;
                        guild_id.create_role(ctx, |r| r
                            .hoist(false)
                            .mentionable(false)
                            .name("they/them")
                            .permissions(Permissions::empty())
                        ).await?;
                        guild_id.create_role(ctx, |r| r
                            .hoist(false)
                            .mentionable(false)
                            .name("other pronouns")
                            .permissions(Permissions::empty())
                        ).await?;
                        interaction.create_interaction_response(ctx, |r| r
                            .interaction_response_data(|d| d
                                .ephemeral(false)
                                .content("Click a button below to get a gender pronoun role. Click again to remove it. Multiple selections allowed.")
                                .components(|c| c
                                    .create_action_row(|r| r
                                        .create_button(|b| b
                                            .label("he/him")
                                            .custom_id("pronouns_he")
                                        )
                                        .create_button(|b| b
                                            .label("she/her")
                                            .custom_id("pronouns_she")
                                        )
                                        .create_button(|b| b
                                            .label("they/them")
                                            .custom_id("pronouns_they")
                                        )
                                        .create_button(|b| b
                                            .label("other")
                                            .custom_id("pronouns_other")
                                        )
                                    )
                                )
                            )
                        ).await?;
                    } else if interaction.data.id == command_ids.schedule {
                        if let Some((mut transaction, startgg_set, game, _, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if team.is_some() || interaction.member.as_ref().expect("/schedule called outside of a guild").permissions.expect("permissions should be included in interaction response").administrator() {
                                let start = match interaction.data.options[0].resolved.as_ref().expect("missing slash command option") {
                                    CommandDataOptionValue::String(start) => start,
                                    _ => panic!("unexpected slash command option type"),
                                };
                                if let Some(start) = parse_timestamp(start) {
                                    if start < Utc::now() + Duration::minutes(30) {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, races must be scheduled at least 30 minutes in advance.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else {
                                        sqlx::query!("UPDATE races SET start = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", start, startgg_set, game).execute(&mut transaction).await?;
                                        transaction.commit().await?;
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(false)
                                                .content(format!("This race is now scheduled for <t:{}:F>.", start.timestamp()))
                                            )
                                        ).await?;
                                    }
                                } else {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, that doesn't look like a Discord timestamp. You can use <https://hammertime.cyou/> to generate one.")
                                        )
                                    ).await?;
                                    transaction.rollback().await?;
                                }
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race and administrators can use this command.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.second {
                        if let Some((mut transaction, startgg_set, game, teams, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(team) = team {
                                if let Some(Json(mut draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, startgg_set, game).fetch_one(&mut transaction).await? {
                                    if draft.state.went_first.is_some() {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, first pick has already been chosen.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if draft.is_active_team(team.id) {
                                        draft.state.went_first = Some(false);
                                        sqlx::query!("UPDATE races SET draft_state = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", Json(&draft) as _, startgg_set, game).execute(&mut transaction).await?;
                                        let guild_id = interaction.guild_id.expect("/second called outside of a guild");
                                        let response_content = MessageBuilder::default()
                                            .mention_team(&mut transaction, guild_id, &team).await?
                                            .push(if team.name_is_plural() { " have" } else { " has" })
                                            .push_line(" chosen to go second in the settings draft.")
                                            .push(draft.next_step(&mut transaction, guild_id, &command_ids, &teams).await?)
                                            .build();
                                        transaction.commit().await?;
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(false)
                                                .content(response_content)
                                            )
                                        ).await?;
                                    } else {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                        )
                                    ).await?;
                                    transaction.rollback().await?;
                                }
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race can use this command.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.skip {
                        if let Some((mut transaction, startgg_set, game, teams, team)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(team) = team {
                                if let Some(Json(mut draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, startgg_set, game).fetch_one(&mut transaction).await? {
                                    if draft.state.went_first.is_none() {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content(MessageBuilder::default()
                                                    .push("Sorry, first pick hasn't been chosen yet, use ")
                                                    .mention_command(command_ids.first, "first")
                                                    .push(" or ")
                                                    .mention_command(command_ids.second, "second")
                                                )
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if !matches!(draft.state.pick_count(), 0 | 1 | 5) {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, this part of the draft can't be skipped.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    } else if draft.is_active_team(team.id) {
                                        let skip_kind = match draft.state.pick_count() {
                                            0 | 1 => "ban",
                                            5 => "final pick",
                                            _ => unreachable!(),
                                        };
                                        draft.state.skipped_bans += 1;
                                        sqlx::query!("UPDATE races SET draft_state = $1 WHERE startgg_set = $2 AND game IS NOT DISTINCT FROM $3", Json(&draft) as _, startgg_set, game).execute(&mut transaction).await?;
                                        let guild_id = interaction.guild_id.expect("/skip called outside of a guild");
                                        let response_content = MessageBuilder::default()
                                            .mention_team(&mut transaction, guild_id, &team).await?
                                            .push(if team.name_is_plural() { " have skipped their " } else { " has skipped their " })
                                            .push(skip_kind)
                                            .push_line('.')
                                            .push(draft.next_step(&mut transaction, guild_id, &command_ids, &teams).await?)
                                            .build();
                                        transaction.commit().await?;
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(false)
                                                .content(response_content)
                                            )
                                        ).await?;
                                    } else {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, it's not your team's turn in the settings draft.")
                                            )
                                        ).await?;
                                        transaction.rollback().await?;
                                    }
                                } else {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                        )
                                    ).await?;
                                    transaction.rollback().await?;
                                }
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, only participants in this race can use this command.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.status {
                        if let Some((mut transaction, startgg_set, game, teams, _)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(Json(draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1 AND game IS NOT DISTINCT FROM $2"#, startgg_set, game).fetch_one(&mut transaction).await? {
                                let guild_id = interaction.guild_id.expect("/status called outside of a guild");
                                let response_content = MessageBuilder::default()
                                    //TODO include scheduling status, both for regular races and for asyncs
                                    .push(draft.next_step(&mut transaction, guild_id, &command_ids, &teams).await?)
                                    .build();
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content(response_content)
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, this race's settings draft has not been initialized. Please contact a tournament organizer to fix this.")
                                    )
                                ).await?;
                                transaction.rollback().await?;
                            }
                        }
                    } else if interaction.data.id == command_ids.watch_roles {
                        let guild_id = interaction.guild_id.expect("/watch-roles called outside of a guild");
                        let watch_party_channel = match interaction.data.options[0].resolved.as_ref().expect("missing slash command option") {
                            CommandDataOptionValue::Channel(channel) => channel.id,
                            _ => panic!("unexpected slash command option type"),
                        };
                        let race_rooms_channel = match interaction.data.options[1].resolved.as_ref().expect("missing slash command option") {
                            CommandDataOptionValue::Channel(channel) => channel.id,
                            _ => panic!("unexpected slash command option type"),
                        };
                        guild_id.create_role(ctx, |r| r
                            .hoist(false)
                            .mentionable(false)
                            .name("restream watcher")
                            .permissions(Permissions::empty())
                        ).await?;
                        let watch_party_role = guild_id.create_role(ctx, |r| r
                            .hoist(false)
                            .mentionable(true)
                            .name("watch party watcher")
                            .permissions(Permissions::empty())
                        ).await?;
                        interaction.create_interaction_response(ctx, |r| r
                            .interaction_response_data(|d| d
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
                                )
                                .components(|c| c
                                    .create_action_row(|r| r
                                        .create_button(|b| b
                                            .label("restream watcher")
                                            .custom_id("watchrole_restream")
                                        )
                                        .create_button(|b| b
                                            .label("watch party watcher")
                                            .custom_id("watchrole_party")
                                        )
                                    )
                                )
                            )
                        ).await?;
                    }
                },
                Interaction::MessageComponent(interaction) => match &*interaction.data.custom_id {
                    "pronouns_he" => {
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "he/him").expect("missing “he/him” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role removed.")
                                )
                            ).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role added.")
                                )
                            ).await?;
                        }
                    }
                    "pronouns_she" => {
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "she/her").expect("missing “she/her” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role removed.")
                                )
                            ).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role added.")
                                )
                            ).await?;
                        }
                    }
                    "pronouns_they" => {
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "they/them").expect("missing “they/them” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role removed.")
                                )
                            ).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role added.")
                                )
                            ).await?;
                        }
                    }
                    "pronouns_other" => {
                        let mut member = interaction.member.clone().expect("/pronoun-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "other pronouns").expect("missing “other pronouns” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role removed.")
                                )
                            ).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role added.")
                                )
                            ).await?;
                        }
                    }
                    "watchrole_restream" => {
                        let mut member = interaction.member.clone().expect("/watch-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "restream watcher").expect("missing “restream watcher” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role removed.")
                                )
                            ).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role added.")
                                )
                            ).await?;
                        }
                    }
                    "watchrole_party" => {
                        let mut member = interaction.member.clone().expect("/watch-roles called outside of a guild");
                        let role = member.guild_id.roles(ctx).await?.into_values().find(|role| role.name == "watch party watcher").expect("missing “watch party watcher” role");
                        if member.roles(ctx).expect("failed to look up member roles").contains(&role) {
                            member.remove_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role removed.")
                                )
                            ).await?;
                        } else {
                            member.add_role(ctx, role).await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Role added.")
                                )
                            ).await?;
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
