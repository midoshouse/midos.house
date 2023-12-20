use {
    std::pin::Pin,
    lazy_regex::Regex,
    serde_with::DeserializeAs,
    crate::{
        event::{
            Data,
            DataError,
            Role,
            SignupStatus,
            Tab,
            teams,
        },
        prelude::*,
    },
};

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Flow {
    requirements: Vec<Requirement>,
}

enum DeserializeRawHtml {}

impl<'de> DeserializeAs<'de, RawHtml<String>> for DeserializeRawHtml {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<RawHtml<String>, D::Error> {
        String::deserialize(deserializer).map(RawHtml)
    }
}

enum DeserializeRegex {}

impl<'de> DeserializeAs<'de, Regex> for DeserializeRegex {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<Regex, D::Error> {
        Regex::new(<&str>::deserialize(deserializer)?).map_err(|e| D::Error::custom(e.to_string()))
    }
}

/// Requirements to enter an event
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Requirement {
    /// Must have a racetime.gg account connected to their Mido's House account
    RaceTime,
    /// Must be on a list of invited racetime.gg users
    RaceTimeInvite {
        invites: HashSet<String>,
        #[serde(default)]
        #[serde_as(as = "Option<DeserializeRawHtml>")]
        text: Option<RawHtml<String>>,
    },
    /// Must have a Discord account connected to their Mido's House account
    Discord,
    /// Must be in the event's Discord guild
    DiscordGuild {
        name: String,
    },
    /// Must have a Challonge account connected to their Mido's House account
    Challonge,
    /// Must fill a custom text field
    #[serde(rename_all = "camelCase")]
    TextField {
        #[serde_as(as = "DeserializeRawHtml")]
        label: RawHtml<String>,
        #[serde_as(as = "DeserializeRegex")]
        regex: Regex,
        #[serde_as(as = "serde_with::Map<DeserializeRegex, _>")]
        regex_error_messages: Vec<(Regex, String)>,
        fallback_error_message: String,
    },
    /// Must fill a custom text field
    #[serde(rename_all = "camelCase")]
    TextField2 {
        #[serde_as(as = "DeserializeRawHtml")]
        label: RawHtml<String>,
        #[serde_as(as = "DeserializeRegex")]
        regex: Regex,
        #[serde_as(as = "serde_with::Map<DeserializeRegex, _>")]
        regex_error_messages: Vec<(Regex, String)>,
        fallback_error_message: String,
    },
    /// Must agree to the event rules
    Rules {
        document: Option<Url>,
    },
    /// Must agree to be restreamed
    RestreamConsent {
        #[serde(default)]
        optional: bool,
        #[serde(default)]
        #[serde_as(as = "Option<DeserializeRawHtml>")]
        note: Option<RawHtml<String>>,
    },
    /// Must either request and submit the qualifier seed as an async, or participate in the live qualifier
    #[serde(rename_all = "camelCase")]
    Qualifier {
        async_start: DateTime<Utc>,
        async_end: DateTime<Utc>,
        live_start: DateTime<Utc>,
    },
    /// Must place within the top n players after all races in the `Qualifier` phase
    #[serde(rename_all = "camelCase")]
    QualifierPlacement {
        num_players: usize,
        #[serde(default)]
        min_races: usize,
    },
    /// A signup requirement that cannot be checked automatically
    External {
        text: String,
    },
}

struct RequirementStatus {
    blocks_submit: bool,
    html_content: Box<dyn FnOnce(&mut Vec<&form::Error<'_>>) -> RawHtml<String> + Send>,
}

impl Requirement {
    fn requires_sign_in(&self) -> bool {
        match self {
            Self::RaceTime => true,
            Self::RaceTimeInvite { .. } => true,
            Self::Discord => true,
            Self::DiscordGuild { .. } => true,
            Self::Challonge => true,
            Self::TextField { .. } => true,
            Self::TextField2 { .. } => true,
            Self::Rules { .. } => true,
            Self::RestreamConsent { .. } => true,
            Self::Qualifier { .. } => true,
            Self::QualifierPlacement { .. } => false,
            Self::External { .. } => false,
        }
    }

