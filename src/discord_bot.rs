use {
    chrono::prelude::*,
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
        cal::{
            self,
            Team,
        },
        config::Config,
        event::mw,
        startgg,
        util::Id,
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
    pronoun_roles: CommandId,
    schedule: CommandId,
    watch_roles: CommandId,
}

impl TypeMapKey for CommandIds {
    type Value = CommandIds;
}

#[derive(Deserialize, Serialize)]
struct Draft {
    high_seed: Id,
    state: mw::S3Draft,
}

async fn advance_draft(ctx: &Context, interaction: &ApplicationCommandInteraction, draft: &Draft) -> serenity::Result<()> {
    match draft.state.next_step() {
        mw::DraftStep::GoFirst => interaction.create_interaction_response(ctx, |r| r
            .interaction_response_data(|d| d
                .ephemeral(false)
                .content("Team A, you have the higher seed. Choose whether you want to go /first or /second") //TODO mention team & commands
            )
        ).await?,
        mw::DraftStep::Ban { team, .. } => interaction.create_interaction_response(ctx, |r| r
            .interaction_response_data(|d| d
                .ephemeral(false)
                .content(format!("{team}, lock a setting to its default using /ban, or use /skip if you don't want to ban anything.")) //TODO mention team & commands
            )
        ).await?,
        mw::DraftStep::Pick { prev_picks, team } => interaction.create_interaction_response(ctx, |r| r
            .interaction_response_data(|d| d
                .ephemeral(false)
                .content(&match prev_picks {
                    0 => format!("{team}, pick a setting using /draft."), //TODO mention team & commands
                    1 => format!("{team}, pick two settings using /draft."), //TODO mention team & commands
                    2 => format!("And your second pick?"),
                    3 => format!("{team}, pick the final setting using /draft. You can also use /skip if you want to leave the settings as they are."), //TODO mention team & commands
                    _ => unreachable!(),
                })
            )
        ).await?,
        mw::DraftStep::Done(_) => interaction.create_interaction_response(ctx, |r| r
            .interaction_response_data(|d| d
                .ephemeral(false)
                .content("Settings draft completed.")
            )
        ).await?,
    }
    Ok(())
}

