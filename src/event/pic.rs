use {
    std::{
        borrow::Cow,
        cmp::Ordering::*,
    },
    chrono::prelude::*,
    futures::stream::{
        self,
        StreamExt as _,
        TryStreamExt as _,
    },
    itertools::Itertools as _,
    rocket::{
        FromForm,
        FromFormField,
        State,
        UriDisplayQuery,
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
    sqlx::PgPool,
    crate::{
        PageStyle,
        auth,
        event::{
            Data,
            Error,
            FindTeamError,
            InfoError,
            Tab,
        },
        page,
        seed,
        user::User,
        util::{
            ContextualExt as _,
            CsrfForm,
            Id,
            IdTable,
            RedirectOrContent,
            StatusOrError,
            field_errors,
            natjoin,
            render_form_error,
        },
    },
};

pub(super) async fn info(pool: &PgPool, event: &str) -> Result<RawHtml<String>, InfoError> {
    let is_random_settings = event.starts_with("rs");
    let settings = match event {
        "5" => html! {
            ul {
                li : "S5 base";
                li : "CSMC off";
                li : "no hints (including altar)";
                li : "Ganon boss key on 20 hearts";
                li : "vanilla bridge (Shadow and Spirit medallions + light arrows)";
                li : "cowsanity";
                li : "dungeon skulls";
                li : "30/40/50 skulls disabled";
                li : "shops 4 (reminder: no numbers allowed)";
            }
            p {
                : "The seed will be rolled on ";
                a(href = "https://github.com/fenhl/OoT-Randomizer/tree/valentine-pictionary") : "a custom branch";
                : " to support the heart wincon. The branch is based on Dev 6.2.1 and contains these settings as a preset called “5th Pictionary Spoiler Log Race”.";
            }
        },
        "6" => html! {
            p : "The settings are mostly a repeat of the 3rd Pictionary spoiler log race (the first one we organized), with the difference that cows are shuffled and 40 and 50 skulls are turned off:";
            ul {
                li : "S5 base";
                li : "CSMC off";
                li : "no hints (including altar)";
                li : "2 medallion bridge";
                li : "Ganon boss key on 6 medallions";
                li : "all skulls shuffled";
                li : "40 and 50 skulls disabled";
                li : "shuffled ocarinas";
                li : "shuffled Gerudo card";
                li : "shuffled cows";
            }
            p {
                : "Settings string for version 6.2: ";
                code: "AJTWFCHYKAA8KLAH2UASAHCCYCHGLTDDAKJ8S8AAJAEAC2AJSDGBLADLED7JKQUXEANKCAJAAENAABFAB";
            }
        },
        "rs1" => html! {
            p {
                : "The seed will be rolled on ";
                a(href = "https://github.com/fenhl/plando-random-settings/tree/a08223927138c6f039c1aa3603130d8bd900fb48") : "version 2.2.10 Fenhl-5";
                : " of the random settings script. We will be using ";
                a(href = "https://github.com/fenhl/plando-random-settings/blob/a08223927138c6f039c1aa3603130d8bd900fb48/weights/pictionary_override.json") : "a special weights override";
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
                    a(href = "https://github.com/fenhl/OoT-Randomizer/tree/d7d16553252b96bd0f50ef96c2af250b7bfbba58") : "Fenhl's branch";
                    : ", so some settings that aren't in Dev-R are added:";
                    ul {
                        li : "Heart container requirements for rainbow bridge and/or Ganon boss key (50% chance each to replace a skulltula token requirement)";
                        li : "Full one-way entrance randomization (owls, warp songs, and spawns can lead to more destinations; 25% chance each)";
                        li : "One bonk KO (5% chance)";
                        li : "Closed Kokiri Forest exit (50% chance, independent of Closed/Open Deku)";
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
        },
        _ => unimplemented!(),
    };
    let sample_seeds = match event {
        "5" => Some(seed::table(stream::iter(vec![
            seed::Data { web: None, file_stem: Cow::Borrowed("OoT_F35CF_7F1NK3FEGY") },
            seed::Data { web: None, file_stem: Cow::Borrowed("OoT_F35CF_XULLQE310I") },
            seed::Data { web: None, file_stem: Cow::Borrowed("OoT_F35CF_3PT90NK69D") },
            seed::Data { web: None, file_stem: Cow::Borrowed("OoT_F35CF_I7BN7K3S2Z") },
            seed::Data { web: None, file_stem: Cow::Borrowed("OoT_F35CF_99YI7I0K6O") },
        ])).await.map_err(InfoError::Io)?),
        "rs1" => Some(seed::table(stream::iter(vec![
            seed::Data { web: Some(seed::OotrWebData { id: 1079630, gen_time: Utc.ymd(2022, 4, 22).and_hms(15, 59, 36) }), file_stem: Cow::Borrowed("OoTR_1079630_V6516H22IW") },
            seed::Data { web: Some(seed::OotrWebData { id: 1079637, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 1, 5) }), file_stem: Cow::Borrowed("OoTR_1079637_HAH75EOAHQ") },
            seed::Data { web: Some(seed::OotrWebData { id: 1079645, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 3, 19) }), file_stem: Cow::Borrowed("OoTR_1079645_6XZJOSDCRW") },
            seed::Data { web: Some(seed::OotrWebData { id: 1079646, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 3, 53) }), file_stem: Cow::Borrowed("OoTR_1079646_AJZWAB1X3U") },
            seed::Data { web: Some(seed::OotrWebData { id: 1079648, gen_time: Utc.ymd(2022, 4, 22).and_hms(16, 4, 11) }), file_stem: Cow::Borrowed("OoTR_1079648_1DHCCQB5AC") },
        ])).await.map_err(InfoError::Io)?),
        _ => None,
    };
    let organizers = stream::iter([
        5961629664912637980, // Tjongejonge_
        2689982510832487907, // ksinjah
        14571800683221815449, // Fenhl
        3722744861553903438, // melqwii
        14099802746436324950, // TeaGrenadier
    ])
        .map(Id)
        .then(|id| async move { User::from_id(pool, id).await?.ok_or(InfoError::OrganizerUserData) })
        .try_collect::<Vec<_>>().await?;
    Ok(html! {
        article {
            h2 : "What is a Pictionary Spoiler Log Race?";
            p : "Each team consists of one Runner and one Spoiler Log Pilot who is drawing. The pilot has to figure out a way through the seed and how to tell their runner in drawing what checks they need to do. Hints are obviously disabled.";
            @if is_random_settings {
                p : "This time, we are doing something slightly different: The settings will be random, with weights based on the Random Settings League but adjusted for Pictionary. To compensate for the additional complexity, the preparation time for the pilot will be 30 minutes instead of the usual 15.";
            }
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
                strong {
                    : "At the ";
                    @if is_random_settings {
                        : "+30";
                    } else {
                        : "+15";
                    }
                    : " minute mark:";
                }
                : " The pilot is allowed to start drawing and the runner is allowed to start the file.";
            }
            h2 : "Rules";
            p {
                : "The race uses the ";
                @if is_random_settings {
                    a(href = "https://rsl-leaderboard.web.app/rules") : "Random Settings League";
                } else {
                    a(href = "https://wiki.ootrandomizer.com/index.php?title=Standard") : "Standard";
                }
                : " ruleset.";
            }
            p : "The pilot is allowed to communicate to their partner only via drawing and may watch and hear the stream of the runner. Runners may talk to their pilot. We would prefer if the pilot did not directly respond to questions, as figuring things out is supposed to be part of the challenge, but in the end it's up to the individual teams.";
            p {
                strong : "Allowed:";
                : " Arrows, question marks, ingame symbols, check marks, “X” for crossing out stuff.";
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
            : settings;
            @if let Some(sample_seeds) = sample_seeds {
                h2 : "Sample seeds";
                p {
                    @if is_random_settings {
                        : "Since the random settings script isn't available online";
                    } else {
                        : "Since the branch we're using isn't available on the website";
                    }
                    : ", we've prepared some sample seeds:";
                }
                : sample_seeds;
                @if event == "5" {
                    p {
                        a(href = "https://ootr.fenhl.net/static/pictionary5-sample-seeds-batch2.zip") : "Download all";
                    }
                    p {
                        : "You can apply these patch files using ";
                        a(href = "https://ootrandomizer.com/generator") : "the regular web patcher";
                        : ".";
                    }
                    p {
                        strong : "Note:";
                        : " These sample seeds were posted on February 11, replacing ";
                        a(href = "https://ootr.fenhl.net/static/pictionary5-sample-seeds.zip") : "the original batch";
                        : " which had a bug where the spoiler log would show the wrong prices for most right-side shop items. Special thanks to ShadowShine57 who found ";
                        a(href = "https://github.com/TestRunnerSRL/OoT-Randomizer/pull/1505") : "the fix";
                        : " for that bug.";
                    }
                }
            }
            h2 : "Further information";
            p {
                : "The race is organized by ";
                : natjoin(organizers);
                : ". We will answer questions and inform about recent events on The Silver Gauntlets Discord in the #pictionary-spoiler-log channel (";
                a(href = "https://discord.gg/m8z8ZqtN8H") : "invite link";
                : " • ";
                a(href = "https://discord.com/channels/663207960432082944/865206020015128586") : "direct channel link";
                : "). If you have any questions, feel free to ask there!";
            }
            p : "Special thanks to winniedemon who will be helping us keep important posts from getting lost in the Discord!";
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq, FromFormField, UriDisplayQuery)]
pub(crate) enum Role {
    #[field(value = "sheikah")]
    Sheikah,
    #[field(value = "gerudo")]
    Gerudo,
}

impl ToHtml for Role {
    fn to_html(&self) -> RawHtml<String> {
        match self {
            Self::Sheikah => html! {
                span(class = "sheikah") : "runner";
            },
            Self::Gerudo => html! {
                span(class = "gerudo") : "pilot";
            },
        }
    }
}

impl TryFrom<super::Role> for Role {
    type Error = ();

    fn try_from(role: super::Role) -> Result<Self, ()> {
        match role {
            super::Role::Sheikah => Ok(Self::Sheikah),
            super::Role::Gerudo => Ok(Self::Gerudo),
            _ => Err(()),
        }
    }
}

impl From<Role> for super::Role {
    fn from(role: Role) -> Self {
        match role {
            Role::Sheikah => Self::Sheikah,
            Role::Gerudo => Self::Gerudo,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, sqlx::Type, FromFormField)]
#[sqlx(type_name = "role_preference", rename_all = "snake_case")]
pub(crate) enum RolePreference {
    #[field(value = "sheikah_only")]
    SheikahOnly,
    #[field(value = "sheikah_preferred")]
    SheikahPreferred,
    #[field(value = "no_preference")]
    NoPreference,
    #[field(value = "gerudo_preferred")]
    GerudoPreferred,
    #[field(value = "gerudo_only")]
    GerudoOnly,
}

impl ToHtml for RolePreference {
    fn to_html(&self) -> RawHtml<String> {
        match self {
            Self::SheikahOnly => html! {
                span(class = "sheikah") : "runner only";
            },
            Self::SheikahPreferred => html! {
                span(class = "sheikah") : "runner preferred";
            },
            Self::NoPreference => html! {
                : "no preference";
            },
            Self::GerudoPreferred => html! {
                span(class = "gerudo") : "pilot preferred";
            },
            Self::GerudoOnly => html! {
                span(class = "gerudo") : "pilot only";
            },
        }
    }
}

pub(super) enum EnterFormDefaults<'a> {
    Context(Context<'a>),
    Values {
        my_role: Option<Role>,
        teammate: Option<Id>,
    },
}

impl<'v> EnterFormDefaults<'v> {
    fn errors(&self) -> Vec<&form::Error<'v>> {
        match self {
            Self::Context(ctx) => ctx.errors().collect(),
            Self::Values { .. } => Vec::default(),
        }
    }

    fn team_name(&self) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value("team_name"),
            Self::Values { .. } => None,
        }
    }

    fn my_role(&self) -> Option<Role> {
        match self {
            Self::Context(ctx) => match ctx.field_value("my_role") {
                Some("sheikah") => Some(Role::Sheikah),
                Some("gerudo") => Some(Role::Gerudo),
                _ => None,
            },
            &Self::Values { my_role, .. } => my_role,
        }
    }

    fn teammate(&self) -> Option<Id> {
        self.teammate_text().and_then(|text| text.parse().ok())
    }

    fn teammate_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Context(ctx) => ctx.field_value("teammate").map(Cow::Borrowed),
            &Self::Values { teammate, .. } => teammate.map(|id| Cow::Owned(id.0.to_string())),
        }
    }
}

