use {
    std::mem,
    futures::stream::TryStreamExt as _,
    horrorshow::{
        RenderBox,
        TemplateBuffer,
        box_html,
        html,
        rocket::TemplateExt as _,
    },
    itertools::Itertools as _,
    rocket::{
        FromForm,
        FromFormField,
        Responder,
        State,
        form::{
            self,
            Context,
            Contextual,
            Form,
        },
        response::{
            Debug,
            Redirect,
            content::Html,
        },
        uri,
    },
    sqlx::PgPool,
    crate::{
        PageError,
        PageResult,
        PageStyle,
        auth,
        favicon::ChestAppearances,
        page,
        user::User,
        util::{
            Id,
            IdTable,
        },
    },
};

#[derive(Debug, sqlx::Decode)]
#[sqlx(type_name = "signup_status", rename_all = "lowercase")]
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

#[derive(Debug, Clone, Copy, sqlx::Type, FromFormField)]
#[sqlx(type_name = "team_role", rename_all = "lowercase")]
pub(crate) enum Role {
    Sheikah,
    Gerudo,
}

impl Role {
    pub(crate) fn to_html(&self) -> Box<dyn RenderBox> { //TODO take parameter to render based on event kind
        match self {
            Self::Sheikah => box_html! {
                span(class = "sheikah") : "runner";
            },
            Self::Gerudo => box_html! {
                span(class = "gerudo") : "pilot";
            },
        }
    }
}

enum Tab {
    Info,
    Teams,
    Enter,
}

fn event_header(tab: Tab) -> Box<dyn RenderBox> {
    box_html! {
        h1 {
            a(class = "nav", href? = (!matches!(tab, Tab::Info)).then(|| uri!(pictionary_random_settings).to_string())) : "1st Random Settings Pictionary Spoiler Log Race";
        }
        h2 : "Saturday, May 14, 2021 • 20:00 CEST • 18:00 UTC • 2PM EDT";
        div(class = "button-row") {
            @if let Tab::Info = tab {
                span(class = "button selected") : "Info";
            } else {
                a(class = "button", href = uri!(pictionary_random_settings).to_string()) : "Info";
            }
            @if let Tab::Teams = tab {
                span(class = "button selected") : "Teams";
            } else {
                a(class = "button", href = uri!(pictionary_random_settings_teams).to_string()) : "Teams";
            }
            //a(class = "button") : "My Status"; //TODO (if in any teams, including unconfirmed ones)
            //TODO hide “Enter” and “Find Teammates” if in a confirmed team
            @if let Tab::Enter = tab {
                span(class = "button selected") : "Enter";
            } else {
                a(class = "button", href = uri!(pictionary_random_settings_enter).to_string()) : "Enter";
            }
            //a(class = "button") : "Find Teammates"; //TODO
            //a(class = "button") : "Volunteer"; //TODO
            //a(class = "button") : "Watch"; //TODO
        }
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum PictionaryRandomSettingsError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("missing user data for a race organizer")]
    OrganizerUserData,
}

