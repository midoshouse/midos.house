use {
    std::{
        borrow::Cow,
        collections::{
            HashMap,
            HashSet,
        },
        convert::identity,
        fmt,
        io,
        mem,
        str::FromStr,
        time::Duration,
    },
    anyhow::anyhow,
    chrono::prelude::*,
    enum_iterator::{
        Sequence,
        all,
    },
    futures::stream::TryStreamExt as _,
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
        http::{
            Status,
            impl_from_uri_param_identity,
            uri::{
                self,
                fmt::{
                    Path,
                    UriDisplay,
                },
            },
        },
        request::FromParam,
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
    serenity::{
        all::{
            Context as DiscordCtx,
            CreateMessage,
            EditMember,
            EditRole,
            MessageBuilder,
        },
        model::prelude::*,
    },
    serenity_utils::RwFuture,
    sqlx::{
        Decode,
        Encode,
        Transaction,
        postgres::{
            Postgres,
            PgArgumentBuffer,
            PgPool,
            PgTypeInfo,
            PgValueRef,
        },
        types::Json,
    },
    url::Url,
    crate::{
        Environment,
        auth,
        cal::{
            self,
            Entrants,
            Race,
            RaceSchedule,
        },
        config::Config,
        draft::{
            self,
            Draft,
        },
        favicon::ChestAppearances,
        http::{
            PageError,
            PageStyle,
            page,
        },
        lang::Language::{
            self,
            *,
        },
        notification::SimpleNotificationKind,
        racetime_bot,
        seed,
        series::*,
        team::Team,
        user::User,
        util::{
            DateTimeFormat,
            DurationUnit,
            EmptyForm,
            Id,
            IdTable,
            MessageBuilderExt as _,
            RedirectOrContent,
            StatusOrError,
            decode_pginterval,
            favicon,
            form_field,
            format_datetime,
            format_duration,
            full_form,
            parse_duration,
        },
    },
};

pub(crate) mod enter;

#[derive(Debug, Clone, Copy, sqlx::Type)]
#[sqlx(type_name = "signup_status", rename_all = "snake_case")]
pub(crate) enum SignupStatus {
    Created,
    Confirmed,
    Unconfirmed,
}

impl SignupStatus {
    fn is_confirmed(&self) -> bool {
        matches!(self, Self::Created | Self::Confirmed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, FromFormField)]
#[sqlx(type_name = "team_role", rename_all = "snake_case")]
pub(crate) enum Role {
    /// For solo events.
    None,
    /// Player 1 of 2. “Runner” in Pictionary.
    Sheikah,
    /// Player 2 of 2. “Pilot” in Pictionary.
    Gerudo,
    /// Player 1 of 3.
    Power,
    /// Player 2 of 3.
    Wisdom,
    /// Player 3 of 3.
    Courage,
}

impl Role {
    pub(crate) fn from_css_class(css_class: &str) -> Option<Self> {
        match css_class {
            "sheikah" => Some(Self::Sheikah),
            "gerudo" => Some(Self::Gerudo),
            "power" => Some(Self::Power),
            "wisdom" => Some(Self::Wisdom),
            "courage" => Some(Self::Courage),
            _ => None,
        }
    }

    fn css_class(&self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Sheikah => Some("sheikah"),
            Self::Gerudo => Some("gerudo"),
            Self::Power => Some("power"),
            Self::Wisdom => Some("wisdom"),
            Self::Courage => Some("courage"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Sequence)]
pub(crate) enum Series {
    League,
    MixedPools,
    Multiworld,
    NineDaysOfSaws,
    Pictionary,
    Rsl,
    Standard,
    TournoiFrancophone,
    TriforceBlitz,
}

impl Series {
    pub(crate) fn to_str(&self) -> &'static str {
        match self {
            Self::League => "league",
            Self::MixedPools => "mp",
            Self::Multiworld => "mw",
            Self::NineDaysOfSaws => "9dos",
            Self::Pictionary => "pic",
            Self::Rsl => "rsl",
            Self::Standard => "s",
            Self::TournoiFrancophone => "fr",
            Self::TriforceBlitz => "tfb",
        }
    }
}

impl FromStr for Series {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        all::<Self>().find(|series| series.to_str() == s).ok_or(())
    }
}

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.to_str(), f)
    }
}

impl<'r> Decode<'r, Postgres> for Series {
    fn decode(value: PgValueRef<'r>) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let series = <&str>::decode(value)?;
        series.parse().map_err(|()| anyhow!("unknown series: {series}").into())
    }
}

impl<'q> Encode<'q, Postgres> for Series {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> sqlx::encode::IsNull {
        self.to_str().encode(buf)
    }

    fn encode(self, buf: &mut PgArgumentBuffer) -> sqlx::encode::IsNull {
        self.to_str().encode(buf)
    }

    fn produces(&self) -> Option<PgTypeInfo> {
        self.to_str().produces()
    }

    fn size_hint(&self) -> usize {
        Encode::size_hint(&self.to_str())
    }
}

impl sqlx::Type<Postgres> for Series {
    fn type_info() -> PgTypeInfo {
        <&str>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str>::compatible(ty)
    }
}

impl<'a> FromParam<'a> for Series {
    type Error = &'a str;

    fn from_param(param: &'a str) -> Result<Self, Self::Error> {
        param.parse().map_err(|()| param)
    }
}

impl UriDisplay<Path> for Series {
    fn fmt(&self, f: &mut uri::fmt::Formatter<'_, Path>) -> fmt::Result {
        UriDisplay::fmt(self.to_str(), f) // assume all series names are URI safe
    }
}

impl_from_uri_param_identity!([Path] Series);

#[derive(PartialEq, Eq, Sequence)]
pub(crate) enum MatchSource {
    Manual,
    League, //TODO automatically scan for new matches and create scheduling threads
    StartGG, //TODO automatically scan for new matches and create scheduling threads
}

pub(crate) enum TeamConfig {
    Solo,
    CoOp,
    Pictionary,
    Multiworld,
}

impl TeamConfig {
    pub(crate) fn roles(&self) -> &'static [(Role, &'static str)] {
        match self {
            Self::Solo => &[
                (Role::None, "Runner"),
            ],
            Self::CoOp => &[
                (Role::Sheikah, "Player 1"),
                (Role::Gerudo, "Player 2"),
            ],
            Self::Pictionary => &[
                (Role::Sheikah, "Runner"),
                (Role::Gerudo, "Pilot"),
            ],
            Self::Multiworld => &[
                (Role::Power, "World 1"),
                (Role::Wisdom, "World 2"),
                (Role::Courage, "World 3"),
            ],
        }
    }

    /// Whether team members with the given role should be invited to race rooms.
    pub(crate) fn role_is_racing(&self, role: Role) -> bool {
        !matches!(self, Self::Pictionary) || matches!(role, Role::Sheikah)
    }

    pub(crate) fn is_racetime_team_format(&self) -> bool {
        self.roles().iter().filter(|&&(role, _)| self.role_is_racing(role)).count() > 1
    }
}

#[derive(Clone)]
pub(crate) struct Data<'a> {
    pub(crate) series: Series,
    pub(crate) event: Cow<'a, str>,
    pub(crate) display_name: String,
    short_name: Option<String>,
    /// The event's originally scheduled starting time, not accounting for the 24-hour deadline extension in the event of an odd number of teams for events with qualifier asyncs.
    pub(crate) base_start: Option<DateTime<Utc>>,
    pub(crate) end: Option<DateTime<Utc>>,
    pub(crate) url: Option<Url>,
    hide_races_tab: bool,
    hide_teams_tab: bool,
    teams_url: Option<Url>,
    enter_url: Option<Url>,
    pub(crate) video_url: Option<Url>,
    pub(crate) discord_guild: Option<GuildId>,
    pub(crate) discord_race_room_channel: Option<ChannelId>,
    pub(crate) discord_race_results_channel: Option<ChannelId>,
    pub(crate) discord_organizer_channel: Option<ChannelId>,
    pub(crate) discord_scheduling_channel: Option<ChannelId>,
    enter_flow: Option<enter::Flow>,
    pub(crate) show_qualifier_times: bool,
    pub(crate) default_game_count: i16,
    pub(crate) min_schedule_notice: Duration,
    pub(crate) language: Language,
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum DataError {
    #[error(transparent)] PgInterval(#[from] crate::util::PgIntervalDecodeError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error("no event with this series and identifier")]
    Missing,
    #[error("team with nonexistent user")]
    NonexistentUser,
}