#[allow(unused_qualifications)] // rocket endpoint and uri macros don't work with relative module paths
pub(super) async fn enter_form(me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, defaults: EnterFormDefaults<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(me.as_ref(), Tab::Enter).await?;
    Ok(page(&data.pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), if me.is_some() {
        let mut errors = defaults.errors();
        let form_content = html! {
            : csrf;
            legend {
                : "Fill out this form to enter the race as a team. Your teammate will receive an invitation they have to accept to confirm the signup. If you don't have a team yet, you can ";
                a(href = uri!(super::find_team(&*data.series, &*data.event)).to_string()) : "look for a teammate";
                : " instead.";
            }
            fieldset {
                : field_errors(&mut errors, "team_name");
                label(for = "team_name") : "Team Name:";
                input(type = "text", name = "team_name", value? = defaults.team_name());
                label(class = "help") : "(Optional unless you want to be on restream. Can be changed later. Organizers may remove inappropriate team names.)";
            }
            fieldset {
                : field_errors(&mut errors, "my_role");
                label(for = "my_role") : "My Role:";
                input(id = "my_role-sheikah", class = "sheikah", type = "radio", name = "my_role", value = "sheikah", checked? = defaults.my_role() == Some(Role::Sheikah));
                label(class = "sheikah", for = "my_role-sheikah") : "Runner";
                input(id = "my_role-gerudo", class = "gerudo", type = "radio", name = "my_role", value = "gerudo", checked? = defaults.my_role() == Some(Role::Gerudo));
                label(class = "gerudo", for = "my_role-gerudo") : "Pilot";
            }
            fieldset {
                : field_errors(&mut errors, "teammate");
                label(for = "teammate") : "Teammate:";
                input(type = "text", name = "teammate", value? = defaults.teammate_text().as_deref());
                label(class = "help") : "(Enter your teammate's Mido's House user ID. It can be found on their profile page.)"; //TODO add JS-based user search?
            }
            fieldset {
                input(type = "submit", value = "Submit");
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
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(super::enter(&*data.series, &*data.event, defaults.my_role(), defaults.teammate()))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to enter this race.";
                }
            }
        }
    }).await?)
}

