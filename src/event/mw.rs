use {
    std::{
        borrow::Cow,
        collections::HashMap,
        fmt,
        iter,
        pin::Pin,
    },
    collect_mac::collect,
    enum_iterator::{
        Sequence,
        all,
    },
    futures::{
        future::{
            self,
            Future,
            FutureExt as _,
        },
        stream::{
            self,
            StreamExt as _,
            TryStreamExt as _,
        },
    },
    itertools::Itertools as _,
    once_cell::sync::Lazy,
    rand::prelude::*,
    rocket::{
        FromForm,
        FromFormField,
        State,
        form::{
            self,
            Context,
            Contextual,
            Form,
        },
        http::Status,
        response::{
            Redirect,
            content::RawHtml,
        },
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        ContextualExt as _,
        CsrfForm,
        Origin,
        ToHtml,
        html,
    },
    serde::{
        Deserialize,
        Serialize,
    },
    serde_json::{
        Value as Json,
        json,
    },
    serde_plain::{
        derive_display_from_serialize,
        derive_fromstr_from_deserialize,
    },
    serenity::{
        client::Context as DiscordCtx,
        model::prelude::*,
    },
    serenity_utils::RwFuture,
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
    wheel::traits::ReqwestResponseExt as _,
    crate::{
        auth,
        event::{
            Data,
            Error,
            FindTeamError,
            InfoError,
            Series,
            SignupStatus,
            Tab,
        },
        favicon::ChestAppearances,
        http::{
            PageStyle,
            page,
        },
        seed::{
            self,
            HashIcon,
        },
        user::User,
        util::{
            DateTimeFormat,
            Id,
            IdTable,
            RedirectOrContent,
            StatusOrError,
            form_field,
            format_datetime,
            natjoin_html,
            natjoin_str,
            render_form_error,
        },
    },
};

const SERIES: Series = Series::Multiworld;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Wincon { #[default] Meds, Scrubs, Th }
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Dungeons { #[default] Tournament, Skulls, Keyrings }
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Er { #[default] Off, Dungeon }
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Trials { #[default] Zero, Two }
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Shops { #[default] Four, Off }
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Scrubs { #[default] Affordable, Off }
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Fountain { #[default] Closed, Open }
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Sequence, Deserialize, Serialize)] pub(crate) enum Spawn { #[default] Tot, Random }

impl Wincon { pub(crate) fn arg(&self) -> &'static str { match self { Self::Meds => "meds", Self::Scrubs => "scrubs", Self::Th => "th" } } }
impl Dungeons { pub(crate) fn arg(&self) -> &'static str { match self { Self::Tournament => "tournament", Self::Skulls => "skulls", Self::Keyrings => "keyrings" } } }
impl Er { pub(crate) fn arg(&self) -> &'static str { match self { Self::Off => "off", Self::Dungeon => "dungeon" } } }
impl Trials { pub(crate) fn arg(&self) -> &'static str { match self { Self::Zero => "0", Self::Two => "2" } } }
impl Shops { pub(crate) fn arg(&self) -> &'static str { match self { Self::Four => "4", Self::Off => "off" } } }
impl Scrubs { pub(crate) fn arg(&self) -> &'static str { match self { Self::Affordable => "affordable", Self::Off => "off" } } }
impl Fountain { pub(crate) fn arg(&self) -> &'static str { match self { Self::Closed => "closed", Self::Open => "open" } } }
impl Spawn { pub(crate) fn arg(&self) -> &'static str { match self { Self::Tot => "tot", Self::Random => "random" } } }

impl fmt::Display for Wincon { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Meds => write!(f, "default wincons"), Self::Scrubs => write!(f, "Scrubs wincons"), Self::Th => write!(f, "Triforce Hunt") } } }
impl fmt::Display for Dungeons { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Tournament => write!(f, "tournament dungeons"), Self::Skulls => write!(f, "dungeon tokens"), Self::Keyrings => write!(f, "keyrings") } } }
impl fmt::Display for Er { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Off => write!(f, "no ER"), Self::Dungeon => write!(f, "dungeon ER") } } }
impl fmt::Display for Trials { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Zero => write!(f, "0 trials"), Self::Two => write!(f, "2 trials") } } }
impl fmt::Display for Shops { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Four => write!(f, "shops 4"), Self::Off => write!(f, "no shops") } } }
impl fmt::Display for Scrubs { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Affordable => write!(f, "affordable scrubs"), Self::Off => write!(f, "no scrubs") } } }
impl fmt::Display for Fountain { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Closed => write!(f, "closed fountain"), Self::Open => write!(f, "open fountain") } } }
impl fmt::Display for Spawn { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Tot => write!(f, "ToT spawns"), Self::Random => write!(f, "random spawns & starting age") } } }

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

pub(crate) enum DraftStep {
    GoFirst,
    Ban {
        prev_bans: u8,
        team: Team,
    },
    Pick {
        prev_picks: u8,
        team: Team,
    },
    Done(S3Settings),
}

#[derive(PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum S3Setting {
    Wincon,
    Dungeons,
    Er,
    Trials,
    Shops,
    Scrubs,
    Fountain,
    Spawn,
}

derive_fromstr_from_deserialize!(S3Setting);
derive_display_from_serialize!(S3Setting);

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize)]
pub(crate) struct S3Draft {
    pub(crate) went_first: Option<bool>,
    pub(crate) skipped_bans: u8,
    pub(crate) wincon: Option<Wincon>,
    pub(crate) dungeons: Option<Dungeons>,
    pub(crate) er: Option<Er>,
    pub(crate) trials: Option<Trials>,
    pub(crate) shops: Option<Shops>,
    pub(crate) scrubs: Option<Scrubs>,
    pub(crate) fountain: Option<Fountain>,
    pub(crate) spawn: Option<Spawn>,
}

impl S3Draft {
    pub(crate) fn pick_count(&self) -> u8 {
        self.skipped_bans
        + u8::from(self.wincon.is_some())
        + u8::from(self.dungeons.is_some())
        + u8::from(self.er.is_some())
        + u8::from(self.trials.is_some())
        + u8::from(self.shops.is_some())
        + u8::from(self.scrubs.is_some())
        + u8::from(self.fountain.is_some())
        + u8::from(self.spawn.is_some())
    }

    pub(crate) fn next_step(&self) -> DraftStep {
        if let Some(went_first) = self.went_first {
            match self.pick_count() {
                prev_bans @ 0..=1 => DraftStep::Ban {
                    team: match (prev_bans, went_first) {
                        (0, true) | (1, false) => Team::HighSeed,
                        (0, false) | (1, true) => Team::LowSeed,
                        (2.., _) => unreachable!(),
                    },
                    prev_bans,
                },
                n @ 2..=5 => DraftStep::Pick {
                    prev_picks: n - 2,
                    team: match (n, went_first) {
                        (2, true) | (3, false) | (4, false) | (5, true) => Team::HighSeed,
                        (2, false) | (3, true) | (4, true) | (5, false) => Team::LowSeed,
                        (0..=1 | 6.., _) => unreachable!(),
                    },
                },
                6.. => DraftStep::Done(S3Settings {
                    wincon: self.wincon.unwrap_or_default(),
                    dungeons: self.dungeons.unwrap_or_default(),
                    er: self.er.unwrap_or_default(),
                    trials: self.trials.unwrap_or_default(),
                    shops: self.shops.unwrap_or_default(),
                    scrubs: self.scrubs.unwrap_or_default(),
                    fountain: self.fountain.unwrap_or_default(),
                    spawn: self.spawn.unwrap_or_default(),
                }),
            }
        } else {
            DraftStep::GoFirst
        }
    }