#[rocket::get("/event/pic/rs1")]
pub(crate) async fn pictionary_random_settings(pool: &State<PgPool>, me: Option<User>) -> Result<Html<String>, PictionaryRandomSettingsError> {
    let tj = User::from_id(pool, Id(5961629664912637980)).await?.ok_or(PictionaryRandomSettingsError::OrganizerUserData)?;
    let fenhl = User::from_id(pool, Id(14571800683221815449)).await?.ok_or(PictionaryRandomSettingsError::OrganizerUserData)?;
    Ok(page(&pool, &me, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, "1st Random Settings Pictionary Spoiler Log Race", html! {
        main {
            : event_header(Tab::Info);
            article {
                h2 : "What is a Pictionary Spoiler Log Race?";
                p : "Each team consists of one Runner and one Spoiler Log Pilot who is drawing. The pilot has to figure out a way through the seed and how to tell their runner in drawing what checks they need to do. Hints are obviously disabled.";
                p : "This time, we are doing something slightly different: The settings will be random, with weights based on the Random Settings League but adjusted for Pictionary. To compensate for the additional complexity, the preparation time for the pilot will be 30 minutes instead of the usual 15.";
                p {
                    : "Before the race we will provide a room on ";
                    a(href = "https://aggie.io/") : "aggie.io";
                    : " to each team. The canvas will be set to 660×460 for restream purposes.";
                }
                p {
                    strong : "At the ±0 minute mark:";
                    : " The pilot is now allowed to look at the spoiler log and can start figuring out the route.";
                }
                p {
                    strong : "At the +30 minute mark:";
                    : " The pilot is allowed to start drawing and the runner is allowed to start the file.";
                }
                h2 : "Rules";
                p {
                    : "The race uses the ";
                    a(href = "https://rsl-leaderboard.web.app/rules") : "Random Settings League";
                    : " ruleset.";
                }
                p : "The pilot is allowed to communicate to their partner only via drawing and may watch and hear the stream of the runner. Runners may talk to their pilot. We would prefer if the pilot did not directly respond to questions, as figuring things out is supposed to be part of the challenge, but in the end it's up to the individual teams.";
                p {
                    strong : "Allowed:";
                    : " Arrows, Question marks, ingame symbols, check marks, “X” for crossing out stuff.";
                }
                p {
                    strong : "Not allowed:";
                    : " Any kind of numbers or letters.";
                }
                h3 : "Examples";
                p : "For having a better idea what we mean in regards with the rules / communication, here are some examples:";
                ol {
                    li {
                        : "The pilot draws 3 spiders and a bow. The runner then asks if there is a bow on 30 skulls. The pilot then draws a smiley or a checkmark for confirmation or a sad face for “no” — that is ";
                        strong : "allowed";
                        : ".";
                    }
                    li {
                        : "The runner just asks without a drawing if it's AD or if a specific check is required — that is ";
                        strong : "not allowed";
                        : ".";
                    }
                    li {
                        : "The team has prepared a language for specific checks to avoid the requirement to draw the check (like morse code etc.) — that is ";
                        strong : "not allowed";
                        : ".";
                    }
                    li {
                        : "The runner says “if I need to do the toilet check, draw a heart” — that is ";
                        strong : "not allowed";
                        : ".";
                    }
                    li {
                        : "The runner says: “since you didn't draw anything in the Lost Woods, I'm gonna skip all the checks there and go immediately to the Sacred Forest Meadow” — that is ";
                        strong : "allowed";
                        : ".";
                    }
                }
                h2 : "Settings";
                p {
                    : "We will be using ";
                    a(href = "https://github.com/fenhl/plando-random-settings/blob/dev-fenhl/weights/pictionary_override.json") : "a special weights override";
                    : " for Pictionary spoiler log races. Changes include:";
                }
                ul {
                    li : "To reduce complexity for the pilot, overworld ER is disabled.";
                    li : "Master Quest dungeons are disabled due to a lack of documentation for spoiler log location names.";
                    li {
                        : "Some of the settings and combinations of settings that are disabled in RSL for information-related reasons are turned back on, since they're not an issue if you have the spoiler log:";
                        ul {
                            li : "Triforce hunt + minimal item pool";
                            li : "Ice trap mayhem/onslaught + quad damage/OHKO";
                            li : "Separate keysanity setting for the Thieves' Hideout";
                            li : "Random scrub prices without a starting wallet";
                            li : "All goals reachable (33% chance)";
                        }
                    }
                    li {
                        : "The seed will be rolled on ";
                        a(href = "https://github.com/fenhl/OoT-Randomizer") : "Fenhl's branch";
                        : ", so some settings that aren't in Dev-R are added:";
                        ul {
                            li : "Heart container requirements for rainbow bridge and/or Ganon boss key (50% chance each to replace a skulltula token requirement)";
                            li : "Full one-way entrance randomization (owls, warp songs, and spawns can lead to more destinations; 25% chance each)";
                            li : "One bonk KO (5% chance)";
                        }
                    }
                    li {
                        : "Some newer settings that are not yet included in RSL due to the ongoing tournament are enabled:";
                        ul {
                            li : "Planted magic beans (50% chance)";
                            li : "Key rings for all dungeons (20% chance)";
                        }
                    }
                    li {
                        : "The following settings that would give the runner hints or similar information are disabled:";
                        ul {
                            li : "Maps & compasses give info";
                            li : "Chest appearance matches contents";
                            li : "Gossip stone hints";
                            li : "Temple of Time altar hints";
                            li : "Ganondorf light arrows hint";
                            li : "Warp song text boxes hinting destinations";
                        }
                    }
                }
                p {
                    : "Everything else is the same as ";
                    a(href = "https://rsl-leaderboard.web.app/weights") : "the usual RSL weights";
                    : ".";
                }
                //TODO sample seeds?
                h2 : "Further information";
                p {
                    : "The race is organized by ";
                    : tj.to_html();
                    : ", ksinjah, ";
                    : fenhl.to_html();
                    : ", melqwii, and TeaGrenadier. We will answer questions and inform about recent events on The Silver Gauntlets Discord in the #pictionary-spoiler-log channel (";
                    a(href = "https://discord.gg/m8z8ZqtN8H") : "invite link";
                    : " • ";
                    a(href = "https://discord.com/channels/663207960432082944/865206020015128586") : "direct channel link";
                    : "). If you have any questions, feel free to ask there!";
                }
                p : "Special thanks to winniedemon who will be helping us keep important posts from getting lost in the Discord!";
            }
        }
    }).await?)
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum PictionaryRandomSettingsTeamsError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("team with nonexistent user")]
    NonexistentUser,
}

