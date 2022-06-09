use {
    std::{
        collections::HashMap,
        fmt,
    },
    enum_iterator::Sequence,
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
        Origin,
        ToHtml,
        html,
    },
    serde::Deserialize,
    sqlx::PgPool,
    crate::{
        PageStyle,
        auth,
        event::{
            Data,
            Error,
            InfoError,
            Series,
            SignupStatus,
            Tab,
        },
        page,
        user::User,
        util::{
            ContextualExt as _,
            CsrfForm,
            Id,
            IdTable,
            RedirectOrContent,
            StatusOrError,
            form_field,
            natjoin,
            render_form_error,
        },
    },
};

const SERIES: Series = Series::Multiworld;

pub(super) async fn info(pool: &PgPool, event: &str) -> Result<RawHtml<String>, InfoError> {
    Ok(match event {
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
                        : "This is a placeholder page for the third Ocarina of Time randomizer multiworld tournament, organized by ";
                        : natjoin(organizers);
                        : ". More infos coming soon.";
                    }
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
struct RaceTimeTeamData {
    name: String,
    slug: String,
    members: Vec<RaceTimeTeamMember>,
}

#[derive(Clone, Deserialize)]
struct RaceTimeTeamMember {
    id: String,
    name: String,
}

async fn validate_team(me: &User, client: &reqwest::Client, context: &mut Context<'_>, team_slug: &str) -> reqwest::Result<Option<RaceTimeTeamData>> {
    Ok(if let Some(ref racetime_id) = me.racetime_id {
        let user = client.get(format!("https://racetime.gg/user/{racetime_id}/data"))
            .send().await?
            .error_for_status()?
            .json::<RaceTimeUser>().await?;
        if user.teams.iter().any(|team| team.slug == team_slug) {
            let team = client.get(format!("https://racetime.gg/team/{team_slug}/data"))
                .send().await?
                .error_for_status()?
                .json::<RaceTimeTeamData>().await?;
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

pub(super) async fn enter_form(me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, context: Context<'_>, client: &reqwest::Client) -> Result<RawHtml<String>, Error> {
    let header = data.header(me.as_ref(), Tab::Enter).await?;
    Ok(page(&data.pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), if let Some(ref me) = me {
        if let Some(ref racetime_id) = me.racetime_id {
            let racetime_user = client.get(format!("https://racetime.gg/user/{racetime_id}/data"))
                .send().await?
                .error_for_status()?
                .json::<RaceTimeUser>().await?;
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
                    form(action = uri!(enter_post(&*data.event)).to_string(), method = "post") {
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

#[derive(FromForm)]
pub(crate) struct EnterForm {
    #[field(default = String::new())]
    csrf: String,
    racetime_team: String,
}

impl CsrfForm for EnterForm { //TODO derive
    fn csrf(&self) -> &String { &self.csrf }
}

#[rocket::post("/event/mw/<event>/enter", data = "<form>")]
pub(crate) async fn enter_post(pool: &State<PgPool>, me: User, uri: Origin<'_>, client: &State<reqwest::Client>, csrf: Option<CsrfToken>, event: &str, form: Form<Contextual<'_, EnterForm>>) -> Result<RawHtml<String>, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), SERIES, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started() {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        let racetime_team = validate_team(&me, client, &mut form.context, &value.racetime_team).await?;
        if form.context.errors().next().is_some() {
            enter_form(Some(me), uri, csrf, data, form.context, client).await?
        } else {
            enter_form_step2(Some(me), uri, csrf, data, EnterFormStep2Defaults::Values { racetime_team: racetime_team.expect("validated") }).await?
        }
    } else {
        enter_form(Some(me), uri, csrf, data, form.context, client).await?
    })
}

enum EnterFormStep2Defaults<'a> {
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

    //TODO fix the lifetime issues and make the HTTP request async
    fn racetime_members(&self) -> impl Future<Output = reqwest::Result<Vec<RaceTimeTeamMember>>> {
        match self {
            Self::Context(ctx) => if let Some(team_slug) = ctx.field_value("racetime_team") {
                let url = format!("https://racetime.gg/team/{team_slug}/data");
                async move {
                    Ok(reqwest::get(url).await?
                        .error_for_status()?
                        .json::<RaceTimeTeamData>().await?
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
}

fn enter_form_step2<'a>(me: Option<User>, uri: Origin<'a>, csrf: Option<CsrfToken>, data: Data<'a>, defaults: EnterFormStep2Defaults<'a>) -> impl Future<Output = Result<RawHtml<String>, Error>> + 'a {
    let team_members = defaults.racetime_members();
    async move {
        let header = data.header(me.as_ref(), Tab::Enter).await?;
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
                }
                //TODO restream consent?
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
        Ok(page(&data.pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), page_content).await?)
    }
}

#[derive(FromForm)]
pub(crate) struct EnterFormStep2 {
    #[field(default = String::new())]
    csrf: String,
    racetime_team: String,
    world_number: HashMap<String, Role>,
}

impl CsrfForm for EnterFormStep2 { //TODO derive
    fn csrf(&self) -> &String { &self.csrf }
}

#[rocket::post("/event/mw/<event>/enter/step2", data = "<form>")]
pub(crate) async fn enter_post_step2<'a>(pool: &State<PgPool>, me: User, uri: Origin<'a>, client: &State<reqwest::Client>, csrf: Option<CsrfToken>, event: &'a str, form: Form<Contextual<'a, EnterFormStep2>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), SERIES, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started() {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        let mut transaction = pool.begin().await?;
        // verify team again since it's sent by the client
        let (team_slug, team_name, users, roles) = if let Some(racetime_team) = validate_team(&me, client, &mut form.context, &value.racetime_team).await? {
            let mut all_accounts_exist = true;
            let mut users = Vec::default();
            let mut roles = Vec::default();
            for member in &racetime_team.members {
                if let Some(user) = User::from_racetime(&mut transaction, &member.id).await? {
                    if user.discord_id.is_none() { //TODO also check tournament server membership?
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
            for role in enum_iterator::all::<Role>() {
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
            (racetime_team.slug, racetime_team.name, users, roles)
        } else {
            Default::default()
        };
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(enter_form_step2(Some(me), uri, csrf, data, EnterFormStep2Defaults::Context(form.context)).await?)
        } else {
            let id = Id::new(&mut transaction, IdTable::Teams).await?;
            sqlx::query!("INSERT INTO teams (id, series, event, name, racetime_slug) VALUES ($1, 'mw', $2, $3, $4)", id as _, event, (!team_name.is_empty()).then(|| team_name), team_slug).execute(&mut transaction).await?;
            for (user, role) in users.into_iter().zip_eq(roles) {
                sqlx::query!(
                    "INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, $3, $4)",
                    id as _, user.id as _, if user == me { SignupStatus::Created } else { SignupStatus::Unconfirmed } as _, super::Role::from(role) as _,
                ).execute(&mut transaction).await?;
            }
            transaction.commit().await?;
            //TODO create and assign Discord roles
            RedirectOrContent::Redirect(Redirect::to(uri!(super::teams(SERIES, event))))
        }
    } else {
        RedirectOrContent::Content(enter_form_step2(Some(me), uri, csrf, data, EnterFormStep2Defaults::Context(form.context)).await?)
    })
}