    pub(crate) fn active_team(&self) -> Option<Team> {
        match self.next_step() {
            DraftStep::GoFirst => Some(Team::HighSeed),
            DraftStep::Ban { team, .. } | DraftStep::Pick { team, .. } => Some(team),
            DraftStep::Done(_) => None,
        }
    }

    pub(crate) fn available_settings(&self) -> Vec<S3Setting> {
        let mut buf = Vec::with_capacity(8);
        if self.wincon.is_none() { buf.push(S3Setting::Wincon) }
        if self.dungeons.is_none() { buf.push(S3Setting::Dungeons) }
        if self.er.is_none() { buf.push(S3Setting::Er) }
        if self.trials.is_none() { buf.push(S3Setting::Trials) }
        if self.shops.is_none() { buf.push(S3Setting::Shops) }
        if self.scrubs.is_none() { buf.push(S3Setting::Scrubs) }
        if self.fountain.is_none() { buf.push(S3Setting::Fountain) }
        if self.spawn.is_none() { buf.push(S3Setting::Spawn) }
        buf
    }
}

#[derive(Default, Clone, Copy)]
pub(crate) struct S3Settings {
    pub(crate) wincon: Wincon,
    pub(crate) dungeons: Dungeons,
    pub(crate) er: Er,
    pub(crate) trials: Trials,
    pub(crate) shops: Shops,
    pub(crate) scrubs: Scrubs,
    pub(crate) fountain: Fountain,
    pub(crate) spawn: Spawn,
}

impl S3Settings {
    pub(crate) fn random(rng: &mut impl Rng) -> Self {
        let mut draft = S3Draft::default();
        loop {
            match draft.next_step() {
                DraftStep::GoFirst => draft.went_first = Some(rng.gen()),
                DraftStep::Ban { .. } => {
                    let available_settings = draft.available_settings();
                    let idx = rng.gen_range(0..=available_settings.len());
                    if let Some(setting) = available_settings.get(idx) {
                        match setting {
                            S3Setting::Wincon => draft.wincon = Some(Wincon::default()),
                            S3Setting::Dungeons => draft.dungeons = Some(Dungeons::default()),
                            S3Setting::Er => draft.er = Some(Er::default()),
                            S3Setting::Trials => draft.trials = Some(Trials::default()),
                            S3Setting::Shops => draft.shops = Some(Shops::default()),
                            S3Setting::Scrubs => draft.scrubs = Some(Scrubs::default()),
                            S3Setting::Fountain => draft.fountain = Some(Fountain::default()),
                            S3Setting::Spawn => draft.spawn = Some(Spawn::default()),
                        }
                    } else {
                        draft.skipped_bans += 1;
                    }
                }
                DraftStep::Pick { .. } => match draft.available_settings().choose(rng).expect("no more picks in DraftStep::Pick") {
                    S3Setting::Wincon => draft.wincon = Some(all().choose(rng).expect("setting values empty")),
                    S3Setting::Dungeons => draft.dungeons = Some(all().choose(rng).expect("setting values empty")),
                    S3Setting::Er => draft.er = Some(all().choose(rng).expect("setting values empty")),
                    S3Setting::Trials => draft.trials = Some(all().choose(rng).expect("setting values empty")),
                    S3Setting::Shops => draft.shops = Some(all().choose(rng).expect("setting values empty")),
                    S3Setting::Scrubs => draft.scrubs = Some(all().choose(rng).expect("setting values empty")),
                    S3Setting::Fountain => draft.fountain = Some(all().choose(rng).expect("setting values empty")),
                    S3Setting::Spawn => draft.spawn = Some(all().choose(rng).expect("setting values empty")),
                },
                DraftStep::Done(settings) => break settings,
            }
        }
    }

    pub(crate) fn resolve(&self) -> serde_json::Map<String, Json> {
        let Self { wincon, dungeons, er, trials, shops, scrubs, fountain, spawn } = self;
        collect![
            format!("user_message") => json!("3rd Multiworld Tournament"),
            format!("world_count") => json!(3),
            format!("open_forest") => json!("open"),
            format!("open_kakariko") => json!("open"),
            format!("open_door_of_time") => json!(true),
            format!("zora_fountain") => match fountain {
                Fountain::Closed => json!("closed"),
                Fountain::Open => json!("open"),
            },
            format!("gerudo_fortress") => json!("fast"),
            format!("bridge") => match wincon {
                Wincon::Meds => json!("medallions"),
                Wincon::Scrubs => json!("stones"),
                Wincon::Th => json!("dungeons"),
            },
            format!("bridge_medallions") => json!(6),
            format!("bridge_stones") => json!(3),
            format!("bridge_rewards") => json!(4),
            format!("triforce_hunt") => json!(matches!(wincon, Wincon::Th)),
            format!("triforce_count_per_world") => json!(30),
            format!("triforce_goal_per_world") => json!(25),
            format!("trials") => match trials {
                Trials::Zero => json!(0),
                Trials::Two => json!(2),
            },
            format!("shuffle_child_trade") => json!("skip_child_zelda"),
            format!("no_escape_sequence") => json!(true),
            format!("no_guard_stealth") => json!(true),
            format!("no_epona_race") => json!(true),
            format!("skip_some_minigame_phases") => json!(true),
            format!("free_scarecrow") => json!(true),
            format!("fast_bunny_hood") => json!(true),
            format!("start_with_rupees") => json!(true),
            format!("start_with_consumables") => json!(true),
            format!("big_poe_count") => json!(1),
            format!("shuffle_dungeon_entrances") => match er {
                Er::Off => json!("off"),
                Er::Dungeon => json!("simple"),
            },
            format!("spawn_positions") => json!(matches!(spawn, Spawn::Random)),
            format!("shuffle_scrubs") => match scrubs {
                Scrubs::Affordable => json!("low"),
                Scrubs::Off => json!("off"),
            },
            format!("shopsanity") => match shops {
                Shops::Four => json!("4"),
                Shops::Off => json!("off"),
            },
            format!("tokensanity") => match dungeons {
                Dungeons::Skulls => json!("dungeons"),
                Dungeons::Tournament | Dungeons::Keyrings => json!("off"),
            },
            format!("shuffle_mapcompass") => json!("startwith"),
            format!("shuffle_smallkeys") => match dungeons {
                Dungeons::Tournament => json!("dungeon"),
                Dungeons::Skulls => json!("vanilla"),
                Dungeons::Keyrings => json!("keysanity"),
            },
            format!("key_rings_choice") => match dungeons {
                Dungeons::Keyrings => json!("all"),
                Dungeons::Tournament | Dungeons::Skulls => json!("off"),
            },
            format!("shuffle_bosskeys") => match dungeons {
                Dungeons::Tournament => json!("dungeon"),
                Dungeons::Skulls | Dungeons::Keyrings => json!("vanilla"),
            },
            format!("shuffle_ganon_bosskey") => match wincon {
                Wincon::Meds => json!("remove"),
                Wincon::Scrubs => json!("on_lacs"),
                Wincon::Th => json!("triforce"),
            },
            format!("enhance_map_compass") => json!(true),
            format!("disabled_locations") => json!([
                "Deku Theater Mask of Truth",
                "Kak 40 Gold Skulltula Reward",
                "Kak 50 Gold Skulltula Reward"
            ]),
            format!("allowed_tricks") => json!([
                "logic_fewer_tunic_requirements",
                "logic_grottos_without_agony",
                "logic_child_deadhand",
                "logic_man_on_roof",
                "logic_dc_jump",
                "logic_rusted_switches",
                "logic_windmill_poh",
                "logic_crater_bean_poh_with_hovers",
                "logic_forest_vines",
                "logic_lens_botw",
                "logic_lens_castle",
                "logic_lens_gtg",
                "logic_lens_shadow",
                "logic_lens_shadow_platform",
                "logic_lens_bongo",
                "logic_lens_spirit",
                "logic_dc_scarecrow_gs"
            ]),
            format!("adult_trade_start") => json!(["Claim Check"]),
            format!("starting_items") => json!([
                "ocarina",
                "farores_wind",
                "lens"
            ]),
            format!("correct_chest_appearances") => json!("both"),
            format!("hint_dist") => json!("mw3"),
            format!("ice_trap_appearance") => json!("junk_only"),
            format!("junk_ice_traps") => json!("off"),
            format!("starting_age") => match spawn {
                Spawn::Tot => json!("adult"),
                Spawn::Random => json!("random"),
            },
        ]
    }