#[derive(FromForm)]
pub(crate) struct EnterForm {
    #[field(default = String::new())]
    csrf: String,
    team_name: String,
    my_role: Role,
    teammate: Id,
}

impl CsrfForm for EnterForm { //TODO derive
    fn csrf(&self) -> &String { &self.csrf }
}

#[rocket::post("/event/pic/<event>/enter", data = "<form>")]
pub(crate) async fn enter_post(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, event: &str, form: Form<Contextual<'_, EnterForm>>) -> Result<RedirectOrContent, StatusOrError<Error>> {
    let data = Data::new((**pool).clone(), "pic", event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started() {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        let mut transaction = pool.begin().await?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = 'pic'
            AND event = $1
            AND member = $2
            AND EXISTS (SELECT 1 FROM team_members WHERE team = id AND member = $3)
        ) AS "exists!""#, event, i64::from(me.id), i64::from(value.teammate)).fetch_one(&mut transaction).await? {
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
        if value.my_role == Role::Sheikah && me.racetime_id.is_none() {
            form.context.push_error(form::Error::validation("A racetime.gg account is required to enter as runner. Go to your profile and select “Connect a racetime.gg account”.").with_name("my_role")); //TODO direct link?
        }
        if value.teammate == me.id {
            form.context.push_error(form::Error::validation("You cannot be your own teammate.").with_name("teammate"));
        }
        if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, i64::from(value.teammate)).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("There is no user with this ID.").with_name("teammate"));
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = 'pic'
            AND event = $1
            AND member = $2
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, event, i64::from(value.teammate)).fetch_one(&mut transaction).await? {
            form.context.push_error(form::Error::validation("This user is already signed up for this race.").with_name("teammate"));
        }
        //TODO check to make sure the teammate hasn't blocked the user submitting the form (or vice versa) or the event
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(enter_form(Some(me), uri, csrf, data, EnterFormDefaults::Context(form.context)).await?)
        } else {
            let id = Id::new(&mut transaction, IdTable::Teams).await?;
            sqlx::query!("INSERT INTO teams (id, series, event, name) VALUES ($1, 'pic', $2, $3)", i64::from(id), event, (!value.team_name.is_empty()).then(|| &value.team_name)).execute(&mut transaction).await?;
            sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'created', $3)", i64::from(id), i64::from(me.id), super::Role::from(value.my_role) as _).execute(&mut transaction).await?;
            sqlx::query!("INSERT INTO team_members (team, member, status, role) VALUES ($1, $2, 'unconfirmed', $3)", i64::from(id), i64::from(value.teammate), match value.my_role { Role::Sheikah => super::Role::Gerudo, Role::Gerudo => super::Role::Sheikah } as _).execute(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(super::teams("pic", event))))
        }
    } else {
        RedirectOrContent::Content(enter_form(Some(me), uri, csrf, data, EnterFormDefaults::Context(form.context)).await?)
    })
}

