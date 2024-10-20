use {
    std::cmp::{
        max_by_key,
        min_by_key,
    },
    serde_json::Value as Json,
    crate::{
        event::teams::{
            self,
            SignupsTeam,
        },
        prelude::*,
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
    // when defining a new variant, make sure to add it to event::Data::draft_kind
    S7,
    MultiworldS3,
    MultiworldS4,
    TournoiFrancoS3,
    TournoiFrancoS4,
}

impl Kind {
    fn language(&self) -> Language {
        match self {
            | Self::S7
            | Self::MultiworldS3
            | Self::MultiworldS4
            | Self::TournoiFrancoS4
                => English,
            | Self::TournoiFrancoS3
                => French,
        }
    }
}

#[derive(Clone)]
pub(crate) struct BanSetting {
    pub(crate) name: &'static str,
    pub(crate) display: &'static str,
    pub(crate) default: &'static str,
    pub(crate) default_display: &'static str,
    pub(crate) description: Cow<'static, str>,
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
    pub(crate) description: Cow<'static, str>,
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
    pub(crate) high_seed: Id<Teams>,
    pub(crate) went_first: Option<bool>,
    #[serde(default)]
    pub(crate) skipped_bans: u8,
    #[serde(flatten)]
    pub(crate) settings: Picks,
}

impl Draft {
    pub(crate) async fn for_game1(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, kind: Kind, event: &event::Data<'_>, phase: Option<&str>, [team1, team2]: [&team::Team; 2]) -> Result<Self, cal::Error> {
        let [high_seed, low_seed] = match kind {
            Kind::S7 => [
                min_by_key(team1, team2, |team| team.qualifier_rank).id,
                max_by_key(team1, team2, |team| team.qualifier_rank).id,
            ],
            Kind::MultiworldS3 | Kind::MultiworldS4 => if phase.is_some_and(|phase| phase == "Top 8") {
                let seeding = [
                    Id::from(8429274534302278572_u64), // Anju's Secret
                    Id::from(12142947927479333421_u64), // ADD
                    Id::from(5548902498821246494_u64), // Donutdog!!! Wuff! Wuff!
                    Id::from(7622448514297787774_u64), // Snack Pack
                    Id::from(13644615382444869291_u64), // The Highest Gorons
                    Id::from(4984265622447250649_u64), // Bongo Akimbo
                    Id::from(592664405695569367_u64), // The Jhegsons
                    Id::from(14405144517033747435_u64), // Pandora's Brot
                ];
                let mut team_ids = [team1.id, team2.id];
                team_ids.sort_unstable_by_key(|team| seeding.iter().position(|iter_team| iter_team == team));
                team_ids
            } else {
                let qualifier_kind = teams::QualifierKind::Single { //TODO adjust to match teams::get?
                    show_times: event.show_qualifier_times && event.is_started(&mut *transaction).await?,
                };
                let signups = teams::signups_sorted(&mut *transaction, http_client, None, event, qualifier_kind, false).await?;
                let SignupsTeam { members: members1, .. } = signups.iter().find(|SignupsTeam { team, .. }| team.as_ref().is_some_and(|team| team == team1)).expect("match with team that didn't sign up");
                let SignupsTeam { members: members2, .. } = signups.iter().find(|SignupsTeam { team, .. }| team.as_ref().is_some_and(|team| team == team2)).expect("match with team that didn't sign up");
                let avg1 = members1.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)).map(|total| total / u32::try_from(members1.len()).expect("too many team members"));
                let avg2 = members2.iter().try_fold(Duration::default(), |acc, member| Some(acc + member.qualifier_time?)).map(|total| total / u32::try_from(members2.len()).expect("too many team members"));
                match [avg1, avg2] {
                    [Some(_), None] => [team1.id, team2.id],
                    [None, Some(_)] => [team2.id, team1.id],
                    [Some(avg1), Some(avg2)] if avg1 < avg2 => [team1.id, team2.id],
                    [Some(avg1), Some(avg2)] if avg1 > avg2 => [team2.id, team1.id],
                    _ => {
                        // tie broken by coin flip
                        let mut team_ids = [team1.id, team2.id];
                        team_ids.shuffle(&mut thread_rng());
                        team_ids
                    }
                }
            },
            Kind::TournoiFrancoS3 | Kind::TournoiFrancoS4 => {
                let mut team_ids = [team1.id, team2.id];
                team_ids.shuffle(&mut thread_rng());
                team_ids
            }
        };
        Ok(Self::for_next_game(transaction, kind, high_seed, low_seed).await?)
    }

    pub(crate) async fn for_next_game(transaction: &mut Transaction<'_, Postgres>, kind: Kind, loser: Id<Teams>, winner: Id<Teams>) -> sqlx::Result<Self> {
        Ok(Self {
            high_seed: loser,
            went_first: None,
            skipped_bans: 0,
            settings: match kind {
                Kind::S7 => HashMap::default(),
                // accessibility accommodation for The Aussie Boiiz in mw/4 to default to CSMC
                Kind::MultiworldS3 | Kind::MultiworldS4 => HashMap::from_iter(
                    (loser == Id::from(17814073240662869290_u64) || winner == Id::from(17814073240662869290_u64))
                        .then_some((Cow::Borrowed("special_csmc"), Cow::Borrowed("yes"))),
                ),
                Kind::TournoiFrancoS3 | Kind::TournoiFrancoS4 => {
                    let team_rows = sqlx::query!("SELECT hard_settings_ok, mq_ok FROM teams WHERE id = $1 OR id = $2", loser as _, winner as _).fetch_all(&mut **transaction).await?;
                    let hard_settings_ok = team_rows.iter().all(|row| row.hard_settings_ok);
                    let mq_ok = team_rows.iter().all(|row| row.mq_ok);
                    collect![as HashMap<_, _>:
                        Cow::Borrowed("hard_settings_ok") => Cow::Borrowed(if hard_settings_ok { "ok" } else { "no" }),
                        Cow::Borrowed("mq_ok") => Cow::Borrowed(if mq_ok { "ok" } else { "no" }),
                    ]
                }
            },
        })
    }

    fn pick_count(&self, kind: Kind) -> u8 {
        match kind {
            Kind::S7 => self.skipped_bans + u8::try_from(self.settings.len()).unwrap(),
            Kind::MultiworldS3 => self.skipped_bans + u8::try_from(mw::S3_SETTINGS.into_iter().filter(|&mw::Setting { name, .. }| self.settings.contains_key(name)).count()).unwrap(),
            Kind::MultiworldS4 => self.skipped_bans + u8::try_from(mw::S4_SETTINGS.into_iter().filter(|&mw::Setting { name, .. }| self.settings.contains_key(name)).count()).unwrap(),
            Kind::TournoiFrancoS3 => self.skipped_bans + u8::try_from(fr::S3_SETTINGS.into_iter().filter(|&fr::Setting { name, .. }| self.settings.contains_key(name)).count()).unwrap(),
            Kind::TournoiFrancoS4 => self.skipped_bans + u8::try_from(fr::S4_SETTINGS.into_iter().filter(|&fr::Setting { name, .. }| self.settings.contains_key(name)).count()).unwrap(),
        }
    }

    pub(crate) async fn next_step(&self, kind: Kind, game: Option<i16>, msg_ctx: &mut MessageContext<'_>) -> sqlx::Result<Step> {
        Ok(match kind {
            Kind::S7 => {
                if let Some(went_first) = self.went_first {
                    match self.pick_count(kind) {
                        prev_bans @ 0..=1 => {
                            let team = match (prev_bans, went_first) {
                                (0, true) | (1, false) => Team::HighSeed,
                                (0, false) | (1, true) => Team::LowSeed,
                                (2.., _) => unreachable!(),
                            };
                            let (major_setings, minor_settings) = s::S7_SETTINGS.into_iter().partition::<Vec<_>, _>(|&s::Setting { major, .. }| major);
                            Step {
                                kind: StepKind::Ban {
                                    available_settings: BanSettings(vec![
                                        ("Major Settings", major_setings.into_iter()
                                            .filter(|&s::Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|setting @ s::Setting { name, display, default_display, .. }| BanSetting {
                                                default: "default",
                                                description: Cow::Owned(setting.description()),
                                                name, display, default_display,
                                            })
                                            .collect()),
                                        ("Minor Settings", minor_settings.into_iter()
                                            .filter(|&s::Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|setting @ s::Setting { name, display, default_display, .. }| BanSetting {
                                                default: "default",
                                                description: Cow::Owned(setting.description()),
                                                name, display, default_display,
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
                                        (if n < 4 { "Major Settings" } else { "Minor Settings" }, s::S7_SETTINGS.into_iter()
                                            .filter(|&s::Setting { name, major, .. }| major == (n < 4) && !self.settings.contains_key(name))
                                            .map(|setting @ s::Setting { name, display, other, .. }| DraftSetting {
                                                options: other.iter().map(|&(name, display, _)| DraftSettingChoice { name, display }).collect(),
                                                description: Cow::Owned(setting.description()),
                                                name, display,
                                            })
                                            .collect()),
                                    ]),
                                    skippable: false,
                                    team,
                                },
                                message: match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => {
                                        let (mut high_seed, mut low_seed) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                        let high_seed = high_seed.remove(0);
                                        let low_seed = low_seed.remove(0);
                                        match n {
                                            2 | 3 => MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(": pick a major setting using ")
                                                .mention_command(command_ids.draft.unwrap(), "draft")
                                                .push('.')
                                                .build(),
                                            4 | 5 => MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(": pick a minor setting using ")
                                                .mention_command(command_ids.draft.unwrap(), "draft")
                                                .push('.')
                                                .build(),
                                            0..=1 | 6.. => unreachable!(),
                                        }
                                    }
                                    MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => match n {
                                        2 => format!("{}, pick a major setting using “!draft <setting> <value>”", team.choose(high_seed_name, low_seed_name)),
                                        3 => format!("{}, pick a major setting.", team.choose(high_seed_name, low_seed_name)),
                                        4 | 5 => format!("{}, pick a minor setting.", team.choose(high_seed_name, low_seed_name)),
                                        0..=1 | 6.. => unreachable!(),
                                    },
                                },
                            }
                        }
                        6.. => Step {
                            kind: StepKind::Done(s::resolve_s7_draft_settings(&self.settings)),
                            message: match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { .. } => format!("Settings draft completed. You will be playing with {}.", s::display_s7_draft_picks(&self.settings)),
                                MessageContext::RaceTime { .. } => s::display_s7_draft_picks(&self.settings),
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
                                let mut builder = MessageBuilder::default();
                                builder.mention_team(transaction, Some(*guild_id), high_seed).await?;
                                if game.is_some_and(|game| game > 1) {
                                    builder.push(": as the loser of the previous race, please choose whether you want to go ");
                                } else {
                                    builder.push(": you have the higher seed. Choose whether you want to go ");
                                }
                                builder.mention_command(command_ids.first.unwrap(), "first");
                                builder.push(" or ");
                                builder.mention_command(command_ids.second.unwrap(), "second");
                                if let Some(game) = game {
                                    builder.push(" in the settings draft for game ");
                                    builder.push(game.to_string());
                                    builder.push('.');
                                } else {
                                    builder.push(" in the settings draft.");
                                }
                                builder
                                    .push(" You can also wait until the race room is opened to draft your settings.")
                                    .build()
                            }
                            MessageContext::RaceTime { high_seed_name, .. } => format!("{high_seed_name}, you have the higher seed. Choose whether you want to go !first or !second"),
                        },
                    }
                }
            }
            Kind::MultiworldS3 => {
                if let Some(went_first) = self.went_first {
                    match self.pick_count(kind) {
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
                                            .filter(|&mw::Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|mw::Setting { name, display, default, default_display, description, .. }| BanSetting {
                                                description: Cow::Borrowed(description),
                                                name, display, default, default_display,
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
                                            .filter(|&mw::Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|mw::Setting { name, display, default, default_display, other, description }| DraftSetting {
                                                options: iter::once(DraftSettingChoice { name: default, display: default_display })
                                                    .chain(other.iter().map(|&(name, display)| DraftSettingChoice { name, display }))
                                                    .collect(),
                                                description: Cow::Borrowed(description),
                                                name, display,
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
                            kind: StepKind::Done(mw::resolve_s3_draft_settings(&self.settings)),
                            message: match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { .. } => format!("Settings draft completed. You will be playing with {}.", mw::display_s3_draft_picks(&self.settings)),
                                MessageContext::RaceTime { .. } => mw::display_s3_draft_picks(&self.settings),
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
                                let mut builder = MessageBuilder::default();
                                builder.mention_team(transaction, Some(*guild_id), high_seed).await?;
                                if game.is_some_and(|game| game > 1) {
                                    builder.push(": as the losers of the previous race, please choose whether you want to go ");
                                } else {
                                    builder.push(": you have the higher seed. Choose whether you want to go ");
                                }
                                builder.mention_command(command_ids.first.unwrap(), "first");
                                builder.push(" or ");
                                builder.mention_command(command_ids.second.unwrap(), "second");
                                if let Some(game) = game {
                                    builder.push(" in the settings draft for game ");
                                    builder.push(game.to_string());
                                    builder.push('.');
                                } else {
                                    builder.push(" in the settings draft.");
                                }
                                builder.build()
                            }
                            MessageContext::RaceTime { high_seed_name, .. } => format!("{high_seed_name}, you have the higher seed. Choose whether you want to go !first or !second"),
                        },
                    }
                }
            }
            Kind::MultiworldS4 => {
                if let Some(went_first) = self.went_first {
                    match self.pick_count(kind) {
                        prev_bans @ (0..=1 | 6..=7) => {
                            let team = match (prev_bans, went_first) {
                                (0, true) | (1, false) | (6, false) | (7, true) => Team::HighSeed,
                                (0, false) | (1, true) | (6, true) | (7, false) => Team::LowSeed,
                                (2..=5 | 8.., _) => unreachable!(),
                            };
                            Step {
                                kind: StepKind::Ban {
                                    available_settings: BanSettings(vec![
                                        ("All Settings", mw::S4_SETTINGS.into_iter()
                                            .filter(|&mw::Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|mw::Setting { name, display, default, default_display, description, .. }|
                                                if name == "camc" && self.settings.get("special_csmc").map(|special_csmc| &**special_csmc).unwrap_or("no") == "yes" {
                                                    BanSetting {
                                                        default: "both",
                                                        default_display: "chest size & texture match contents",
                                                        description: Cow::Borrowed("camc (Chest Appearance Matches Contents): both (default: size & texture) or off"),
                                                        name, display,
                                                    }
                                                } else {
                                                    BanSetting {
                                                        description: Cow::Borrowed(description),
                                                        name, display, default, default_display,
                                                    }
                                                }
                                            )
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
                        n @ (2..=5 | 8..=9) => {
                            let team = match (n, went_first) {
                                (2, true) | (3, false) | (4, false) | (5, true) | (8, false) | (9, true) => Team::HighSeed,
                                (2, false) | (3, true) | (4, true) | (5, false) | (8, true) | (9, false) => Team::LowSeed,
                                (0..=1 | 6..=7 | 10.., _) => unreachable!(),
                            };
                            Step {
                                kind: StepKind::Pick {
                                    available_choices: DraftSettings(vec![
                                        ("All Settings", mw::S4_SETTINGS.into_iter()
                                            .filter(|&mw::Setting { name, .. }| !self.settings.contains_key(name))
                                            .map(|mw::Setting { name, display, default, default_display, other, description }|
                                                if name == "camc" && self.settings.get("special_csmc").map(|special_csmc| &**special_csmc).unwrap_or("no") == "yes" {
                                                    DraftSetting {
                                                        options: vec![
                                                            DraftSettingChoice { name: "both", display: "chest size & texture match contents" },
                                                            DraftSettingChoice { name: "off", display: "vanilla chest appearances" },
                                                        ],
                                                        description: Cow::Borrowed("camc (Chest Appearance Matches Contents): both (default: size & texture) or off"),
                                                        name, display,
                                                    }
                                                } else {
                                                    DraftSetting {
                                                        options: iter::once(DraftSettingChoice { name: default, display: default_display })
                                                            .chain(other.iter().map(|&(name, display)| DraftSettingChoice { name, display }))
                                                            .collect(),
                                                        description: Cow::Borrowed(description),
                                                        name, display,
                                                    }
                                                }
                                            )
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
                                        match n {
                                            2 | 5 | 8 => MessageBuilder::default()
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
                                            9 => MessageBuilder::default()
                                                .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                .push(": pick a setting using ")
                                                .mention_command(command_ids.draft.unwrap(), "draft")
                                                .push(". You can also use ")
                                                .mention_command(command_ids.skip.unwrap(), "skip")
                                                .push(" if you want to leave the settings as they are.")
                                                .build(),
                                            0..=1 | 6..=7 | 10.. => unreachable!(),
                                        }
                                    }
                                    MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => match n {
                                        2 => format!("{}, pick a setting using “!draft <setting> <value>”", team.choose(high_seed_name, low_seed_name)),
                                        3 => format!("{}, pick two settings.", team.choose(high_seed_name, low_seed_name)),
                                        4 => format!("And your second pick?"),
                                        5 | 8 => format!("{}, pick a setting.", team.choose(high_seed_name, low_seed_name)),
                                        9 => format!("{}, pick the final setting. You can also use “!skip” if you want to leave the settings as they are.", team.choose(high_seed_name, low_seed_name)),
                                        0..=1 | 6..=7 | 10.. => unreachable!(),
                                    },
                                },
                            }
                        }
                        10.. => Step {
                            kind: StepKind::Done(mw::resolve_s4_draft_settings(&self.settings)),
                            message: match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { .. } => format!("Settings draft completed. You will be playing with {}.", mw::display_s4_draft_picks(&self.settings)),
                                MessageContext::RaceTime { .. } => mw::display_s4_draft_picks(&self.settings),
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
                                let mut builder = MessageBuilder::default();
                                builder.mention_team(transaction, Some(*guild_id), high_seed).await?;
                                if game.is_some_and(|game| game > 1) {
                                    builder.push(": as the losers of the previous race, please choose whether you want to go ");
                                } else {
                                    builder.push(": you have the higher seed. Choose whether you want to go ");
                                }
                                builder.mention_command(command_ids.first.unwrap(), "first");
                                builder.push(" or ");
                                builder.mention_command(command_ids.second.unwrap(), "second");
                                if let Some(game) = game {
                                    builder.push(" in the settings draft for game ");
                                    builder.push(game.to_string());
                                    builder.push('.');
                                } else {
                                    builder.push(" in the settings draft.");
                                }
                                if self.settings.get("special_csmc").map(|special_csmc| &**special_csmc).unwrap_or("no") == "yes" {
                                    builder.push_line("");
                                    builder.push("Please note that for accessibility reasons, the Chest Appearance Matches Contents setting will default to Both Size and Texture for this match. It can be locked to Both Size and Texture using a ban or pick, or changed to Off using a pick. Texture Only is not available in this match.");
                                }
                                builder.build()
                            }
                            MessageContext::RaceTime { high_seed_name, .. } => format!("{high_seed_name}, you have the higher seed. Choose whether you want to go !first or !second"),
                        },
                    }
                }
            }
            Kind::TournoiFrancoS3 | Kind::TournoiFrancoS4 => {
                let all_settings = match kind {
                    Kind::TournoiFrancoS3 => &fr::S3_SETTINGS[..],
                    Kind::TournoiFrancoS4 => &fr::S4_SETTINGS[..],
                    Kind::MultiworldS3 | Kind::MultiworldS4 | Kind::S7 => unreachable!(),
                };
                if let Some(went_first) = self.went_first {
                    let mut pick_count = self.pick_count(kind);
                    let select_mixed_dungeons = !self.settings.contains_key("mixed-dungeons") && self.settings.get("dungeon-er").map(|dungeon_er| &**dungeon_er).unwrap_or("off") == "on" && self.settings.get("mixed-er").map(|mixed_er| &**mixed_er).unwrap_or("off") == "on";
                    if select_mixed_dungeons {
                        // chosen by the same team that chose the previous setting
                        pick_count -= 1;
                    }
                    let team = match (kind, pick_count, went_first) {
                        (_, 0, true) | (_, 1, false) | (_, 2, true) | (_, 3, false) | (_, 4, false) | (_, 5, true) | (_, 6, true) | (_, 7, false) | (Kind::TournoiFrancoS3, 8, true) | (Kind::TournoiFrancoS3, 9, false) => Team::HighSeed,
                        (_, 0, false) | (_, 1, true) | (_, 2, false) | (_, 3, true) | (_, 4, true) | (_, 5, false) | (_, 6, false) | (_, 7, true) | (Kind::TournoiFrancoS3, 8, false) | (Kind::TournoiFrancoS3, 9, true) => Team::LowSeed,
                        (Kind::TournoiFrancoS3, 10.., _) | (Kind::TournoiFrancoS4, 8.., _) => return Ok(Step {
                            kind: StepKind::Done(match kind {
                                Kind::TournoiFrancoS3 => fr::resolve_s3_draft_settings(&self.settings),
                                Kind::TournoiFrancoS4 => fr::resolve_s4_draft_settings(&self.settings),
                                Kind::MultiworldS3 | Kind::MultiworldS4 | Kind::S7 => unreachable!(),
                            }),
                            message: match msg_ctx {
                                MessageContext::None => String::default(),
                                MessageContext::Discord { .. } => if let French = kind.language() {
                                    format!("Fin du draft ! Voici un récapitulatif : {}.", fr::display_draft_picks(kind.language(), all_settings, &self.settings))
                                } else {
                                    format!("Settings draft completed. You will be playing with {}.", fr::display_draft_picks(kind.language(), all_settings, &self.settings))
                                },
                                MessageContext::RaceTime { .. } => fr::display_draft_picks(kind.language(), all_settings, &self.settings),
                            },
                        }),
                        (Kind::MultiworldS3 | Kind::MultiworldS4 | Kind::S7, _, _) => unreachable!(),
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
                                        .push(" : Est-ce que les donjons seront mixés avec les intérieurs et les grottos ? Répondez en utilisant ")
                                        .mention_command(command_ids.yes.unwrap(), "yes")
                                        .push(" ou ")
                                        .mention_command(command_ids.no.unwrap(), "no")
                                        .push('.')
                                        .build()
                                }
                                MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => if let French = kind.language() {
                                    format!(
                                        "{}, est-ce que les donjons seront mixés avec les intérieurs et les grottos ? Répondez en utilisant !yes ou !no",
                                        team.choose(high_seed_name, low_seed_name),
                                    )
                                } else {
                                    format!(
                                        "{}, should dungeon entrances be mixed with interiors and grottos? Use !yes or !no",
                                        team.choose(high_seed_name, low_seed_name),
                                    )
                                },
                            },
                        }
                    } else {
                        match self.pick_count(kind) {
                            prev_bans @ 0..=1 => {
                                let hard_settings_ok = self.settings.get("hard_settings_ok").map(|hard_settings_ok| &**hard_settings_ok).unwrap_or("no") == "ok";
                                let (hard_settings, classic_settings) = all_settings.iter()
                                    .filter(|&&fr::Setting { name, .. }| !self.settings.contains_key(name) && match name {
                                        "keysy" => self.settings.get("keysanity").map_or(true, |keysanity| keysanity == "off"),
                                        "keysanity" => self.settings.get("keysy").map_or(true, |keysy| keysy == "off"),
                                        _ => true,
                                    })
                                    .filter_map(|fr::Setting { name, display, default, default_display, other, description }| {
                                        let (is_hard, is_empty) = if hard_settings_ok {
                                            (other.iter().all(|&(_, hard, _)| hard), other.is_empty())
                                        } else {
                                            (false, other.iter().all(|&(_, hard, _)| hard))
                                        };
                                        (!is_empty).then(|| (is_hard, BanSetting {
                                            display: if *display == "enemy souls" && !hard_settings_ok { "boss souls" } else { display },
                                            description: Cow::Borrowed(description),
                                            name, default, default_display,
                                        }))
                                    })
                                    .partition::<Vec<_>, _>(|&(is_hard, _)| is_hard);
                                let mut available_settings = vec![
                                    (if let French = kind.language() { "Settings classiques" } else { "Classic Settings" }, classic_settings.into_iter().map(|(_, setting)| setting).collect()),
                                ];
                                if hard_settings_ok && !hard_settings.is_empty() {
                                    available_settings.push((if let French = kind.language() { "Settings difficiles" } else { "Hard Settings" }, hard_settings.into_iter().map(|(_, setting)| setting).collect()));
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
                                                .push(if let French = kind.language() {
                                                    " : Veuillez ban un setting en utilisant "
                                                } else {
                                                    ": lock a setting to its default using "
                                                })
                                                .mention_command(command_ids.ban.unwrap(), "ban")
                                                .push('.')
                                                .build()
                                        }
                                        MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => if let French = kind.language() {
                                            format!(
                                                "{}, veuillez ban un setting en utilisant “!ban <setting>”.{}",
                                                team.choose(high_seed_name, low_seed_name),
                                                if prev_bans == 0 { " Utilisez “!settings” pour la liste des settings." } else { "" },
                                            )
                                        } else {
                                            format!(
                                                "{}, lock a setting to its default using “!ban <setting>”.{}",
                                                team.choose(high_seed_name, low_seed_name),
                                                if prev_bans == 0 { " Use “!settings” for a list of available settings." } else { "" },
                                            )
                                        },
                                    },
                                }
                            }
                            n @ 2..=9 => {
                                let round_count = match kind {
                                    Kind::TournoiFrancoS3 => 10,
                                    Kind::TournoiFrancoS4 => 8,
                                    Kind::MultiworldS3 | Kind::MultiworldS4 | Kind::S7 => unreachable!(),
                                };
                                let hard_settings_ok = self.settings.get("hard_settings_ok").map(|hard_settings_ok| &**hard_settings_ok).unwrap_or("no") == "ok";
                                let can_ban = n < round_count - 2 || self.settings.get(team.choose("high_seed_has_picked", "low_seed_has_picked")).map(|has_picked| &**has_picked).unwrap_or("no") == "yes";
                                let skippable = n == round_count - 1 && can_ban;
                                let (hard_settings, classic_settings) = all_settings.iter()
                                    .filter(|&&fr::Setting { name, .. }| !self.settings.contains_key(name))
                                    .filter_map(|&fr::Setting { name, display, default, default_display, other, description }| {
                                        let (is_hard, other) = if hard_settings_ok {
                                            (other.iter().all(|&(_, hard, _)| hard), other.to_owned())
                                        } else {
                                            (false, other.iter().filter(|(_, hard, _)| !hard).copied().collect_vec())
                                        };
                                        (!other.is_empty()).then(|| (is_hard, DraftSetting {
                                            display: if display == "enemy souls" && !hard_settings_ok { "boss souls" } else { display },
                                            options: can_ban.then(|| DraftSettingChoice { name: default, display: default_display }).into_iter()
                                                .chain(other.into_iter().map(|(name, _, display)| DraftSettingChoice { name, display }))
                                                .collect(),
                                            description: Cow::Borrowed(description),
                                            name,
                                        }))
                                    })
                                    .partition::<Vec<_>, _>(|&(is_hard, _)| is_hard);
                                let mut available_choices = vec![
                                    (if let French = kind.language() { "Settings classiques" } else { "Classic Settings" }, classic_settings.into_iter().map(|(_, setting)| setting).collect()),
                                ];
                                if hard_settings_ok && !hard_settings.is_empty() {
                                    available_choices.push((if let French = kind.language() { "Settings difficiles" } else { "Hard Settings" }, hard_settings.into_iter().map(|(_, setting)| setting).collect()));
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
                                            match (kind, n) {
                                                (_, 9) | (Kind::TournoiFrancoS4, 7) => if let French = kind.language() {
                                                    let mut builder = MessageBuilder::default();
                                                    builder.mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?;
                                                    builder.push(" : Choisissez un setting avec ");
                                                    builder.mention_command(command_ids.draft.unwrap(), "draft");
                                                    builder.push('.');
                                                    if skippable {
                                                        builder.push(" Vous pouvez également utiliser ");
                                                        builder.mention_command(command_ids.skip.unwrap(), "skip");
                                                        builder.push(" si vous voulez laisser les settings comme ils sont.");
                                                    }
                                                    builder.build()
                                                } else {
                                                    let mut builder = MessageBuilder::default();
                                                    builder.mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?;
                                                    builder.push(": pick a setting using ");
                                                    builder.mention_command(command_ids.draft.unwrap(), "draft");
                                                    builder.push('.');
                                                    if skippable {
                                                        builder.push(" You can also use ");
                                                        builder.mention_command(command_ids.skip.unwrap(), "skip");
                                                        builder.push(" if you want to leave the settings as they are.");
                                                    }
                                                    builder.build()
                                                },
                                                (_, 2 | 7 | 8) => if let French = kind.language() {
                                                    MessageBuilder::default()
                                                        .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                        .push(" : Choisissez un setting en utilisant ")
                                                        .mention_command(command_ids.draft.unwrap(), "draft")
                                                        .push('.')
                                                        .build()
                                                } else {
                                                    MessageBuilder::default()
                                                        .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                        .push(": pick a setting using ")
                                                        .mention_command(command_ids.draft.unwrap(), "draft")
                                                        .push('.')
                                                        .build()
                                                },
                                                (_, 3 | 5) => if let French = kind.language() {
                                                    MessageBuilder::default()
                                                        .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                        .push(" : Choisissez un setting avec ")
                                                        .mention_command(command_ids.draft.unwrap(), "draft")
                                                        .push(". Vous aurez un autre pick après celui-ci.")
                                                        .build()
                                                } else {
                                                    MessageBuilder::default()
                                                        .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                        .push(": pick a setting using ")
                                                        .mention_command(command_ids.draft.unwrap(), "draft")
                                                        .push(". You will have another pick after this.")
                                                        .build()
                                                },
                                                (_, 4 | 6) => if let French = kind.language() {
                                                    MessageBuilder::default()
                                                        .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                        .push(" : Choisissez votre second setting avec ")
                                                        .mention_command(command_ids.draft.unwrap(), "draft")
                                                        .push('.')
                                                        .build()
                                                } else {
                                                    MessageBuilder::default()
                                                        .mention_team(transaction, Some(*guild_id), team.choose(high_seed, low_seed)).await?
                                                        .push(": pick your second setting using ")
                                                        .mention_command(command_ids.draft.unwrap(), "draft")
                                                        .push('.')
                                                        .build()
                                                },
                                                (_, 0..=1 | 10..) => unreachable!(),
                                            }
                                        }
                                        MessageContext::RaceTime { high_seed_name, low_seed_name, .. } => match (kind, n) {
                                            (Kind::TournoiFrancoS4, 7) | (_, 9) if skippable => if let French = kind.language() {
                                                format!("{}, choisissez le dernier setting. Vous pouvez également utiliser “!skip” si vous voulez laisser les settings comme ils sont.", team.choose(high_seed_name, low_seed_name))
                                            } else {
                                                format!("{},  pick the final setting. You can also use “!skip” if you want to leave the settings as they are.", team.choose(high_seed_name, low_seed_name))
                                            },
                                            (Kind::TournoiFrancoS4, 7) | (_, 9) => if let French = kind.language() {
                                                format!("{}, choisissez votre dernier setting.", team.choose(high_seed_name, low_seed_name))
                                            } else {
                                                format!("{}, pick the final setting.", team.choose(high_seed_name, low_seed_name))
                                            },
                                            (_, 2) => if let French = kind.language() {
                                                format!("{}, choisissez un setting avec “!draft <setting> <configuration>”. <configuration> signifie la valeur du setting. Par exemple pour tokensanity, la configuration peut être {{all, dungeon, overworld}}.", team.choose(high_seed_name, low_seed_name))
                                            } else {
                                                format!("{}, pick a setting using “!draft <setting> <value>”", team.choose(high_seed_name, low_seed_name))
                                            },
                                            (_, 3 | 5) => if let French = kind.language() {
                                                format!("{}, choisissez deux settings. Quel est votre premier ?", team.choose(high_seed_name, low_seed_name))
                                            } else {
                                                format!("{}, pick two settings.", team.choose(high_seed_name, low_seed_name))
                                            },
                                            (_, 4 | 6) => if let French = kind.language() {
                                                format!("Et votre second ?")
                                            } else {
                                                format!("And your second pick?")
                                            },
                                            (_, 7 | 8) => if let French = kind.language() {
                                                format!("{}, choisissez un setting.", team.choose(high_seed_name, low_seed_name))
                                            } else {
                                                format!("{}, pick a setting.", team.choose(high_seed_name, low_seed_name))
                                            },
                                            (_, 0..=1 | 10..) => unreachable!(),
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
                            MessageContext::Discord { transaction, guild_id, command_ids, teams, .. } => if let French = kind.language() {
                                let (mut high_seed, _) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                let high_seed = high_seed.remove(0);
                                let mut builder = MessageBuilder::default();
                                builder.mention_team(transaction, Some(*guild_id), high_seed).await?;
                                builder.push(" : Vous avez été sélectionné pour décider qui commencera le draft en premier. Si vous voulez commencer, veuillez entrer ");
                                builder.mention_command(command_ids.first.unwrap(), "first");
                                builder.push(". Autrement, entrez ");
                                builder.mention_command(command_ids.second.unwrap(), "second");
                                builder.push(".");
                                if self.settings.get("mq_ok").map(|mq_ok| &**mq_ok).unwrap_or("no") == "ok" {
                                    builder.push(" Veuillez choisir combien de donjons Master Quest seront présents. Vous devez vous concerter pour choisir ce nombre. Utilisez “/first” ou “/second” suivi de “mq:{nombre}”");
                                }
                                builder.build()
                            } else {
                                let (mut high_seed, _) = teams.iter().partition::<Vec<_>, _>(|team| team.id == self.high_seed);
                                let high_seed = high_seed.remove(0);
                                let mut builder = MessageBuilder::default();
                                builder.mention_team(transaction, Some(*guild_id), high_seed).await?;
                                builder.push(": you have won the coin flip. Choose whether you want to go ");
                                builder.mention_command(command_ids.first.unwrap(), "first");
                                builder.push(" or ");
                                builder.mention_command(command_ids.second.unwrap(), "second");
                                builder.push(" in the settings draft.");
                                if self.settings.get("mq_ok").map(|mq_ok| &**mq_ok).unwrap_or("no") == "ok" {
                                    builder.push(" Please include the number of MQ dungeons.");
                                }
                                builder.build()
                            },
                            MessageContext::RaceTime { high_seed_name, .. } => if let French = kind.language() {
                                format!("{high_seed_name}, vous avez été sélectionné pour décider qui commencera le draft en premier. Si vous voulez commencer, veuillez entrer “!first”. Autrement, entrez “!second”.")
                            } else {
                                format!("{high_seed_name}, you have won the coin flip. Choose whether you want to go !first or !second in the settings draft.")
                            },
                        },
                    }
                }
            }
        })
    }

    pub(crate) async fn active_team(&self, kind: Kind, game: Option<i16>) -> sqlx::Result<Option<Team>> {
        Ok(match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
            StepKind::GoFirst => Some(Team::HighSeed),
            StepKind::Ban { team, .. } | StepKind::Pick { team, .. } | StepKind::BooleanChoice { team } => Some(team),
            StepKind::Done(_) => None,
        })
    }

    /// Assumes that the caller has checked that the team is part of the race in the first place.
    pub(crate) async fn is_active_team(&self, kind: Kind, game: Option<i16>, team: Id<Teams>) -> sqlx::Result<bool> {
        Ok(match self.active_team(kind, game).await? {
            Some(Team::HighSeed) => team == self.high_seed,
            Some(Team::LowSeed) => team != self.high_seed,
            None => false,
        })
    }

    pub(crate) async fn apply(&mut self, kind: Kind, game: Option<i16>, msg_ctx: &mut MessageContext<'_>, action: Action) -> sqlx::Result<Result<String, String>> {
        Ok(match kind {
            Kind::S7 => {
                let resolved_action = match action {
                    Action::Ban { setting } => if let Some(setting) = s::S7_SETTINGS.into_iter().find(|&s::Setting { name, .. }| *name == setting) {
                        Action::Pick { setting: setting.name.to_owned(), value: format!("default") }
                    } else {
                        return Ok(Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => {
                                let mut content = MessageBuilder::default();
                                content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                for (i, setting) in s::S7_SETTINGS.into_iter().enumerate() {
                                    if i > 0 {
                                        content.push(" or ");
                                    }
                                    content.push_mono(setting.name);
                                }
                                content.build()
                            }
                            MessageContext::RaceTime { reply_to, .. } => format!(
                                "Sorry {reply_to}, I don't recognize that setting. Use one of the following: {}",
                                s::S7_SETTINGS.into_iter().map(|setting| setting.name).format(" or "),
                            ),
                        }))
                    },
                    Action::BooleanChoice(value) if matches!(self.next_step(kind, game, &mut MessageContext::None).await?.kind, StepKind::GoFirst) => Action::GoFirst(value),
                    _ => action,
                };
                match resolved_action {
                    Action::GoFirst(first) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                    Action::Pick { setting, value } => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                            let exists = s::S7_SETTINGS.into_iter().any(|s::Setting { name, .. }| setting == name);
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
                            if let Some(option) = setting.options.iter().find(|option| option.name == value) {
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
                                        for (i, value) in setting.options.into_iter().enumerate() {
                                            if i > 0 {
                                                content.push(" or ");
                                            }
                                            content.push_mono(value.name);
                                        }
                                        content.build()
                                    }
                                    MessageContext::RaceTime { reply_to, .. } => format!(
                                        "Sorry {reply_to}, that's not a possible value for this setting. Use one of the following: {}",
                                        setting.options.into_iter().map(|value| value.name).format(" or "),
                                    ),
                                })
                            }
                        } else {
                            let exists = s::S7_SETTINGS.into_iter().any(|s::Setting { name, .. }| setting == name);
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
                    Action::Skip => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                            let skip_kind = match self.pick_count(kind) {
                                0 | 1 => "ban",
                                _ => "final pick",
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
                    Action::BooleanChoice(_) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
            Kind::MultiworldS3 => {
                let resolved_action = match action {
                    Action::Ban { setting } => if let Some(setting) = mw::S3_SETTINGS.into_iter().find(|&mw::Setting { name, .. }| *name == setting) {
                        Action::Pick { setting: setting.name.to_owned(), value: setting.default.to_owned() }
                    } else {
                        return Ok(Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => {
                                let mut content = MessageBuilder::default();
                                content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                for (i, setting) in mw::S3_SETTINGS.into_iter().enumerate() {
                                    if i > 0 {
                                        content.push(" or ");
                                    }
                                    content.push_mono(setting.name);
                                }
                                content.build()
                            }
                            MessageContext::RaceTime { reply_to, .. } => format!(
                                "Sorry {reply_to}, I don't recognize that setting. Use one of the following: {}",
                                mw::S3_SETTINGS.into_iter().map(|setting| setting.name).format(" or "),
                            ),
                        }))
                    },
                    Action::BooleanChoice(value) if matches!(self.next_step(kind, game, &mut MessageContext::None).await?.kind, StepKind::GoFirst) => Action::GoFirst(value),
                    _ => action,
                };
                match resolved_action {
                    Action::GoFirst(first) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                    Action::Pick { setting, value } => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                            let exists = mw::S3_SETTINGS.into_iter().any(|mw::Setting { name, .. }| setting == name);
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
                            if let Some(option) = setting.options.iter().find(|option| option.name == value) {
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
                                        for (i, value) in setting.options.into_iter().enumerate() {
                                            if i > 0 {
                                                content.push(" or ");
                                            }
                                            content.push_mono(value.name);
                                        }
                                        content.build()
                                    }
                                    MessageContext::RaceTime { reply_to, .. } => format!(
                                        "Sorry {reply_to}, that's not a possible value for this setting. Use one of the following: {}",
                                        setting.options.into_iter().map(|value| value.name).format(" or "),
                                    ),
                                })
                            }
                        } else {
                            let exists = mw::S3_SETTINGS.into_iter().any(|mw::Setting { name, .. }| setting == name);
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
                    Action::Skip => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                            let skip_kind = match self.pick_count(kind) {
                                0 | 1 => "ban",
                                _ => "final pick",
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
                    Action::BooleanChoice(_) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
            Kind::MultiworldS4 => {
                let resolved_action = match action {
                    Action::Ban { setting } => if let Some(setting) = mw::S4_SETTINGS.into_iter().find(|&mw::Setting { name, .. }| *name == setting) {
                        Action::Pick {
                            setting: setting.name.to_owned(),
                            value: if setting.name == "camc" && self.settings.get("special_csmc").map(|special_csmc| &**special_csmc).unwrap_or("no") == "yes" {
                                format!("both")
                            } else {
                                setting.default.to_owned()
                            },
                        }
                    } else {
                        return Ok(Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => {
                                let mut content = MessageBuilder::default();
                                content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                for (i, setting) in mw::S4_SETTINGS.into_iter().enumerate() {
                                    if i > 0 {
                                        content.push(" or ");
                                    }
                                    content.push_mono(setting.name);
                                }
                                content.build()
                            }
                            MessageContext::RaceTime { reply_to, .. } => format!(
                                "Sorry {reply_to}, I don't recognize that setting. Use one of the following: {}",
                                mw::S4_SETTINGS.into_iter().map(|setting| setting.name).format(" or "),
                            ),
                        }))
                    },
                    Action::BooleanChoice(value) if matches!(self.next_step(kind, game, &mut MessageContext::None).await?.kind, StepKind::GoFirst) => Action::GoFirst(value),
                    _ => action,
                };
                match resolved_action {
                    Action::GoFirst(first) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                    Action::Pick { setting, value } => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                            let exists = mw::S4_SETTINGS.into_iter().any(|mw::Setting { name, .. }| setting == name);
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
                            if let Some(option) = setting.options.iter().find(|option| option.name == value) {
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
                                        for (i, value) in setting.options.into_iter().enumerate() {
                                            if i > 0 {
                                                content.push(" or ");
                                            }
                                            content.push_mono(value.name);
                                        }
                                        content.build()
                                    }
                                    MessageContext::RaceTime { reply_to, .. } => format!(
                                        "Sorry {reply_to}, that's not a possible value for this setting. Use one of the following: {}",
                                        setting.options.into_iter().map(|value| value.name).format(" or "),
                                    ),
                                })
                            }
                        } else {
                            let exists = mw::S4_SETTINGS.into_iter().any(|mw::Setting { name, .. }| setting == name);
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
                    Action::Skip => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                            let skip_kind = match self.pick_count(kind) {
                                0 | 1 | 6 | 7 => "ban",
                                9 => "final pick",
                                _ => "pick",
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
                    Action::BooleanChoice(_) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
            Kind::TournoiFrancoS3 | Kind::TournoiFrancoS4 => {
                let all_settings = match kind {
                    Kind::TournoiFrancoS3 => &fr::S3_SETTINGS[..],
                    Kind::TournoiFrancoS4 => &fr::S4_SETTINGS[..],
                    Kind::MultiworldS3 | Kind::MultiworldS4 | Kind::S7 => unreachable!(),
                };
                let resolved_action = match action {
                    Action::Ban { setting } => if let Some(setting) = all_settings.iter().find(|&&fr::Setting { name, .. }| *name == setting) {
                        Action::Pick { setting: setting.name.to_owned(), value: setting.default.to_owned() }
                    } else {
                        return Ok(Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => {
                                let mut content = MessageBuilder::default();
                                content.push("Sorry, I don't recognize that setting. Use one of the following: ");
                                for (i, setting) in all_settings.iter().enumerate() {
                                    if i > 0 {
                                        content.push(" or ");
                                    }
                                    content.push_mono(setting.name);
                                }
                                content.build()
                            }
                            MessageContext::RaceTime { reply_to, .. } => format!(
                                "Sorry {reply_to}, I don't recognize that setting. Use one of the following: {}",
                                all_settings.iter().map(|setting| setting.name).format(" or "),
                            ),
                        }))
                    },
                    _ => action,
                };
                match resolved_action {
                    Action::GoFirst(first) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
                        StepKind::GoFirst => {
                            self.went_first = Some(first);
                            Ok(match msg_ctx {
                                MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                MessageContext::Discord { transaction, guild_id, team, .. } => if let French = kind.language() {
                                    let mut content = MessageBuilder::default();
                                    content.mention_team(transaction, Some(*guild_id), team).await?;
                                    content.push(" a choisi de partir ");
                                    content.push(if first { "premier" } else { "second" });
                                    content.push(" pour le draft");
                                    if self.settings.get("mq_ok").map(|mq_ok| &**mq_ok).unwrap_or("no") == "ok" {
                                        let mq_dungeons_count = self.settings.get("mq_dungeons_count").map(|mq_dungeons_count| &**mq_dungeons_count).unwrap_or("0");
                                        content.push(" et a choisi ");
                                        content.push(mq_dungeons_count);
                                        content.push(" donjon");
                                        if mq_dungeons_count != "1" {
                                            content.push('s');
                                        }
                                        content.push(" MQ");
                                    }
                                    content
                                        .push('.')
                                        .build()
                                } else {
                                    let mut content = MessageBuilder::default();
                                    content.mention_team(transaction, Some(*guild_id), team).await?;
                                    content.push(" has chosen to go ");
                                    content.push(if first { "first" } else { "second" });
                                    content.push(" in the settings draft");
                                    if self.settings.get("mq_ok").map(|mq_ok| &**mq_ok).unwrap_or("no") == "ok" {
                                        let mq_dungeons_count = self.settings.get("mq_dungeons_count").map(|mq_dungeons_count| &**mq_dungeons_count).unwrap_or("0");
                                        content.push(" and has selected ");
                                        content.push(mq_dungeons_count);
                                        content.push(" MQ dungeon");
                                        if mq_dungeons_count != "1" {
                                            content.push('s');
                                        }
                                    }
                                    content
                                        .push('.')
                                        .build()
                                },
                            })
                        }
                        StepKind::Ban { .. } | StepKind::Pick { .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => if let French = kind.language() {
                                format!("Désolé, le premier pick a déjà été sélectionné.")
                            } else {
                                format!("Sorry, first pick has already been chosen.")
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, le premier pick a déjà été sélectionné.")
                            } else {
                                format!("Sorry {reply_to}, first pick has already been chosen.")
                            },
                        }),
                        StepKind::BooleanChoice { .. } => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { command_ids, .. } => if let French = kind.language() {
                                MessageBuilder::default()
                                    .push("Désolé, avant que le draft ne puisse continuer, vous devez d'abord choisir si les donjons seront mixés ou non avec le reste. Utilisez ")
                                    .mention_command(command_ids.yes.unwrap(), "yes")
                                    .push(" ou ")
                                    .mention_command(command_ids.no.unwrap(), "no")
                                    .push('.')
                                    .build()
                            } else {
                                MessageBuilder::default()
                                    .push("Sorry, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use ")
                                    .mention_command(command_ids.yes.unwrap(), "yes")
                                    .push(" or ")
                                    .mention_command(command_ids.no.unwrap(), "no")
                                    .push('.')
                                    .build()
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, avant que le draft ne puisse continuer, vous devez d'abord choisir si les donjons seront mixés ou non avec le reste. Utilisez !yes ou !no")
                            } else {
                                format!("Sorry {reply_to}, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use !yes or !no")
                            },
                        }),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => if let French = kind.language() {
                                format!("Désolé, ce draft est terminé.")
                            } else {
                                format!("Sorry, this settings draft is already completed.")
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, ce draft est terminé.")
                            } else {
                                format!("Sorry {reply_to}, this settings draft is already completed.")
                            },
                        }),
                    },
                    Action::Ban { .. } => unreachable!("normalized to Action::Pick above"),
                    Action::Pick { setting, value } => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                                let hard_settings_ok = self.settings.get("hard_settings_ok").map(|hard_settings_ok| &**hard_settings_ok).unwrap_or("no") == "ok";
                                self.settings.insert(Cow::Borrowed(setting.name), Cow::Borrowed(setting.default));
                                Ok(match msg_ctx {
                                    MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                    MessageContext::Discord { transaction, guild_id, team, .. } => if let French = kind.language() {
                                        MessageBuilder::default()
                                            .mention_team(transaction, Some(*guild_id), team).await?
                                            .push(" a banni ")
                                            .push(match setting.name { "camc" => "no CAMC", "souls" if !hard_settings_ok => "boss souls", _ => setting.display })
                                            .push('.')
                                            .build()
                                    } else {
                                        MessageBuilder::default()
                                            .mention_team(transaction, Some(*guild_id), team).await?
                                            .push(" has banned ")
                                            .push(match setting.name { "camc" => "no CAMC", "souls" if !hard_settings_ok => "boss souls", _ => setting.display })
                                            .push('.')
                                            .build()
                                    },
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
                            let exists = all_settings.iter().any(|&fr::Setting { name, .. }| setting == name);
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
                            if let Some(option) = setting.options.iter().find(|option| option.name == value) {
                                let hard_settings_ok = self.settings.get("hard_settings_ok").map(|hard_settings_ok| &**hard_settings_ok).unwrap_or("no") == "ok";
                                let is_default = value == all_settings.iter().find(|&&fr::Setting { name, .. }| setting.name == name).unwrap().default;
                                if !is_default {
                                    self.settings.insert(Cow::Borrowed(self.active_team(kind, game).await?.unwrap().choose("high_seed_has_picked", "low_seed_has_picked")), Cow::Borrowed("yes"));
                                }
                                self.settings.insert(Cow::Borrowed(setting.name), Cow::Borrowed(option.name));
                                Ok(match msg_ctx {
                                    MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                    MessageContext::Discord { transaction, guild_id, team, .. } => if let French = kind.language() {
                                        MessageBuilder::default()
                                            .mention_team(transaction, Some(*guild_id), team).await?
                                            .push(if is_default { " a banni " } else { " a choisi " })
                                            .push(if is_default { match setting.name { "camc" => "no CAMC", "souls" if !hard_settings_ok => "boss souls", _ => setting.display } } else { option.display })
                                            .push('.')
                                            .build()
                                    } else {
                                        MessageBuilder::default()
                                            .mention_team(transaction, Some(*guild_id), team).await?
                                            .push(if is_default { " has banned " } else { " has picked " })
                                            .push(if is_default { match setting.name { "camc" => "no CAMC", "souls" if !hard_settings_ok => "boss souls", _ => setting.display } } else { option.display })
                                            .push('.')
                                            .build()
                                    },
                                })
                            } else {
                                Err(match msg_ctx {
                                    MessageContext::None => String::default(),
                                    MessageContext::Discord { .. } => {
                                        let mut content = MessageBuilder::default();
                                        content.push("Sorry, that's not a possible value for this setting. Use one of the following: ");
                                        for (i, value) in setting.options.into_iter().enumerate() {
                                            if i > 0 {
                                                content.push(" or ");
                                            }
                                            content.push_mono(value.name);
                                        }
                                        content.build()
                                    }
                                    MessageContext::RaceTime { reply_to, .. } => format!(
                                        "Sorry {reply_to}, that's not a possible value for this setting. Use one of the following: {}",
                                        setting.options.into_iter().map(|value| value.name).format(" or "),
                                    ),
                                })
                            }
                        } else {
                            let exists = all_settings.iter().any(|&fr::Setting { name, .. }| setting == name);
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
                            MessageContext::Discord { command_ids, .. } => if let French = kind.language() {
                                MessageBuilder::default()
                                    .push("Désolé, avant que le draft ne puisse continuer, vous devez d'abord choisir si les donjons seront mixés ou non avec le reste. Utilisez ")
                                    .mention_command(command_ids.yes.unwrap(), "yes")
                                    .push(" ou ")
                                    .mention_command(command_ids.no.unwrap(), "no")
                                    .push('.')
                                    .build()
                            } else {
                                MessageBuilder::default()
                                    .push("Sorry, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use ")
                                    .mention_command(command_ids.yes.unwrap(), "yes")
                                    .push(" or ")
                                    .mention_command(command_ids.no.unwrap(), "no")
                                    .push('.')
                                    .build()
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, avant que le draft ne puisse continuer, vous devez d'abord choisir si les donjons seront mixés ou non avec le reste. Utilisez !yes ou !no")
                            } else {
                                format!("Sorry {reply_to}, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use !yes or !no")
                            },
                        }),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => if let French = kind.language() {
                                format!("Désolé, ce draft est terminé.")
                            } else {
                                format!("Sorry, this settings draft is already completed.")
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, ce draft est terminé.")
                            } else {
                                format!("Sorry {reply_to}, this settings draft is already completed.")
                            },
                        }),
                    },
                    Action::Skip => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
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
                            let skip_kind = match self.pick_count(kind) {
                                0 | 1 => "ban",
                                _ => "final pick",
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
                            MessageContext::Discord { command_ids, .. } => if let French = kind.language() {
                                MessageBuilder::default()
                                    .push("Désolé, avant que le draft ne puisse continuer, vous devez d'abord choisir si les donjons seront mixés ou non avec le reste. Utilisez ")
                                    .mention_command(command_ids.yes.unwrap(), "yes")
                                    .push(" ou ")
                                    .mention_command(command_ids.no.unwrap(), "no")
                                    .push('.')
                                    .build()
                            } else {
                                MessageBuilder::default()
                                    .push("Sorry, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use ")
                                    .mention_command(command_ids.yes.unwrap(), "yes")
                                    .push(" or ")
                                    .mention_command(command_ids.no.unwrap(), "no")
                                    .push('.')
                                    .build()
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, avant que le draft ne puisse continuer, vous devez d'abord choisir si les donjons seront mixés ou non avec le reste. Utilisez !yes ou !no")
                            } else {
                                format!("Sorry {reply_to}, before the settings draft can continue, you first have to choose whether dungeons entrances should be mixed. Use !yes or !no")
                            },
                        }),
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => if let French = kind.language() {
                                format!("Désolé, ce draft est terminé.")
                            } else {
                                format!("Sorry, this settings draft is already completed.")
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, ce draft est terminé.")
                            } else {
                                format!("Sorry {reply_to}, this settings draft is already completed.")
                            },
                        }),
                    },
                    Action::BooleanChoice(value) => match self.next_step(kind, game, &mut MessageContext::None).await?.kind {
                        StepKind::BooleanChoice { .. } => {
                            self.settings.insert(Cow::Borrowed("mixed-dungeons"), Cow::Borrowed(if value { "mixed" } else { "separate" }));
                            Ok(match msg_ctx {
                                MessageContext::None | MessageContext::RaceTime { .. } => String::default(),
                                MessageContext::Discord { transaction, guild_id, team, .. } => if let French = kind.language() {
                                    MessageBuilder::default()
                                        .mention_team(&mut *transaction, Some(*guild_id), team).await?
                                        .push(if value {
                                            " a choisi les trois ER mixés."
                                        } else {
                                            " a choisi de n'avoir que grottos et interior mixés."
                                        })
                                        .build()
                                } else {
                                    MessageBuilder::default()
                                        .mention_team(&mut *transaction, Some(*guild_id), team).await?
                                        .push(if value {
                                            " has selected mixed dungeon entrances."
                                        } else {
                                            " has selected separate dungeon entrances."
                                        })
                                        .build()
                                },
                            })
                        }
                        StepKind::Done(_) => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => if let French = kind.language() {
                                format!("Désolé, ce draft est terminé.")
                            } else {
                                format!("Sorry, this settings draft is already completed.")
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, ce draft est terminé.")
                            } else {
                                format!("Sorry {reply_to}, this settings draft is already completed.")
                            },
                        }),
                        _ => Err(match msg_ctx {
                            MessageContext::None => String::default(),
                            MessageContext::Discord { .. } => if let French = kind.language() {
                                format!("Désolé, vous n'avez pas à répondre oui ou non.")
                            } else {
                                format!("Sorry, the current step is not a yes/no question.")
                            },
                            MessageContext::RaceTime { reply_to, .. } => if let French = kind.language() {
                                format!("Désolé {reply_to}, vous n'avez pas à répondre oui ou non.")
                            } else {
                                format!("Sorry {reply_to}, the current step is not a yes/no question.")
                            },
                        }),
                    },
                }
            }
        })
    }

    pub(crate) async fn complete_randomly(mut self, kind: Kind) -> sqlx::Result<Picks> {
        Ok(loop {
            let action = match self.next_step(kind, None, &mut MessageContext::None).await?.kind {
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
            self.apply(kind, None, &mut MessageContext::None, action).await?.expect("random draft made illegal action");
        })
    }
}