    pub(crate) fn chests(&self) -> ChestAppearances {
        static WEIGHTS: Lazy<HashMap<String, Vec<(ChestAppearances, usize)>>> = Lazy::new(|| serde_json::from_str(include_str!("../../assets/event/mw/chests-3-6.2.181.json")).expect("failed to parse chest weights")); //TODO update to 6.2.205

        if let Some(settings_weights) = WEIGHTS.get(&self.to_string()) {
            settings_weights.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
        } else {
            ChestAppearances::INVISIBLE
        }
    }
}

impl fmt::Display for S3Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut not_default = Vec::with_capacity(8);
        if self.wincon != Wincon::default() { not_default.push(self.wincon.to_string()) }
        if self.dungeons != Dungeons::default() { not_default.push(self.dungeons.to_string()) }
        if self.er != Er::default() { not_default.push(self.er.to_string()) }
        if self.trials != Trials::default() { not_default.push(self.trials.to_string()) }
        if self.shops != Shops::default() { not_default.push(self.shops.to_string()) }
        if self.scrubs != Scrubs::default() { not_default.push(self.scrubs.to_string()) }
        if self.fountain != Fountain::default() { not_default.push(self.fountain.to_string()) }
        if self.spawn != Spawn::default() { not_default.push(self.spawn.to_string()) }
        if let Some(not_default) = natjoin_str(not_default) {
            not_default.fmt(f)
        } else {
            write!(f, "base settings")
        }
    }
}

