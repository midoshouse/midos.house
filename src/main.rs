#![deny(rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::time::Duration,
    futures::future::FutureExt as _,
    rocket::Rocket,
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
    sqlx::{
        PgPool,
        postgres::PgConnectOptions,
    },
    crate::{
        config::Config,
        util::Id,
    },
};

mod auth;
mod cal;
mod config;
mod event;
mod favicon;
mod http;
mod notification;
mod racetime_bot;
mod seed;
mod user;
mod util;

const FENHL: UserId = UserId(86841168427495424);

#[derive(Clone, Copy)]
struct CommandIds {
    pronoun_roles: CommandId,
    watch_roles: CommandId,
}

impl TypeMapKey for CommandIds {
    type Value = CommandIds;
}

fn parse_view_as(arg: &str) -> Result<(Id, Id), anyhow::Error> {
    let (from, to) = arg.split_once(':').ok_or(anyhow::anyhow!("missing colon in view-as option"))?;
    Ok((from.parse()?, to.parse()?))
}

#[derive(Default, Clone, Copy, clap::ValueEnum)]
enum Environment {
    #[cfg_attr(not(debug_assertions), default)]
    Production,
    #[cfg_attr(debug_assertions, default)]
    Dev,
    Local,
}

impl Environment {
    fn is_dev(&self) -> bool {
        match self {
            Self::Production => false,
            Self::Dev => true,
            Self::Local => true,
        }
    }

    fn racetime_host(&self) -> &'static str {
        if self.is_dev() { "racetime.midos.house" } else { "racetime.gg" }
    }
}

#[derive(clap::Parser)]
struct Args {
    #[clap(long, value_enum, default_value_t)]
    env: Environment,
    #[clap(long, parse(try_from_str = parse_view_as))]
    view_as: Vec<(Id, Id)>,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)] Any(#[from] anyhow::Error),
    #[error(transparent)] Base64(#[from] base64::DecodeError),
    #[error(transparent)] Racetime(#[from] racetime::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Rocket(#[from] rocket::Error),
    #[error(transparent)] Serenity(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Task(#[from] tokio::task::JoinError),
}

#[wheel::main(rocket, debug)]
async fn main(Args { env, view_as }: Args) -> Result<(), Error> {
    let config = Config::load().await?;
    let http_client = reqwest::Client::builder()
        .user_agent(concat!("MidosHouse/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .use_rustls_tls()
        .trust_dns(true)
        .https_only(true)
        .build()?;
    let discord_config = if env.is_dev() { &config.discord_dev } else { &config.discord_production };
    let discord_builder = serenity_utils::builder(discord_config.client_id, discord_config.bot_token.clone()).await?;
    let pool = PgPool::connect_with(PgConnectOptions::default().username("mido").database(if env.is_dev() { "fados_house" } else { "midos_house" }).application_name("midos-house")).await?;
    let rocket = http::rocket(pool, discord_builder.ctx_fut.clone(), http_client.clone(), &config, env, view_as.into_iter().collect()).await?;
    let shutdown = rocket.shutdown();
    let discord_builder = discord_builder
        .error_notifier(ErrorNotifier::User(FENHL))
        .on_guild_create(false, |ctx, guild, _| Box::pin(async move {
            let pronoun_roles = guild.create_application_command(ctx, |c| c
                .name("pronoun-roles")
                .kind(serenity::model::application::command::CommandType::ChatInput)
                .default_member_permissions(Permissions::ADMINISTRATOR)
                .dm_permission(false)
                .description("Creates gender pronoun roles and posts a message here that allows members to self-assign them.")
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
            ctx.data.write().await.insert::<CommandIds>(CommandIds { pronoun_roles, watch_roles });
            Ok(())
        }))
        .on_interaction_create(|ctx, interaction| Box::pin(async move {
            match interaction {
                Interaction::ApplicationCommand(interaction) => if let Some(&command_ids) = ctx.data.read().await.get::<CommandIds>() {
                    if interaction.data.id == command_ids.pronoun_roles {
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
        });
    let racetime_task = tokio::spawn(racetime_bot::main(
        http_client,
        config.ootr_api_key.clone(),
        env.racetime_host(),
        if env.is_dev() { config.racetime_bot_dev.clone() } else { config.racetime_bot_production.clone() },
        rocket.shutdown(),
    )).map(|res| match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let rocket_task = tokio::spawn(rocket.launch()).map(|res| match res {
        Ok(Ok(Rocket { .. })) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let discord_task = tokio::spawn(discord_builder.run()).map(|res| match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Error::from(e)),
        Err(e) => Err(Error::from(e)),
    });
    let ((), (), ()) = tokio::try_join!(discord_task, racetime_task, rocket_task)?;
    Ok(())
}
