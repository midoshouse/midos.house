use {
    std::{
        borrow::Cow,
        collections::HashMap,
        pin::Pin,
    },
    chrono::prelude::*,
    futures::future::Future,
    itertools::Itertools as _,
    rocket::{
        FromForm,
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
        html,
    },
    serde::Deserialize,
    serenity::all::Context as DiscordCtx,
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
            DataError,
            Role,
            Series,
            SignupStatus,
            Tab,
            TeamConfig,
        },
        http::{
            PageError,
            PageStyle,
            page,
        },
        series::{
            mw,
            pic,
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
            full_form,
        },
    },
};

#[derive(Debug, Deserialize)]
pub(super) struct Flow {
    requirements: Vec<Requirement>,
}

/// Requirements to enter an event
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Requirement {
    /// Must have a racetime.gg account connected to their Mido's House account
    RaceTime,
    /// Must have a Discord account connected to their Mido's House account
    Discord,
    /// Must be in the event's Discord guild
    DiscordGuild {
        name: String,
    },
    /// Must either request and submit the qualifier seed as an async, or participate in the live qualifier
    #[serde(rename_all = "camelCase")]
    Qualifier {
        async_start: NaiveDate,
        async_end: DateTime<Utc>,
        live_start: DateTime<Utc>,
    },
}

impl Requirement {
    fn requires_sign_in(&self) -> bool {
        match self {
            Self::RaceTime => true,
            Self::Discord => true,
            Self::DiscordGuild { .. } => true,
            Self::Qualifier { .. } => true,
        }
    }