pub(super) async fn info(pool: &PgPool, event: &str) -> Result<RawHtml<String>, InfoError> {
    Ok(match event {
        "2" => {
            let organizers = stream::iter([
                10663518306823692018, // Alaszun
                12937129924130129092, // Hamsda
            ])
                .map(Id)
                .then(|id| async move { User::from_id(pool, id).await?.ok_or(InfoError::OrganizerUserData) })
                .try_collect::<Vec<_>>().await?;
            html! {
                article {
                    p {
                        : "This is an archive of the second Ocarina of Time randomizer multiworld tournament, organized by ";
                        : natjoin_html(organizers);
                        : ". Click the “teams” link above to see the results of the qualifier async.";
                    }
                    h2 : "See also";
                    ul {
                        li {
                            a(href = "https://docs.google.com/document/d/e/2PACX-1vS6vGCH8ZTA5bDCv3Z8meiUK4hMEfWN3vLttjNIOXbIAbRFNuGi-NzwJ68o31gVJgUigblLmW2tkZRu/pub") : "Tournament format, rules, and settings";
                        }
                        li {
                            a(href = "https://challonge.com/OoTRMWSeason2Swiss") : "Swiss results";
                        }
                        li {
                            a(href = "https://docs.google.com/spreadsheets/d/101zNpL1uvmIONb59kXYVyoa7YaHy8Y_OJv3M3vOKdBA/edit#gid=104642672") : "Tiebreaker scoresheet";
                        }
                        li {
                            a(href = "https://challonge.com/OoTRMWSeason2Finals") : "Top 8 results";
                        }
                    }
                }
            }
        }
        "3" => {
            let organizers = stream::iter([
                11983715422555811980, // ACreativeUsername
                10663518306823692018, // Alaszun
                11587964556511372305, // Bliven
                6374388881117205057, // felixoide4
                14571800683221815449, // Fenhl
                12937129924130129092, // Hamsda
                2315005348393237449, // rockchalk
                5305697930717211129, // tenacious_toad
            ])
                .map(Id)
                .then(|id| async move { User::from_id(pool, id).await?.ok_or(InfoError::OrganizerUserData) })
                .try_collect::<Vec<_>>().await?;
            html! {
                article {
                    p {
                        : "Hello and welcome to the official rules document for the Ocarina of Time Randomizer Multiworld Tournament Season 3, organized by ";
                        : natjoin_html(organizers);
                        : ".";
                    }
                    h2 : "Tournament Format";
                    p : "All teams are required to play a single asynchronous seed with the default race settings to participate. The results of this seed will be used to seed the settings draft.";
                    p {
                        : "The tournament itself will begin with a randomly seeded series of ";
                        a(href = "https://en.wikipedia.org/wiki/Swiss-system_tournament") : "Swiss";
                        : " rounds. These will be played as best of 1. There will be 6 rounds, with each round lasting two weeks. In the event that teams do not schedule in time, tournament organizers will use their discretion to determine the correct outcome based on the failures to schedule. In unusual circumstances that affect the entire tournament base (such as a GDQ), a round can be extended at the discretion of tournament organizers.";
                    }
                    p : "After all Swiss rounds are done, plus an additional tiebreaker async, the top 8 teams will advance to a single elimination bracket to crown the champions. The bracket stage of the tournament will be played as best of 3.";
                    h2 : "Match Format";
                    p : "Each match will consist of a 3v3 Multiworld where both sides compete to see which team will have all three of its members beat the game with the lowest average time of finish:";
                    ul {
                        li : "In a Triforce Hunt seed, timing for an individual player ends on the first completely black frame after that player has obtained the last required piece. (Due to how Triforce Hunt works in multiworld, all players on a team will normally have the same finish time, but if a player savescums a Triforce piece they found, they can have a lower Triforce count than their teammates.)";
                        li : "In all other seeds, timing for an individual player ends on the first frame of the cutscene that plays upon killing Ganon. Players are allowed to kill Ganon to stop their timer and then reset their game, allowing them to continue collecting items for their team if necessary.";
                    }
                    h2 : "Fair Play Agreement";
                    p {
                        : "By joining this tournament, teams must accept the terms of the ";
                        a(href = "https://docs.google.com/document/d/e/2PACX-1vQd3S28r8SOBy-4C5Lxeu6nFAYpWgQqN9lCEKhLGTT3zcaXDSKj0iUnZv6UPo_GargUVQx5F-wOPUtJ/pub") : "Fair Play Agreement (FPA)";
                        : ", a system that can be invoked in the event of technical issues. If playing on BizHawk, it is a strong, strong suggestion to make sure you enable backup saves as documented ";
                        a(href = "https://wiki.ootrandomizer.com/index.php?title=Bizhawk#Enable_Save_Backup_In_Case_of_Crashes") : "here";
                        : ".";
                    }
                    h2 : "Seed Settings";
                    p {
                        : "Starting with Swiss round 2, all tournament matches will be played on ";
                        a(href = "https://ootrandomizer.com/generatorDev?version=dev_6.2.205") : "version 6.2.205";
                        : " of the randomizer. (The qualifier async and Swiss round 1 were played on ";
                        a(href = "https://ootrandomizer.com/generatorDev?version=dev_6.2.181") : "version 6.2.181";
                        : ".)";
                    }
                    p : "The default settings for each race have the following differences to the S5 tournament preset:";
                    ul {
                        li : "Forest: Open";
                        li : "Scrub Shuffle: On (Affordable)";
                        li : "Shopsanity: 4 Items per Shop";
                        li : "Starting Inventory: Ocarina, FW, Lens of Truth, Consumables, Rupees (no Deku Shield)";
                        li : "Free Scarecrow's Song";
                        li : "Starting Age: Adult";
                        li : "Randomize Overworld Spawns: Off";
                        li : "Excluded Locations: Kak 40/50 Gold Skulltula Reward";
                        li : "Adult Trade Quest: Claim Check Only";
                        li : "Enable Tricks: Dodongo's Cavern Scarecrow GS with Armos Statue";
                        li : "Chest Appearance Matches Contents: Both Size and Texture";
                        li : "Maps and Compasses Give Information: On";
                    }
                    p : "You can use the “Multiworld Tournament Season 3” preset to load these settings.";
                    p : "However, in every race several of the settings may be modified by the teams. During Swiss, the team that placed higher in the qualifier async gets to pick who starts the procedure. For the first game of a top 8 match, this choice is made by the team with the higher seed in the bracket, and for subsequent games of a match, by the team that lost the previous game. Ties are broken by coin flip. The draft itself follows this pattern:";
                    ul {
                        li(class = "sheikah") : "Ban";
                        li(class = "gerudo") : "Ban";
                        li(class = "sheikah") : "Pick";
                        li(class = "gerudo") : "2x Pick";
                        li(class = "sheikah") : "Pick";
                    }
                    p {
                        : "A ";
                        em : "ban";
                        : " allows a team to lock in a setting of their choice to the default. A ";
                        em : "pick";
                        : " will function just like last season, allowing a team to change a setting or lock it to the default as well. This drafting procedure takes place in the scheduling thread for the match and must be completed at least 30 minutes before the scheduled starting time so the seed can be rolled.";
                    }
                    p {
                        : "The settings that can be modified and their respective options (";
                        strong : "first";
                        : " being the default) are, roughly ordered by impact on how drastically seeds and their length can change:";
                    }
                    ul {
                        li {
                            : "Win Condition:";
                            ul {
                                li {
                                    strong : "6 Medallion Rainbow Bridge + Remove Ganon’s Boss Key";
                                }
                                li : "3 Stone Rainbow Bridge + Vanilla LACS Ganon’s Boss Key";
                                li : "Triforce Hunt with 25/30 Triforce pieces per world + 4 Reward Rainbow Bridge";
                            }
                        }
                        li {
                            : "Dungeons:";
                            ul {
                                li {
                                    strong : "Small Keys and Boss Keys shuffled inside their own dungeons";
                                }
                                li : "Small Keys and Boss Keys in their vanilla chests + dungeon Tokensanity";
                                li : "Boss Keys in their vanilla chests + Small Keys anywhere + Keyrings on";
                            }
                        }
                        li {
                            : "Shuffle Dungeon Entrances:";
                            ul {
                                li {
                                    strong : "Off";
                                }
                                li : "On";
                            }
                        }
                        li {
                            : "Ganon's Trials:";
                            ul {
                                li {
                                    strong : "0";
                                }
                                li : "2";
                            }
                        }
                        li {
                            : "Shopsanity:";
                            ul {
                                li {
                                    strong : "4 Items per Shop + Random Prices";
                                }
                                li : "Off";
                            }
                        }
                        li {
                            : "Scrub Shuffle:";
                            ul {
                                li {
                                    strong : "On (Affordable)";
                                }
                                li : "Off";
                            }
                        }
                        li {
                            : "Zora's Fountain:";
                            ul {
                                li {
                                    strong : "Default Behavior (Closed)";
                                }
                                li : "Always Open";
                            }
                        }
                        li {
                            : "Starting Age/Randomize Overworld Spawns:";
                            ul {
                                li {
                                    strong : "Adult/Off";
                                }
                                li : "Random/On";
                            }
                        }
                    }
                    h2 : "Hint Distribution";
                    p : "Because of the somewhat unique settings of multiworld, there will be a custom hint distribution for this tournament. With 40 hint stones, the hint distribution will be as follows, with each hint having one duplicated hint:";
                    ul {
                        li {
                            : "7 Goal/Path hints:";
                            ul {
                                li : "No dungeon limit";
                                li : "Zelda's Lullaby is never directly hinted";
                            }
                        }
                        li : "0 Foolish hints";
                        li {
                            : "5-8 “Always” Hints (Settings Dependent):";
                            ul {
                                li : "2 active Trials (if enabled)";
                                li : "Song from Ocarina of Time";
                                li : "Sheik in Kakariko";
                                li : "Deku Theater Skull Mask";
                                li : "Kak 30 Gold Skulltula Reward";
                                li : "ZR Frogs Ocarina Game";
                                li : "DMC Deku Scrub (if Scrubsanity enabled)";
                            }
                        }
                    }
                    p : "The remainder of the hints will be filled out with selections from the “Sometimes” hint pool for a total of 20 paired hints. The following additional locations are Sometimes hints (if dungeon Tokensanity is enabled):";
                    ul {
                        li : "Deku Tree GS Basement Back Room";
                        li : "Water Temple GS River";
                        li : "Spirit Temple GS Hall After Sun Block Room";
                    }
                    p : "Always and Sometimes hint are upgraded to Dual hints where available.";
                    p : "The following Sometimes hints have been removed:";
                    ul {
                        li : "Sheik in Crater";
                        li : "Song from Royal Familys Tomb";
                        li : "Sheik in Forest";
                        li : "Sheik at Temple";
                        li : "Sheik at Colossus";
                        li : "LH Sun";
                        li : "GC Maze Left Chest";
                        li : "GV Chest";
                        li : "Graveyard Royal Familys Tomb Chest";
                        li : "GC Pot Freestanding PoH";
                        li : "LH Lab Dive";
                        li : "Fire Temple Megaton Hammer Chest";
                        li : "Fire Temple Scarecrow Chest";
                        li : "Water Temple Boss Key Chest";
                        li : "Water Temple GS Behind Gate";
                        li : "Gerudo Training Ground Maze Path Final Chest";
                        li : "Spirit Temple Silver Gauntlets Chest";
                        li : "Spirit Temple Mirror Shield Chest";
                        li : "Shadow Temple Freestanding Key";
                        li : "Ganons Castle Shadow Trial Golden Gauntlets Chest";
                    }
                    h2 : "Rules";
                    p {
                        : "This tournament will take place under the ";
                        a(href = "https://wiki.ootrandomizer.com/index.php?title=Standard") : "Standard";
                        : " racing ruleset, with some slight modifications:";
                    }
                    ul {
                        li : "Fire Arrow Entry is allowed";
                        li : "Playing Treasure Chest Game without magic and lens is banned";
                        li : "DMC “pot push” is banned";
                    }
                    h2 : "Multiworld Plugins";
                    p {
                        : "There are two plugins that can be used for the item sharing: ";
                        a(href = "https://github.com/TestRunnerSRL/bizhawk-co-op#readme") : "bizhawk-co-op";
                        : " (also known as Multiworld 1.0) and ";
                        a(href = "https://github.com/midoshouse/ootr-multiworld#readme") : "Mido's House Multiworld";
                        : ". While we recommend using the Mido's House plugin since it supports Project64 in addition to BizHawk and is easier to use (see ";
                        a(href = "https://github.com/midoshouse/ootr-multiworld#feature-comparison") : "feature comparison";
                        : "), both plugins are legal in this tournament.";
                    }
                    p : "We were hopeful to host this season of the tournament on Multiworld 2.0, but there have been further delays with its release. In the event that it does release during this tournament, the plan is to allow Multiworld 2.0 to be used after being cleared by the tournament staff. However, be aware that by using this your team accepts the risks with using it and must abide by the standard FPA rules.";
                }
            }
        }
        _ => unimplemented!(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq, FromFormField, Sequence)]
pub(crate) enum Role {
    #[field(value = "power")]
    Power,
    #[field(value = "wisdom")]
    Wisdom,
    #[field(value = "courage")]
    Courage,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Power => write!(f, "player 1"),
            Self::Wisdom => write!(f, "player 2"),
            Self::Courage => write!(f, "player 3"),
        }
    }
}