impl<'a> Data<'a> {
    pub(crate) async fn new(transaction: &mut Transaction<'_, Postgres>, series: Series, event: impl Into<Cow<'a, str>>) -> Result<Option<Data<'a>>, DataError> {
        let event = event.into();
        sqlx::query!(r#"SELECT
            display_name,
            short_name,
            start,
            end_time,
            url,
            hide_races_tab,
            hide_teams_tab,
            teams_url,
            enter_url,
            video_url,
            discord_guild AS "discord_guild: Id",
            discord_race_room_channel AS "discord_race_room_channel: Id",
            discord_race_results_channel AS "discord_race_results_channel: Id",
            discord_organizer_channel AS "discord_organizer_channel: Id",
            discord_scheduling_channel AS "discord_scheduling_channel: Id",
            enter_flow AS "enter_flow: Json<enter::Flow>",
            show_qualifier_times,
            default_game_count,
            min_schedule_notice,
            language AS "language: Language"
        FROM events WHERE series = $1 AND event = $2"#, series as _, &event).fetch_optional(transaction).await?
            .map(|row| Ok::<_, DataError>(Self {
                display_name: row.display_name,
                short_name: row.short_name,
                base_start: row.start,
                end: row.end_time,
                url: row.url.map(|url| url.parse()).transpose()?,
                hide_races_tab: row.hide_races_tab,
                hide_teams_tab: row.hide_teams_tab,
                teams_url: row.teams_url.map(|url| url.parse()).transpose()?,
                enter_url: row.enter_url.map(|url| url.parse()).transpose()?,
                video_url: row.video_url.map(|url| url.parse()).transpose()?,
                discord_guild: row.discord_guild.map(|Id(id)| id.into()),
                discord_race_room_channel: row.discord_race_room_channel.map(|Id(id)| id.into()),
                discord_race_results_channel: row.discord_race_results_channel.map(|Id(id)| id.into()),
                discord_organizer_channel: row.discord_organizer_channel.map(|Id(id)| id.into()),
                discord_scheduling_channel: row.discord_scheduling_channel.map(|Id(id)| id.into()),
                enter_flow: row.enter_flow.map(|Json(flow)| flow),
                show_qualifier_times: row.show_qualifier_times,
                default_game_count: row.default_game_count,
                min_schedule_notice: decode_pginterval(row.min_schedule_notice)?,
                language: row.language,
                series, event,
            }))
            .transpose()
    }

    pub(crate) fn short_name(&self) -> &str {
        self.short_name.as_deref().unwrap_or(&self.display_name)
    }