    async fn is_checked(&self, transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, discord_ctx: &RwFuture<DiscordCtx>, me: Option<&User>, data: &Data<'_>) -> Result<Option<bool>, Error> {
        Ok(match self {
            Self::RaceTime => Some(me.map_or(false, |me| me.racetime.is_some())),
            Self::RaceTimeInvite { invites, .. } => Some(me.and_then(|me| me.racetime.as_ref()).map_or(false, |racetime| invites.contains(&racetime.id))),
            Self::Discord => Some(me.map_or(false, |me| me.discord.is_some())),
            Self::DiscordGuild { .. } => Some({
                let discord_guild = data.discord_guild.ok_or(Error::DiscordGuild)?;
                if let Some(discord) = me.and_then(|me| me.discord.as_ref()) {
                    discord_guild.member(&*discord_ctx.read().await, discord.id).await.is_ok()
                } else {
                    false
                }
            }),
            Self::Challonge => Some(me.map_or(false, |me| me.challonge_id.is_some())),
            Self::TextField { .. } => Some(false),
            Self::TextField2 { .. } => Some(false),
            Self::Rules { .. } => Some(false),
            Self::RestreamConsent { .. } => Some(false),
            Self::Qualifier { .. } => Some(false),
            Self::QualifierPlacement { num_players, min_races } => Some(if_chain! {
                // All qualifiers must be completed to ensure the qualifier placements are final.
                //TODO This could be relaxed by calculating whether the player has secured a spot ahead of time.
                if Race::for_event(&mut *transaction, http_client, env, config, data).await?.into_iter().all(|race| race.phase.as_ref().map_or(true, |phase| phase != "Qualifier") || race.schedule.is_ended());
                if let Some(me) = me;
                let teams = teams::signups_sorted(transaction, http_client, env, config, Some(me), data, teams::QualifierKind::Multiple).await?;
                if let Some((placement, team)) = teams.iter().enumerate().find(|(_, team)| team.members.iter().any(|member| match member.user {
                    teams::MemberUser::MidosHouse(ref user) => user == me,
                    teams::MemberUser::RaceTime { ref id, .. } => me.racetime.as_ref().map_or(false, |racetime| racetime.id == *id),
                }));
                if let teams::Qualification::Multiple { num_qualifiers, .. } = team.qualification;
                then {
                    placement < *num_players //TODO adjust for opt-outs
                    && num_qualifiers >= *min_races
                } else {
                    false
                }
            }),
            Self::External { .. } => None,
        })
    }