impl ToHtml for Role {
    fn to_html(&self) -> RawHtml<String> {
        match self {
            Self::Power => html! {
                span(class = "power") : "player 1";
            },
            Self::Wisdom => html! {
                span(class = "wisdom") : "player 2";
            },
            Self::Courage => html! {
                span(class = "courage") : "player 3";
            },
        }
    }
}

impl TryFrom<super::Role> for Role {
    type Error = ();

    fn try_from(role: super::Role) -> Result<Self, ()> {
        match role {
            super::Role::Power => Ok(Self::Power),
            super::Role::Wisdom => Ok(Self::Wisdom),
            super::Role::Courage => Ok(Self::Courage),
            _ => Err(()),
        }
    }
}

impl From<Role> for super::Role {
    fn from(role: Role) -> Self {
        match role {
            Role::Power => Self::Power,
            Role::Wisdom => Self::Wisdom,
            Role::Courage => Self::Courage,
        }
    }
}

#[derive(Deserialize)]
struct RaceTimeUser {
    teams: Vec<RaceTimeTeam>,
}

#[derive(Deserialize)]
struct RaceTimeTeam {
    name: String,
    slug: String,
}

#[derive(Deserialize)]
pub(super) struct RaceTimeTeamData {
    name: String,
    slug: String,
    members: Vec<RaceTimeTeamMember>,
}

#[derive(Clone, Deserialize)]
struct RaceTimeTeamMember {
    id: String,
    name: String,
}

pub(super) async fn validate_team(me: &User, client: &reqwest::Client, context: &mut Context<'_>, team_slug: &str) -> Result<Option<RaceTimeTeamData>, Error> {
    Ok(if let Some(ref racetime_id) = me.racetime_id {
        let user = client.get(format!("https://racetime.gg/user/{racetime_id}/data"))
            .send().await?
            .detailed_error_for_status().await?
            .json_with_text_in_error::<RaceTimeUser>().await?;
        if user.teams.iter().any(|team| team.slug == team_slug) {
            let team = client.get(format!("https://racetime.gg/team/{team_slug}/data"))
                .send().await?
                .detailed_error_for_status().await?
                .json_with_text_in_error::<RaceTimeTeamData>().await?;
            if team.members.len() != 3 {
                context.push_error(form::Error::validation(format!("Multiworld teams must have exactly 3 members, but this team has {}", team.members.len())))
            }
            //TODO get each team member's Mido's House account for displaying below
            Some(team)
        } else {
            context.push_error(form::Error::validation("This racetime.gg team does not exist or you're not in it.").with_name("racetime_team"));
            None
        }
    } else {
        context.push_error(form::Error::validation("A racetime.gg account is required to enter this tournament. Go to your profile and select “Connect a racetime.gg account”.")); //TODO direct link?
        None
    })
}

pub(super) async fn enter_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, context: Context<'_>, client: &reqwest::Client) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::Enter).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), if let Some(ref me) = me {
        if let Some(ref racetime_id) = me.racetime_id {
            let racetime_user = client.get(format!("https://racetime.gg/user/{racetime_id}/data"))
                .send().await?
                .detailed_error_for_status().await?
                .json_with_text_in_error::<RaceTimeUser>().await?;
            let mut errors = context.errors().collect_vec();
            if racetime_user.teams.is_empty() {
                html! {
                    : header;
                    article {
                        p {
                            a(href = "https://racetime.gg/account/teams/create") : "Create a racetime.gg team";
                            : " to enter this tournament.";
                        }
                    }
                }
            } else {
                let form_content = html! {
                    : csrf;
                    : form_field("racetime_team", &mut errors, html! {
                        label(for = "racetime_team") : "racetime.gg Team:";
                        select(name = "racetime_team") {
                            @for team in racetime_user.teams {
                                option(value = team.slug) : team.name;
                            }
                        }
                        label(class = "help") {
                            : "(Or ";
                            a(href = "https://racetime.gg/account/teams/create") : "create a new team";
                            : ", then come back here.)";
                        }
                    });
                    fieldset {
                        input(type = "submit", value = "Next");
                    }
                };
                html! {
                    : header;
                    form(action = uri!(super::enter_post(data.series, &*data.event)).to_string(), method = "post") {
                        @for error in errors {
                            : render_form_error(error);
                        }
                        : form_content;
                    }
                }
            }
        } else {
            html! {
                : header;
                article {
                    p {
                        a(href = uri!(crate::auth::racetime_login(Some(uri!(super::enter(data.series, &*data.event, _, _))))).to_string()) : "Connect a racetime.gg account to your Mido's House account";
                        : " to enter this tournament.";
                    }
                }
            }
        }
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(super::enter(data.series, &*data.event, _, _))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to enter this tournament.";
                }
            }
        }
    }).await?)
}

pub(super) enum EnterFormStep2Defaults<'a> {
    Context(Context<'a>),
    Values {
        racetime_team: RaceTimeTeamData,
    },
}

impl<'v> EnterFormStep2Defaults<'v> {
    fn errors(&self) -> Vec<&form::Error<'v>> {
        match self {
            Self::Context(ctx) => ctx.errors().collect(),
            Self::Values { .. } => Vec::default(),
        }
    }

    fn racetime_team_name(&self) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value("racetime_team_name"),
            Self::Values { racetime_team: RaceTimeTeamData { name, .. } } => Some(name),
        }
    }

    fn racetime_team_slug(&self) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value("racetime_team"),
            Self::Values { racetime_team: RaceTimeTeamData { slug, .. } } => Some(slug),
        }
    }

    fn racetime_members(&self, client: &reqwest::Client) -> impl Future<Output = Result<Vec<RaceTimeTeamMember>, Error>> {
        match self {
            Self::Context(ctx) => if let Some(team_slug) = ctx.field_value("racetime_team") {
                let client = client.clone();
                let url = format!("https://racetime.gg/team/{team_slug}/data");
                async move {
                    Ok(client.get(url)
                        .send().await?
                        .detailed_error_for_status().await?
                        .json_with_text_in_error::<RaceTimeTeamData>().await?
                        .members
                    )
                }.boxed()
            } else {
                future::ok(Vec::default()).boxed()
            }
            Self::Values { racetime_team } => future::ok(racetime_team.members.clone()).boxed(),
        }
    }

    fn world_number(&self, racetime_id: &str) -> Option<Role> {
        match self {
            Self::Context(ctx) => match ctx.field_value(&*format!("world_number[{racetime_id}]")) {
                Some("power") => Some(Role::Power),
                Some("wisdom") => Some(Role::Wisdom),
                Some("courage") => Some(Role::Courage),
                _ => None,
            },
            Self::Values { .. } => None,
        }
    }

    fn startgg_id(&self, racetime_id: &str) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value(&*format!("startgg_id[{racetime_id}]")),
            Self::Values { .. } => None,
        }
    }

    fn restream_consent(&self) -> bool {
        match self {
            Self::Context(ctx) => ctx.field_value("restream_consent") == Some("on"),
            Self::Values { .. } => false,
        }
    }
}