    pub(crate) async fn chests(&self) -> ChestAppearances {
        macro_rules! from_file {
            ($path:literal) => {{
                static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!($path)).expect("failed to parse chest weights"));

                WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
            }};
        }
        //TODO parse weights at compile time

        match (self.series, &*self.event) {
            (Series::League, "4") => from_file!("../../assets/event/league/chests-4-7.1.94.json"),
            (Series::League, _) => unimplemented!(),
            (Series::MixedPools, "1") => from_file!("../../assets/event/mp/chests-1-6.2.100-fenhl.4.json"),
            (Series::MixedPools, "2") => from_file!("../../assets/event/mp/chests-2-7.1.117-fenhl.17.json"),
            (Series::MixedPools, _) => unimplemented!(),
            (Series::Multiworld, "1" | "2") => ChestAppearances::VANILLA, // CAMC off or classic and no keys in overworld
            (Series::Multiworld, "3") => mw::chests(&Draft {
                high_seed: Id(0), // Draft::complete_randomly doesn't check for active team
                went_first: None,
                skipped_bans: 0,
                settings: HashMap::default(),
            }.complete_randomly(draft::Kind::MultiworldS3).await.unwrap()),
            (Series::Multiworld, _) => unimplemented!(),
            (Series::NineDaysOfSaws, _) => ChestAppearances::VANILLA, // no CAMC in SAWS
            (Series::Pictionary, _) => ChestAppearances::VANILLA, // no CAMC in Pictionary
            (Series::Rsl, "1") => from_file!("../../assets/event/rsl/chests-1-4c526c2.json"),
            (Series::Rsl, "2") => from_file!("../../assets/event/rsl/chests-2-7028072.json"),
            (Series::Rsl, "3") => from_file!("../../assets/event/rsl/chests-3-a0f568b.json"),
            (Series::Rsl, "4") => from_file!("../../assets/event/rsl/chests-4-da4dae5.json"),
            (Series::Rsl, "5") => {
                // rsl/5 moved from version 20cd31a of the RSL script to version 05bfcd2 after the first two races of the first Swiss round.
                // For the sake of simplicity, only the new version is used for chests weights right now.
                //TODO After the event, the version should be randomized based on the total number of races played on each version.
                from_file!("../../assets/event/rsl/chests-5-05bfcd2.json")
            }
            (Series::Rsl, _) => unimplemented!(),
            (Series::Standard, "6") => from_file!("../../assets/event/s/chests-6-6.9.10.json"),
            (Series::Standard, _) => unimplemented!(),
            (Series::TournoiFrancophone, "3") => from_file!("../../assets/event/fr/chests-3-7.1.83-r.1.json"),
            (Series::TournoiFrancophone, _) => unimplemented!(),
            (Series::TriforceBlitz, "2") => from_file!("../../assets/event/tfb/chests-2-7.1.3-blitz.42.json"),
            (Series::TriforceBlitz, _) => unimplemented!(),
        }
    }

    pub(crate) fn is_single_race(&self) -> bool {
        match self.series {
            Series::League => false,
            Series::MixedPools => false,
            Series::Multiworld => false,
            Series::NineDaysOfSaws => true,
            Series::Pictionary => true,
            Series::Rsl => false,
            Series::Standard => false,
            Series::TournoiFrancophone => false,
            Series::TriforceBlitz => false,
        }
    }

    pub(crate) fn team_config(&self) -> TeamConfig {
        match self.series {
            Series::League => TeamConfig::Solo,
            Series::MixedPools => TeamConfig::Solo,
            Series::Multiworld => TeamConfig::Multiworld,
            Series::NineDaysOfSaws => match &*self.event {
                "1" => TeamConfig::Solo,
                "2" => TeamConfig::CoOp,
                "3" => TeamConfig::Solo,
                "4" => TeamConfig::Solo,
                "5" => TeamConfig::Solo,
                "6" => TeamConfig::Multiworld,
                "7" => TeamConfig::Solo,
                "8" => TeamConfig::CoOp,
                "9" => TeamConfig::Solo,
                _ => unimplemented!(),
            },
            Series::Pictionary => TeamConfig::Pictionary,
            Series::Rsl => TeamConfig::Solo,
            Series::Standard => TeamConfig::Solo,
            Series::TournoiFrancophone => TeamConfig::Solo,
            Series::TriforceBlitz => TeamConfig::Solo,
        }
    }

    pub(crate) fn match_source(&self) -> MatchSource {
        match self.url.as_ref().and_then(Url::host_str) {
            //TODO challonge.com support? (waiting for reply from support regarding API errors)
            Some("league.ootrandomizer.com") => MatchSource::League,
            Some("start.gg" | "www.start.gg") => MatchSource::StartGG,
            _ => MatchSource::Manual,
        }
    }

    pub(crate) fn draft_kind(&self) -> Option<draft::Kind> {
        match (self.series, &*self.event) {
            (Series::Multiworld, "3") => Some(draft::Kind::MultiworldS3),
            (Series::TournoiFrancophone, "3") => Some(draft::Kind::TournoiFrancoS3),
            (_, _) => None,
        }
    }

    pub(crate) async fn start(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<Option<DateTime<Utc>>, DataError> {
        Ok(if let Some(mut start) = self.base_start {
            if let Some(max_delay) = sqlx::query_scalar!("SELECT max_delay FROM asyncs WHERE series = $1 AND event = $2 AND kind = 'qualifier'", self.series as _, &self.event).fetch_optional(&mut *transaction).await? {
                let mut num_qualified_teams = 0;
                let mut last_submission_time = None::<DateTime<Utc>>;
                let mut teams = sqlx::query_scalar!(r#"SELECT submitted AS "submitted!" FROM teams LEFT OUTER JOIN async_teams ON (id = team) WHERE
                    series = $1
                    AND event = $2
                    AND NOT resigned
                    AND submitted IS NOT NULL
                    AND kind = 'qualifier'
                "#, self.series as _, &self.event).fetch(transaction);
                while let Some(submitted) = teams.try_next().await? {
                    num_qualified_teams += 1;
                    last_submission_time = Some(if let Some(last_submission_time) = last_submission_time {
                        last_submission_time.max(submitted)
                    } else {
                        submitted
                    });
                }
                if num_qualified_teams % 2 == 0 {
                    if let Some(last_submission_time) = last_submission_time {
                        start = start.max(last_submission_time);
                    }
                } else {
                    if start <= Utc::now() {
                        start += chrono::Duration::from_std(decode_pginterval(max_delay)?).expect("max delay on async too long");
                    }
                }
            }
            Some(start)
        } else {
            None
        })
    }

    pub(crate) async fn is_started(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<bool, DataError> {
        Ok(self.start(transaction).await?.map_or(false, |start| start <= Utc::now()))
    }

    fn is_ended(&self) -> bool {
        self.end.map_or(false, |end| end <= Utc::now())
    }

    pub(crate) async fn organizers(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<Vec<User>, Error> {
        let mut buf = Vec::<User>::default();
        for id in sqlx::query_scalar!(r#"SELECT organizer AS "organizer: Id" FROM organizers WHERE series = $1 AND event = $2"#, self.series as _, &self.event).fetch_all(&mut *transaction).await? {
            let user = User::from_id(&mut *transaction, id).await?.ok_or(Error::OrganizerUserData)?;
            let (Ok(idx) | Err(idx)) = buf.binary_search_by(|probe| probe.display_name().cmp(user.display_name()).then_with(|| probe.id.cmp(&user.id)));
            buf.insert(idx, user);
        }
        Ok(buf)
    }

    pub(crate) async fn restreamers(&self, transaction: &mut Transaction<'_, Postgres>) -> Result<Vec<User>, Error> {
        let mut buf = Vec::<User>::default();
        for id in sqlx::query_scalar!(r#"SELECT restreamer AS "restreamer: Id" FROM restreamers WHERE series = $1 AND event = $2"#, self.series as _, &self.event).fetch_all(&mut *transaction).await? {
            let user = User::from_id(&mut *transaction, id).await?.ok_or(Error::RestreamerUserData)?;
            let (Ok(idx) | Err(idx)) = buf.binary_search_by(|probe| probe.display_name().cmp(user.display_name()).then_with(|| probe.id.cmp(&user.id)));
            buf.insert(idx, user);
        }
        Ok(buf)
    }

    pub(crate) async fn active_async(&self, transaction: &mut Transaction<'_, Postgres>, team_id: Option<Id>) -> Result<Option<AsyncKind>, DataError> {
        for kind in sqlx::query_scalar!(r#"SELECT kind AS "kind: AsyncKind" FROM asyncs WHERE series = $1 AND event = $2"#, self.series as _, &self.event).fetch_all(&mut *transaction).await? {
            match kind {
                AsyncKind::Qualifier => if !self.is_started(&mut *transaction).await? {
                    return Ok(Some(kind))
                },
                AsyncKind::Tiebreaker1 | AsyncKind::Tiebreaker2 => if let Some(team_id) = team_id {
                    if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM async_teams WHERE team = $1 AND kind = $2) AS "exists!""#, team_id as _, kind as _).fetch_one(&mut *transaction).await? {
                        return Ok(Some(kind))
                    }
                },
            }
        }
        Ok(None)
    }

    pub(crate) async fn signups_sorted(&self, transaction: &mut Transaction<'_, Postgres>, me: Option<&User>, show_qualifier_times: bool) -> Result<Vec<(Team, Vec<(Role, User, bool, Option<Duration>, Option<String>)>, bool, Option<i16>)>, DataError> {
        let teams = sqlx::query!(r#"SELECT id AS "id!: Id", name, racetime_slug, plural_name, submitted IS NOT NULL AS "qualified!", pieces, restream_consent FROM teams LEFT OUTER JOIN async_teams ON (id = team) WHERE
            series = $1
            AND event = $2
            AND NOT resigned
            AND (
                EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                OR NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            )
            AND (kind = 'qualifier' OR kind IS NULL)
        "#, self.series as _, &self.event, me.as_ref().map(|me| i64::from(me.id))).fetch_all(&mut *transaction).await?;
        let roles = self.team_config().roles();
        let mut signups = Vec::with_capacity(teams.len());
        for team in teams {
            let mut members = Vec::with_capacity(roles.len());
            for &(role, _) in roles {
                let row = sqlx::query!(r#"
                    SELECT member AS "id: Id", status AS "status: SignupStatus", time, vod
                    FROM team_members LEFT OUTER JOIN async_players ON (member = player AND series = $1 AND event = $2 AND kind = 'qualifier')
                    WHERE team = $3 AND role = $4
                "#, self.series as _, &self.event, team.id as _, role as _).fetch_one(&mut *transaction).await?;
                let is_confirmed = row.status.is_confirmed();
                let user = User::from_id(&mut *transaction, row.id).await?.ok_or(DataError::NonexistentUser)?;
                members.push((role, user, is_confirmed, row.time.map(decode_pginterval).transpose()?, row.vod));
            }
            signups.push((Team { id: team.id, name: team.name, racetime_slug: team.racetime_slug, plural_name: team.plural_name, restream_consent: team.restream_consent }, members, team.qualified, team.pieces));
        }
        if show_qualifier_times {
            signups.sort_unstable_by(|(team1, members1, qualified1, pieces1), (team2, members2, qualified2, pieces2)| {
                #[derive(PartialEq, Eq, PartialOrd, Ord)]
                enum Qualification {
                    Finished(Option<i16>, Duration),
                    DidNotFinish,
                    NotYetQualified,
                }

                impl Qualification {
                    fn new(qualified: bool, pieces: Option<i16>, members: &[(Role, User, bool, Option<Duration>, Option<String>)]) -> Self {
                        if qualified {
                            if let Some(time) = members.iter().try_fold(Duration::default(), |acc, &(_, _, _, time, _)| Some(acc + time?)) {
                                Self::Finished(
                                    pieces.map(|pieces| -pieces), // list teams with more pieces first
                                    time,
                                )
                            } else {
                                Self::DidNotFinish
                            }
                        } else {
                            Self::NotYetQualified
                        }
                    }
                }

                Qualification::new(*qualified1, *pieces1, members1).cmp(&Qualification::new(*qualified2, *pieces2, members2))
                .then_with(|| team1.cmp(team2))
            });
        } else {
            signups.sort_unstable_by(|(team1, _, qualified1, _), (team2, _, qualified2, _)|
                qualified2.cmp(qualified1) // reversed to list qualified teams first
                .then_with(|| team1.cmp(team2))
            );
        }
        Ok(signups)
    }

    pub(crate) async fn header(&self, transaction: &mut Transaction<'_, Postgres>, env: Environment, me: Option<&User>, tab: Tab, is_subpage: bool) -> Result<RawHtml<String>, Error> {
        let signed_up = if let Some(me) = me {
            sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                id = team
                AND series = $1
                AND event = $2
                AND member = $3
                AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
            ) AS "exists!""#, self.series as _, &self.event, me.id as _).fetch_one(&mut *transaction).await?
        } else {
            false
        };
        Ok(html! {
            h1 {
                a(class = "nav", href? = (!matches!(tab, Tab::Info) || is_subpage).then(|| uri!(info(self.series, &*self.event)).to_string())) : &self.display_name;
            }
            @if let Some(start) = self.start(&mut *transaction).await? {
                h2 : format_datetime(start, DateTimeFormat { long: true, running_text: false });
            }
            div(class = "button-row") {
                @if let Tab::Info = tab {
                    a(class = "button selected", href? = is_subpage.then(|| uri!(info(self.series, &*self.event)).to_string())) : "Info";
                } else {
                    a(class = "button", href = uri!(info(self.series, &*self.event)).to_string()) : "Info";
                }
                @let teams_label = if let TeamConfig::Solo = self.team_config() { "Entrants" } else { "Teams" };
                @if !self.hide_teams_tab {
                    @if let Tab::Teams = tab {
                        a(class = "button selected", href? = is_subpage.then(|| uri!(teams(self.series, &*self.event)).to_string())) : teams_label;
                    } else if let Some(ref teams_url) = self.teams_url {
                        a(class = "button", href = teams_url.to_string()) {
                            : favicon(teams_url);
                            : teams_label;
                        }
                    } else {
                        a(class = "button", href = uri!(teams(self.series, &*self.event)).to_string()) : teams_label;
                    }
                }
                @if !self.hide_races_tab && !self.is_single_race() {
                    @if let Tab::Races = tab {
                        a(class = "button selected", href? = is_subpage.then(|| uri!(races(self.series, &*self.event)).to_string())) : "Races";
                    } else {
                        a(class = "button", href = uri!(races(self.series, &*self.event)).to_string()) : "Races";
                    }
                }
                @if signed_up {
                    @if let Tab::MyStatus = tab {
                        a(class = "button selected", href? = is_subpage.then(|| uri!(status(self.series, &*self.event)).to_string())) : "My Status";
                    } else {
                        a(class = "button", href = uri!(status(self.series, &*self.event)).to_string()) : "My Status";
                    }
                } else if !self.is_started(transaction).await? {
                    @if let Tab::Enter = tab {
                        a(class = "button selected", href? = is_subpage.then(|| uri!(enter::get(self.series, &*self.event, _, _)).to_string())) : "Enter";
                    } else if let Some(ref enter_url) = self.enter_url {
                        a(class = "button", href = enter_url.to_string()) {
                            : favicon(enter_url);
                            : "Enter";
                        }
                    } else {
                        a(class = "button", href = uri!(enter::get(self.series, &*self.event, _, _)).to_string()) : "Enter";
                    }
                    @if !matches!(self.team_config(), TeamConfig::Solo) {
                        @if let Tab::FindTeam = tab {
                            a(class = "button selected", href? = is_subpage.then(|| uri!(find_team(self.series, &*self.event)).to_string())) : "Find Teammates";
                        } else {
                            a(class = "button", href = uri!(find_team(self.series, &*self.event)).to_string()) : "Find Teammates";
                        }
                    }
                }
                @if let Some(goal) = racetime_bot::Goal::for_event(self.series, &self.event) {
                    @if goal.is_custom() { //TODO also support non-custom goals, needs either a list of the internal goal IDs or an adjustment to the startrace page's GET parameter parsing
                        @let mut practice_url = Url::parse(&format!("https://{}/{}/startrace", env.racetime_host(), racetime_bot::CATEGORY))?;
                        @let practice_url = practice_url
                            .query_pairs_mut()
                            .append_pair(if goal.is_custom() { "custom_goal" } else { "goal" }, goal.as_str())
                            .extend_pairs(self.team_config().is_racetime_team_format().then_some([("team_race", "1"), ("require_even_teams", "1")]).into_iter().flatten())
                            .append_pair("hide_comments", "1")
                            .finish();
                        a(class = "button", href = practice_url.to_string()) {
                            : favicon(practice_url);
                            : "Practice";
                        }
                    }
                }
                @if matches!(self.series, Series::League | Series::TriforceBlitz) && !self.is_ended() {
                    @if let Tab::Volunteer = tab {
                        a(class = "button selected", href? = is_subpage.then(|| uri!(volunteer(self.series, &*self.event)).to_string())) : "Volunteer";
                    } else {
                        a(class = "button", href = uri!(volunteer(self.series, &*self.event)).to_string()) : "Volunteer";
                    }
                }
                @if let Some(ref video_url) = self.video_url {
                    a(class = "button", href = video_url.to_string()) {
                        : favicon(video_url);
                        : "Watch";
                    }
                }
                @if let Some(ref url) = self.url {
                    a(class = "button", href = url.to_string()) {
                        : favicon(url);
                        @match url.host_str() {
                            Some("racetime.gg") => : "Race Room";
                            Some("challonge.com" | "www.challonge.com" | "start.gg" | "www.start.gg") => : "Brackets";
                            _ => : "Website";
                        }
                    }
                }
            }
        })
    }
}

impl ToHtml for Data<'_> {
    fn to_html(&self) -> RawHtml<String> {
        html! {
            a(href = uri!(info(self.series, &*self.event)).to_string()) : self.display_name;
        }
    }
}

pub(crate) enum Tab {
    Info,
    Teams,
    Races,
    MyStatus,
    Enter,
    FindTeam,
    Volunteer,
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Calendar(#[from] cal::Error),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Discord(#[from] crate::discord_bot::Error),
    #[error(transparent)] Io(#[from] io::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] SeedData(#[from] seed::ExtraDataError),
    #[error(transparent)] Serenity(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Url(#[from] url::ParseError),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("missing user data for an event organizer")]
    OrganizerUserData,
    #[error("missing user data for a restreamer")]
    RestreamerUserData,
}

impl<E: Into<Error>> From<E> for StatusOrError<Error> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum InfoError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Event(#[from] Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] SeedData(#[from] seed::ExtraDataError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

impl<E: Into<InfoError>> From<E> for StatusOrError<InfoError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::get("/event/<series>/<event>")]
pub(crate) async fn info(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<InfoError>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, **env, me.as_ref(), Tab::Info, false).await?;
    let content = match data.series {
        Series::League => league::info(&mut transaction, &data).await?,
        Series::MixedPools => mp::info(&mut transaction, &data).await?,
        Series::Multiworld => mw::info(&mut transaction, &data).await?,
        Series::NineDaysOfSaws => Some(ndos::info(&mut transaction, &data).await?),
        Series::Pictionary => pic::info(&mut transaction, &data).await?,
        Series::Rsl => rsl::info(&mut transaction, &data).await?,
        Series::Standard => s::info(event),
        Series::TournoiFrancophone => fr::info(&mut transaction, &data).await?,
        Series::TriforceBlitz => tfb::info(&mut transaction, &data).await?,
    };
    let content = html! {
        : header;
        @if let Some(content) = content {
            : content;
        } else if let Some(organizers) = English.join_html(data.organizers(&mut transaction).await?) {
            article {
                p {
                    : "This event ";
                    @if data.is_ended() {
                        : "was";
                    } else {
                        : "is";
                    }
                    : " organized by ";
                    : organizers;
                    : ".";
                }
            }
        } else {
            article {
                p : "No information about this event available yet.";
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &data.display_name, content).await?)
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum TeamsError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Event(#[from] Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] PgInterval(#[from] crate::util::PgIntervalDecodeError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

impl<E: Into<TeamsError>> From<E> for StatusOrError<TeamsError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::get("/event/<series>/<event>/teams")]
pub(crate) async fn teams(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<TeamsError>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, **env, me.as_ref(), Tab::Teams, false).await?;
    let has_qualifier = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM asyncs WHERE series = $1 AND event = $2 AND kind = 'qualifier') AS "exists!""#, series as _, event).fetch_one(&mut transaction).await?;
    let show_qualifier_times = data.show_qualifier_times && (
        sqlx::query_scalar!(r#"SELECT submitted IS NOT NULL AS "qualified!" FROM async_teams, team_members WHERE async_teams.team = team_members.team AND member = $1 AND kind = 'qualifier'"#, me.as_ref().map(|me| i64::from(me.id))).fetch_optional(&mut *transaction).await?.unwrap_or(false)
        || data.is_started(&mut transaction).await?
    );
    let show_restream_consent = if let Some(ref me) = me {
        data.organizers(&mut transaction).await?.contains(me) || data.restreamers(&mut transaction).await?.contains(me)
    } else {
        false
    };
    let roles = data.team_config().roles();
    let signups = data.signups_sorted(&mut transaction, me.as_ref(), show_qualifier_times).await?;
    let mut footnotes = Vec::default();
    let teams_label = if let TeamConfig::Solo = data.team_config() { "Entrants" } else { "Teams" };
    let content = html! {
        : header;
        table {
            thead {
                tr {
                    @if !matches!(data.team_config(), TeamConfig::Solo) {
                        th : "Team Name";
                    }
                    @for &(role, display_name) in roles {
                        th(class = role.css_class()) : display_name;
                    }
                    @if has_qualifier {
                        @if show_qualifier_times {
                            @if series == Series::TriforceBlitz {
                                th : "Pieces Found";
                            }
                        } else {
                            th : "Qualified";
                        }
                    }
                    @if show_restream_consent {
                        th : "Restream Consent";
                    }
                }
            }
            tbody {
                @if signups.is_empty() {
                    tr {
                        td(colspan =
                            if let TeamConfig::Solo = data.team_config() { 0 } else { 1 } + roles.len()
                            + if has_qualifier { if show_qualifier_times { if series == Series::TriforceBlitz { 1 } else { 0 } } else { 1 } } else { 0 }
                        ) {
                            i : "(no signups yet)";
                        }
                    }
                } else {
                    @for (team, members, qualified, pieces) in signups {
                        tr {
                            @if !matches!(data.team_config(), TeamConfig::Solo) {
                                td {
                                    : team.to_html(&mut transaction, **env, false).await?;
                                    @if show_qualifier_times && qualified {
                                        br;
                                        small {
                                            @if let Some(time) = members.iter().try_fold(Duration::default(), |acc, &(_, _, _, time, _)| Some(acc + time?)) {
                                                : format_duration(time / u32::try_from(members.len()).expect("too many team members"), false);
                                            } else {
                                                : "DNF";
                                            }
                                        }
                                    }
                                }
                            }
                            @for (role, user, is_confirmed, qualifier_time, qualifier_vod) in &members {
                                td(class? = role.css_class()) {
                                    : user;
                                    @if *is_confirmed {
                                        @if me.as_ref().map_or(false, |me| me == user) && members.iter().any(|(_, _, is_confirmed, _, _)| !is_confirmed) {
                                            : " ";
                                            span(class = "button-row") {
                                                form(action = uri!(resign_post(series, event, team.id)).to_string(), method = "post") {
                                                    : csrf;
                                                    input(type = "submit", value = "Retract");
                                                }
                                            }
                                        }
                                    } else {
                                        : " ";
                                        @if me.as_ref().map_or(false, |me| me == user) {
                                            span(class = "button-row") {
                                                form(action = uri!(confirm_signup(series, event, team.id)).to_string(), method = "post") {
                                                    : csrf;
                                                    input(type = "submit", value = "Accept");
                                                }
                                                form(action = uri!(resign_post(series, event, team.id)).to_string(), method = "post") {
                                                    : csrf;
                                                    input(type = "submit", value = "Decline");
                                                }
                                                //TODO options to block sender or event
                                            }
                                        } else {
                                            : "(unconfirmed)";
                                        }
                                    }
                                    @if show_qualifier_times && qualified {
                                        br;
                                        small {
                                            @let time = if let Some(time) = qualifier_time { format_duration(*time, false) } else { format!("DNF") }; //TODO include number of pieces found in Triforce Blitz
                                            @if let Some(vod) = qualifier_vod {
                                                @if let Some(Ok(vod_url)) = (!vod.contains(' ')).then(|| Url::parse(vod)) {
                                                    a(href = vod_url.to_string()) : time;
                                                } else {
                                                    : time;
                                                    sup {
                                                        @let footnote_id = { footnotes.push(vod.clone()); footnotes.len() };
                                                        a(href = format!("#footnote{footnote_id}")) {
                                                            : "[";
                                                            : footnote_id;
                                                            : "]";
                                                        }
                                                    };
                                                }
                                            } else {
                                                : time;
                                            }
                                        }
                                    }
                                }
                            }
                            @if has_qualifier {
                                @if show_qualifier_times {
                                    @if series == Series::TriforceBlitz {
                                        td : pieces;
                                    }
                                } else {
                                    td {
                                        @if qualified {
                                            : "✓";
                                        }
                                    }
                                }
                            }
                            @if show_restream_consent {
                                td {
                                    @if team.restream_consent {
                                        : "✓";
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        @for (i, footnote) in footnotes.into_iter().enumerate() {
            p(id = format!("footnote{}", i + 1)) {
                : "[";
                : i + 1;
                : "]";
                @for word in footnote.split(' ') {
                    : " ";
                    @if let Ok(word_url) = Url::parse(word) {
                        a(href = word_url.to_string()) : word;
                    } else {
                        : word;
                    }
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("{teams_label} — {}", data.display_name), content).await?)
}

#[rocket::get("/event/<series>/<event>/races")]
pub(crate) async fn races(discord_ctx: &State<RwFuture<DiscordCtx>>, env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    async fn race_table(transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, env: Environment, http_client: &reqwest::Client, data: &Data<'_>, show_multistreams: bool, can_create: bool, can_edit: bool, show_restream_consent: bool, races: &[Race]) -> Result<RawHtml<String>, Error> {
        let has_games = races.iter().any(|race| race.game.is_some());
        let has_seeds = races.iter().any(|race| race.seed.is_some());
        let has_buttons = can_create || can_edit;
        let now = Utc::now();
        Ok(html! {
            table {
                thead {
                    tr {
                        th : "Start";
                        th : "Round";
                        @if has_games {
                            th : "Game";
                        }
                        th(colspan = "6") : "Entrants";
                        th : "Links";
                        @if has_seeds {
                            : seed::table_header_cells(true);
                        }
                        @if show_restream_consent {
                            th : "Restream Consent";
                        }
                        @if has_buttons {
                            th {
                                @if can_create {
                                    a(class = "button", href = uri!(crate::cal::create_race(races[0].series, &*races[0].event)).to_string()) : "New Race";
                                }
                            }
                        }
                    }
                }
                tbody {
                    @for race in races {
                        tr {
                            td {
                                @match race.schedule {
                                    RaceSchedule::Unscheduled => {}
                                    RaceSchedule::Live { start, .. } => : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                    RaceSchedule::Async { .. } => : "(async)";
                                }
                            }
                            td {
                                : race.phase;
                                : " ";
                                : race.round;
                            }
                            @if has_games {
                                td {
                                    @if let Some(game) = race.game {
                                        : game;
                                    }
                                }
                            }
                            @match race.entrants {
                                Entrants::Open => td(colspan = "6") : "(open)";
                                Entrants::Count { total, finished } => td(colspan = "6") {
                                    : total;
                                    : " (";
                                    : finished;
                                    : " finishers)";
                                }
                                Entrants::Named(ref entrants) => td(colspan = "6") : entrants;
                                Entrants::Two([ref team1, ref team2]) => {
                                    td(class = "vs1", colspan = "3") {
                                        : team1.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                        @if let RaceSchedule::Async { start1: Some(start), .. } = race.schedule {
                                            br;
                                            small {
                                                : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                            }
                                        }
                                    }
                                    td(class = "vs2", colspan = "3") {
                                        : team2.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                        @if let RaceSchedule::Async { start2: Some(start), .. } = race.schedule {
                                            br;
                                            small {
                                                : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                            }
                                        }
                                    }
                                }
                                Entrants::Three([ref team1, ref team2, ref team3]) => {
                                    td(colspan = "2") : team1.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                    td(colspan = "2") : team2.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                    td(colspan = "2") : team3.to_html(&mut *transaction, env, discord_ctx, false).await?;
                                }
                            }
                            td {
                                div(class = "favicon-container") {
                                    @for video_url in race.video_urls.values() {
                                        a(class = "favicon", href = video_url.to_string()) : favicon(video_url);
                                    }
                                    @if show_multistreams && race.video_urls.is_empty() {
                                        @if let Some(multistream_url) = race.multistream_url(&mut *transaction, env, http_client, data).await? {
                                            a(class = "favicon", href = multistream_url.to_string()) : favicon(&multistream_url);
                                        }
                                    }
                                    @if let Some(startgg_url) = race.startgg_set_url()? {
                                        a(class = "favicon", href = startgg_url.to_string()) : favicon(&startgg_url);
                                    }
                                    @for room in race.rooms() {
                                        //TODO hide room of 1st async half until 2nd half finished
                                        a(class = "favicon", href = room.to_string()) : favicon(&room);
                                    }
                                }
                            }
                            @if has_seeds {
                                @if let Some(ref seed) = race.seed {
                                    //TODO hide seed if unfinished async
                                    : seed::table_cells(now, seed, true, can_edit.then(|| uri!(cal::add_file_hash(race.series, &*race.event, race.id)))).await?;
                                } else {
                                    : seed::table_empty_cells(true);
                                }
                            }
                            @if show_restream_consent {
                                td {
                                    @if race.teams().all(|team| team.restream_consent) {
                                        : "✓";
                                    }
                                }
                            }
                            @if has_buttons {
                                td {
                                    @if can_edit {
                                        a(class = "button", href = uri!(crate::cal::edit_race(race.series, &race.event, race.id)).to_string()) : "Edit";
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, **env, me.as_ref(), Tab::Races, false).await?;
    let (mut past_races, ongoing_and_upcoming_races) = Race::for_event(&mut transaction, http_client, env, config, &data).await?
        .into_iter()
        .partition::<Vec<_>, _>(|race| race.schedule.is_ended());
    past_races.reverse();
    let any_races_ongoing_or_upcoming = !ongoing_and_upcoming_races.is_empty();
    let (can_create, show_restream_consent, can_edit) = if let Some(ref me) = me {
        let can_create = data.organizers(&mut transaction).await?.contains(me);
        let show_restream_consent = can_create || data.restreamers(&mut transaction).await?.contains(me);
        let can_edit = show_restream_consent || me.is_archivist;
        (can_create, show_restream_consent, can_edit)
    } else {
        (false, false, false)
    };
    let content = html! {
        : header;
        //TODO copiable calendar link (with link to index for explanation?)
        @if any_races_ongoing_or_upcoming {
            //TODO split into ongoing and upcoming, show headers for both
            : race_table(&mut transaction, &*discord_ctx.read().await, **env, http_client, &data, true, can_create, can_edit, show_restream_consent, &ongoing_and_upcoming_races).await?;
        }
        @if !past_races.is_empty() {
            @if any_races_ongoing_or_upcoming {
                h2 : "Past races";
            }
            : race_table(&mut transaction, &*discord_ctx.read().await, **env, http_client, &data, false, can_create && !any_races_ongoing_or_upcoming, can_edit, false, &past_races).await?;
        } else if can_create && !any_races_ongoing_or_upcoming {
            div(class = "button-row") {
                a(class = "button", href = uri!(crate::cal::create_race(series, &event)).to_string()) : "New Race";
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("Races — {}", data.display_name), content).await?)
}

pub(crate) enum StatusContext<'v> {
    None,
    RequestAsync(Context<'v>),
    SubmitAsync(Context<'v>),
    Edit(Context<'v>),
}

impl<'v> StatusContext<'v> {
    pub(crate) fn take_request_async(&mut self) -> Context<'v> {
        match mem::replace(self, Self::None) {
            Self::RequestAsync(ctx) => ctx,
            old_val => {
                *self = old_val;
                Context::default()
            }
        }
    }

    pub(crate) fn take_submit_async(&mut self) -> Context<'v> {
        match mem::replace(self, Self::None) {
            Self::SubmitAsync(ctx) => ctx,
            old_val => {
                *self = old_val;
                Context::default()
            }
        }
    }
    fn take_edit(&mut self) -> Context<'v> {
        match mem::replace(self, Self::None) {
            Self::Edit(ctx) => ctx,
            old_val => {
                *self = old_val;
                Context::default()
            }
        }
    }
}

async fn status_page(mut transaction: Transaction<'_, Postgres>, env: Environment, discord_ctx: &DiscordCtx, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, data: Data<'_>, mut ctx: StatusContext<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, env, me.as_ref(), Tab::MyStatus, false).await?;
    let content = if let Some(ref me) = me {
        if let Some(row) = sqlx::query!(r#"SELECT id AS "id: Id", name, racetime_slug, role AS "role: Role", resigned, restream_consent FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, data.series as _, &data.event, me.id as _).fetch_optional(&mut transaction).await? {
            html! {
                : header;
                @if !matches!(data.team_config(), TeamConfig::Solo) {
                    p {
                        : "You are signed up as part of ";
                        //TODO use Team type
                        @if let Some(racetime_slug) = row.racetime_slug {
                            a(href = format!("https://racetime.gg/team/{racetime_slug}")) {
                                @if let Some(name) = row.name {
                                    i : name;
                                } else {
                                    : "an unnamed team";
                                }
                            }
                        } else {
                            @if let Some(name) = row.name {
                                i : name;
                            } else {
                                : "an unnamed team";
                            }
                        }
                        //TODO list teammates
                        : ".";
                    }
                }
                @if row.resigned {
                    p : "You have resigned from this event.";
                } else {
                    @match data.series {
                        Series::League => @unimplemented // no signups on Mido's House
                        Series::MixedPools => @unimplemented // no signups on Mido's House
                        Series::Multiworld => : mw::status(&mut transaction, discord_ctx, csrf, &data, row.id, &mut ctx).await?;
                        Series::NineDaysOfSaws => @if data.is_ended() {
                            p : "This race has been completed."; //TODO ranking and finish time
                        } else if let Some(ref race_room) = data.url {
                            p {
                                : "Please join ";
                                a(href = race_room.to_string()) : "the race room";
                                : " as soon as possible. You will receive further instructions there.";
                            }
                        } else {
                            : "Waiting for the race room to be opened, which should happen around 30 minutes before the scheduled starting time. Keep an eye out for an announcement on Discord.";
                        }
                        Series::Pictionary => @if data.is_ended() {
                            p : "This race has been completed."; //TODO ranking and finish time
                        } else if let Some(ref race_room) = data.url {
                            @match row.role.try_into().expect("non-Pictionary role in Pictionary team") {
                                pic::Role::Sheikah => p {
                                    : "Please join ";
                                    a(href = race_room.to_string()) : "the race room";
                                    : " as soon as possible. You will receive further instructions there.";
                                }
                                pic::Role::Gerudo => p {
                                    : "Please keep an eye on ";
                                    a(href = race_room.to_string()) : "the race room";
                                    : " (but do not join). The spoiler log will be posted there.";
                                }
                            }
                        } else {
                            : "Waiting for the race room to be opened, which should happen around 30 minutes before the scheduled starting time. Keep an eye out for an announcement on Discord.";
                        }
                        Series::Rsl => @unimplemented // no signups on Mido's House
                        Series::Standard => @unimplemented // no signups on Mido's House
                        Series::TournoiFrancophone => p : "Planifiez vos matches dans les fils du canal dédié.";
                        Series::TriforceBlitz => : tfb::status(&mut transaction, csrf, &data, Some(row.id), &mut ctx).await?;
                    }
                    @if !data.is_ended() {
                        h2 : "Options";
                        @let ctx = ctx.take_edit();
                        @let mut errors = ctx.errors().collect_vec();
                        : full_form(uri!(status_post(data.series, &*data.event)), csrf, html! {
                            : form_field("restream_consent", &mut errors, html! {
                                input(type = "checkbox", id = "restream_consent", name = "restream_consent", checked? = ctx.field_value("restream_consent").map_or(row.restream_consent, |value| value == "on"));
                                label(for = "restream_consent") {
                                    @if let TeamConfig::Solo = data.team_config() {
                                        : "I am okay with being restreamed.";
                                    } else {
                                        : "We are okay with being restreamed.";
                                    }
                                }
                            });
                            //TODO options to change team name or swap roles
                        }, errors, "Save");
                        p {
                            a(href = uri!(resign(data.series, &*data.event, row.id)).to_string()) : "Resign";
                        }
                    }
                }
            }
        } else {
            html! {
                : header;
                article {
                    p : "You are not signed up for this event.";
                    p : "You can accept, decline, or retract unconfirmed team invitations on the teams page.";
                }
            }
        }
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(status(data.series, &*data.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to view your status for this event.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("My Status — {}", data.display_name), content).await?)
}

#[rocket::get("/event/<series>/<event>/status")]
pub(crate) async fn status(pool: &State<PgPool>, env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(status_page(transaction, **env, &*discord_ctx.read().await, me, uri, csrf.as_ref(), data, StatusContext::None).await?)
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct StatusForm {
    #[field(default = String::new())]
    csrf: String,
    restream_consent: bool,
}

#[rocket::post("/event/<series>/<event>/status", data = "<form>")]
pub(crate) async fn status_post(env: &State<Environment>, config: &State<Config>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, StatusForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_ended() {
        form.context.push_error(form::Error::validation("This event has already ended."));
    }
    let row = sqlx::query!(r#"SELECT id AS "id: Id", restream_consent FROM teams, team_members WHERE
        id = team
        AND series = $1
        AND event = $2
        AND member = $3
        AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        AND NOT resigned
    "#, data.series as _, &data.event, me.id as _).fetch_one(&mut transaction).await?;
    Ok(if let Some(ref value) = form.value {
        if row.restream_consent && !value.restream_consent {
            //TODO check if restream consent can still be revoked according to tournament rules, offer to resign if not
            if Race::for_event(&mut transaction, http_client, env, config, &data).await?.into_iter().any(|race| !race.schedule.is_ended() && !race.video_urls.is_empty()) {
                form.context.push_error(form::Error::validation("There is a restream planned for one of your upcoming races. Please contact an event organizer if you would like to cancel.").with_name("restream_consent"));
            }
        }
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(status_page(transaction, **env, &*discord_ctx.read().await, Some(me), uri, csrf.as_ref(), data, StatusContext::Edit(form.context)).await?)
        } else {
            sqlx::query!("UPDATE teams SET restream_consent = $1 WHERE id = $2", value.restream_consent, row.id as _).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(status(series, event))))
        }
    } else {
        RedirectOrContent::Content(status_page(transaction, **env, &*discord_ctx.read().await, Some(me), uri, csrf.as_ref(), data, StatusContext::Edit(form.context)).await?)
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum FindTeamError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Event(#[from] Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("unknown user")]
    UnknownUser,
}

impl<E: Into<FindTeamError>> From<E> for StatusOrError<FindTeamError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

async fn find_team_form(mut transaction: Transaction<'_, Postgres>, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, data: Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, FindTeamError> {
    Ok(match data.team_config() {
        TeamConfig::Solo => {
            let header = data.header(&mut transaction, env, me.as_ref(), Tab::FindTeam, false).await?;
            page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("Find Teammates — {}", data.display_name), html! {
                : header;
                : "This is a solo event.";
            }).await?
        }
        TeamConfig::CoOp => ndos::coop_find_team_form(transaction, env, me, uri, csrf, data, ctx).await?,
        TeamConfig::Pictionary => pic::find_team_form(transaction, env, me, uri, csrf, data, ctx).await?,
        TeamConfig::Multiworld => mw::find_team_form(transaction, env, me, uri, csrf, data, ctx).await?,
    })
}

#[rocket::get("/event/<series>/<event>/find-team")]
pub(crate) async fn find_team(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<FindTeamError>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(find_team_form(transaction, **env, me, uri, csrf.as_ref(), data, Context::default()).await?)
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct FindTeamForm {
    #[field(default = String::new())]
    csrf: String,
    #[field(default = String::new())]
    availability: String,
    #[field(default = String::new())]
    notes: String,
    role: Option<pic::RolePreference>,
}

#[rocket::post("/event/<series>/<event>/find-team", data = "<form>")]
pub(crate) async fn find_team_post(pool: &State<PgPool>, env: &State<Environment>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, FindTeamForm>>) -> Result<RedirectOrContent, StatusOrError<FindTeamError>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started(&mut transaction).await? {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM looking_for_team WHERE
            series = $1
            AND event = $2
            AND user_id = $3
        ) AS "exists!""#, series as _, event, me.id as _).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("You are already on the list."));
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, series as _, event, me.id as _).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("You are already signed up for this event."));
        }
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(find_team_form(transaction, **env, Some(me), uri, csrf.as_ref(), data, form.context).await?)
        } else {
            sqlx::query!("INSERT INTO looking_for_team (series, event, user_id, role, availability, notes) VALUES ($1, $2, $3, $4, $5, $6)", series as _, event, me.id as _, value.role.unwrap_or_default() as _, value.availability, value.notes).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(find_team(series, event))))
        }
    } else {
        RedirectOrContent::Content(find_team_form(transaction, **env, Some(me), uri, csrf.as_ref(), data, form.context).await?)
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum AcceptError {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Discord(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("failed to verify CSRF token")]
    Csrf,
    #[error("you can no longer enter this event since it has already started")]
    EventStarted,
    #[error("you haven't been invited to this team")]
    NotInTeam,
    #[error("a racetime.gg account is required to enter as runner")]
    RaceTimeAccountRequired,
}

impl<E: Into<AcceptError>> From<E> for StatusOrError<AcceptError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::post("/event/<series>/<event>/confirm/<team>", data = "<form>")]
pub(crate) async fn confirm_signup(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, team: Id, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, EmptyForm>>) -> Result<Redirect, StatusOrError<AcceptError>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if form.context.errors().next().is_some() { return Err(AcceptError::Csrf.into()) }
    if data.is_started(&mut transaction).await? { return Err(AcceptError::EventStarted.into()) }
    if let Some(role) = sqlx::query_scalar!(r#"SELECT role AS "role: Role" FROM team_members WHERE team = $1 AND member = $2 AND status = 'unconfirmed'"#, team as _, me.id as _).fetch_optional(&mut transaction).await? {
        if role == Role::Sheikah && me.racetime.is_none() {
            return Err(AcceptError::RaceTimeAccountRequired.into())
        }
        for member in sqlx::query_scalar!(r#"SELECT member AS "id: Id" FROM team_members WHERE team = $1 AND (status = 'created' OR status = 'confirmed')"#, team as _).fetch_all(&mut transaction).await? {
            let id = Id::new(&mut transaction, IdTable::Notifications).await?;
            sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, 'accept', $3, $4, $5)", id as _, member as _, series as _, event, me.id as _).execute(&mut transaction).await?;
        }
        sqlx::query!("UPDATE team_members SET status = 'confirmed' WHERE team = $1 AND member = $2", team as _, me.id as _).execute(&mut transaction).await?;
        if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND status = 'unconfirmed') AS "exists!""#, team as _).fetch_one(&mut transaction).await? {
            // this confirms the team
            // remove all members from looking_for_team
            sqlx::query!("DELETE FROM looking_for_team WHERE EXISTS (SELECT 1 FROM team_members WHERE team = $1 AND member = user_id)", team as _).execute(&mut transaction).await?;
            //TODO also remove all other teams with member overlap, and notify
            // create and assign Discord roles
            if let Some(discord_guild) = data.discord_guild {
                let discord_ctx = discord_ctx.read().await;
                for row in sqlx::query!(r#"SELECT discord_id AS "discord_id!: Id", role AS "role: Role" FROM users, team_members WHERE id = member AND discord_id IS NOT NULL AND team = $1"#, team as _).fetch_all(&mut transaction).await? {
                    if let Ok(mut member) = discord_guild.member(&*discord_ctx, UserId::new(row.discord_id.0)).await {
                        let mut roles_to_assign = member.roles.iter().copied().collect::<HashSet<_>>();
                        if let Some(Id(participant_role)) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND series = $2 AND event = $3"#, i64::from(discord_guild), series as _, event).fetch_optional(&mut transaction).await? {
                            roles_to_assign.insert(RoleId::new(participant_role));
                        }
                        if let Some(Id(role_role)) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND role = $2"#, i64::from(discord_guild), row.role as _).fetch_optional(&mut transaction).await? {
                            roles_to_assign.insert(RoleId::new(role_role));
                        }
                        if let Some(racetime_slug) = sqlx::query_scalar!("SELECT racetime_slug FROM teams WHERE id = $1", team as _).fetch_one(&mut transaction).await? {
                            if let Some(Id(team_role)) = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND racetime_team = $2"#, i64::from(discord_guild), racetime_slug).fetch_optional(&mut transaction).await? {
                                roles_to_assign.insert(RoleId::new(team_role));
                            } else {
                                let team_name = sqlx::query_scalar!(r#"SELECT name AS "name!" FROM teams WHERE id = $1"#, team as _).fetch_one(&mut transaction).await?;
                                let team_role = discord_guild.create_role(&*discord_ctx, EditRole::new().hoist(false).mentionable(true).name(team_name).permissions(Permissions::empty())).await?.id;
                                sqlx::query!("INSERT INTO discord_roles (id, guild, racetime_team) VALUES ($1, $2, $3)", i64::from(team_role), i64::from(discord_guild), racetime_slug).execute(&mut transaction).await?;
                                roles_to_assign.insert(team_role);
                            }
                        }
                        member.edit(&*discord_ctx, EditMember::new().roles(roles_to_assign)).await?;
                    }
                }
            }
        }
        transaction.commit().await?;
        Ok(Redirect::to(uri!(teams(series, event))))
    } else {
        transaction.rollback().await?;
        Err(AcceptError::NotInTeam.into())
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum ResignError {
    #[error(transparent)] Csrf(#[from] rocket_csrf::VerificationFailure),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Discord(#[from] serenity::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("you can no longer resign from this event since it has already ended")]
    EventEnded,
    #[error("can't delete teams you're not part of")]
    NotInTeam,
}

impl<E: Into<ResignError>> From<E> for StatusOrError<ResignError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[rocket::get("/event/<series>/<event>/resign/<team>")]
pub(crate) async fn resign(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, team: Id) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    if data.is_ended() {
        return Err(StatusOrError::Status(Status::Forbidden))
    }
    let is_started = data.is_started(&mut transaction).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("Resign — {}", data.display_name), html! {
        p {
            @if is_started {
                @if let TeamConfig::Solo = data.team_config() {
                    : "Are you sure you want to resign from ";
                    : data;
                    : "?";
                } else {
                    : "Are you sure you want to remove your team from ";
                    : data;
                    : "?";
                }
            } else {
                @if let TeamConfig::Solo = data.team_config() {
                    : "Are you sure you want to retract your registration from ";
                    : data;
                    : "?";
                } else {
                    : "Are you sure you want to retract your team's registration from ";
                    : data;
                    : "? If you change your mind later, you will need to invite your teammates again.";
                }
            }
        }
        div(class = "button-row") {
            form(action = uri!(crate::event::resign_post(series, event, team)).to_string(), method = "post") {
                : csrf;
                input(type = "submit", value = "Yes, resign");
            }
        }
    }).await?)
}

#[rocket::post("/event/<series>/<event>/resign/<team>", data = "<form>")]
pub(crate) async fn resign_post(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, csrf: Option<CsrfToken>, series: Series, event: &str, team: Id, form: Form<Contextual<'_, EmptyForm>>) -> Result<Redirect, StatusOrError<ResignError>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let team = Team::from_id(&mut transaction, team).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf); //TODO option to resubmit on error page (with some “are you sure?” wording)
    if data.is_ended() { return Err(ResignError::EventEnded.into()) }
    let keep_record = data.is_started(&mut transaction).await? || sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM async_teams WHERE team = $1) AS "exists!""#, team.id as _).fetch_one(&mut transaction).await?;
    let members = if keep_record {
        sqlx::query!(r#"UPDATE teams SET resigned = TRUE WHERE id = $1"#, team.id as _).execute(&mut transaction).await?;
        sqlx::query!(r#"SELECT member AS "id: Id", status AS "status: SignupStatus" FROM team_members WHERE team = $1"#, team.id as _).fetch(&mut transaction)
            .map_ok(|row| (row.id, row.status))
            .try_collect::<Vec<_>>().await?
    } else {
        sqlx::query!(r#"DELETE FROM team_members WHERE team = $1 RETURNING member AS "id: Id", status AS "status: SignupStatus""#, team.id as _).fetch(&mut transaction)
            .map_ok(|row| (row.id, row.status))
            .try_collect().await?
    };
    let mut me_in_team = false;
    let mut notification_kind = SimpleNotificationKind::Resign;
    for &(member_id, status) in &members {
        if member_id == me.id {
            me_in_team = true;
            if !status.is_confirmed() { notification_kind = SimpleNotificationKind::Decline }
            break
        }
    }
    if me_in_team {
        for (member_id, status) in members {
            if member_id != me.id && status.is_confirmed() {
                let notification_id = Id::new(&mut transaction, IdTable::Notifications).await?;
                sqlx::query!("INSERT INTO notifications (id, rcpt, kind, series, event, sender) VALUES ($1, $2, $3, $4, $5, $6)", notification_id as _, member_id as _, notification_kind as _, series as _, event, me.id as _).execute(&mut transaction).await?;
            }
        }
        if !keep_record {
            sqlx::query!("DELETE FROM teams WHERE id = $1", team.id as _).execute(&mut transaction).await?;
        }
        if let Some(organizer_channel) = data.discord_organizer_channel {
            organizer_channel.say(&*discord_ctx.read().await, MessageBuilder::default()
                .mention_team(&mut transaction, data.discord_guild, &team).await?
                .push(if team.name_is_plural() { " have resigned from " } else { " has resigned from " })
                .push_safe(data.display_name)
                .push(".")
                .build(),
            ).await?;
        }
        transaction.commit().await?;
        Ok(Redirect::to(uri!(teams(series, event))))
    } else {
        transaction.rollback().await?;
        Err(ResignError::NotInTeam.into())
    }
}

#[derive(sqlx::Type)]
#[sqlx(type_name = "async_kind", rename_all = "lowercase")]
pub(crate) enum AsyncKind {
    Qualifier,
    /// The tiebreaker for the highest Swiss points group with more than one team.
    Tiebreaker1,
    /// The tiebreaker for the 2nd-highest Swiss points group with more than one team.
    Tiebreaker2,
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct RequestAsyncForm {
    #[field(default = String::new())]
    csrf: String,
    confirm: bool,
}

#[rocket::post("/event/<series>/<event>/request-async", data = "<form>")]
pub(crate) async fn request_async(pool: &State<PgPool>, env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, RequestAsyncForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if let Some(ref value) = form.value {
        let team = sqlx::query_as!(Team, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name, restream_consent FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT resigned
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, series as _, event, me.id as _).fetch_optional(&mut transaction).await?;
        let async_kind = if let Some(ref team) = team {
            if let Some(async_kind) = data.active_async(&mut transaction, Some(team.id)).await? {
                let requested = sqlx::query_scalar!(r#"SELECT requested IS NOT NULL AS "requested!" FROM async_teams WHERE team = $1 AND kind = $2"#, team.id as _, async_kind as _).fetch_optional(&mut transaction).await?;
                if requested.map_or(false, identity) {
                    form.context.push_error(form::Error::validation("Your team has already requested this async."));
                }
                Some(async_kind)
            } else {
                form.context.push_error(form::Error::validation("There is no active async for your team."));
                None
            }
        } else {
            //TODO if this is a solo event, check signup requirements and sign up?
            form.context.push_error(form::Error::validation("You are not signed up for this event."));
            None
        };
        if !value.confirm {
            form.context.push_error(form::Error::validation("This field is required.").with_name("confirm"));
        }
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(status_page(pool.begin().await?, **env, &*discord_ctx.read().await, Some(me), uri, csrf.as_ref(), data, StatusContext::RequestAsync(form.context)).await?)
        } else {
            let team = team.expect("validated");
            let async_kind = async_kind.expect("validated");
            sqlx::query!("INSERT INTO async_teams (team, kind, requested) VALUES ($1, $2, NOW()) ON CONFLICT (team, kind) DO UPDATE SET requested = EXCLUDED.requested", team.id as _, async_kind as _).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(status(series, event))))
        }
    } else {
        transaction.rollback().await?;
        RedirectOrContent::Content(status_page(pool.begin().await?, **env, &*discord_ctx.read().await, Some(me), uri, csrf.as_ref(), data, StatusContext::RequestAsync(form.context)).await?)
    })
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct SubmitAsyncForm {
    #[field(default = String::new())]
    csrf: String,
    pieces: Option<i16>,
    #[field(default = String::new())]
    time1: String,
    #[field(default = String::new())]
    vod1: String,
    #[field(default = String::new())]
    time2: String,
    #[field(default = String::new())]
    vod2: String,
    #[field(default = String::new())]
    time3: String,
    #[field(default = String::new())]
    vod3: String,
    #[field(default = String::new())]
    fpa: String,
}

#[rocket::post("/event/<series>/<event>/submit-async", data = "<form>")]
pub(crate) async fn submit_async(pool: &State<PgPool>, env: &State<Environment>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, SubmitAsyncForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if let Some(ref value) = form.value {
        let team = sqlx::query_as!(Team, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name, restream_consent FROM teams, team_members WHERE
            id = team
            AND series = $1
            AND event = $2
            AND member = $3
            AND NOT resigned
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        "#, series as _, event, me.id as _).fetch_optional(&mut transaction).await?;
        let async_kind = if let Some(ref team) = team {
            if let Some(async_kind) = data.active_async(&mut transaction, Some(team.id)).await? {
                let row = sqlx::query!(r#"SELECT requested IS NOT NULL AS "requested!", submitted IS NOT NULL AS "submitted!" FROM async_teams WHERE team = $1 AND kind = $2"#, team.id as _, async_kind as _).fetch_optional(&mut transaction).await?;
                if row.as_ref().map_or(false, |row| row.submitted) {
                    form.context.push_error(form::Error::validation("You have already submitted times for this async. To make a correction or add vods, please contact the tournament organizers.")); //TODO allow adding vods via form but no other edits
                }
                if !row.map_or(false, |row| row.requested) {
                    form.context.push_error(form::Error::validation("You have not requested this async yet."));
                }
                Some(async_kind)
            } else {
                form.context.push_error(form::Error::validation("There is no active async for your team."));
                None
            }
        } else {
            form.context.push_error(form::Error::validation("You are not signed up for this event."));
            None
        };
        if let Series::TriforceBlitz = series {
            if let Some(pieces) = value.pieces {
                if pieces < 0 || pieces > 3 {
                    form.context.push_error(form::Error::validation("Must be a number from 0 to 3.").with_name("pieces"));
                }
            } else {
                form.context.push_error(form::Error::validation("This field is required.").with_name("pieces"));
            }
        }
        let times = vec![
            if value.time1.is_empty() {
                None
            } else if let Some(time) = parse_duration(&value.time1, DurationUnit::Hours) {
                Some(time)
            } else {
                form.context.push_error(form::Error::validation("Duration must be formatted like “1:23:45” or “1h 23m 45s”.").with_name("time1"));
                None
            },
            if value.time2.is_empty() {
                None
            } else if let Some(time) = parse_duration(&value.time2, DurationUnit::Hours) {
                Some(time)
            } else {
                form.context.push_error(form::Error::validation("Duration must be formatted like “1:23:45” or “1h 23m 45s”.").with_name("time2"));
                None
            },
            if value.time3.is_empty() {
                None
            } else if let Some(time) = parse_duration(&value.time3, DurationUnit::Hours) {
                Some(time)
            } else {
                form.context.push_error(form::Error::validation("Duration must be formatted like “1:23:45” or “1h 23m 45s”.").with_name("time3"));
                None
            },
        ];
        let vods = vec![
            value.vod1.clone(),
            value.vod2.clone(),
            value.vod3.clone(),
        ];
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(status_page(pool.begin().await?, **env, &*discord_ctx.read().await, Some(me), uri, csrf.as_ref(), data, StatusContext::SubmitAsync(form.context)).await?)
        } else {
            let team = team.expect("validated");
            let async_kind = async_kind.expect("validated");
            sqlx::query!("UPDATE async_teams SET submitted = NOW(), pieces = $1, fpa = $2 WHERE team = $3 AND kind = $4", value.pieces, (!value.fpa.is_empty()).then(|| &value.fpa), team.id as _, async_kind as _).execute(&mut transaction).await?;
            let mut players = Vec::default();
            for (((role, _), time), vod) in data.team_config().roles().iter().zip(&times).zip(&vods) {
                let player = sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1 AND role = $2"#, team.id as _, role as _).fetch_one(&mut transaction).await?;
                sqlx::query!("INSERT INTO async_players (series, event, player, kind, time, vod) VALUES ($1, $2, $3, $4, $5, $6)", series as _, event, player as _, async_kind as _, time as _, (!vod.is_empty()).then_some(vod)).execute(&mut transaction).await?;
                players.push(player);
            }
            if let Some(discord_guild) = data.discord_guild {
                let asyncs_row = sqlx::query!(r#"SELECT discord_role AS "discord_role: Id", discord_channel AS "discord_channel: Id" FROM asyncs WHERE series = $1 AND event = $2 AND kind = $3"#, series as _, event, async_kind as _).fetch_one(&mut transaction).await?;
                let members = sqlx::query_scalar!(r#"SELECT discord_id AS "discord_id!: Id" FROM users, team_members WHERE id = member AND discord_id IS NOT NULL AND team = $1"#, team.id as _).fetch_all(&mut transaction).await?;
                if let Some(Id(discord_role)) = asyncs_row.discord_role {
                    for &Id(user_id) in &members {
                        if let Ok(mut member) = discord_guild.member(&*discord_ctx.read().await, user_id).await {
                            member.add_role(&*discord_ctx.read().await, discord_role).await?;
                        }
                    }
                }
                if let Some(Id(discord_channel)) = asyncs_row.discord_channel {
                    let mut message = MessageBuilder::default();
                    message.push("Please welcome ");
                    message.mention_team(&mut transaction, Some(discord_guild), &team).await?;
                    if let Some(sum) = times.iter().try_fold(Duration::default(), |acc, &time| Some(acc + time?)) {
                        message.push(" who finished with a time of ");
                        message.push(format_duration(sum / u32::try_from(times.len()).expect("too many players in team"), true));
                        message.push_line('!');
                    } else {
                        message.push_line(" who did not finish.");
                    }
                    if players.len() > 1 {
                        for (i, ((player, time), vod)) in players.into_iter().zip(&times).zip(&vods).enumerate() {
                            if let Some(player) = User::from_id(&mut transaction, player).await? {
                                message.mention_user(&player);
                            } else {
                                message.push("player ");
                                message.push((i + 1).to_string());
                            }
                            message.push(": ");
                            if let Some(time) = *time {
                                message.push(format_duration(time, false));
                            } else {
                                message.push("DNF");
                            }
                            if vod.is_empty() {
                                message.push_line("");
                            } else {
                                message.push(' ');
                                message.push_line_safe(vod);
                            }
                        }
                    }
                    if !value.fpa.is_empty() {
                        message.push("FPA call:");
                        message.quote_rest();
                        message.push_safe(&value.fpa);
                    }
                    ChannelId::new(discord_channel).send_message(&*discord_ctx.read().await, CreateMessage::new()
                        .content(message.build())
                        .flags(MessageFlags::SUPPRESS_EMBEDS)
                    ).await?;
                }
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(status(series, event))))
        }
    } else {
        transaction.rollback().await?;
        RedirectOrContent::Content(status_page(pool.begin().await?, **env, &*discord_ctx.read().await, Some(me), uri, csrf.as_ref(), data, StatusContext::SubmitAsync(form.context)).await?)
    })
}

#[rocket::get("/event/<series>/<event>/volunteer")]
pub(crate) async fn volunteer(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, series: Series, event: &str) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let header = data.header(&mut transaction, **env, me.as_ref(), Tab::Volunteer, false).await?;
    let content = match data.series {
        Series::League => html! {
            @let chuckles = User::from_id(&mut transaction, Id(3480396938053963767)).await?.ok_or(Error::OrganizerUserData)?;
            article {
                p {
                    : "The primary role of league volunteers is to complete race reviews to ensure runners are following league rules and to conduct initial FPA checks on races where FPA was called. If you are interested in being a volunteer for league, please complete ";
                    a(href = "https://forms.gle/8fr8jk3eXXQ1xeEEA") : "this form";
                    : ", then DM ";
                    : chuckles;
                    : " on Discord.";
                }
                p {
                    : "If you or an organised restream team want to restream matches, please complete ";
                    a(href = "https://forms.gle/eCJsvdE7CQY7Wofp6") : "this form";
                    : " (only one person from the team needs to complete it), then DM ";
                    : chuckles;
                    : " on Discord.";
                }
            }
        },
        Series::TriforceBlitz => html! {
            article {
                p {
                    : "If you are interested in restreaming, commentating, or tracking a race for this tournament, please contact ";
                    : User::from_id(&mut transaction, Id(13528320435736334110)).await?.ok_or(Error::OrganizerUserData)?;
                    : ".";
                }
                p : "If a race already has a restream, you can volunteer through that channel's Discord.";
            }
        },
        _ => unimplemented!(),
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &data.display_name, html! {
        : header;
        : content;
    }).await?)
}