    fn check_get(&self, data: &Data<'_>, is_checked: Option<bool>, redirect_uri: rocket::http::uri::Origin<'_>, defaults: &pic::EnterFormDefaults<'_>) -> Result<RequirementStatus, Error> {
        Ok(match self {
            Self::RaceTime => {
                let mut html_content = html! {
                    : "Connect a racetime.gg account to your Mido's House account";
                };
                if !is_checked.unwrap() {
                    //TODO offer to merge accounts like on profile
                    html_content = html! {
                        a(href = uri!(crate::auth::racetime_login(Some(redirect_uri))).to_string()) : html_content;
                    };
                }
                RequirementStatus {
                    blocks_submit: !is_checked.unwrap(),
                    html_content: Box::new(move |_| html_content),
                }
            }
            Self::RaceTimeInvite { text, .. } => {
                let text = text.clone();
                RequirementStatus {
                    blocks_submit: !is_checked.unwrap(),
                    html_content: Box::new(move |_| html! {
                        @if let Some(text) = text {
                            : text;
                        } else {
                            : "You must be on a list of invited racetime.gg users";
                        }
                    }),
                }
            }
            Self::Discord => {
                let mut html_content = html! {
                    : "Connect a Discord account to your Mido's House account";
                };
                if !is_checked.unwrap() {
                    //TODO offer to merge accounts like on profile
                    html_content = html! {
                        a(href = uri!(crate::auth::discord_login(Some(redirect_uri))).to_string()) : html_content;
                    };
                }
                RequirementStatus {
                    blocks_submit: !is_checked.unwrap(),
                    html_content: Box::new(move |_| html_content),
                }
            }
            Self::DiscordGuild { name } => {
                let name = name.clone();
                let invite_url = data.discord_invite_url.as_ref().map(|url| url.to_string());
                RequirementStatus {
                    blocks_submit: !is_checked.unwrap(),
                    html_content: Box::new(move |_| html! {
                        @if let Some(invite_url) = invite_url {
                            a(href = invite_url) {
                                : "Join the ";
                                : name;
                                : " Discord server";
                            }
                        } else {
                            : "Join the ";
                            : name;
                            : " Discord server";
                        }
                    }),
                }
            }
            Self::Challonge => {
                let mut html_content = html! {
                    : "Connect a Challonge account to your Mido's House account";
                };
                if !is_checked.unwrap() {
                    html_content = html! {
                        a(href = uri!(crate::auth::challonge_login(Some(redirect_uri))).to_string()) : html_content;
                    };
                }
                RequirementStatus {
                    blocks_submit: !is_checked.unwrap(),
                    html_content: Box::new(move |_| html_content),
                }
            }
            Self::TextField { label, .. } => {
                let label = label.clone();
                let value = defaults.field_value("text_field").map(|value| value.to_owned());
                RequirementStatus {
                    blocks_submit: false,
                    html_content: Box::new(move |errors| html! {
                        : label;
                        : form_field("text_field", errors, html! {
                            input(type = "text", name = "text_field", value? = value);
                        });
                    }),
                }
            }
            Self::TextField2 { label, .. } => {
                let label = label.clone();
                let value = defaults.field_value("text_field").map(|value| value.to_owned());
                RequirementStatus {
                    blocks_submit: false,
                    html_content: Box::new(move |errors| html! {
                        : label;
                        : form_field("text_field2", errors, html! {
                            input(type = "text", name = "text_field2", value? = value);
                        });
                    }),
                }
            }
            Self::Rules { document } => {
                let checked = defaults.field_value("confirm").is_some_and(|value| value == "on");
                let team_config = data.team_config();
                let rules_url = if let Some(document) = document {
                    document.to_string()
                } else {
                    uri!(crate::event::info(data.series, &*data.event)).to_string()
                };
                RequirementStatus {
                    blocks_submit: false,
                    html_content: Box::new(move |errors| html! {
                        : form_field("confirm", errors, html! {
                            input(type = "checkbox", id = "confirm", name = "confirm", checked? = checked);
                            label(for = "confirm") {
                                @if let TeamConfig::Solo = team_config {
                                    : "I have read and agree to ";
                                } else {
                                    : "We have read and agree to ";
                                }
                                a(href = rules_url) : "the event rules";
                                : ".";
                            }
                        });
                    }),
                }
            }
            Self::RestreamConsent { optional: false, note } => {
                let checked = defaults.field_value("restream_consent").is_some_and(|value| value == "on");
                let team_config = data.team_config();
                let note = note.clone();
                RequirementStatus {
                    blocks_submit: false,
                    html_content: Box::new(move |errors| html! {
                        : form_field("restream_consent", errors, html! {
                            input(type = "checkbox", id = "restream_consent", name = "restream_consent", checked? = checked);
                            label(for = "restream_consent") {
                                @if let TeamConfig::Solo = team_config {
                                    : "I am okay with being restreamed.";
                                } else {
                                    : "We are okay with being restreamed.";
                                }
                                @if let Some(note) = note {
                                    br;
                                    : note;
                                }
                            }
                        });
                    }),
                }
            }
            Self::RestreamConsent { optional: true, note } => {
                let yes_checked = defaults.field_value("confirm").is_some_and(|value| value == "yes");
                let no_checked = defaults.field_value("confirm").is_some_and(|value| value == "no");
                let note = note.clone();
                RequirementStatus {
                    blocks_submit: false,
                    html_content: Box::new(move |errors| html! {
                        : form_field("restream_consent_radio", errors, html! {
                            label(for = "restream_consent_radio") {
                                : "Let us know whether you are okay with being restreamed:";
                            }
                            br;
                            input(id = "restream_consent_radio-yes", type = "radio", name = "restream_consent_radio", value = "yes", checked? = yes_checked);
                            label(for = "restream_consent_radio-yes") : "Yes";
                            input(id = "restream_consent_radio-no", type = "radio", name = "restream_consent_radio", value = "no", checked? = no_checked);
                            label(for = "restream_consent_radio-no") : "No";
                            @if let Some(note) = note {
                                br;
                                label(for = "restream_consent_radio") : note;
                            }
                        });
                    }),
                }
            }
            &Self::Qualifier { async_start, async_end, live_start } => {
                let now = Utc::now();
                let async_available = now >= async_start && now < async_end;
                let series = data.series;
                let checked = defaults.field_value("confirm").is_some_and(|value| value == "on");
                RequirementStatus {
                    blocks_submit: !async_available,
                    html_content: Box::new(move |errors| html! {
                        @if async_available {
                            : "Play the qualifier seed, either live on ";
                            : format_datetime(live_start, DateTimeFormat { long: true, running_text: true });
                            : " or request it as an async using this form by ";
                            : format_datetime(async_end, DateTimeFormat { long: true, running_text: true });
                            : ".";
                            @match series {
                                Series::TriforceBlitz => : tfb::qualifier_async_rules();
                                _ => @unimplemented
                            }
                            : form_field("confirm", errors, html! {
                                input(type = "checkbox", id = "confirm", name = "confirm", checked? = checked);
                                label(for = "confirm") : "I have read the above and am ready to play the seed";
                            });
                        } else {
                            : "Play the qualifier seed, either live on ";
                            : format_datetime(live_start, DateTimeFormat { long: true, running_text: true });
                            : " or async between ";
                            : format_datetime(async_start, DateTimeFormat { long: false, running_text: true });
                            : " and ";
                            : format_datetime(async_end, DateTimeFormat { long: false, running_text: true });
                            @if now < async_start {
                                : ". The form to request the async will appear on this page.";
                            }
                        }
                    }),
                }
            }
            &Self::QualifierPlacement { num_players, min_races } => {
                RequirementStatus {
                    blocks_submit: !is_checked.unwrap(),
                    html_content: Box::new(move |_| html! {
                        @if min_races == 0 {
                            : "Place";
                        } else {
                            : "Enter at least ";
                            : min_races;
                            : " qualifier races and place";
                        }
                        : " in the top ";
                        : num_players.to_string();
                        : " of qualifier scores.";
                        br;
                        : "Note: You may be eligible to enter even if you don't initially place in the top ";
                        : num_players.to_string();
                        : " due to other players opting out. You will be notified by an organizer if this is the case.";
                    }),
                }
                }
            Self::External { text } => {
                let text = text.clone();
                RequirementStatus {
                    blocks_submit: true,
                    html_content: Box::new(move |_| html! {
                        : text;
                    }),
                }
            }
        })
    }