    async fn check(&self, discord_ctx: &RwFuture<DiscordCtx>, me: Option<&User>, data: &Data<'_>, redirect_uri: rocket::http::uri::Origin<'_>) -> Result<(bool, RawHtml<String>, &'static str), Error> {
        Ok(match self {
            Requirement::RaceTime => {
                let is_connected = me.expect("this check requires sign-in").racetime_id.is_some();
                let mut content = html! {
                    : "Connect a racetime.gg account to your Mido's House account";
                };
                if !is_connected {
                    //TODO offer to merge accounts like on profile
                    content = html! {
                        a(href = uri!(crate::auth::racetime_login(Some(redirect_uri))).to_string()) : content;
                    };
                }
                (is_connected, content, "A racetime.gg account is required to enter this event. Go to your profile and select “Connect a racetime.gg account”.") //TODO direct link?
            }
            Requirement::Discord => {
                let is_connected = me.expect("this check requires sign-in").discord_id.is_some();
                let mut content = html! {
                    : "Connect a Discord account to your Mido's House account";
                };
                if !is_connected {
                    //TODO offer to merge accounts like on profile
                    content = html! {
                        a(href = uri!(crate::auth::discord_login(Some(redirect_uri))).to_string()) : content;
                    };
                }
                (is_connected, content, "A Discord account is required to enter this event. Go to your profile and select “Connect a Discord account”.") //TODO direct link?
            }
            Requirement::DiscordGuild { name } => {
                let discord_guild = data.discord_guild.ok_or(Error::DiscordGuild)?;
                let is_joined = if let Some(discord_id) = me.expect("this check requires sign-in").discord_id {
                    discord_guild.member(&*discord_ctx.read().await, discord_id).await.is_ok()
                } else {
                    false
                };
                (
                    is_joined,
                    html! {
                        : "Join the ";
                        : name; //TODO invite link if not joined
                        : " Discord server";
                    },
                    "You must join the event's Discord server to enter.", //TODO invite link?
                )
            }
            Requirement::Qualifier { async_start, async_end, live_start } => ( //TODO qualifier request form if available
                false, //TODO
                html! {
                    : "Play the qualifier seed, either live on ";
                    : format_datetime(*live_start, DateTimeFormat { long: true, running_text: true });
                    : " or async between ";
                    : async_start.format("%B %-d, %Y").to_string();
                    : " and ";
                    : format_datetime(*async_end, DateTimeFormat { long: true, running_text: true });
                },
                "The qualifier seed is not yet available.", //TODO no error if qualifier is available
            ),
        })
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Data(#[from] DataError),
    #[error(transparent)] Event(#[from] crate::event::Error),
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

enum FormDefaultsRef<'a, 'c> {
    Context(&'a Context<'c>),
    Values {
        my_role: Option<pic::Role>,
        teammate: Option<Id>,
    },
}

impl FormDefaultsRef<'_, '_> {
    fn my_role(&self) -> Option<pic::Role> {
        match self {
            Self::Context(ctx) => match ctx.field_value("my_role") {
                Some("sheikah") => Some(pic::Role::Sheikah),
                Some("gerudo") => Some(pic::Role::Gerudo),
                _ => None,
            },
            &Self::Values { my_role, .. } => my_role,
        }
    }

    pub(crate) fn teammate(&self) -> Option<Id> {
        self.teammate_text().and_then(|text| text.parse().ok())
    }

    fn teammate_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Context(ctx) => ctx.field_value("teammate").map(Cow::Borrowed),
            &Self::Values { teammate, .. } => teammate.map(|id| Cow::Owned(id.0.to_string())),
        }
    }
}

impl<'a, 'c> From<&'a pic::EnterFormDefaults<'c>> for FormDefaultsRef<'a, 'c> {
    fn from(defaults: &'a pic::EnterFormDefaults<'c>) -> Self {
        match defaults {
            pic::EnterFormDefaults::Context(ctx) => Self::Context(ctx),
            &pic::EnterFormDefaults::Values { my_role, teammate } => Self::Values { my_role, teammate },
        }
    }
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct EnterForm {
    #[field(default = String::new())]
    csrf: String,
    racetime_team: Option<String>,
    #[field(default = String::new())]
    team_name: String,
    my_role: Option<pic::Role>,
    teammate: Option<Id>,
    step2: bool,
    roles: HashMap<String, Role>,
    startgg_id: HashMap<String, String>,
    restream_consent: bool,
}

async fn enter_form(mut transaction: Transaction<'_, Postgres>, discord_ctx: &RwFuture<DiscordCtx>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, client: &reqwest::Client, data: Data<'_>, defaults: pic::EnterFormDefaults<'_>) -> Result<RawHtml<String>, Error> {
    //TODO if already entered, redirect to status page
    let content = if data.is_started(&mut transaction).await? {
        html! {
            article {
                p : "You can no longer enter this event since it has already started.";
            }
        }
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
                                            @if me.as_ref().map_or(false, |me| me.racetime_id.is_some()) {
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
                        html! {
                            article {
                                p : "To enter this event:";
                                @for requirement in requirements {
                                    @let (is_checked, content, _) = requirement.check(discord_ctx, me.as_ref(), &data, uri!(get(data.series, &*data.event, defaults.my_role(), defaults.teammate()))).await?;
                                    div(class = "check-item") {
                                        div(class = "checkmark") {
                                            @if is_checked {
                                                : "✓";
                                            }
                                        }
                                        div : content;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    html! {
                        article {
                            p : "Signups for this event aren't open yet."; //TODO option to be notified when signups open
                        }
                    }
                }
            }
            TeamConfig::Pictionary => return Ok(pic::enter_form(transaction, me, uri, csrf, data, defaults).await?),
            TeamConfig::CoOp | TeamConfig::Multiworld => return Ok(mw::enter_form(transaction, me, uri, csrf, data, defaults.into_context(), client).await?),
        }
    };
    let header = data.header(&mut transaction, me.as_ref(), Tab::Enter, false).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), html! {
        : header;
        : content;
    }).await?)
}

fn enter_form_step2<'a, 'b: 'a, 'c: 'a, 'd: 'a>(mut transaction: Transaction<'a, Postgres>, me: Option<User>, uri: Origin<'b>, client: &reqwest::Client, csrf: Option<CsrfToken>, data: Data<'c>, defaults: mw::EnterFormStep2Defaults<'d>) -> Pin<Box<dyn Future<Output = Result<RawHtml<String>, Error>> + Send + 'a>> {
    let team_members = defaults.racetime_members(client);
    Box::pin(async move {
        let header = data.header(&mut transaction, me.as_ref(), Tab::Enter, true).await?;
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
        Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), page_content).await?)
    })
}

