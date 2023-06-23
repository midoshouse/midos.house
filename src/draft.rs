use {
    std::{
        borrow::Cow,
        collections::HashMap,
        fmt,
        iter,
    },
    itertools::Itertools as _,
    rand::prelude::*,
    serde::{
        Deserialize,
        Serialize,
    },
    serde_json::Value as Json,
    serenity::{
        all::MessageBuilder,
        model::prelude::*,
    },
    serenity_utils::message::MessageBuilderExt as _,
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        discord_bot::CommandIds,
        series::*,
        team,
        util::{
            Id,
            MessageBuilderExt as _,
        },
    },
};

pub(crate) type Picks = HashMap<Cow<'static, str>, Cow<'static, str>>;

#[derive(Clone, Copy)]
pub(crate) enum Team {
    HighSeed,
    LowSeed,
}

impl Team {
    pub(crate) fn choose<T>(&self, high_seed: T, low_seed: T) -> T {
        match self {
            Self::HighSeed => high_seed,
            Self::LowSeed => low_seed,
        }
    }
}

impl fmt::Display for Team {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HighSeed => write!(f, "Team A"),
            Self::LowSeed => write!(f, "Team B"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    MultiworldS3,
    TournoiFrancoS3,
}

#[derive(Clone)]
pub(crate) struct BanSetting {
    pub(crate) name: &'static str,
    pub(crate) display: &'static str,
    pub(crate) default: &'static str,
    pub(crate) default_display: &'static str,
    pub(crate) description: &'static str,
}

pub(crate) struct BanSettings(Vec<(&'static str, Vec<BanSetting>)>);

impl BanSettings {
    pub(crate) fn num_settings(&self) -> usize {
        self.0.iter().map(|(_, page)| page.len()).sum()
    }

    pub(crate) fn page(&self, idx: usize) -> Option<(&'static str, &[BanSetting])> {
        self.0.get(idx).map(|(name, settings)| (*name, &**settings))
    }

    pub(crate) fn all(self) -> impl Iterator<Item = BanSetting> {
        self.0.into_iter().flat_map(|(_, settings)| settings)
    }

    pub(crate) fn get(&self, name: &str) -> Option<BanSetting> {
        self.0.iter().flat_map(|(_, settings)| settings).find(|setting| setting.name == name).cloned()
    }
}

#[derive(Clone)]
pub(crate) struct DraftSettingChoice {
    pub(crate) name: &'static str,
    pub(crate) display: &'static str,
}

#[derive(Clone)]
pub(crate) struct DraftSetting {
    pub(crate) name: &'static str,
    pub(crate) display: &'static str,
    pub(crate) options: Vec<DraftSettingChoice>,
    pub(crate) description: &'static str,
}

pub(crate) struct DraftSettings(Vec<(&'static str, Vec<DraftSetting>)>);

impl DraftSettings {
    pub(crate) fn num_settings(&self) -> usize {
        self.0.iter().map(|(_, page)| page.len()).sum()
    }

    pub(crate) fn page(&self, idx: usize) -> Option<(&'static str, &[DraftSetting])> {
        self.0.get(idx).map(|(name, settings)| (*name, &**settings))
    }

    pub(crate) fn all(self) -> impl Iterator<Item = DraftSetting> {
        self.0.into_iter().flat_map(|(_, settings)| settings)
    }

    pub(crate) fn get(&self, name: &str) -> Option<DraftSetting> {
        self.0.iter().flat_map(|(_, settings)| settings).find(|setting| setting.name == name).cloned()
    }
}

pub(crate) enum StepKind {
    /// The high seed chooses whether to go first or second.
    GoFirst,
    /// The given team sets one of the available settings to its default value.
    Ban {
        team: Team,
        /// Grouped into named pages in case they exceed the button limit for Discord message components.
        available_settings: BanSettings,
        skippable: bool,
    },
    Pick {
        team: Team,
        /// Grouped into named pages in case they exceed the button limit for Discord message components.
        available_choices: DraftSettings,
        skippable: bool,
    },
    BooleanChoice {
        team: Team,
    },
    Done(serde_json::Map<String, Json>), //TODO use ootr_utils::Settings instead?
}

pub(crate) struct Step {
    pub(crate) kind: StepKind,
    pub(crate) message: String,
}

pub(crate) enum MessageContext<'a> {
    None,
    Discord {
        transaction: Transaction<'a, Postgres>,
        guild_id: GuildId,
        command_ids: CommandIds,
        teams: Vec<team::Team>,
        team: team::Team,
    },
    RaceTime {
        high_seed_name: &'a str,
        low_seed_name: &'a str,
        reply_to: &'a str,
    },
}

impl<'a> MessageContext<'a> {
    //HACK: convenience method to get the database transaction back out of MessageContext::Discord. Panics if called on another variant
    pub(crate) fn into_transaction(self) -> Transaction<'a, Postgres> {
        let Self::Discord { transaction, .. } = self else { panic!("called into_transaction on non-Discord draft message context") };
        transaction
    }
}

pub(crate) enum Action {
    GoFirst(bool),
    Ban {
        setting: String,
    },
    Pick {
        setting: String,
        value: String,
    },
    Skip,
    BooleanChoice(bool),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Draft {
    pub(crate) high_seed: Id,
    pub(crate) went_first: Option<bool>,
    #[serde(default)]
    pub(crate) skipped_bans: u8,
    #[serde(flatten)]
    pub(crate) settings: Picks,
}

impl Draft {
    fn pick_count(&self) -> u8 {
        self.skipped_bans + u8::try_from(self.settings.len()).unwrap()
    }

    pub(crate) async fn next_step(&self, kind: Kind, msg_ctx: &mut MessageContext<'_>) -> sqlx::Result<Step> {
        Ok(match kind {
            Kind::MultiworldS3 => {
                if let Some(went_first) = self.went_first {
                    match self.pick_count() {
                        prev_bans @ 0..=1 => {
                            let team = match (prev_bans, went_first) {
                                (0, true) | (1, false) => Team::HighSeed,
                                (0, false) | (1, true) => Team::LowSeed,
                                (2.., _) => unreachable!(),
                            };
                            Step {
                                kind: StepKind::Ban {
                                    available_settings: BanSettings(vec![
                                        ("All Settings", mw::S3_SETTINGS.into_iter()
                                            .filter(|&mw::S3Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|mw::S3Setting { name, display, default, default_display, description, .. }| BanSetting {
                                                name, display, default, default_display, description,
                                            })
                                            .collect()),
                                    ]),
                                    skippable: true,
                                    team,
                                },
                                message: match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                        let (mut high_seed, mut low_seed) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                        let high_seed = high_seed.remove(0);
                                        let low_seed = low_seed.remove(0);
                                        MessageBuilder::default()
                                            .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                            .push(": lock a setting to its default using ")
                                            .mention_command(command_ids.ban.unwrap(), "ban")
                                            .push(", or use ")
                                            .mention_command(command_ids.skip.unwrap(), "skip")
                                            .push(" if you don't want to ban anything.")
                                            .build()
                                    }
                                    MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => format!(
                                        "{}, lock a setting to its default using “!ban <setting>”, or use “!skip” if you don't want to ban anything.{}",
                                        team.choose(high_seed_name, low_seed_name),
                                        if prev_bans == 0 { " Use “!settings” for a list of available settings." } else { "" },
                                    ),
                                },
                            }
                        }
                        n @ 2..=5 => {
                            let team = match (n, went_first) {
                                (2, true) | (3, false) | (4, false) | (5, true) => Team::HighSeed,
                                (2, false) | (3, true) | (4, true) | (5, false) => Team::LowSeed,
                                (0..=1 | 6.., _) => unreachable!(),
                            };
                            Step {
                                kind: StepKind::Pick {
                                    available_choices: DraftSettings(vec![
                                        ("All Settings", mw::S3_SETTINGS.into_iter()
                                            .filter(|&mw::S3Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|mw::S3Setting { name, display, default, default_display, other, description }| DraftSetting {
                                                options: iter::once(DraftSettingChoice { name: default, display: default_display })
                                                    .chain(other.iter().map(|&(name, display)| DraftSettingChoice { name, display }))
                                                    .collect(),
                                                name, display, description,
                                            })
                                            .collect()),
                                    ]),
                                    skippable: n == 5,
                                    team,
                                },
                                message: match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                        let (mut high_seed, mut low_seed) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                        let high_seed = high_seed.remove(0);
                                        let low_seed = low_seed.remove(0);
                                        match n {
                                            2 => MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(": pick a setting using ")
                                                .mention_command(command_ids.draft.unwrap(), "draft")
                                                .push('.')
                                                .build(),
                                            3 => MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(": pick a setting using ")
                                                .mention_command(command_ids.draft.unwrap(), "draft")
                                                .push(". You will have another pick after this.")
                                                .build(),
                                            4 => MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(": pick your second setting using ")
                                                .mention_command(command_ids.draft.unwrap(), "draft")
                                                .push('.')
                                                .build(),
                                            5 => MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(": pick a setting using ")
                                                .mention_command(command_ids.draft.unwrap(), "draft")
                                                .push(". You can also use ")
                                                .mention_command(command_ids.skip.unwrap(), "skip")
                                                .push(" if you want to leave the settings as they are.")
                                                .build(),
                                            0..=1 | 6.. => unreachable!(),
                                        }
                                    }
                                    MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => match n {
                                        2 => format!("{}, pick a setting using “!draft <setting> <value>”", team.choose(high_seed_name, low_seed_name)),
                                        3 => format!("{}, pick two settings.", team.choose(high_seed_name, low_seed_name)),
                                        4 => format!("And your second pick?"),
                                        5 => format!("{}, pick the final setting. You can also use “!skip” if you want to leave the settings as they are.", team.choose(high_seed_name, low_seed_name)),
                                        0..=1 | 6.. => unreachable!(),
                                    },
                                },
                            }
                        }
                        6.. => Step {
                            kind: StepKind::Done(mw::resolve_draft_settings(&self.settings)),
                            message: match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { .. } => format!("Settings draft completed. You will be playing with {}.", mw::display_draft_picks(&self.settings)),
                                MessageContext::RaceTime { .. } => mw::display_draft_picks(&self.settings),
                            },
                        },
                    }
                } else {
                    Step {
                        kind: StepKind::GoFirst,
                        message: match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                let (mut high_seed, _) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                let high_seed = high_seed.remove(0);
                                MessageBuilder::default()
                                    .mention_team(transaction, Some(*guild_id), high_seed).await?
                                    .push(": you have the higher seed. Choose whether you want to go ")
                                    .mention_command(command_ids.first.unwrap(), "first")
                                    .push(" or ")
                                    .mention_command(command_ids.second.unwrap(), "second")
                                    .push(" in the settings draft.")
                                    .build()
                            }
                            MessageContext::RaceTime { high_seed_name, .. } => format!("{high_seed_name}, you have the higher seed. Choose whether you want to go !first or !second"),
                        },
                    }
                }
            }
            Kind::TournoiFrancoS3 => {
                if let Some(went_first) = self.went_first {
                    let mut pick_count = self.pick_count();
                    let select_mixed_dungeons = !self.settings.contains_key("mixed-dungeons") && self.settings.get("dungeon-er").map(|dungeon_er| &**dungeon_er).unwrap_or("off") == "on" && self.settings.get("mixed-er").map(|mixed_er| &**mixed_er).unwrap_or("off") == "on";
                    if select_mixed_dungeons {
                        // chosen by the same team that chose the previous setting
                        pick_count -= 1;
                    }
                    let team = match (pick_count, went_first) {
                        (0, true) | (1, false) | (2, true) | (3, false) | (4, false) | (5, true) | (6, true) | (7, false) | (8, true) | (9, false) => Team::HighSeed,
                        (0, false) | (1, true) | (2, false) | (3, true) | (4, true) | (5, false) | (6, false) | (7, true) | (8, false) | (9, true) => Team::LowSeed,
                        (10.., _) => return Ok(Step {
                            kind: StepKind::Done(fr::resolve_draft_settings(&self.settings)),
                            message: match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { .. } => format!("Settings draft completed. You will be playing with {}.", fr::display_draft_picks(&self.settings)),
                                MessageContext::RaceTime { .. } => fr::display_draft_picks(&self.settings),
                            },
                        }),
                    };
                    if select_mixed_dungeons {
                        Step {
                            kind: StepKind::BooleanChoice { team },
                            message: match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                    let (mut high_seed, mut low_seed) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                    let high_seed = high_seed.remove(0);
                                    let low_seed = low_seed.remove(0);
                                    MessageBuilder::default()
                                        .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                        .push(": should dungeon entrances be mixed with interiors and grottos? Use ")
                                        .mention_command(command_ids.yes.unwrap(), "yes")
                                        .push(" or ")
                                        .mention_command(command_ids.no.unwrap(), "no")
                                        .push('.')
                                        .build()
                                }
                                MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => format!(
                                    "{}, should dungeon entrances be mixed with interiors and grottos? Use !yes or !no",
                                    team.choose(high_seed_name, low_seed_name),
                                ),
                            },
                        }
                    } else {
                        match self.pick_count() {
                            prev_bans @ 0..=1 => {
                                let hard_settings_ok = self.settings.get("hard_settings_ok").map(|hard_settings_ok| &**hard_settings_ok).unwrap_or("no") == "ok";
                                let (hard_settings, classic_settings) = fr::S3_SETTINGS.into_iter()
                                    .filter(|&fr::S3Setting { name, .. }| !self.settings.contains_key(name))
                                    .filter_map(|fr::S3Setting { name, display, default, default_display, other, description }| {
                                        let (is_hard, is_empty) = if hard_settings_ok {
                                            (other.iter().all(|&(_, hard, _)| hard), other.is_empty())
                                        } else {
                                            (false, other.iter().any(|(_, hard, _)| !hard))
                                        };
                                        (!is_empty).then(|| (is_hard, BanSetting { name, display, default, default_display, description }))
                                    })
                                    .partition::<Vec<_>, _>(|&(is_hard, _)| is_hard);
                                let mut available_settings = vec![
                                    ("Settings classiques", classic_settings.into_iter().map(|(_, setting)| setting).collect()),
                                ];
                                if hard_settings_ok && !hard_settings.is_empty() {
                                    available_settings.push(("Settings difficiles", hard_settings.into_iter().map(|(_, setting)| setting).collect()));
                                }
                                Step {
                                    kind: StepKind::Ban {
                                        available_settings: BanSettings(available_settings),
                                        skippable: false,
                                        team,
                                    },
                                    message: match msg_ctx {
                                        MessageContext::None => String::default(),
                                        MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                            let (mut high_seed, mut low_seed) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                            let high_seed = high_seed.remove(0);
                                            let low_seed = low_seed.remove(0);
                                            MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(" : Veuillez ban un setting en utilisant ")
                                                .mention_command(command_ids.ban.unwrap(), "ban")
                                                .push('.')
                                                .build()
                                        }
                                        MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => format!(
                                            "{}, Veuillez ban un setting en utilisant “!ban <setting>”.{}",
                                            team.choose(high_seed_name, low_seed_name),
                                            if prev_bans == 0 { " Use “!settings” for a list of available settings." } else { "" },
                                        ),
                                    },
                                }
                            }
                            n @ 2..=9 => {
                                let hard_settings_ok = self.settings.get("hard_settings_ok").map(|hard_settings_ok| &**hard_settings_ok).unwrap_or("no") == "ok";
                                let can_ban = n < 8 || self.settings.get(team.choose("high_seed_has_picked", "low_seed_has_picked")).map(|has_picked| &**has_picked).unwrap_or("no") == "yes";
                                let skippable = n == 9 && can_ban;
                                let (hard_settings, classic_settings) = fr::S3_SETTINGS.into_iter()
                                    .filter(|&fr::S3Setting { name, .. }| !self.settings.contains_key(name))
                                    .filter_map(|fr::S3Setting { name, display, default, default_display, other, description }| {
                                        let (is_hard, other) = if hard_settings_ok {
                                            (other.iter().all(|&(_, hard, _)| hard), other.to_owned())
                                        } else {
                                            (false, other.iter().filter(|(_, hard, _)| !hard).copied().collect_vec())
                                        };
                                        (!other.is_empty()).then(|| (is_hard, DraftSetting {
                                            options: can_ban.then(|| DraftSettingChoice { name: default, display: default_display }).into_iter()
                                                .chain(other.into_iter().map(|(name, _, display)| DraftSettingChoice { name, display }))
                                                .collect(),
                                            name, display, description,
                                        }))
                                    })
                                    .partition::<Vec<_>, _>(|&(is_hard, _)| is_hard);
                                let mut available_choices = vec![
                                    ("Settings classiques", classic_settings.into_iter().map(|(_, setting)| setting).collect()),
                                ];
                                if hard_settings_ok && !hard_settings.is_empty() {
                                    available_choices.push(("Settings difficiles", hard_settings.into_iter().map(|(_, setting)| setting).collect()));
                                }
                                Step {
                                    kind: StepKind::Pick {
                                        available_choices: DraftSettings(available_choices),
                                        team, skippable,
                                    },
                                    message: match msg_ctx {
                                        MessageContext::None => String::default(),
                                        MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                            let (mut high_seed, mut low_seed) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                            let high_seed = high_seed.remove(0);
                                            let low_seed = low_seed.remove(0);
                                            match n {
                                                2 | 7 | 8 => MessageBuilder::default()
                                                    .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                    .push(": pick a setting using ")
                                                    .mention_command(command_ids.draft.unwrap(), "draft")
                                                    .push('.')
                                                    .build(),
                                                3 | 5 => MessageBuilder::default()
                                                    .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                    .push(": pick a setting using ")
                                                    .mention_command(command_ids.draft.unwrap(), "draft")
                                                    .push(". You will have another pick after this.")
                                                    .build(),
                                                4 | 6 => MessageBuilder::default()
                                                    .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                    .push(": pick your second setting using ")
                                                    .mention_command(command_ids.draft.unwrap(), "draft")
                                                    .push('.')
                                                    .build(),
                                                9 => {
                                                    let mut builder = MessageBuilder::default();
                                                    builder.mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?;
                                                    builder.push(": pick a setting using ");
                                                    builder.mention_command(command_ids.draft.unwrap(), "draft");
                                                    builder.push('.');
                                                    if skippable {
                                                        builder.push(". You can also use ");
                                                        builder.mention_command(command_ids.skip.unwrap(), "skip");
                                                        builder.push(" if you want to leave the settings as they are.");
                                                    }
                                                    builder.build()
                                                }
                                                0..=1 | 10.. => unreachable!(),
                                            }
                                        }
                                        MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => match n {
                                            2 => format!("{}, pick a setting using “!draft <setting> <value>”", team.choose(high_seed_name, low_seed_name)),
                                            3 | 5 => format!("{}, pick two settings.", team.choose(high_seed_name, low_seed_name)),
                                            4 | 6 => format!("And your second pick?"),
                                            7 | 8 => format!("{}, pick a setting.", team.choose(high_seed_name, low_seed_name)),
                                            9 if skippable => format!("{}, pick the final setting. You can also use “!skip” if you want to leave the settings as they are.", team.choose(high_seed_name, low_seed_name)),
                                            9 => format!("{}, pick the final setting.", team.choose(high_seed_name, low_seed_name)),
                                            0..=1 | 10.. => unreachable!(),
                                        },
                                    },
                                }
                            }
                            10.. => unreachable!(),
                        }
                    }
                } else {
                    Step {
                        kind: StepKind::GoFirst,
                        message: match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                let (mut high_seed, _) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                let high_seed = high_seed.remove(0);
                                let mut builder = MessageBuilder::default();
                                builder.mention_team(transaction, Some(*guild_id), high_seed).await?;
                                builder.push(": you have the higher seed. Choose whether you want to go ");
                                builder.mention_command(command_ids.first.unwrap(), "first");
                                builder.push(" or ");
                                builder.mention_command(command_ids.second.unwrap(), "second");
                                builder.push(" in the settings draft.");
                                if self.settings.get("mq_ok").map(|mq_ok| &**mq_ok).unwrap_or("no") == "ok" {
                                    builder.push(" Please include the number of MQ dungeons.");
                                }
                                builder.build()
                            }
                            MessageContext::RaceTime { high_seed_name, .. } => format!("{high_seed_name}, you have the higher seed. Choose whether you want to go !first or !second"),
                        },
                    }
                }
            }
        })
    }

    pub(crate) async fn active_team(&self, kind: Kind) -> sqlx::Result<Option<Team>> {
        Ok(match self.next_step(kind, &mut MessageContext::None).await?.kind {
            StepKind::GoFirst => Some(Team::HighSeed),
            StepKind::Ban { team, .. } | StepKind::Pick { team, .. } | StepKind::BooleanChoice { team } => Some(team),
            StepKind::Done(_) => None,
        })
    }

    /// Assumes that the caller has checked that the team is part of the race in the first place.
    pub(crate) async fn is_active_team(&self, kind: Kind, team: Id) -> sqlx::Result<bool> {
        Ok(match self.active_team(kind).await? {
            Some(Team::HighSeed) => team == self.high_seed,
            Some(Team::LowSeed) => team != self.high_seed,
            None => false,
        })
    }

    pub(crate) async fn apply(&mut self, kind: Kind, msg_ctx: &mut MessageContext<'_>, action: Action) -> sqlx::Result<Result<String, String>> {
        Ok(match kind {
            Kind::MultiworldS3 => {
                let resolved_action = match action {
                    Action::Ban { setting } => Action::Pick { value: mw::S3_SETTINGS.into_iter().find(|&mw::S3Setting { name, .. }| *name == setting).unwrap().default.to_owned(), setting },
                    Action::BooleanChoice(value) if matches!(self.next_step(kind, &mut MessageContext::None).await?.kind, StepKind::GoFirst) => Action::GoFirst(value),
                    _ => action,
                };
                match resolved_action {
                    Action::GoFirst(first) => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => {
                            self.went_first = Some(first);
                            Ok(match msg_ctx {
                                MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                    .mention_team(transaction, Some(*guild_id), team).await?
                                    .push(if team.name_is_plural() { " have" } else { " has" })
                                    .push(" chosen to go ")
                                    .push(if first { "first" } else { "second" })
                                    .push(" in the settings draft.")
                                    .build(),
                            })
                        }
                        StepKind::Ban { .. } | StepKind::Pick { .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, first pick has already been chosen."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, first pick has already been chosen."),
                        }),
                        StepKind::BooleanChoice { .. } => unreachable!(),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                    },
                    Action::Ban { .. } => unreachable!("normalized to Action::Pick above"),
                    Action::Pick { setting, value } => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                .push("Sorry, first pick hasn't been chosen yet, use ")
                                .mention_command(command_ids.first.unwrap(), "first")
                                .push(" or ")
                                .mention_command(command_ids.second.unwrap(), "second")
                                .build(),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”"),
                        }),
                        StepKind::Ban { available_settings, skippable, .. } => if let Some(setting) = available_settings.get(&setting) {
                            if value == setting.default {
                                self.settings.insert(Cow::Borrowed(setting.name), Cow::Borrowed(setting.default));
                                Ok(match msg_ctx {
                                    MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                    MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                        .mention_team(transaction, Some(*guild_id), team).await?
                                        .push(if team.name_is_plural() { " have locked in " } else { " has locked in " })
                                        .push(setting.default_display)
                                        .push('.')
                                        .build(),
                                })
                            } else {
                                Err(match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                        .push("Sorry, bans haven't been chosen yet, use ")
                                        .mention_command(command_ids.ban.unwrap(), "ban")
                                        .build(),
                                    MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, bans haven't been chosen yet. Use “!ban <setting>”"),
                                })
                            }
                        } else {
                            let exists = mw::S3_SETTINGS.into_iter().any(|mw::S3Setting { name, .. }| setting == name);
                            Err(match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { command_ids, .. } => {
                                    let mut content = MessageBuilder::default();
                                    if exists {
                                        content.push("Sorry, that setting is already locked in. Use one of the following: ");
                                    } else {
                                        content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                    }
                                    for (i, setting) in available_settings.all().enumerate() {
                                        if i > 0 {
                                            content.push(" or ");
                                        }
                                        content.push_mono(setting.name);
                                    }
                                    if exists && skippable {
                                        content.push(". Use ");
                                        content.mention_command(command_ids.skip.unwrap(), "skip");
                                        content.push(" if you don't want to ban anything.");
                                    }
                                    content.build()
                                }
                                MessageContext::RaceTime { reply_to, .. } => format!(
                                    "Sorry {reply_to}, {}. Use one of the following: {}{}",
                                    if exists { "that setting is already locked in" } else { "I don't recognize that setting" },
                                    available_settings.all().map(|setting| setting.name).format(" or "),
                                    if exists && skippable { ". Use “!skip” if you don't want to ban anything." } else { "" },
                                ),
                            })
                        },
                        StepKind::Pick { available_choices, skippable, .. } => if let Some(setting) = available_choices.get(&setting) {
                            if let Some(option) = setting.options.into_iter().find(|option| option.name == value) {
                                self.settings.insert(Cow::Borrowed(setting.name), Cow::Borrowed(option.name));
                                Ok(match msg_ctx {
                                    MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                    MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                        .mention_team(transaction, Some(*guild_id), team).await?
                                        .push(if team.name_is_plural() { " have picked " } else { " has picked " })
                                        .push(option.display)
                                        .push('.')
                                        .build(),
                                })
                            } else {
                                Err(match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { .. } => {
                                        let mut content = MessageBuilder::default();
                                        content.push("Sorry, that's not a possible value for this setting. Use one of the following: ");
                                        for (i, setting) in available_choices.all().enumerate() {
                                            if i > 0 {
                                                content.push(" or ");
                                            }
                                            content.push_mono(setting.name);
                                        }
                                        content.build()
                                    }
                                    MessageContext::RaceTime { reply_to, .. } => format!(
                                        "Sorry {reply_to}, that's not a possible value for this setting. Use one of the following: {}",
                                        available_choices.all().map(|setting| setting.name).format(" or "),
                                    ),
                                })
                            }
                        } else {
                            let exists = mw::S3_SETTINGS.into_iter().any(|mw::S3Setting { name, .. }| setting == name);
                            Err(match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { command_ids, .. } => {
                                    let mut content = MessageBuilder::default();
                                    if exists {
                                        content.push("Sorry, that setting is already locked in. Use one of the following: ");
                                    } else {
                                        content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                    }
                                    for (i, setting) in available_choices.all().enumerate() {
                                        if i > 0 {
                                            content.push(" or ");
                                        }
                                        content.push_mono(setting.name);
                                    }
                                    if exists && skippable {
                                        content.push(". Use ");
                                        content.mention_command(command_ids.skip.unwrap(), "skip");
                                        content.push(" if you don't want to pick anything.");
                                    }
                                    content.build()
                                }
                                MessageContext::RaceTime { reply_to, .. } => format!(
                                    "Sorry {reply_to}, {}. Use one of the following: {}{}",
                                    if exists { "that setting is already locked in" } else { "I don't recognize that setting" },
                                    available_choices.all().map(|setting| setting.name).format(" or "),
                                    if exists && skippable { ". Use “!skip” if you don't want to pick anything." } else { "" },
                                ),
                            })
                        },
                        StepKind::BooleanChoice { .. } => unreachable!(),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                    },
                    Action::Skip => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                .push("Sorry, first pick hasn't been chosen yet, use ")
                                .mention_command(command_ids.first.unwrap(), "first")
                                .push(" or ")
                                .mention_command(command_ids.second.unwrap(), "second")
                                .build(),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”")
                        }),
                        StepKind::Ban { skippable: true, .. } | StepKind::Pick { skippable: true, .. } => {
                            let skip_kind = match self.pick_count() {
                                0 | 1 => "ban",
                                5 => "final pick",
                                _ => unreachable!(),
                            };
                            self.skipped_bans += 1;
                            Ok(match msg_ctx {
                                MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                    .mention_team(&mut *transaction, Some(*guild_id), team).await?
                                    .push(if team.name_is_plural() { " have skipped " } else { " has skipped " })
                                    .push(team.possessive_determiner(transaction).await?)
                                    .push(' ')
                                    .push(skip_kind)
                                    .push('.')
                                    .build(),
                            })
                        }
                        StepKind::Ban { skippable: false, .. } | StepKind::Pick { skippable: false, .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this part of the draft can't be skipped."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this part of the draft can't be skipped."),
                        }),
                        StepKind::BooleanChoice { .. } => unreachable!(),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                    },
                    Action::BooleanChoice(_) => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => unreachable!("normalized to Action::GoFirst above"),
                        StepKind::BooleanChoice { .. } => unreachable!(),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                        _ => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, the current step is not a yes/no question."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, the current step is not a yes/no question."),
                        }),
                    },
                }
            }
            Kind::TournoiFrancoS3 => {
                let resolved_action = match action {
                    Action::Ban { setting } => Action::Pick { value: fr::S3_SETTINGS.into_iter().find(|&fr::S3Setting { name, .. }| *name == setting).unwrap().default.to_owned(), setting },
                    Action::BooleanChoice(value) if matches!(self.next_step(kind, &mut MessageContext::None).await?.kind, StepKind::GoFirst) => Action::GoFirst(value),
                    _ => action,
                };
                match resolved_action {
                    Action::GoFirst(first) => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => {
                            self.went_first = Some(first);
                            Ok(match msg_ctx {
                                MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                    .mention_team(transaction, Some(*guild_id), team).await?
                                    .push(" a choisi de partir ")
                                    .push(if first { "premier" } else { "second" })
                                    .push(" pour le draft.")
                                    .build(),
                            })
                        }
                        StepKind::Ban { .. } | StepKind::Pick { .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, first pick has already been chosen."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, first pick has already been chosen."),
                        }),
                        StepKind::BooleanChoice { .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                .push("Sorry, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use ")
                                .mention_command(command_ids.yes.unwrap(), "yes")
                                .push(" or ")
                                .mention_command(command_ids.no.unwrap(), "no")
                                .push('.')
                                .build(),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use !yes or !no"),
                        }),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                    },
                    Action::Ban { .. } => unreachable!("normalized to Action::Pick above"),
                    Action::Pick { setting, value } => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                .push("Sorry, first pick hasn't been chosen yet, use ")
                                .mention_command(command_ids.first.unwrap(), "first")
                                .push(" or ")
                                .mention_command(command_ids.second.unwrap(), "second")
                                .build(),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”"),
                        }),
                        StepKind::Ban { available_settings, skippable, .. } => if let Some(setting) = available_settings.get(&setting) {
                            if value == setting.default {
                                self.settings.insert(Cow::Borrowed(setting.name), Cow::Borrowed(setting.default));
                                Ok(match msg_ctx {
                                    MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                    MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                        .mention_team(transaction, Some(*guild_id), team).await?
                                        .push(if team.name_is_plural() { " have locked in " } else { " has locked in " })
                                        .push(setting.default_display)
                                        .push('.')
                                        .build(),
                                })
                            } else {
                                //TODO check if this setting is disabled because it is hard
                                Err(match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                        .push("Sorry, bans haven't been chosen yet, use ")
                                        .mention_command(command_ids.ban.unwrap(), "ban")
                                        .build(),
                                    MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, bans haven't been chosen yet. Use “!ban <setting>”"),
                                })
                            }
                        } else {
                            let exists = fr::S3_SETTINGS.into_iter().any(|fr::S3Setting { name, .. }| setting == name);
                            Err(match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { command_ids, .. } => {
                                    let mut content = MessageBuilder::default();
                                    if exists {
                                        //TODO check if this setting is disabled because it is hard
                                        content.push("Sorry, that setting is already locked in. Use one of the following: ");
                                    } else {
                                        content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                    }
                                    for (i, setting) in available_settings.all().enumerate() {
                                        if i > 0 {
                                            content.push(" or ");
                                        }
                                        content.push_mono(setting.name);
                                    }
                                    if exists && skippable {
                                        content.push(". Use ");
                                        content.mention_command(command_ids.skip.unwrap(), "skip");
                                        content.push(" if you don't want to ban anything.");
                                    }
                                    content.build()
                                }
                                MessageContext::RaceTime { reply_to, .. } => format!(
                                    "Sorry {reply_to}, {}. Use one of the following: {}{}",
                                    if exists { "that setting is already locked in" } else { "I don't recognize that setting" },
                                    available_settings.all().map(|setting| setting.name).format(" or "),
                                    if exists && skippable { ". Use “!skip” if you don't want to ban anything." } else { "" },
                                ),
                            })
                        },
                        StepKind::Pick { available_choices, skippable, .. } => if let Some(setting) = available_choices.get(&setting) {
                            if let Some(option) = setting.options.into_iter().find(|option| option.name == value) {
                                self.settings.insert(Cow::Borrowed(setting.name), Cow::Borrowed(option.name));
                                Ok(match msg_ctx {
                                    MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                    MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                        .mention_team(transaction, Some(*guild_id), team).await?
                                        .push(if team.name_is_plural() { " have picked " } else { " has picked " })
                                        .push(option.display)
                                        .push('.')
                                        .build(),
                                })
                            } else {
                                Err(match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { .. } => {
                                        let mut content = MessageBuilder::default();
                                        content.push("Sorry, that's not a possible value for this setting. Use one of the following: ");
                                        for (i, setting) in available_choices.all().enumerate() {
                                            if i > 0 {
                                                content.push(" or ");
                                            }
                                            content.push_mono(setting.name);
                                        }
                                        content.build()
                                    }
                                    MessageContext::RaceTime { reply_to, .. } => format!(
                                        "Sorry {reply_to}, that's not a possible value for this setting. Use one of the following: {}",
                                        available_choices.all().map(|setting| setting.name).format(" or "),
                                    ),
                                })
                            }
                        } else {
                            let exists = fr::S3_SETTINGS.into_iter().any(|fr::S3Setting { name, .. }| setting == name);
                            Err(match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { command_ids, .. } => {
                                    let mut content = MessageBuilder::default();
                                    if exists {
                                        content.push("Sorry, that setting is already locked in. Use one of the following: ");
                                    } else {
                                        content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                    }
                                    for (i, setting) in available_choices.all().enumerate() {
                                        if i > 0 {
                                            content.push(" or ");
                                        }
                                        content.push_mono(setting.name);
                                    }
                                    if exists && skippable {
                                        content.push(". Use ");
                                        content.mention_command(command_ids.skip.unwrap(), "skip");
                                        content.push(" if you don't want to pick anything.");
                                    }
                                    content.build()
                                }
                                MessageContext::RaceTime { reply_to, .. } => format!(
                                    "Sorry {reply_to}, {}. Use one of the following: {}{}",
                                    if exists { "that setting is already locked in" } else { "I don't recognize that setting" },
                                    available_choices.all().map(|setting| setting.name).format(" or "),
                                    if exists && skippable { ". Use “!skip” if you don't want to pick anything." } else { "" },
                                ),
                            })
                        },
                        StepKind::BooleanChoice { .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                .push("Sorry, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use ")
                                .mention_command(command_ids.yes.unwrap(), "yes")
                                .push(" or ")
                                .mention_command(command_ids.no.unwrap(), "no")
                                .push('.')
                                .build(),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use !yes or !no"),
                        }),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                    },
                    Action::Skip => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                .push("Sorry, first pick hasn't been chosen yet, use ")
                                .mention_command(command_ids.first.unwrap(), "first")
                                .push(" or ")
                                .mention_command(command_ids.second.unwrap(), "second")
                                .build(),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, first pick hasn't been chosen yet, use “!first” or “!second”")
                        }),
                        StepKind::Ban { skippable: true, .. } | StepKind::Pick { skippable: true, .. } => {
                            let skip_kind = match self.pick_count() {
                                0 | 1 => "ban",
                                5 => "final pick",
                                _ => unreachable!(),
                            };
                            self.skipped_bans += 1;
                            Ok(match msg_ctx {
                                MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                    .mention_team(&mut *transaction, Some(*guild_id), team).await?
                                    .push(if team.name_is_plural() { " have skipped " } else { " has skipped " })
                                    .push(team.possessive_determiner(transaction).await?)
                                    .push(' ')
                                    .push(skip_kind)
                                    .push('.')
                                    .build(),
                            })
                        }
                        StepKind::Ban { skippable: false, .. } | StepKind::Pick { skippable: false, .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this part of the draft can't be skipped."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this part of the draft can't be skipped."),
                        }),
                        StepKind::BooleanChoice { .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => MessageBuilder::default()
                                .push("Sorry, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use ")
                                .mention_command(command_ids.yes.unwrap(), "yes")
                                .push(" or ")
                                .mention_command(command_ids.no.unwrap(), "no")
                                .push('.')
                                .build(),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use !yes or !no"),
                        }),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                    },
                    Action::BooleanChoice(value) => match self.next_step(kind, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => unreachable!("normalized to Action::GoFirst above"),
                        StepKind::BooleanChoice { .. } => {
                            self.settings.insert(Cow::Borrowed("mixed-dungeons"), Cow::Borrowed(if value { "mixed" } else { "separate" }));
                            Ok(match msg_ctx {
                                MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                MessageContext::Discord { transaction, guild_id, team, .. } => MessageBuilder::default()
                                    .mention_team(&mut *transaction, Some(*guild_id), team).await?
                                    .push(if team.name_is_plural() { " have selected " } else { " has selected " })
                                    .push(if value { "mixed" } else { "separate" })
                                    .push(" dungeon entrances.")
                                    .build(),
                            })
                        }
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, this settings draft is already completed."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, this settings draft is already completed."),
                        }),
                        _ => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => format!("Sorry, the current step is not a yes/no question."),
                            MessageContext::RaceTime { reply_to, .. } => format!("Sorry {reply_to}, the current step is not a yes/no question."),
                        }),
                    },
                }
            }
        })
    }

    pub(crate) async fn complete_randomly(mut self, kind: Kind) -> sqlx::Result<Picks> {
        Ok(loop {
            let action = match self.next_step(kind, &mut MessageContext::None).await?.kind {
                StepKind::GoFirst => Action::GoFirst(thread_rng().gen()),
                StepKind::Ban { available_settings, skippable, .. } => {
                    let mut settings = available_settings.all().map(Some).collect_vec();
                    if skippable {
                        settings.push(None);
                    }
                    if let Some(setting) = settings.into_iter().choose(&mut thread_rng()).expect("no available settings") {
                        Action::Ban { setting: setting.name.to_owned() }
                    } else {
                        Action::Skip
                    }
                }
                StepKind::Pick { available_choices, skippable, .. } => {
                    let mut settings = available_choices.all().map(Some).collect_vec();
                    if skippable {
                        settings.push(None);
                    }
                    if let Some(setting) = settings.into_iter().choose(&mut thread_rng()).expect("no available settings") {
                        Action::Pick { setting: setting.name.to_owned(), value: setting.options.choose(&mut thread_rng()).expect("no available values").name.to_owned() }
                    } else {
                        Action::Skip
                    }
                }
                StepKind::BooleanChoice { .. } => Action::BooleanChoice(thread_rng().gen()),
                StepKind::Done(_) => break self.settings,
            };
            self.apply(kind, &mut MessageContext::None, action).await?.expect("random draft made illegal action");
        })
    }
}
