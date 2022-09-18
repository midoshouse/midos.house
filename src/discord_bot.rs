use {
    chrono::prelude::*,
    lazy_regex::regex_captures,
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
    },
    sqlx::PgPool,
};

const FENHL: UserId = UserId(86841168427495424);

enum DbPool {}

impl TypeMapKey for DbPool {
    type Value = PgPool;
}

#[derive(Clone, Copy)]
struct CommandIds {
    assign: CommandId,
    pronoun_roles: CommandId,
    schedule: CommandId,
    watch_roles: CommandId,
}

impl TypeMapKey for CommandIds {
    type Value = CommandIds;
}

fn parse_timestamp(timestamp: &str) -> Option<DateTime<Utc>> {
    regex_captures!("^<t:(-?[0-9]+)(?::[tTdDfFR])?>$", timestamp)
        .and_then(|(_, timestamp)| timestamp.parse().ok())
        .map(|timestamp| Utc.timestamp(timestamp, 0))
}

pub(crate) fn configure_builder(discord_builder: serenity_utils::Builder, db_pool: PgPool, shutdown: rocket::Shutdown) -> serenity_utils::Builder {
    discord_builder
        .error_notifier(ErrorNotifier::User(FENHL))
        .data::<DbPool>(db_pool)
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
            ctx.data.write().await.insert::<CommandIds>(CommandIds { assign, pronoun_roles, schedule, watch_roles });
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
                            sqlx::query!("INSERT INTO races (startgg_set, series, event, scheduling_thread) VALUES ($1, $2, $3, $4) ON CONFLICT (startgg_set) DO UPDATE SET scheduling_thread = EXCLUDED.scheduling_thread", startgg_set, event_row.series, event_row.event, i64::from(interaction.channel_id)).execute(&mut transaction).await?;
                            transaction.commit().await?;
                        } else {
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Sorry, this Discord server is not associated with a Mido's House event.")
                                )
                            ).await?;
                        }
                    } if interaction.data.id == command_ids.pronoun_roles {
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
                        let mut transaction = ctx.data.read().await.get::<DbPool>().as_ref().expect("database connection pool missing from Discord context").begin().await?;
                        if let Some(startgg_set) = sqlx::query_scalar!("SELECT startgg_set FROM races WHERE scheduling_thread = $1", i64::from(interaction.channel_id)).fetch_optional(&mut transaction).await? {
                            //TODO only let players in this set and event organizers use this command
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
                        } else {
                            interaction.create_interaction_response(ctx, |r| r
                                .interaction_response_data(|d| d
                                    .ephemeral(true)
                                    .content("Sorry, this thread is not associated with a match. Please contact a tournament organizer to fix this.")
                                )
                            ).await?;
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