pub(super) fn enter_form_step2<'a, 'b: 'a, 'c: 'a, 'd: 'a>(mut transaction: Transaction<'a, Postgres>, me: Option<User>, uri: Origin<'b>, client: &reqwest::Client, csrf: Option<CsrfToken>, data: Data<'c>, defaults: EnterFormStep2Defaults<'d>) -> Pin<Box<dyn Future<Output = Result<RawHtml<String>, Error>> + Send + 'a>> {
    let team_members = defaults.racetime_members(client);
    Box::pin(async move {
        let header = data.header(&mut transaction, me.as_ref(), Tab::Enter).await?;
        let page_content = {
            let team_members = team_members.await?;
            let mut errors = defaults.errors();
            let form_content = html! {
                : csrf;
                : form_field("racetime_team", &mut errors, html! {
                    label(for = "racetime_team") {
                        : "racetime.gg Team: ";
                        a(href = format!("https://racetime.gg/team/{}", defaults.racetime_team_slug().expect("missing racetime team slug"))) : defaults.racetime_team_name().expect("missing racetime team name");
                        : " • ";
                        a(href = uri!(super::enter(data.series, &*data.event, _, _)).to_string()) : "Change";
                    }
                    input(type = "hidden", name = "racetime_team", value = defaults.racetime_team_slug());
                    input(type = "hidden", name = "racetime_team_name", value = defaults.racetime_team_name());
                });
                @for team_member in team_members {
                    : form_field(&format!("world_number[{}]", team_member.id), &mut errors, html! {
                        label(for = &format!("world_number[{}]", team_member.id)) : &team_member.name; //TODO Mido's House display name, falling back to racetime display name if no Mido's House account
                        input(id = &format!("world_number[{}]-power", team_member.id), class = "power", type = "radio", name = &format!("world_number[{}]", team_member.id), value = "power", checked? = defaults.world_number(&team_member.id) == Some(Role::Power));
                        label(class = "power", for = &format!("world_number[{}]-power", team_member.id)) : "World 1";
                        input(id = &format!("world_number[{}]-wisdom", team_member.id), class = "wisdom", type = "radio", name = &format!("world_number[{}]", team_member.id), value = "wisdom", checked? = defaults.world_number(&team_member.id) == Some(Role::Wisdom));
                        label(class = "wisdom", for = &format!("world_number[{}]-wisdom", team_member.id)) : "World 2";
                        input(id = &format!("world_number[{}]-courage", team_member.id), class = "courage", type = "radio", name = &format!("world_number[{}]", team_member.id), value = "courage", checked? = defaults.world_number(&team_member.id) == Some(Role::Courage));
                        label(class = "courage", for = &format!("world_number[{}]-courage", team_member.id)) : "World 3";
                    });
                    : form_field(&format!("startgg_id[{}]", team_member.id), &mut errors, html! {
                        label(for = &format!("startgg_id[{}]", team_member.id)) : "start.gg User ID:";
                        input(type = "text", name = &format!("startgg_id[{}]", team_member.id), value? = defaults.startgg_id(&team_member.id));
                        label(class = "help") {
                            : "(Optional. Can be found by going to your ";
                            a(href = "https://start.gg/") : "start.gg";
                            : " profile and clicking your name.)";
                        }
                    });
                }
                : form_field("restream_consent", &mut errors, html! {
                    input(type = "checkbox", id = "restream_consent", name = "restream_consent", checked? = defaults.restream_consent());
                    label(for = "restream_consent") : "We are okay with being restreamed. (Optional for Swiss, required for top 8. Can be changed later.)"; //TODO allow changing on Status page during Swiss, except revoking while a restream is planned
                });
                fieldset {
                    input(type = "submit", value = "Submit");
                }
            };
            html! {
                : header;
                form(action = uri!(enter_post_step2(&*data.event)).to_string(), method = "post") {
                    @for error in errors {
                        : render_form_error(error);
                    }
                    : form_content;
                }
            }
        };
        Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), page_content).await?)
    })
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct EnterFormStep2 {
    #[field(default = String::new())]
    csrf: String,
    racetime_team: String,
    world_number: HashMap<String, Role>,
    startgg_id: HashMap<String, String>,
    restream_consent: bool,
}

#[rocket::post("/event/mw/<event>/enter/step2", data = "<form>")]
pub(crate) async fn enter_post_step2(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, client: &State<reqwest::Client>, csrf: Option<CsrfToken>, event: &str, form: Form<Contextual<'_, EnterFormStep2>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, SERIES, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started(&mut transaction).await? {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        // verify team again since it's sent by the client
        let (team_slug, team_name, users, roles, startgg_ids) = if let Some(racetime_team) = validate_team(&me, client, &mut form.context, &value.racetime_team).await? {
            let mut all_accounts_exist = true;
            let mut users = Vec::default();
            let mut roles = Vec::default();
            let mut startgg_ids = Vec::default();
            for member in &racetime_team.members {
                if let Some(user) = User::from_racetime(&mut transaction, &member.id).await? {
                    if let Some(discord_id) = user.discord_id {
                        if let Some(discord_guild) = data.discord_guild {
                            if discord_guild.member(&*discord_ctx.read().await, discord_id).await.is_err() {
                                form.context.push_error(form::Error::validation("This user has not joined the tournament's Discord server.").with_name(format!("world_number[{}]", member.id)));
                            }
                        }
                    } else {
                        form.context.push_error(form::Error::validation("This Mido's House account is not associated with a Discord account.").with_name(format!("world_number[{}]", member.id)));
                    }
                    if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                        id = team
                        AND series = 'mw'
                        AND event = $1
                        AND member = $2
                        AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                    ) AS "exists!""#, event, i64::from(user.id)).fetch_one(&mut transaction).await? {
                        form.context.push_error(form::Error::validation("This user is already signed up for this tournament."));
                    }
                    users.push(user);
                } else {
                    form.context.push_error(form::Error::validation("This racetime.gg account is not associated with a Mido's House account.").with_name(format!("world_number[{}]", member.id)));
                    all_accounts_exist = false;
                }
                if let Some(&role) = value.world_number.get(&member.id) {
                    roles.push(role);
                } else {
                    form.context.push_error(form::Error::validation("This field is required.").with_name(format!("world_number[{}]", member.id)));
                }
                if let Some(id) = value.startgg_id.get(&member.id) {
                    if id.is_empty() {
                        startgg_ids.push(None);
                    } else if id.len() != 8 {
                        form.context.push_error(form::Error::validation("User IDs on start.gg are exactly 8 characters in length.").with_name(format!("startgg_id[{}]", member.id)));
                    } else {
                        startgg_ids.push(Some(id.clone()));
                    }
                } else {
                    startgg_ids.push(None);
                }
            }
            if all_accounts_exist {
                match &*users {
                    [u1, u2, u3] => if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
                        series = 'mw'
                        AND event = $1
                        AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $2)
                        AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                        AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $4)
                    ) AS "exists!""#, event, i64::from(u1.id), i64::from(u2.id), i64::from(u3.id)).fetch_one(&mut transaction).await? {
                        form.context.push_error(form::Error::validation("A team with these members is already proposed for this tournament. Check your notifications to accept the invite, and/or ask your teammates to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
                    },
                    _ => unimplemented!("exact proposed team check for {} members", users.len()),
                }
            }
            for role in all::<Role>() {
                let mut found = false;
                for (member_id, world_number) in &value.world_number {
                    if *world_number == role {
                        if found {
                            form.context.push_error(form::Error::validation("Each team member must have a different world number.").with_name(format!("world_number[{member_id}]")));
                        } else {
                            found = true;
                        }
                    }
                }
                if !found {
                    form.context.push_error(form::Error::validation(format!("No team member is assigned as {role}.")));
                }
            }
            (racetime_team.slug, racetime_team.name, users, roles, startgg_ids)
        } else {
            Default::default()
        };
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(enter_form_step2(transaction, Some(me), uri, client, csrf, data, EnterFormStep2Defaults::Context(form.context)).await?)
        } else {
            let id = Id::new(&mut transaction, IdTable::Teams).await?;
            sqlx::query!("INSERT INTO teams (id, series, event, name, racetime_slug, restream_consent) VALUES ($1, 'mw', $2, $3, $4, $5)", id as _, event, (!team_name.is_empty()).then(|| team_name), team_slug, value.restream_consent).execute(&mut transaction).await?;
            for ((user, role), startgg_id) in users.into_iter().zip_eq(roles).zip_eq(startgg_ids) {
                sqlx::query!(
                    "INSERT INTO team_members (team, member, status, role, startgg_id) VALUES ($1, $2, $3, $4, $5)",
                    id as _, user.id as _, if user == me { SignupStatus::Created } else { SignupStatus::Unconfirmed } as _, super::Role::from(role) as _, startgg_id,
                ).execute(&mut transaction).await?;
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(super::teams(SERIES, event))))
        }
    } else {
        RedirectOrContent::Content(enter_form_step2(transaction, Some(me), uri, client, csrf, data, EnterFormStep2Defaults::Context(form.context)).await?)
    })
}