    async fn check_form(&self, transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, discord_ctx: &RwFuture<DiscordCtx>, me: &User, data: &Data<'_>, form_ctx: &mut Context<'_>, value: &EnterForm) -> Result<(), Error> {
        match self {
            Self::TextField { regex, regex_error_messages, fallback_error_message, .. } => if !regex.is_match(&value.text_field) {
                let error_message = if let Some((_, error_message)) = regex_error_messages.iter().find(|(regex, _)| regex.is_match(&value.text_field)) {
                    error_message.clone()
                } else {
                    fallback_error_message.clone()
                };
                form_ctx.push_error(form::Error::validation(error_message).with_name("text_field"));
            },
            Self::TextField2 { regex, regex_error_messages, fallback_error_message, .. } => if !regex.is_match(&value.text_field2) {
                let error_message = if let Some((_, error_message)) = regex_error_messages.iter().find(|(regex, _)| regex.is_match(&value.text_field2)) {
                    error_message.clone()
                } else {
                    fallback_error_message.clone()
                };
                form_ctx.push_error(form::Error::validation(error_message).with_name("text_field2"));
            },
            Self::Rules { .. } => if !value.confirm {
                form_ctx.push_error(form::Error::validation("This field is required.").with_name("confirm"));
            },
            Self::RestreamConsent { optional: false, .. } => if !value.restream_consent {
                form_ctx.push_error(form::Error::validation("Restream consent is required to enter this event.").with_name("restream_consent"));
            },
            Self::RestreamConsent { optional: true, .. } => if value.restream_consent_radio.is_none() {
                form_ctx.push_error(form::Error::validation("Please select one of the options.").with_name("restream_consent_radio"));
            },
            Self::Qualifier { async_start, async_end, .. } => {
                let now = Utc::now();
                if now >= *async_start && now < *async_end {
                    if !value.confirm {
                        form_ctx.push_error(form::Error::validation("This field is required.").with_name("confirm"));
                    }
                } else {
                    form_ctx.push_error(form::Error::validation("The qualifier seed is not yet available."));
                }
            }
            Self::External { .. } => form_ctx.push_error(form::Error::validation("Please complete event entry via the external method.")),
            _ => if !self.is_checked(transaction, http_client, env, config, discord_ctx, Some(me), data).await?.unwrap_or(false) {
                form_ctx.push_error(form::Error::validation(match self {
                    Self::RaceTime => "A racetime.gg account is required to enter this event. Go to your Mido's House profile and select “Connect a racetime.gg account”.", //TODO direct link?
                    Self::RaceTimeInvite { .. } => if me.racetime.is_some() {
                        "This is an invitational event and it looks like you're not invited."
                    } else {
                        "This event uses an invite list of racetime.gg users. Go to your Mido's House profile and select “Connect a racetime.gg account” to check whether you're invited." //TODO direct link?
                    },
                    Self::Discord => "A Discord account is required to enter this event. Go to your Mido's House profile and select “Connect a Discord account”.", //TODO direct link?
                    Self::DiscordGuild { .. } => "You must join the event's Discord server to enter.", //TODO invite link?
                    Self::Challonge => "A Challonge account is required to enter this event.", //TODO link to /login/challonge
                    Self::QualifierPlacement { .. } => "You have not secured a qualifying placement.",
                    Self::TextField { .. } | Self::TextField2 { .. } | Self::Rules { .. } | Self::RestreamConsent { .. } | Self::Qualifier { .. } | Self::External { .. } => unreachable!(),
                }));
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Cal(#[from] crate::cal::Error),
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Event(#[from] crate::event::Error),
    #[error(transparent)] Notification(#[from] crate::notification::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("event has a discordGuild entry requirement but no Discord guild")]
    DiscordGuild,
}

impl<E: Into<Error>> From<E> for StatusOrError<Error> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, FromFormField)]
enum BoolRadio {
    Yes,
    No,
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct EnterForm {
    #[field(default = String::new())]
    csrf: String,
    confirm: bool,
    racetime_team: Option<String>,
    #[field(default = String::new())]
    team_name: String,
    my_role: Option<pic::Role>,
    teammate: Option<Id<Users>>,
    step2: bool,
    roles: HashMap<String, Role>,
    startgg_id: HashMap<String, String>,
    mw_impl: Option<mw::Impl>,
    restream_consent: bool,
    restream_consent_radio: Option<BoolRadio>,
    #[field(default = String::new())]
    text_field: String,
    #[field(default = String::new())]
    text_field2: String,
}

async fn enter_form(mut transaction: Transaction<'_, Postgres>, http_client: &reqwest::Client, env: Environment, config: &Config, discord_ctx: &RwFuture<DiscordCtx>, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, client: &reqwest::Client, data: Data<'_>, defaults: pic::EnterFormDefaults<'_>) -> Result<RawHtml<String>, Error> {
    //TODO if already entered, redirect to status page
    let my_invites = if let Some(ref me) = me {
        sqlx::query_scalar!(r#"SELECT team AS "team: Id<Teams>" FROM teams, team_members WHERE series = $1 AND event = $2 AND member = $3 AND status = 'unconfirmed'"#, data.series as _, &*data.event, me.id as _).fetch_all(&mut *transaction).await?
    } else {
        Vec::default()
    };
    let content = if data.is_started(&mut transaction).await? {
        html! {
            article {
                p : "You can no longer enter this event since it has already started.";
            }
        }
    } else if let (Series::Standard, "7") = (data.series, &*data.event) {
        s::enter_form()
    } else {
        match data.team_config() {
            TeamConfig::Solo => {
                if let Some(Flow { ref requirements }) = data.enter_flow {
                    if requirements.is_empty() {
                        if data.is_single_race() {
                            html! {
                                article {
                                    p {
                                        @if let Some(ref url) = data.url {
                                            : "Enter ";
                                            a(href = url.to_string()) : "the race room";
                                            : " to participate in this race.";
                                        } else {
                                            : "The race room will be opened around 30 minutes before the scheduled starting time. ";
                                            @if me.as_ref().map_or(false, |me| me.racetime.is_some()) {
                                                : "You don't need to sign up beforehand.";
                                            } else {
                                                : "You will need a ";
                                                a(href = "https://racetime.gg/") : "racetime.gg";
                                                : " account to participate.";
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            full_form(uri!(post(data.series, &*data.event)), csrf, html! {}, defaults.errors(), "Enter")
                        }
                    } else if requirements.iter().any(Requirement::requires_sign_in) && me.is_none() {
                        html! {
                            article {
                                p {
                                    a(href = uri!(auth::login(Some(uri!(get(data.series, &*data.event, defaults.my_role(), defaults.teammate()))))).to_string()) : "Sign in or create a Mido's House account";
                                    : " to enter this event.";
                                }
                            }
                        }
                    } else {
                        let mut can_submit = true;
                        let mut requirements_display = Vec::with_capacity(requirements.len());
                        for requirement in requirements {
                            let status = requirement.check_get(&data, requirement.is_checked(&mut transaction, http_client, env, config, discord_ctx, me.as_ref(), &data).await?, uri!(get(data.series, &*data.event, defaults.my_role(), defaults.teammate())), &defaults)?;
                            if status.blocks_submit { can_submit = false }
                            requirements_display.push((requirement.is_checked(&mut transaction, http_client, env, config, discord_ctx, me.as_ref(), &data).await?, status.html_content));
                        }
                        if can_submit {
                            let mut errors = defaults.errors();
                            full_form(uri!(post(data.series, &*data.event)), csrf, html! {
                                p : "To enter this event:";
                                @for (is_checked, html_content) in requirements_display {
                                    div(class = "check-item") {
                                        div(class = "checkmark") {
                                            @match is_checked {
                                                Some(true) => : "✓";
                                                Some(false) => {}
                                                None => : "?";
                                            }
                                        }
                                        div : html_content(&mut errors);
                                    }
                                }
                            }, errors, "Enter")
                        } else {
                            html! {
                                article {
                                    p : "To enter this event:";
                                    @for (is_checked, html_content) in requirements_display {
                                        div(class = "check-item") {
                                            div(class = "checkmark") {
                                                @match is_checked {
                                                    Some(true) => : "✓";
                                                    Some(false) => {}
                                                    None => : "?";
                                                }
                                            }
                                            div : html_content(&mut Vec::default());
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE series = $1 AND event = $2) AS "exists!""#, data.series as _, &*data.event).fetch_one(&mut *transaction).await? {
                    if me.is_none() {
                        html! {
                            article {
                                p {
                                    : "This is an invitational event. ";
                                    a(href = uri!(auth::login(Some(uri!(get(data.series, &*data.event, defaults.my_role(), defaults.teammate()))))).to_string()) : "Sign in or create a Mido's House account";
                                    : " to see if you're invited.";
                                }
                            }
                        }
                    } else if my_invites.is_empty() {
                        html! {
                            article {
                                p : "This is an invitational event and it looks like you're not invited.";
                            }
                        }
                    } else {
                        html! {} // invite should be rendered above this content
                    }
                } else {
                    html! {
                        article {
                            p : "Signups for this event aren't open yet."; //TODO option to be notified when signups open
                        }
                    }
                }
            }
            TeamConfig::Pictionary => return Ok(pic::enter_form(transaction, env, me, uri, csrf, data, defaults).await?),
            TeamConfig::CoOp | TeamConfig::Multiworld => return Ok(mw::enter_form(transaction, env, me, uri, csrf, data, defaults.into_context(), client).await?),
        }
    };
    let header = data.header(&mut transaction, env, me.as_ref(), Tab::Enter, false).await?;
    let invites = html! {
        @for team_id in my_invites {
            : crate::notification::team_invite(&mut transaction, env, me.as_ref().expect("got a team invite while not logged in"), csrf, team_id).await?;
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("Enter — {}", data.display_name), html! {
        : header;
        : invites;
        : content;
    }).await?)
}

fn enter_form_step2<'a, 'b: 'a, 'c: 'a, 'd: 'a>(mut transaction: Transaction<'a, Postgres>, env: Environment, me: Option<User>, uri: Origin<'b>, client: &reqwest::Client, csrf: Option<&'a CsrfToken>, data: Data<'c>, defaults: mw::EnterFormStep2Defaults<'d>) -> Pin<Box<dyn Future<Output = Result<RawHtml<String>, Error>> + Send + 'a>> {
    let team_members = defaults.racetime_members(client);
    Box::pin(async move {
        let header = data.header(&mut transaction, env, me.as_ref(), Tab::Enter, true).await?;
        let page_content = {
            let team_members = team_members.await?;
            let mut errors = defaults.errors();
            html! {
                : header;
                : full_form(uri!(post(data.series, &*data.event)), csrf, html! {
                    input(type = "hidden", name = "step2", value = "true");
                    : form_field("racetime_team", &mut errors, html! {
                        label(for = "racetime_team") {
                            : "racetime.gg Team: ";
                            a(href = format!("https://racetime.gg/team/{}", defaults.racetime_team_slug().expect("missing racetime team slug"))) : defaults.racetime_team_name().expect("missing racetime team name");
                            : " • ";
                            a(href = uri!(get(data.series, &*data.event, _, _)).to_string()) : "Change";
                        }
                        input(type = "hidden", name = "racetime_team", value = defaults.racetime_team_slug());
                        input(type = "hidden", name = "racetime_team_name", value = defaults.racetime_team_name());
                    });
                    @for team_member in team_members {
                        : form_field(&format!("roles[{}]", team_member.id), &mut errors, html! {
                            label(for = &format!("roles[{}]", team_member.id)) : &team_member.name; //TODO Mido's House display name, falling back to racetime display name if no Mido's House account
                            @for (role, display_name) in data.team_config().roles() {
                                @let css_class = role.css_class().expect("tried to render enter_form_step2 for a solo event");
                                input(id = &format!("roles[{}]-{css_class}", team_member.id), class = css_class, type = "radio", name = &format!("roles[{}]", team_member.id), value = css_class, checked? = defaults.role(&team_member.id) == Some(*role));
                                label(class = css_class, for = &format!("roles[{}]-{css_class}", team_member.id)) : display_name;
                            }
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
                    @if let TeamConfig::Multiworld = data.team_config() {
                        : form_field("mw_impl", &mut errors, html! {
                            label(for = "mw_impl") : "Multiworld plugin:";
                            input(id = "mw_impl-bizhawk_co_op", type = "radio", name = "mw_impl", value = "bizhawk_co_op", checked? = defaults.mw_impl() == Some(mw::Impl::BizHawkCoOp));
                            label(for = "mw_impl-bizhawk_co_op") : "bizhawk-co-op";
                            input(id = "mw_impl-midos_house", type = "radio", name = "mw_impl", value = "midos_house", checked? = defaults.mw_impl() == Some(mw::Impl::MidosHouse));
                            label(for = "mw_impl-midos_house") : "Mido's House Multiworld";
                        });
                    }
                    : form_field("restream_consent", &mut errors, html! {
                        input(type = "checkbox", id = "restream_consent", name = "restream_consent", checked? = defaults.restream_consent());
                        label(for = "restream_consent") {
                            @if data.is_single_race() {
                                : "We are okay with being restreamed. (Optional. Can be changed later.)";
                            } else {
                                //TODO allow changing on Status page during Swiss, except revoking while a restream is planned
                                //TODO change text depending on tournament structure
                                : "We are okay with being restreamed. (Optional for Swiss, required for top 8. Can be changed later.)";
                            }
                        }
                    });
                }, errors, "Enter");
            }
        };
        Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("Enter — {}", data.display_name), page_content).await?)
    })
}

#[rocket::get("/event/<series>/<event>/enter?<my_role>&<teammate>")]
pub(crate) async fn get(pool: &State<PgPool>, http_client: &State<reqwest::Client>, env: &State<Environment>, config: &State<Config>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, client: &State<reqwest::Client>, series: Series, event: &str, my_role: Option<crate::series::pic::Role>, teammate: Option<Id<Users>>) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(enter_form(transaction, http_client, **env, config, discord_ctx, me, uri, csrf.as_ref(), client, data, pic::EnterFormDefaults::Values { my_role, teammate }).await?)
}

#[rocket::post("/event/<series>/<event>/enter", data = "<form>")]
pub(crate) async fn post(pool: &State<PgPool>, http_client: &State<reqwest::Client>, env: &State<Environment>, config: &State<Config>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, client: &State<reqwest::Client>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, EnterForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if let Some(ref value) = form.value {
        if data.is_started(&mut transaction).await? {
            form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
        }
        match data.team_config() {
            TeamConfig::Solo => {
                let mut request_qualifier = false;
                if let Some(Flow { ref requirements }) = data.enter_flow {
                    if requirements.is_empty() {
                        if data.is_single_race() {
                            form.context.push_error(form::Error::validation("Signups for this event are not handled by Mido's House."));
                        }
                    } else {
                        for requirement in requirements {
                            requirement.check_form(&mut transaction, http_client, **env, config, discord_ctx, &me, &data, &mut form.context, value).await?;
                            if let Requirement::Qualifier { .. } = requirement {
                                request_qualifier = true;
                            }
                        }
                    }
                } else {
                    form.context.push_error(form::Error::validation("Signups for this event aren't open yet."));
                }
                if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                    id = team
                    AND series = $1
                    AND event = $2
                    AND member = $3
                    AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                ) AS "exists!""#, series as _, event, me.id as _).fetch_one(&mut *transaction).await? {
                    form.context.push_error(form::Error::validation("You are already signed up for this event."));
                }
                if form.context.errors().next().is_none() {
                    let id = Id::<Teams>::new(&mut transaction).await?;
                    sqlx::query!("INSERT INTO teams (id, series, event, plural_name) VALUES ($1, $2, $3, FALSE)", id as _, series as _, event).execute(&mut *transaction).await?;
                    sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', 'none')", id as _, me.id as _).execute(&mut *transaction).await?;
                    if request_qualifier {
                        sqlx::query!("INSERT INTO async_teams (team, kind, requested) VALUES ($1, 'qualifier', NOW())", id as _).execute(&mut *transaction).await?;
                    }
                    transaction.commit().await?;
                    return Ok(RedirectOrContent::Redirect(Redirect::to(uri!(super::status(series, event)))))
                }
            }
            TeamConfig::Pictionary => {
                let (my_role, teammate) = match (value.my_role, value.teammate) {
                    (Some(my_role), Some(teammate)) => {
                        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                            id = team
                            AND series = $1
                            AND event = $2
                            AND member = $3
                            AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $4)
                        ) AS "exists!""#, series as _, event, me.id as _, teammate as _).fetch_one(&mut *transaction).await? {
                            form.context.push_error(form::Error::validation("A team with these members is already proposed for this race. Check your notifications to accept the invite, or ask your teammate to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
                        }
                        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                            id = team
                            AND series = $1
                            AND event = $2
                            AND member = $3
                            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                        ) AS "exists!""#, series as _, event, me.id as _).fetch_one(&mut *transaction).await? {
                            form.context.push_error(form::Error::validation("You are already signed up for this race."));
                        }
                        if !value.team_name.is_empty() && sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
                            series = $1
                            AND event = $2
                            AND name = $3
                            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                        ) AS "exists!""#, series as _, event, value.team_name).fetch_one(&mut *transaction).await? {
                            form.context.push_error(form::Error::validation("A team with this name is already signed up for this race.").with_name("team_name"));
                        }
                        if my_role == pic::Role::Sheikah && me.racetime.is_none() {
                            form.context.push_error(form::Error::validation("A racetime.gg account is required to enter as runner. Go to your profile and select “Connect a racetime.gg account”.").with_name("my_role")); //TODO direct link?
                        }
                        if teammate == me.id {
                            form.context.push_error(form::Error::validation("You cannot be your own teammate.").with_name("teammate"));
                        }
                        if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, teammate as _).fetch_one(&mut *transaction).await? {
                            form.context.push_error(form::Error::validation("There is no user with this ID.").with_name("teammate"));
                        }
                        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                            id = team
                            AND series = $1
                            AND event = $2
                            AND member = $3
                            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                        ) AS "exists!""#, series as _, event, teammate as _).fetch_one(&mut *transaction).await? {
                            form.context.push_error(form::Error::validation("This user is already signed up for this race.").with_name("teammate"));
                        }
                        //TODO check to make sure the teammate hasn't blocked the user submitting the form (or vice versa) or the event
                        (Some(my_role), Some(teammate))
                    }
                    (Some(_), None) => {
                        form.context.push_error(form::Error::validation("This field is required.").with_name("teammate"));
                        (None, None)
                    }
                    (None, Some(_)) => {
                        form.context.push_error(form::Error::validation("This field is required.").with_name("my_role"));
                        (None, None)
                    }
                    (None, None) => {
                        form.context.push_error(form::Error::validation("This field is required.").with_name("my_role"));
                        form.context.push_error(form::Error::validation("This field is required.").with_name("teammate"));
                        (None, None)
                    }
                };
                if form.context.errors().next().is_none() {
                    let id = Id::<Teams>::new(&mut transaction).await?;
                    sqlx::query!("INSERT INTO teams (id, series, event, name) VALUES ($1, $2, $3, $4)", id as _, series as _, event, (!value.team_name.is_empty()).then(|| &value.team_name)).execute(&mut *transaction).await?;
                    sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', $3)", id as _, me.id as _, Role::from(my_role.expect("validated")) as _).execute(&mut *transaction).await?;
                    sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'unconfirmed', $3)", id as _, teammate.expect("validated") as _, match my_role.expect("validated") { pic::Role::Sheikah => Role::Gerudo, pic::Role::Gerudo => Role::Sheikah } as _).execute(&mut *transaction).await?;
                    transaction.commit().await?;
                    return Ok(RedirectOrContent::Redirect(Redirect::to(uri!(super::status(series, event)))))
                }
            }
            _ => {
                let racetime_team = if let Some(ref racetime_team) = value.racetime_team {
                    if let Some(ref racetime) = me.racetime {
                        let user = client.get(format!("https://racetime.gg/user/{}/data", racetime.id))
                            .send().await?
                            .detailed_error_for_status().await?
                            .json_with_text_in_error::<mw::RaceTimeUser>().await?;
                        if user.teams.iter().any(|team| team.slug == *racetime_team) {
                            let team = client.get(format!("https://racetime.gg/team/{racetime_team}/data"))
                                .send().await?
                                .detailed_error_for_status().await?
                                .json_with_text_in_error::<mw::RaceTimeTeamData>().await?;
                            let expected_size = data.team_config().roles().len();
                            if team.members.len() != expected_size {
                                form.context.push_error(form::Error::validation(format!("Teams for this event must have exactly {expected_size} members, but this team has {}", team.members.len())))
                            }
                            //TODO get each team member's Mido's House account for displaying in step 2
                            Some(team)
                        } else {
                            form.context.push_error(form::Error::validation("This racetime.gg team does not exist or you're not in it.").with_name("racetime_team"));
                            None
                        }
                    } else {
                        form.context.push_error(form::Error::validation("A racetime.gg account is required to enter this event. Go to your profile and select “Connect a racetime.gg account”.")); //TODO direct link?
                        None
                    }
                } else {
                    form.context.push_error(form::Error::validation("This field is required.").with_name("racetime_team"));
                    None
                };
                let (team_slug, team_name, users, roles, startgg_ids) = if value.step2 {
                    if let Some(ref racetime_team) = racetime_team {
                        let mut all_accounts_exist = true;
                        let mut users = Vec::default();
                        let mut roles = Vec::default();
                        let mut startgg_ids = Vec::default();
                        for member in &racetime_team.members {
                            if let Some(user) = User::from_racetime(&mut *transaction, &member.id).await? {
                                if let Some(ref discord) = user.discord {
                                    if let Some(discord_guild) = data.discord_guild {
                                        if discord_guild.member(&*discord_ctx.read().await, discord.id).await.is_err() {
                                            //TODO only check if Requirement::DiscordGuild is present
                                            form.context.push_error(form::Error::validation("This user has not joined the tournament's Discord server.").with_name(format!("roles[{}]", member.id)));
                                        }
                                    }
                                } else {
                                    //TODO only check if Requirement::Discord is present
                                    form.context.push_error(form::Error::validation("This Mido's House account is not associated with a Discord account.").with_name(format!("roles[{}]", member.id)));
                                }
                                if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                                    id = team
                                    AND series = $1
                                    AND event = $2
                                    AND member = $3
                                    AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                                ) AS "exists!""#, series as _, event, user.id as _).fetch_one(&mut *transaction).await? {
                                    form.context.push_error(form::Error::validation("This user is already signed up for this tournament."));
                                }
                                users.push(user);
                            } else {
                                form.context.push_error(form::Error::validation("This racetime.gg account is not associated with a Mido's House account.").with_name(format!("roles[{}]", member.id)));
                                all_accounts_exist = false;
                            }
                            if let Some(&role) = value.roles.get(&member.id) {
                                roles.push(role);
                            } else {
                                form.context.push_error(form::Error::validation("This field is required.").with_name(format!("roles[{}]", member.id)));
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
                                [u1, u2] => if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
                                    series = $1
                                    AND event = $2
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $4)
                                ) AS "exists!""#, series as _, event, u1.id as _, u2.id as _).fetch_one(&mut *transaction).await? {
                                    form.context.push_error(form::Error::validation("A team with these members is already proposed for this tournament. Check your notifications to accept the invite, or ask your teammate to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
                                },
                                [u1, u2, u3] => if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
                                    series = $1
                                    AND event = $2
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $4)
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $5)
                                ) AS "exists!""#, series as _, event, u1.id as _, u2.id as _, u3.id as _).fetch_one(&mut *transaction).await? {
                                    form.context.push_error(form::Error::validation("A team with these members is already proposed for this tournament. Check your notifications to accept the invite, and/or ask your teammates to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
                                },
                                _ => unimplemented!("exact proposed team check for {} members", users.len()),
                            }
                        }
                        for (required_role, label) in data.team_config().roles() {
                            let mut found = false;
                            for (member_id, role) in &value.roles {
                                if role == required_role {
                                    if found {
                                        form.context.push_error(form::Error::validation("Each team member must have a different role.").with_name(format!("roles[{member_id}]")));
                                    } else {
                                        found = true;
                                    }
                                }
                            }
                            if !found {
                                form.context.push_error(form::Error::validation(format!("No team member is assigned as {label}.")));
                            }
                        }
                        if let (TeamConfig::Multiworld, None) = (data.team_config(), value.mw_impl) {
                            form.context.push_error(form::Error::validation("This field is required.").with_name("mw_impl"));
                        }
                        (racetime_team.slug.clone(), racetime_team.name.clone(), users, roles, startgg_ids)
                    } else {
                        Default::default()
                    }
                } else {
                    Default::default()
                };
                if form.context.errors().next().is_none() {
                    return Ok(if value.step2 {
                        let id = Id::<Teams>::new(&mut transaction).await?;
                        sqlx::query!(
                            "INSERT INTO teams (id, series, event, name, racetime_slug, restream_consent, text_field, text_field2, mw_impl) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                            id as _,
                            series as _,
                            event,
                            (!team_name.is_empty()).then(|| team_name),
                            team_slug,
                            value.restream_consent || value.restream_consent_radio == Some(BoolRadio::Yes),
                            value.text_field,
                            value.text_field2,
                            value.mw_impl as _,
                        ).execute(&mut *transaction).await?;
                        for ((user, role), startgg_id) in users.into_iter().zip_eq(roles).zip_eq(startgg_ids) {
                            sqlx::query!(
                                "INSERT INTO team_members (team, member, status, role, startgg_id) VALUES ($1, $2, $3, $4, $5)",
                                id as _, user.id as _, if user == me { SignupStatus::Created } else { SignupStatus::Unconfirmed } as _, role as _, startgg_id,
                            ).execute(&mut *transaction).await?;
                        }
                        transaction.commit().await?;
                        RedirectOrContent::Redirect(Redirect::to(uri!(super::status(series, event))))
                    } else {
                        RedirectOrContent::Content(enter_form_step2(transaction, **env, Some(me), uri, client, csrf.as_ref(), data, mw::EnterFormStep2Defaults::Values { racetime_team: racetime_team.expect("validated") }).await?)
                    })
                }
            }
        }
        if value.step2 {
            return Ok(RedirectOrContent::Content(enter_form_step2(transaction, **env, Some(me), uri, client, csrf.as_ref(), data, mw::EnterFormStep2Defaults::Context(form.context)).await?))
        }
    }
    Ok(RedirectOrContent::Content(enter_form(transaction, http_client, **env, config, discord_ctx, Some(me), uri, csrf.as_ref(), client, data, pic::EnterFormDefaults::Context(form.context)).await?))
}