async fn check_scheduling_thread_permissions<'a>(ctx: &'a Context, interaction: &ApplicationCommandInteraction) -> Result<Option<(Transaction<'a, Postgres>, String)>, Box<dyn std::error::Error + Send + Sync>> {
    let mut transaction = ctx.data.read().await.get::<DbPool>().expect("database connection pool missing from Discord context").begin().await?;
    Ok(if let Some(startgg_set) = sqlx::query_scalar!("SELECT startgg_set FROM races WHERE scheduling_thread = $1", i64::from(interaction.channel_id)).fetch_optional(&mut transaction).await? {
        //TODO don't allow running commands on races that already have rooms
        let mut authorized = false; //TODO also allow event organizers to use this command
        if let startgg::set_query::ResponseData {
            set: Some(startgg::set_query::SetQuerySet {
                slots: Some(slots),
                .. //TODO separate query with only the data used?
            }),
        } = startgg::query::<startgg::SetQuery>(
            ctx.data.read().await.get::<HttpClient>().expect("HTTP client missing from Discord context"),
            ctx.data.read().await.get::<StartggToken>().expect("start.gg auth token missing from Discord context"),
            startgg::set_query::Variables { set_id: startgg::ID(startgg_set.clone()) },
        ).await? {
            for slot in slots {
                if let Some(startgg::set_query::SetQuerySetSlots {
                    entrant: Some(startgg::set_query::SetQuerySetSlotsEntrant {
                        team: Some(startgg::set_query::SetQuerySetSlotsEntrantTeam {
                            id: Some(startgg::ID(ref team)),
                            on: _,
                        }),
                    }),
                }) = slot {
                    let team = Team::from_startgg(&mut transaction, team).await?.ok_or(cal::Error::UnknownTeam)?;
                    if team.members(&mut transaction).await?.into_iter().any(|member| member.discord_id == Some(interaction.user.id)) {
                        authorized = true;
                        break
                    }
                } else {
                    return Err(cal::Error::Teams.into())
                }
            }
        } else {
            return Err(cal::Error::Teams.into())
        }
        if authorized {
            Some((transaction, startgg_set))
        } else {
            interaction.create_interaction_response(ctx, |r| r
                .interaction_response_data(|d| d
                    .ephemeral(true)
                    .content("Sorry, only participants in this race can use this command.")
                )
            ).await?;
            transaction.rollback().await?;
            None
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
        .map(|timestamp| Utc.timestamp(timestamp, 0))
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
            ctx.data.write().await.insert::<CommandIds>(CommandIds { assign, ban, pronoun_roles, schedule, watch_roles });
            Ok(())
        }))
        .on_interaction_create(|ctx, interaction| Box::pin(async move {
            match interaction {
                Interaction::ApplicationCommand(interaction) => if let Some(&command_ids) = ctx.data.read().await.get::<CommandIds>() {
                    if interaction.data.id == command_ids.assign {
                        let mut transaction = ctx.data.read().await.get::<DbPool>().as_ref().expect("database connection pool missing from Discord context").begin().await?;
                        let guild_id = interaction.guild_id.expect("/assign called outside of a guild");
                        if let Some(event_row) = sqlx::query!("SELECT series, event FROM events WHERE discord_guild = $1", i64::from(guild_id)).fetch_optional(&mut transaction).await? {
                            let startgg_set = match interaction.data.options[0].resolved.as_ref().expect("missing slash command option") {
                                CommandDataOptionValue::String(startgg_set) => startgg_set,
                                _ => panic!("unexpected slash command option type"),
                            };
                            sqlx::query!("INSERT INTO races (startgg_set, series, event, scheduling_thread) VALUES ($1, $2, $3, $4) ON CONFLICT (startgg_set) DO UPDATE SET scheduling_thread = EXCLUDED.scheduling_thread", &startgg_set, event_row.series, event_row.event, i64::from(interaction.channel_id)).execute(&mut transaction).await?;
                            transaction.commit().await?;
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(false)
                                    .content(MessageBuilder::default()
                                        .push("This thread is now assigned to set ")
                                        .push_safe(startgg_set) //TODO linkify set page, use phase/round/identifier
                                        .push(". Use </schedule:")
                                        .push(command_ids.schedule) //TODO impl Mentionable for CommandId
                                        .push("> to schedule as a regular race, or ping a tournament organizer to schedule as an async.")
                                        //TODO start settings draft, mentioning which team should use /first or /second
                                    )
                                )
                            ).await?;
                        } else {
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Sorry, this Discord server is not associated with a Mido's House event.")
                                )
                            ).await?;
                        }
                    } else if interaction.data.id == command_ids.ban {
                        if let Some((mut transaction, startgg_set)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            if let Some(Json(mut draft)) = sqlx::query_scalar!(r#"SELECT draft_state AS "draft_state: Json<Draft>" FROM races WHERE startgg_set = $1"#, startgg_set).fetch_one(&mut transaction).await? {
                                if draft.state.went_first.is_none() {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, first pick hasn't been chosen yet, use /first or /second") //TODO mention commands
                                        )
                                    ).await?;
                                } else if draft.state.pick_count() >= 2 {
                                    interaction.create_interaction_response(ctx, |r| r
                                        .interaction_response_data(|d| d
                                            .ephemeral(true)
                                            .content("Sorry, bans have already been chosen.")
                                        )
                                    ).await?;
                                } else {
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
                                        sqlx::query!("UPDATE races SET draft_state = $1 WHERE startgg_set = $2", Json(&draft) as _, startgg_set).execute(&mut transaction).await?;
                                        transaction.commit().await?;
                                        advance_draft(ctx, interaction, &draft).await?; //TODO include the setting that was banned in the reply
                                    } else {
                                        interaction.create_interaction_response(ctx, |r| r
                                            .interaction_response_data(|d| d
                                                .ephemeral(true)
                                                .content("Sorry, that setting is already locked in. Use /skip if you don't want to ban anything.") //TODO mention command
                                            )
                                        ).await?;
                                    }
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
                        if let Some((mut transaction, startgg_set)) = check_scheduling_thread_permissions(ctx, interaction).await? {
                            let start = match interaction.data.options[0].resolved.as_ref().expect("missing slash command option") {
                                CommandDataOptionValue::String(start) => start,
                                _ => panic!("unexpected slash command option type"),
                            };
                            if let Some(start) = parse_timestamp(start) {
                                sqlx::query!("UPDATE races SET start = $1 WHERE startgg_set = $2", start, startgg_set).execute(&mut transaction).await?;
                                transaction.commit().await?;
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(false)
                                        .content(format!("This race is now scheduled for <t:{}:F>.", start.timestamp()))
                                    )
                                ).await?;
                            } else {
                                interaction.create_interaction_response(ctx, |r| r
                                    .interaction_response_data(|d| d
                                        .ephemeral(true)
                                        .content("Sorry, that doesn't look like a Discord timestamp. You can use <https://hammertime.cyou/> to generate one.")
                                    )
                                ).await?;
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