pub(super) async fn find_team_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, context: Context<'_>) -> Result<RawHtml<String>, FindTeamError> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::FindTeam).await?;
    let mut me_listed = false;
    let mut looking_for_team = Vec::default();
    for row in sqlx::query!(r#"SELECT user_id AS "user!: Id", availability, notes FROM looking_for_team WHERE series = $1 AND event = $2"#, data.series as _, &data.event).fetch_all(&mut *transaction).await? {
        let user = User::from_id(&mut transaction, row.user).await?.ok_or(FindTeamError::UnknownUser)?;
        if me.as_ref().map_or(false, |me| user.id == me.id) { me_listed = true }
        looking_for_team.push((user, row.availability, row.notes));
    }
    let form = if me.is_some() {
        let mut errors = context.errors().collect_vec();
        if me_listed {
            None
        } else {
            let form_content = html! {
                : csrf;
                legend {
                    : "Fill out this form to add yourself to the list below.";
                }
                : form_field("availability", &mut errors, html! {
                    label(for = "availability") : "Timezone/Availability/Commitment:";
                    input(type = "text", name = "availability", value? = context.field_value("availability"));
                });
                : form_field("notes", &mut errors, html! {
                    label(for = "notes") : "Any Other Notes?";
                    input(type = "text", name = "notes", value? = context.field_value("notes"));
                });
                fieldset {
                    input(type = "submit", value = "Submit");
                }
            };
            Some(html! {
                form(action = uri!(find_team_post(&*data.event)).to_string(), method = "post") {
                    @for error in errors {
                        : render_form_error(error);
                    }
                    : form_content;
                }
            })
        }
    } else {
        Some(html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(super::find_team(data.series, &*data.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to add yourself to this list.";
                }
            }
        })
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Find Teammates — {}", data.display_name), html! {
        : header;
        : form;
        table {
            thead {
                tr {
                    th : "User";
                    th : "Timezone/Availability/Commitment";
                    th : "Notes";
                }
            }
            tbody {
                @if looking_for_team.is_empty() {
                    tr {
                        td(colspan = "3") {
                            i : "(no one currently looking for teammates)";
                        }
                    }
                } else {
                    @for (user, availability, notes) in looking_for_team {
                        tr {
                            td : user;
                            td : availability;
                            td : notes;
                        }
                    }
                }
            }
        }
    }).await?)
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct FindTeamForm {
    #[field(default = String::new())]
    csrf: String,
    availability: String,
    notes: String,
}

#[rocket::post("/event/mw/<event>/find-team", data = "<form>")]
pub(crate) async fn find_team_post(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, event: &str, form: Form<Contextual<'_, FindTeamForm>>) -> Result<RedirectOrContent, StatusOrError<FindTeamError>> {
    let mut transaction = pool.begin().await.map_err(FindTeamError::Sql)?;
    let data = Data::new(&mut transaction, SERIES, event).await.map_err(FindTeamError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started(&mut transaction).await.map_err(FindTeamError::Sql)? {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM looking_for_team WHERE
            series = 'mw'
            AND event = $1
            AND user_id = $2
        ) AS "exists!""#, event, i64::from(me.id)).fetch_one(&mut transaction).await.map_err(FindTeamError::Sql)? {
            form.context.push_error(form::Error::validation("You are already on the list."));
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = 'mw'
            AND event = $1
            AND member = $2
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, event, i64::from(me.id)).fetch_one(&mut transaction).await.map_err(FindTeamError::Sql)? {
            form.context.push_error(form::Error::validation("You are already signed up for this race."));
        }
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(find_team_form(transaction, Some(me), uri, csrf, data, form.context).await?)
        } else {
            sqlx::query!("INSERT INTO looking_for_team (series, event, user_id, role, availability, notes) VALUES ('mw', $1, $2, 'no_preference', $3, $4)", event, me.id as _, value.availability, value.notes).execute(&mut transaction).await.map_err(FindTeamError::Sql)?;
            transaction.commit().await.map_err(FindTeamError::Sql)?;
            RedirectOrContent::Redirect(Redirect::to(uri!(super::find_team(SERIES, event))))
        }
    } else {
        RedirectOrContent::Content(find_team_form(transaction, Some(me), uri, csrf, data, form.context).await?)
    })
}