#[rocket::get("/event/<series>/<event>/enter?<my_role>&<teammate>")]
pub(crate) async fn get(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, client: &State<reqwest::Client>, series: Series, event: &str, my_role: Option<crate::series::pic::Role>, teammate: Option<Id>) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(enter_form(transaction, discord_ctx, me, uri, csrf, client, data, pic::EnterFormDefaults::Values { my_role, teammate }).await?)
}

#[rocket::post("/event/<series>/<event>/enter", data = "<form>")]
pub(crate) async fn post(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'_>, client: &State<reqwest::Client>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, EnterForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
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
                if let Some(Flow { ref requirements }) = data.enter_flow {
                    if requirements.is_empty() {
                        if data.is_single_race() {
                            form.context.push_error(form::Error::validation("Signups for this event are not handled by Mido's House."));
                        }
                    } else {
                        for requirement in requirements {
                            let (is_checked, _, error_message) = {
                                let defaults = FormDefaultsRef::Context(&form.context);
                                requirement.check(discord_ctx, Some(&me), &data, uri!(get(data.series, &*data.event, defaults.my_role(), defaults.teammate())))
                            }.await?;
                            if !is_checked {
                                form.context.push_error(form::Error::validation(error_message));
                            }
                        }
                    }
                } else {
                    form.context.push_error(form::Error::validation("Signups for this event aren't open yet."));
                }
                if form.context.errors().next().is_none() {
                    let id = Id::new(&mut transaction, IdTable::Teams).await?;
                    sqlx::query!("INSERT INTO teams (id, series, event, plural_name) VALUES ($1, $2, $3, FALSE)", id as _, series as _, event).execute(&mut transaction).await?;
                    sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', 'none')", id as _, me.id as _).execute(&mut transaction).await?;
                    transaction.commit().await?;
                    return Ok(RedirectOrContent::Redirect(Redirect::to(uri!(super::status(series, event)))))
                }
            }
            TeamConfig::Pictionary => {
                let (my_role, teammate) = match (value.my_role, value.teammate) {
                    (Some(my_role), Some(teammate)) => {
                        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                            id = team
                            AND series = 'pic'
                            AND event = $1
                            AND member = $2
                            AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                        ) AS "exists!""#, event, i64::from(me.id), i64::from(teammate)).fetch_one(&mut transaction).await? {
                            form.context.push_error(form::Error::validation("A team with these members is already proposed for this race. Check your notifications to accept the invite, or ask your teammate to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
                        }
                        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                            id = team
                            AND series = 'pic'
                            AND event = $1
                            AND member = $2
                            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                        ) AS "exists!""#, event, i64::from(me.id)).fetch_one(&mut transaction).await? {
                            form.context.push_error(form::Error::validation("You are already signed up for this race."));
                        }
                        if !value.team_name.is_empty() && sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
                            series = 'pic'
                            AND event = $1
                            AND name = $2
                            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                        ) AS "exists!""#, event, value.team_name).fetch_one(&mut transaction).await? {
                            form.context.push_error(form::Error::validation("A team with this name is already signed up for this race.").with_name("team_name"));
                        }
                        if my_role == pic::Role::Sheikah && me.racetime_id.is_none() {
                            form.context.push_error(form::Error::validation("A racetime.gg account is required to enter as runner. Go to your profile and select “Connect a racetime.gg account”.").with_name("my_role")); //TODO direct link?
                        }
                        if teammate == me.id {
                            form.context.push_error(form::Error::validation("You cannot be your own teammate.").with_name("teammate"));
                        }
                        if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, i64::from(teammate)).fetch_one(&mut transaction).await? {
                            form.context.push_error(form::Error::validation("There is no user with this ID.").with_name("teammate"));
                        }
                        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                            id = team
                            AND series = 'pic'
                            AND event = $1
                            AND member = $2
                            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                        ) AS "exists!""#, event, i64::from(teammate)).fetch_one(&mut transaction).await? {
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
                    let id = Id::new(&mut transaction, IdTable::Teams).await?;
                    sqlx::query!("INSERT INTO teams (id, series, event, name) VALUES ($1, 'pic', $2, $3)", id as _, event, (!value.team_name.is_empty()).then(|| &value.team_name)).execute(&mut transaction).await?;
                    sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', $3)", id as _, me.id as _, Role::from(my_role.expect("validated")) as _).execute(&mut transaction).await?;
                    sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'unconfirmed', $3)", id as _, teammate.expect("validated") as _, match my_role.expect("validated") { pic::Role::Sheikah => Role::Gerudo, pic::Role::Gerudo => Role::Sheikah } as _).execute(&mut transaction).await?;
                    transaction.commit().await?;
                    return Ok(RedirectOrContent::Redirect(Redirect::to(uri!(super::status(series, event)))))
                }
            }
            _ => {
                let racetime_team = if let Some(ref racetime_team) = value.racetime_team {
                    if let Some(ref racetime_id) = me.racetime_id {
                        let user = client.get(format!("https://racetime.gg/user/{racetime_id}/data"))
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
                            if let Some(user) = User::from_racetime(&mut transaction, &member.id).await? {
                                if let Some(discord_id) = user.discord_id {
                                    if let Some(discord_guild) = data.discord_guild {
                                        if discord_guild.member(&*discord_ctx.read().await, discord_id).await.is_err() {
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
                                ) AS "exists!""#, series as _, event, i64::from(user.id)).fetch_one(&mut transaction).await? {
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
                                ) AS "exists!""#, series as _, event, i64::from(u1.id), i64::from(u2.id)).fetch_one(&mut transaction).await? {
                                    form.context.push_error(form::Error::validation("A team with these members is already proposed for this tournament. Check your notifications to accept the invite, or ask your teammate to do so.")); //TODO linkify notifications? More specific message based on whether viewer has confirmed?
                                },
                                [u1, u2, u3] => if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE
                                    series = $1
                                    AND event = $2
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $4)
                                    AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $5)
                                ) AS "exists!""#, series as _, event, i64::from(u1.id), i64::from(u2.id), i64::from(u3.id)).fetch_one(&mut transaction).await? {
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
                        (racetime_team.slug.clone(), racetime_team.name.clone(), users, roles, startgg_ids)
                    } else {
                        Default::default()
                    }
                } else {
                    Default::default()
                };
                if form.context.errors().next().is_none() {
                    return Ok(if value.step2 {
                        let id = Id::new(&mut transaction, IdTable::Teams).await?;
                        sqlx::query!("INSERT INTO teams (id, series, event, name, racetime_slug, restream_consent) VALUES ($1, $2, $3, $4, $5, $6)", id as _, series as _, event, (!team_name.is_empty()).then(|| team_name), team_slug, value.restream_consent).execute(&mut transaction).await?;
                        for ((user, role), startgg_id) in users.into_iter().zip_eq(roles).zip_eq(startgg_ids) {
                            sqlx::query!(
                                "INSERT INTO team_members (team, member, status, role, startgg_id) VALUES ($1, $2, $3, $4, $5)",
                                id as _, user.id as _, if user == me { SignupStatus::Created } else { SignupStatus::Unconfirmed } as _, role as _, startgg_id,
                            ).execute(&mut transaction).await?;
                        }
                        transaction.commit().await.map_err(Error::Sql)?;
                        RedirectOrContent::Redirect(Redirect::to(uri!(super::status(series, event))))
                    } else {
                        RedirectOrContent::Content(enter_form_step2(transaction, Some(me), uri, client, csrf, data, mw::EnterFormStep2Defaults::Values { racetime_team: racetime_team.expect("validated") }).await?)
                    })
                }
            }
        }
        if value.step2 {
            return Ok(RedirectOrContent::Content(enter_form_step2(transaction, Some(me), uri, client, csrf, data, mw::EnterFormStep2Defaults::Context(form.context)).await?))
        }
    }
    Ok(RedirectOrContent::Content(enter_form(transaction, discord_ctx, Some(me), uri, csrf, client, data, pic::EnterFormDefaults::Context(form.context)).await?))
}
