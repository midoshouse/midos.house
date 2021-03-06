use {
    std::{
        borrow::Cow,
        collections::HashMap,
        fmt,
        iter,
    },
    chrono::prelude::*,
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
        ContextualExt as _,
        CsrfForm,
        Origin,
        ToHtml,
        html,
    },
    serde::Deserialize,
    serenity::client::Context as DiscordCtx,
    serenity_utils::RwFuture,
    sqlx::PgPool,
    crate::{
        auth,
        event::{
            Data,
            Error,
            InfoError,
            Series,
            SignupStatus,
            Tab,
        },
        http::{
            PageStyle,
            page,
        },
        seed,
        user::User,
        util::{
            DateTimeFormat,
            Id,
            IdTable,
            RedirectOrContent,
            StatusOrError,
            form_field,
            format_datetime,
            natjoin,
            render_form_error,
        },
    },
};

const SERIES: Series = Series::Multiworld;

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
                        : natjoin(organizers);
                        : ". Click the ???teams??? link above to see the results of the qualifier async.";
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
        context.push_error(form::Error::validation("A racetime.gg account is required to enter this tournament. Go to your profile and select ???Connect a racetime.gg account???.")); //TODO direct link?
        None
    })
}

pub(super) async fn enter_form(me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, context: Context<'_>, client: &reqwest::Client) -> Result<RawHtml<String>, Error> {
    let header = data.header(me.as_ref(), Tab::Enter).await?;
    Ok(page(&data.pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter ??? {}", data.display_name), if let Some(ref me) = me {
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

#[derive(FromForm, CsrfForm)]
pub(crate) struct EnterForm {
    #[field(default = String::new())]
    csrf: String,
    racetime_team: String,
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
                        : " ??? ";
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
        Ok(page(&data.pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter ??? {}", data.display_name), page_content).await?)
    }
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct EnterFormStep2 {
    #[field(default = String::new())]
    csrf: String,
    racetime_team: String,
    world_number: HashMap<String, Role>,
}

#[rocket::post("/event/mw/<event>/enter/step2", data = "<form>")]
pub(crate) async fn enter_post_step2<'a>(pool: &State<PgPool>, discord_ctx: &State<RwFuture<DiscordCtx>>, me: User, uri: Origin<'a>, client: &State<reqwest::Client>, csrf: Option<CsrfToken>, event: &'a str, form: Form<Contextual<'a, EnterFormStep2>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
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
            RedirectOrContent::Redirect(Redirect::to(uri!(super::teams(SERIES, event))))
        }
    } else {
        RedirectOrContent::Content(enter_form_step2(Some(me), uri, csrf, data, EnterFormStep2Defaults::Context(form.context)).await?)
    })
}

pub(super) async fn status(pool: &PgPool, csrf: Option<CsrfToken>, data: &Data<'_>, team_id: Id, context: Context<'_>) -> sqlx::Result<RawHtml<String>> {
    Ok(if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM asyncs WHERE series = $1 AND event = $2) AS "exists!""#, data.series as _, &data.event).fetch_one(pool).await? {
        if let Some(row) = sqlx::query!("SELECT requested, submitted FROM async_teams WHERE team = $1", i64::from(team_id)).fetch_optional(pool).await? {
            if row.submitted.is_some() {
                //TODO if any vods are still missing, show form to add them
                html! {
                    p : "Waiting for the start of the tournament and round 1 pairings. Keep an eye out for an announcement on Discord."; //TODO include start date?
                }
            } else {
                let seed = seed::Data { web: Some(seed::OotrWebData { id: 1114700, gen_time: Utc.ymd(2022, 6, 11).and_hms(19, 1, 32) }), file_stem: Cow::Borrowed("OoTR_1114700_T639YJS1TZ") }; //TODO replace with mw/3 qualifier async, get seed data from database
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
                        label(class = "help") : "(If you plan on uploading the VoD to YouTube later, leave this field blank and DM an admin once it is ready.)"; //TODO option to submit vods later
                    });
                    : form_field("time2", &mut errors, html! {
                        label(for = "time2", class = "wisdom") : "Player 2 Finishing Time:";
                        input(type = "text", name = "time2", value? = context.field_value("time2")); //TODO h:m:s fields?
                        label(class = "help") : "(If player 2 did not finish, leave this field blank.)";
                    });
                    : form_field("vod2", &mut errors, html! {
                        label(for = "vod2", class = "wisdom") : "Player 2 VoD:";
                        input(type = "text", name = "vod2", value? = context.field_value("vod2"));
                        label(class = "help") : "(If you plan on uploading the VoD to YouTube later, leave this field blank and DM an admin once it is ready.)"; //TODO option to submit vods later
                    });
                    : form_field("time3", &mut errors, html! {
                        label(for = "time3", class = "courage") : "Player 3 Finishing Time:";
                        input(type = "text", name = "time3", value? = context.field_value("time3")); //TODO h:m:s fields?
                        label(class = "help") : "(If player 3 did not finish, leave this field blank.)";
                    });
                    : form_field("vod3", &mut errors, html! {
                        label(for = "vod3", class = "courage") : "Player 3 VoD:";
                        input(type = "text", name = "vod3", value? = context.field_value("vod3"));
                        label(class = "help") : "(If you plan on uploading the VoD to YouTube later, leave this field blank and DM an admin once it is ready.)"; //TODO option to submit vods later
                    });
                    : form_field("fpa", &mut errors, html! {
                        label(for = "fpa") {
                            : "If you would like to invoke the ";
                            a(href = "https://docs.google.com/document/d/1BbvHJF8vtyrte76jpoCVQBTy9MYStpN3vr2PLdiCIMk/edit") : "Fair Play Agreement";
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
                            : format_datetime(row.requested, DateTimeFormat { long: true, running_text: true });
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
                        li : "This seed must be attempted in order to play in the tournament. In the event of a forfeit, you will be granted a bottom-half seed for the first Swiss round.";
                        li : "The time must be submitted by the starting time of the tournament, which is yet to be announced."; //TODO explicitly state deadline if there is one
                        li : "You must start the seed within 30 minutes of obtaining it and submit your time within 30 minutes of the last finish. Any additional time taken will be added to your final time. If anything prevents you from obtaining the seed/submitting your time, please DM an admin (or ping the Discord role) to get it sorted out.";
                        li : "While required for the tournament, the results from the qualifier seed will only be used in the first round of Swiss pairings. The teams in the top half of finishers will be paired with a team from the bottom half of finishers for the first round. After the first round, pairings will be purely based on Swiss matchmaking.";
                        li : "While you are not strictly required to stream, you must have video proof of your run. Feel free to simply record your run and upload it to YouTube and provide a link. If you do stream or make your upload public, please make sure it is clearly marked so people can avoid spoilers. If you're a big streamer, be extra sure to note what is happening, as several of your viewers are likely going to want to participate as well.";
                        li : "Do not spoil yourself on this seed by watching another playthrough. If you do stream, you are responsible for what your chat says, so either do not read chat, set it to emote only, or take the risk at your own discretion. If you do get spoiled, please report it to the admins, we will try to work out something equitable.";
                        li : "You must use the world numbers with which you signed up for this seed. Once you request the seed, the world numbers you selected are the world numbers you play with for the rest of the tournament. If you wish to change your player order, do not request the qualifier and contact an admin."; //TODO allow changing player order in options below
                        li {
                            : "This should be run like an actual race. In the event of a technical issue, teams are allowed to invoke the ";
                            a(href = "https://docs.google.com/document/d/1BbvHJF8vtyrte76jpoCVQBTy9MYStpN3vr2PLdiCIMk/edit") : "Fair Play Agreement";
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