pub(super) async fn status(transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, csrf: Option<CsrfToken>, data: &Data<'_>, team_id: Id, context: Context<'_>) -> sqlx::Result<RawHtml<String>> {
    Ok(if let Some(async_row) = sqlx::query!(r#"SELECT discord_channel AS "discord_channel: Id", web_id as "web_id: Id", web_gen_time, file_stem, hash1 AS "hash1: HashIcon", hash2 AS "hash2: HashIcon", hash3 AS "hash3: HashIcon", hash4 AS "hash4: HashIcon", hash5 AS "hash5: HashIcon" FROM asyncs WHERE series = $1 AND event = $2"#, data.series as _, &data.event).fetch_optional(&mut *transaction).await? {
        if let Some(team_row) = sqlx::query!("SELECT requested, submitted FROM async_teams WHERE team = $1", i64::from(team_id)).fetch_optional(&mut *transaction).await? {
            if team_row.submitted.is_some() {
                if data.is_started(transaction).await? {
                    //TODO get this team's known matchup(s) from start.gg
                    html! {
                        p : "Please schedule your matches using Discord threads in the scheduling channel.";
                    }
                    //TODO form to submit matches
                } else {
                    //TODO if any vods are still missing, show form to add them
                    html! {
                        p : "Waiting for the start of the tournament and round 1 pairings. Keep an eye out for an announcement on Discord."; //TODO include start date?
                    }
                }
            } else {
                let web = match (async_row.web_id, async_row.web_gen_time, async_row.hash1, async_row.hash2, async_row.hash3, async_row.hash4, async_row.hash5) {
                    (Some(Id(id)), Some(gen_time), Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some(seed::OotrWebData { id, gen_time, file_hash: [hash1, hash2, hash3, hash4, hash5] }),
                    (None, None, None, None, None, None, None) => None,
                    _ => unreachable!("only some web data present, should be prevented by SQL constraint"),
                };
                let seed = seed::Data { web, file_stem: Cow::Owned(async_row.file_stem) };
                let seed_table = seed::table(stream::iter(iter::once(seed)), false).await?;
                let mut errors = context.errors().collect_vec();
                let form_content = html! {
                    : csrf;
                    : form_field("time1", &mut errors, html! {
                        label(for = "time1", class = "power") : "Player 1 Finishing Time:";
                        input(type = "text", name = "time1", value? = context.field_value("time1")); //TODO h:m:s fields?
                        label(class = "help") : "(If player 1 did not finish, leave this field blank.)";
                    });
                    : form_field("vod1", &mut errors, html! {
                        label(for = "vod1", class = "power") : "Player 1 VoD:";
                        input(type = "text", name = "vod1", value? = context.field_value("vod1"));
                        label(class = "help") {
                            : "(If you plan on uploading the VoD to YouTube later, leave this field blank and ";
                            @if let Some(Id(discord_channel)) = async_row.discord_channel {
                                : "post it in ";
                                @if let Some(discord_channel) = ChannelId(discord_channel).to_channel_cached(discord_ctx).and_then(|c| c.guild()) {
                                    : "#";
                                    : discord_channel.name;
                                } else {
                                    : "the results channel for this async";
                                }
                            } else {
                                : "DM an admin";
                            }
                            : " once it is ready.)";
                            //TODO form to submit vods later
                        }
                    });
                    : form_field("time2", &mut errors, html! {
                        label(for = "time2", class = "wisdom") : "Player 2 Finishing Time:";
                        input(type = "text", name = "time2", value? = context.field_value("time2")); //TODO h:m:s fields?
                        label(class = "help") : "(If player 2 did not finish, leave this field blank.)";
                    });
                    : form_field("vod2", &mut errors, html! {
                        label(for = "vod2", class = "wisdom") : "Player 2 VoD:";
                        input(type = "text", name = "vod2", value? = context.field_value("vod2"));
                        label(class = "help") {
                            : "(If you plan on uploading the VoD to YouTube later, leave this field blank and ";
                            @if let Some(Id(discord_channel)) = async_row.discord_channel {
                                : "post it in ";
                                @if let Some(discord_channel) = ChannelId(discord_channel).to_channel_cached(discord_ctx).and_then(|c| c.guild()) {
                                    : "#";
                                    : discord_channel.name;
                                } else {
                                    : "the results channel for this async";
                                }
                            } else {
                                : "DM an admin";
                            }
                            : " once it is ready.)";
                            //TODO form to submit vods later
                        }
                    });
                    : form_field("time3", &mut errors, html! {
                        label(for = "time3", class = "courage") : "Player 3 Finishing Time:";
                        input(type = "text", name = "time3", value? = context.field_value("time3")); //TODO h:m:s fields?
                        label(class = "help") : "(If player 3 did not finish, leave this field blank.)";
                    });
                    : form_field("vod3", &mut errors, html! {
                        label(for = "vod3", class = "courage") : "Player 3 VoD:";
                        input(type = "text", name = "vod3", value? = context.field_value("vod3"));
                        label(class = "help") {
                            : "(If you plan on uploading the VoD to YouTube later, leave this field blank and ";
                            @if let Some(Id(discord_channel)) = async_row.discord_channel {
                                : "post it in ";
                                @if let Some(discord_channel) = ChannelId(discord_channel).to_channel_cached(discord_ctx).and_then(|c| c.guild()) {
                                    : "#";
                                    : discord_channel.name;
                                } else {
                                    : "the results channel for this async";
                                }
                            } else {
                                : "DM an admin";
                            }
                            : " once it is ready.)";
                            //TODO form to submit vods later
                        }
                    });
                    : form_field("fpa", &mut errors, html! {
                        label(for = "fpa") {
                            : "If you would like to invoke the ";
                            a(href = "https://docs.google.com/document/d/e/2PACX-1vQd3S28r8SOBy-4C5Lxeu6nFAYpWgQqN9lCEKhLGTT3zcaXDSKj0iUnZv6UPo_GargUVQx5F-wOPUtJ/pub") : "Fair Play Agreement";
                            : ", describe the break(s) you took below. Include the reason, starting time, and duration.";
                        }
                        textarea(name = "fpa");
                    });
                    fieldset {
                        input(type = "submit", value = "Submit");
                    }
                };
                html! {
                    div(class = "info") {
                        p {
                            : "You requested the qualifier async on ";
                            : format_datetime(team_row.requested, DateTimeFormat { long: true, running_text: true });
                            : ".";
                        };
                        : seed_table;
                        p : "After playing the async, fill out the form below.";
                        form(action = uri!(super::submit_async(data.series, &*data.event)).to_string(), method = "post") {
                            @for error in errors {
                                : render_form_error(error);
                            }
                            : form_content;
                        }
                    }
                }
            }
        } else {
            let mut errors = context.errors().collect_vec();
            let form_content = html! {
                : csrf;
                : form_field("confirm", &mut errors, html! {
                    input(type = "checkbox", id = "confirm", name = "confirm");
                    label(for = "confirm") : "We have read the above and are ready to play the seed";
                });
                fieldset {
                    input(type = "submit", value = "Request Now");
                }
            };
            html! {
                div(class = "info") {
                    p : "Play the qualifier async to qualify for the tournament.";
                    p : "Rules:";
                    ol {
                        li : "In order to play in the tournament, your team must make a reasonable attempt at completing this seed. In the event of a forfeit, you can still participate, but will be considered the bottom seed for settings draft purposes.";
                        li {
                            @if let Some(base_start) = data.base_start {
                                : "The time must be submitted by ";
                                : format_datetime(base_start, DateTimeFormat { long: true, running_text: true });
                                : ". In the event that an odd number of teams is qualified at the time of the deadline, one additional team may qualify within 24 hours.";
                            } else {
                                : "The time must be submitted by the starting time of the tournament, which is yet to be announced.";
                            }
                        }
                        li : "You must start the seed within 30 minutes of obtaining it and submit your time within 30 minutes of the last finish. Any additional time taken will be added to your final time. If anything prevents you from obtaining the seed/submitting your time, please DM an admin (or ping the Discord role) to get it sorted out.";
                        li : "While required for the tournament, the results from the qualifier seed will only determine which team chooses who goes first in the settings draft. Swiss pairings will be seeded randomly.";
                        li : "While you are not strictly required to stream, you must have video proof of your run. Feel free to simply record your run and upload it to YouTube and provide a link. If you do stream or make your upload public, please make sure it is clearly marked so people can avoid spoilers. If you're a big streamer, be extra sure to note what is happening, as several of your viewers are likely going to want to participate as well.";
                        li : "Do not spoil yourself on this seed by watching another playthrough. If you do stream, you are responsible for what your chat says, so either do not read chat, set it to emote only, or take the risk at your own discretion. If you do get spoiled, please report it to the admins, we will try to work out something equitable.";
                        li : "You must use the world numbers with which you entered the tournament for this seed. Once you request the seed, the world numbers you selected are the world numbers you play with for the rest of the tournament. If you wish to change your player order, do not request the qualifier and contact an admin."; //TODO allow changing player order in options below
                        li {
                            : "This should be run like an actual race. In the event of a technical issue, teams are allowed to invoke the ";
                            a(href = "https://docs.google.com/document/d/e/2PACX-1vQd3S28r8SOBy-4C5Lxeu6nFAYpWgQqN9lCEKhLGTT3zcaXDSKj0iUnZv6UPo_GargUVQx5F-wOPUtJ/pub") : "Fair Play Agreement";
                            : " and have up to a 15 minute time where the affected runner can try to catch back up. If you do this, you must fill out the appropriate field when submitting your time so it can be authenticated.";
                        }
                    }
                    form(action = uri!(super::request_async(data.series, &*data.event)).to_string(), method = "post") {
                        @for error in errors {
                            : render_form_error(error);
                        }
                        : form_content;
                    }
                }
            }
        }
    } else {
        html! {
            p : "Waiting for the qualifier async to be published. Keep an eye out for an announcement on Discord.";
        }
    })
}