#[rocket::get("/event/pic/rs1/teams")]
pub(crate) async fn pictionary_random_settings_teams(pool: &State<PgPool>, me: Option<User>) -> Result<Html<String>, PictionaryRandomSettingsTeamsError> {
    let mut signups = Vec::default();
    let mut teams_query = sqlx::query!(r#"SELECT id AS "id!: Id", name FROM teams WHERE
        series = 'pic'
        AND event = 'rs1'
        AND (
            EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $1)
            OR NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        )
    "#, me.as_ref().map(|me| i64::from(me.id))).fetch(&**pool);
    while let Some(team) = teams_query.try_next().await? {
        let runner = sqlx::query!(r#"SELECT member AS "id: Id", status AS "status: SignupStatus" FROM team_members WHERE team = $1 AND role = 'sheikah'"#, i64::from(team.id)).fetch_one(&**pool).await?;
        let pilot = sqlx::query!(r#"SELECT member AS "id: Id", status AS "status: SignupStatus" FROM team_members WHERE team = $1 AND role = 'gerudo'"#, i64::from(team.id)).fetch_one(&**pool).await?;
        let runner_confirmed = runner.status.is_confirmed();
        let runner = User::from_id(&pool, runner.id).await?.ok_or(PictionaryRandomSettingsTeamsError::NonexistentUser)?;
        let pilot_confirmed = pilot.status.is_confirmed();
        let pilot = User::from_id(&pool, pilot.id).await?.ok_or(PictionaryRandomSettingsTeamsError::NonexistentUser)?;
        signups.push((team.name, runner, runner_confirmed, pilot, pilot_confirmed));
    }
    Ok(page(&pool, &me, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, "Teams — 1st Random Settings Pictionary Spoiler Log Race", html! {
        main {
            : event_header(Tab::Teams);
            table {
                thead {
                    tr {
                        th : "Team Name";
                        th(class = "sheikah") : "Runner";
                        th(class = "gerudo") : "Pilot";
                    }
                }
                tbody {
                    @if signups.is_empty() {
                        tr {
                            td(colspan = "3") {
                                i : "(no signups yet)";
                            }
                        }
                    } else {
                        @for (team_name, runner, runner_confirmed, pilot, pilot_confirmed) in signups {
                            tr {
                                td : team_name.unwrap_or_default();
                                td(class = "sheikah") {
                                    : runner.to_html();
                                    @if !runner_confirmed {
                                        : " (unconfirmed)";
                                    }
                                }
                                td(class = "gerudo") {
                                    : pilot.to_html();
                                    @if !pilot_confirmed {
                                        : " (unconfirmed)";
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }).await?)
}

fn render_form_error(tmpl: &mut TemplateBuffer<'_>, error: &form::Error<'_>) {
    tmpl << html! {
        p(class = "error") : error.to_string();
    };
}

fn field_errors(tmpl: &mut TemplateBuffer<'_>, errors: &mut Vec<&form::Error<'_>>, name: &str) {
    let field_errors;
    (field_errors, *errors) = mem::take(errors).into_iter().partition(|error| error.is_for(name));
    tmpl << html! {
        @for error in field_errors {
            |tmpl| render_form_error(tmpl, error);
        }
    };
}

async fn pictionary_random_settings_enter_form(pool: &PgPool, me: Option<User>, context: Context<'_>) -> PageResult {
    page(pool, &me, PageStyle { chests: ChestAppearances::VANILLA, ..PageStyle::default() }, "Enter — 1st Random Settings Pictionary Spoiler Log Race", if me.is_some() {
        let mut errors = context.errors().collect_vec();
        let form_content = html! {
            //TODO CSRF protection (rocket_csrf crate?)
            legend {
                : "Fill out this form to enter the race as a team.";
                /*
                : " If you don't have a teammate yet, you can ";
                a(href = unimplemented!(/*TODO*/)) : "look for a teammate";
                : " instead.";
                */
            }
            fieldset {
                |tmpl| field_errors(tmpl, &mut errors, "my_role");
                label(for = "my_role") : "My Role:";
                input(id = "my_role-sheikah", class = "sheikah", type = "radio", name = "my_role", value = "sheikah", checked? = context.field_value("my_role") == Some("sheikah"));
                label(class = "sheikah", for = "my_role-sheikah") : "Runner";
                input(id = "my_role-gerudo", class = "gerudo", type = "radio", name = "my_role", value = "gerudo", checked? = context.field_value("my_role") == Some("gerudo"));
                label(class = "gerudo", for = "my_role-gerudo") : "Pilot";
            }
            fieldset {
                |tmpl| field_errors(tmpl, &mut errors, "teammate");
                label(for = "teammate") : "Teammate:";
                input(type = "text", name = "teammate", value? = context.field_value("teammate"));
                label(class = "help") : "(Enter your teammate's Mido's House user ID. It can be found on their profile page.)"; //TODO add JS-based user search?
            }
            fieldset {
                input(type = "submit", value = "Submit");
            }
        }.write_to_html()?;
        html! {
            main {
                : event_header(Tab::Enter);
                form(action = uri!(pictionary_random_settings_enter_post).to_string(), method = "post") {
                    @for error in errors {
                        |tmpl| render_form_error(tmpl, error);
                    }
                    : form_content;
                }
            }
        }.write_to_html()?
    } else {
        html! {
            main {
                : event_header(Tab::Enter);
                article {
                    p {
                        a(href = uri!(auth::login).to_string()) : "Sign in or create a Mido's House account";
                        : " to enter this race.";
                    }
                }
            }
        }.write_to_html()?
    }).await
}

#[rocket::get("/event/pic/rs1/enter")]
pub(crate) async fn pictionary_random_settings_enter(pool: &State<PgPool>, me: Option<User>) -> PageResult {
    pictionary_random_settings_enter_form(&pool, me, Context::default()).await
}

#[derive(FromForm)]
pub(crate) struct EnterForm {
    my_role: Role,
    teammate: Id,
}

#[derive(Responder)]
pub(crate) enum PictionaryRandomSettingsEnterPostResponse {
    Redirect(Redirect),
    Content(Html<String>),
}

#[rocket::post("/event/pic/rs1/enter", data = "<form>")]
pub(crate) async fn pictionary_random_settings_enter_post(pool: &State<PgPool>, me: User, form: Form<Contextual<'_, EnterForm>>) -> Result<PictionaryRandomSettingsEnterPostResponse, PageError> {
    let mut form = form.into_inner();
    if let Some(ref value) = form.value {
        let mut transaction = pool.begin().await?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = 'pic'
            AND event = 'rs1'
            AND member = $1
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) as "exists!""#, i64::from(me.id)).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("You are already signed up for this race."));
        }
        if value.teammate == me.id {
            form.context.push_error(form::Error::validation("You cannot be your own teammate.").with_name("teammate"));
        } else {
            if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) as "exists!""#, i64::from(value.teammate)).fetch_one(&mut transaction).await? {
                form.context.push_error(form::Error::validation("There is no user with this ID.").with_name("teammate"));
            } else {
                if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
                    id = team
                    AND series = 'pic'
                    AND event = 'rs1'
                    AND member = $1
                    AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
                ) as "exists!""#, i64::from(value.teammate)).fetch_one(&mut transaction).await? {
                    form.context.push_error(form::Error::validation("This user is already signed up for this race.").with_name("teammate"));
                }
                //TODO check to make sure the teammate hasn't blocked the user submitting the form (or vice versa) or the event
            }
        }
        if form.context.errors().next().is_some() {
            pictionary_random_settings_enter_form(&pool, Some(me), form.context).await
                .map(PictionaryRandomSettingsEnterPostResponse::Content)
        } else {
            let id = Id::new(&mut transaction, IdTable::Teams).await?;
            sqlx::query!("INSERT INTO teams (id, series, event) VALUES ($1, 'pic', 'rs1')", i64::from(id)).execute(&mut transaction).await?; //TODO allow setting team name
            sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', $3)", i64::from(id), i64::from(me.id), value.my_role as Role).execute(&mut transaction).await?;
            sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'unconfirmed', $3)", i64::from(id), i64::from(value.teammate), match value.my_role { Role::Sheikah => Role::Gerudo, Role::Gerudo => Role::Sheikah } as Role).execute(&mut transaction).await?;
            transaction.commit().await?;
            Ok(PictionaryRandomSettingsEnterPostResponse::Redirect(Redirect::to(uri!(pictionary_random_settings_teams)))) //TODO redirect to “My Status” page instead
        }
    } else {
        pictionary_random_settings_enter_form(&pool, Some(me), form.context).await
            .map(PictionaryRandomSettingsEnterPostResponse::Content)
    }
}

#[rocket::post("/event/pic/rs1/confirm/<team>")]
pub(crate) async fn pictionary_random_settings_confirm_signup(pool: &State<PgPool>, me: User, team: Id) -> Result<Redirect, Debug<sqlx::Error>> {
    //TODO CSRF protection
    //TODO send notification to everyone else on the team with status 'created' or 'confirmed'
    sqlx::query!("UPDATE team_members SET status = 'confirmed' WHERE team = $1 AND member = $2 AND status = 'unconfirmed'", i64::from(team), i64::from(me.id)).execute(&**pool).await?;
    Ok(Redirect::to(uri!(pictionary_random_settings_teams)))
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum PictionaryRandomSettingsResignError {
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("can't delete teams you're not part of")]
    NotInTeam,
}

#[rocket::post("/event/pic/rs1/resign/<team>")]
pub(crate) async fn pictionary_random_settings_resign(pool: &State<PgPool>, me: User, team: Id) -> Result<Redirect, PictionaryRandomSettingsResignError> {
    //TODO CSRF protection
    let mut transaction = pool.begin().await?;
    let delete = sqlx::query!(r#"DELETE FROM team_members WHERE team = $1 RETURNING member AS "id: Id", status AS "status: SignupStatus""#, i64::from(team)).fetch_all(&mut transaction).await?;
    let mut me_in_team = false;
    for member in delete {
        if member.id == me.id {
            me_in_team = true;
        } else if member.status.is_confirmed() {
            //TODO use 'decline' notification kind if this was a declined invite
            sqlx::query!("INSERT INTO notifications (rcpt, kind, series, event, sender) VALUES ($1, 'resign', 'pic', 'rs1', $2)", i64::from(member.id), i64::from(me.id)).execute(&mut transaction).await?;
        }
    }
    if me_in_team {
        sqlx::query!("DELETE FROM teams WHERE id = $1", i64::from(team)).execute(&mut transaction).await?;
        transaction.commit().await?;
        Ok(Redirect::to(uri!(pictionary_random_settings_teams)))
    } else {
        transaction.rollback().await?;
        Err(PictionaryRandomSettingsResignError::NotInTeam)
    }
}