#[allow(unused_qualifications)] // rocket endpoint and uri macros don't work with relative module paths
pub(super) async fn find_team_form(me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, context: Context<'_>) -> Result<RawHtml<String>, FindTeamError> {
    let header = data.header(me.as_ref(), Tab::FindTeam).await?;
    let mut my_role = None;
    let mut looking_for_team = Vec::default();
    let mut looking_for_team_query = sqlx::query!(r#"SELECT user_id AS "user!: Id", role AS "role: RolePreference" FROM looking_for_team WHERE series = $1 AND event = $2"#, &data.series, &data.event).fetch(&data.pool);
    while let Some(row) = looking_for_team_query.try_next().await? {
        let user = User::from_id(&data.pool, row.user).await?.ok_or(FindTeamError::UnknownUser)?;
        if me.as_ref().map_or(false, |me| user.id == me.id) { my_role = Some(row.role) }
        let can_invite = me.as_ref().map_or(true, |me| user.id != me.id) && true /*TODO not already in a team with that user */;
        looking_for_team.push((user, row.role, can_invite));
    }
    let form = if me.is_some() {
        let mut errors = context.errors().collect_vec();
        if my_role.is_none() {
            let form_content = html! {
                : csrf;
                legend {
                    : "Fill out this form to add yourself to the list below.";
                }
                fieldset {
                    : field_errors(&mut errors, "role");
                    label(for = "role") : "Role:";
                    input(id = "role-sheikah_only", class = "sheikah", type = "radio", name = "role", value = "sheikah_only", checked? = context.field_value("role") == Some("sheikah_only"));
                    label(class = "sheikah", for = "role-sheikah_only") : "Runner only";
                    input(id = "role-sheikah_preferred", class = "sheikah", type = "radio", name = "role", value = "sheikah_preferred", checked? = context.field_value("role") == Some("sheikah_preferred"));
                    label(class = "sheikah", for = "role-sheikah_preferred") : "Runner preferred";
                    input(id = "role-no_preference", type = "radio", name = "role", value = "no_preference", checked? = context.field_value("role").map_or(true, |role| role == "no_preference"));
                    label(for = "role-no_preference") : "No preference";
                    input(id = "role-gerudo_preferred", class = "gerudo", type = "radio", name = "role", value = "gerudo_preferred", checked? = context.field_value("role") == Some("gerudo_preferred"));
                    label(class = "gerudo", for = "role-gerudo_preferred") : "Pilot preferred";
                    input(id = "role-gerudo_only", class = "gerudo", type = "radio", name = "role", value = "gerudo_only", checked? = context.field_value("role") == Some("gerudo_only"));
                    label(class = "gerudo", for = "role-gerudo_only") : "Pilot only";
                }
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
        } else {
            None
        }
    } else {
        Some(html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(super::find_team(&*data.series, &*data.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to add yourself to this list.";
                }
            }
        })
    };
    let can_invite_any = looking_for_team.iter().any(|&(_, _, can_invite)| can_invite);
    let looking_for_team = looking_for_team.into_iter()
        .map(|(user, role, can_invite)| (user, role, can_invite.then(|| match (my_role, role) {
            // if I haven't signed up looking for team, default to the role opposite the invitee's preference
            (None, RolePreference::SheikahOnly | RolePreference::SheikahPreferred) => Some(Role::Gerudo),
            (None, RolePreference::GerudoOnly | RolePreference::GerudoPreferred) => Some(Role::Sheikah),
            (None, RolePreference::NoPreference) => None,
            // if I have signed up looking for team, take the role that's more preferred by me than by the invitee
            (Some(my_role), _) => match my_role.cmp(&role) {
                Less => Some(Role::Sheikah),
                Equal => None,
                Greater => Some(Role::Gerudo),
            },
        })))
        .collect_vec();
    Ok(page(&data.pool, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Find Teammates — {}", data.display_name), html! {
        : header;
        : form;
        table {
            thead {
                tr {
                    th : "User";
                    th : "Role";
                    @if can_invite_any {
                        th;
                    }
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
                    @for (user, role, invite) in looking_for_team {
                        tr {
                            td : user;
                            td : role;
                            @if can_invite_any {
                                td {
                                    @if let Some(my_role) = invite {
                                        a(class = "button", href = uri!(super::enter(&*data.series, &*data.event, my_role, Some(user.id))).to_string()) : "Invite";
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

#[derive(FromForm)]
pub(crate) struct FindTeamForm {
    #[field(default = String::new())]
    csrf: String,
    role: RolePreference,
}

impl CsrfForm for FindTeamForm { //TODO derive
    fn csrf(&self) -> &String { &self.csrf }
}

#[rocket::post("/event/pic/<event>/find-team", data = "<form>")]
pub(crate) async fn find_team_post(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, event: &str, form: Form<Contextual<'_, FindTeamForm>>) -> Result<RedirectOrContent, StatusOrError<FindTeamError>> {
    let data = Data::new((**pool).clone(), "pic", event).await.map_err(FindTeamError::Data)?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    if data.is_started() {
        form.context.push_error(form::Error::validation("You can no longer enter this event since it has already started."));
    }
    Ok(if let Some(ref value) = form.value {
        let mut transaction = pool.begin().await.map_err(FindTeamError::Sql)?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM looking_for_team WHERE
            series = 'pic'
            AND event = $1
            AND user_id = $2
        ) AS "exists!""#, event, i64::from(me.id)).fetch_one(&mut transaction).await.map_err(FindTeamError::Sql)? {
            form.context.push_error(form::Error::validation("You are already on the list."));
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams, team_members WHERE
            id = team
            AND series = 'pic'
            AND event = $1
            AND member = $2
            AND NOT EXISTS (SELECT 1 FROM team_members WHERE team = id AND status = 'unconfirmed')
        ) AS "exists!""#, event, i64::from(me.id)).fetch_one(&mut transaction).await.map_err(FindTeamError::Sql)? {
            form.context.push_error(form::Error::validation("You are already signed up for this race."));
        }
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(find_team_form(Some(me), uri, csrf, data, form.context).await?)
        } else {
            sqlx::query!("INSERT INTO looking_for_team (series, event, user_id, role) VALUES ('pic', $1, $2, $3)", event, i64::from(me.id), value.role as _).execute(&mut transaction).await.map_err(FindTeamError::Sql)?;
            transaction.commit().await.map_err(FindTeamError::Sql)?;
            RedirectOrContent::Redirect(Redirect::to(uri!(super::find_team("pic", event))))
        }
    } else {
        RedirectOrContent::Content(find_team_form(Some(me), uri, csrf, data, form.context).await?)
    })
}
