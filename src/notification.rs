use crate::{
    event::{
        Role,
        SignupStatus,
    },
    prelude::*,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum GetError {
    #[error(transparent)] Cal(#[from] cal::Error),
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

impl IsNetworkError for GetError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Cal(e) => e.is_network_error(),
            Self::Event(_) => false,
            Self::Sql(_) => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IntoHtmlError {
    #[error(transparent)] Cal(#[from] cal::Error),
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("unknown event")]
    UnknownEvent,
    #[error("unknown user")]
    UnknownUser,
}

impl IsNetworkError for IntoHtmlError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Cal(e) => e.is_network_error(),
            Self::Event(_) => false,
            Self::Sql(_) => false,
            Self::UnknownEvent => false,
            Self::UnknownUser => false,
        }
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Cal(#[from] cal::Error),
    #[error(transparent)] Event(#[from] event::Error),
    #[error(transparent)] EventData(#[from] event::DataError),
    #[error(transparent)] Get(#[from] GetError),
    #[error(transparent)] IntoHtml(#[from] IntoHtmlError),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
}

#[derive(sqlx::Type)]
#[sqlx(type_name = "notification_kind", rename_all = "snake_case")]
pub(crate) enum SimpleNotificationKind {
    Accept,
    Decline,
    Resign,
}

pub(crate) enum Notification {
    /// A notification from the `notifications` table that can only be dismissed
    Simple(Id<Notifications>),
    TeamInvite(Id<Teams>),
    ConfirmTsgRestream(Race),
}

impl Notification {
    pub(crate) async fn get(transaction: &mut Transaction<'_, Postgres>, http_client: &reqwest::Client, me: &User) -> Result<Vec<Self>, GetError> {
        let mut notifications = sqlx::query_scalar!(r#"SELECT id AS "id: Id<Notifications>" FROM notifications WHERE rcpt = $1"#, me.id as _)
            .fetch(&mut **transaction)
            .map_ok(Self::Simple)
            .try_collect::<Vec<_>>().await?;
        for team_id in sqlx::query_scalar!(r#"SELECT team AS "team: Id<Teams>" FROM team_members WHERE member = $1 AND status = 'unconfirmed'"#, me.id as _).fetch_all(&mut **transaction).await? {
            let team_row = sqlx::query!(r#"SELECT series AS "series: Series", event, name, racetime_slug FROM teams WHERE id = $1"#, team_id as _).fetch_one(&mut **transaction).await?;
            let event = event::Data::new(&mut *transaction, team_row.series, team_row.event).await?.expect("enforced by database constraint");
            if !event.is_started(&mut *transaction).await? {
                notifications.push(Self::TeamInvite(team_id));
            }
        }
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM volunteers WHERE organization = 'tsg' AND language = 'en' AND volunteer = $1 AND role = 'restreamer') as "exists!""#, me.id as _).fetch_one(&mut **transaction).await? {
            let cc9 = event::Data::new(&mut *transaction, Series::Standard, "9cc").await?.expect("event s/9cc removed from database"); //TODO roll out to other events after beta
            for race in Race::for_event(&mut *transaction, http_client, &cc9).await? {
                if race.restreamers.get(&English).and_then(as_variant!(cal::Restreamer::MidosHouse)).is_some_and(|restreamer| *restreamer == me.id)
                    && let RaceSchedule::Live { start, .. } = race.schedule
                    && start > Utc::now()
                    && !race.video_urls.contains_key(&English)
                    && race.commentators.get(&English).map(|commentators| commentators.len().get()).unwrap_or_default() >= 2
                    && race.trackers.get(&English).map(|trackers| trackers.len().get()).unwrap_or_default() >= race.num_trackers(&mut *transaction).await?
                {
                    notifications.push(Self::ConfirmTsgRestream(race));
                }
            }
        }
        Ok(notifications)
    }

    async fn into_html(self, transaction: &mut Transaction<'_, Postgres>, global: &GlobalState, me: &User, csrf: Option<&CsrfToken>, errors: Vec<&form::Error<'_>>, source: TeamInviteSource) -> Result<RawHtml<String>, IntoHtmlError> {
        Ok(match self {
            Self::Simple(id) => {
                let text = match sqlx::query_scalar!(r#"SELECT kind AS "kind: SimpleNotificationKind" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await? {
                    SimpleNotificationKind::Accept => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id<Users>", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
                        let sender = User::from_id(&mut **transaction, row.sender).await?.ok_or(IntoHtmlError::UnknownUser)?;
                        let event = event::Data::new(&mut *transaction, row.series, row.event).await?.ok_or(IntoHtmlError::UnknownEvent)?;
                        html! {
                            : sender;
                            : " accepted your invitation to join a team for ";
                            : event;
                            : ".";
                        }
                    }
                    SimpleNotificationKind::Decline => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id<Users>", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
                        let sender = User::from_id(&mut **transaction, row.sender).await?.ok_or(IntoHtmlError::UnknownUser)?;
                        let event = event::Data::new(&mut *transaction, row.series, row.event).await?.ok_or(IntoHtmlError::UnknownEvent)?;
                        html! {
                            : sender;
                            : " declined your invitation to form a team for ";
                            : event;
                            : ".";
                        }
                    }
                    SimpleNotificationKind::Resign => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id<Users>", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
                        let sender = User::from_id(&mut **transaction, row.sender).await?.ok_or(IntoHtmlError::UnknownUser)?;
                        let event = event::Data::new(&mut *transaction, row.series, row.event).await?.ok_or(IntoHtmlError::UnknownEvent)?;
                        html! {
                            : sender;
                            : " resigned your team from ";
                            : event;
                            : ".";
                        }
                    }
                };
                html! {
                    : text;
                    @let (errors, button) = button_form(uri!(dismiss(id)), csrf, errors, "Dismiss Notification");
                    : errors;
                    div(class = "button-row") : button;
                }
            }
            Self::TeamInvite(team_id) => team_invite(transaction, me, csrf, errors, source, team_id).await?,
            Self::ConfirmTsgRestream(race) => html! {
                : "A volunteer crew is available to restream the following race:";
                : cal::race_table(transaction, global, Some(me), &Origin(uri!(notifications)), csrf, None, cal::RaceTableOptions {
                    game_count: false,
                    show_multistreams: true,
                    can_create: false,
                    can_edit: false,
                    restreams: cal::RaceTableRestreams::Volunteers { can_restream: false, can_commentate: false, can_track: false },
                    challonge_import_ctx: None,
                }, std::slice::from_ref(&race)).await?;
                @let (errors_html, confirm_button) = button_form(uri!(confirm_tsg_restream(race.id)), csrf, errors, "Confirm Restream");
                : errors_html;
                @let (errors_html, retract_button) = button_form(uri!(cal::volunteer_retract_post(race.series, race.event, race.id, VolunteerRole::Restreamer, Some(uri!(notifications)))), csrf, Vec::default(), "Remove Restreamer Signup");
                : errors_html;
                div(class = "button-row") {
                    : confirm_button;
                    : retract_button;
                }
            },
        })
    }
}

/// Metadata to ensure the correct page is displayed on form validation failure.
#[derive(Clone, Copy)]
pub(crate) enum TeamInviteSource {
    Enter,
    Notifications,
}

impl From<TeamInviteSource> for event::AcceptFormSource {
    fn from(value: TeamInviteSource) -> Self {
        match value {
            TeamInviteSource::Enter => Self::Enter,
            TeamInviteSource::Notifications => Self::Notifications,
        }
    }
}

impl From<TeamInviteSource> for event::ResignFormSource {
    fn from(value: TeamInviteSource) -> Self {
        match value {
            TeamInviteSource::Enter => Self::Enter,
            TeamInviteSource::Notifications => Self::Notifications,
        }
    }
}

pub(crate) async fn team_invite(transaction: &mut Transaction<'_, Postgres>, me: &User, csrf: Option<&CsrfToken>, errors: Vec<&form::Error<'_>>, source: TeamInviteSource, team_id: Id<Teams>) -> Result<RawHtml<String>, IntoHtmlError> {
    let team_row = sqlx::query!(r#"SELECT series AS "series: Series", event, name, racetime_slug FROM teams WHERE id = $1"#, team_id as _).fetch_one(&mut **transaction).await?;
    let event = event::Data::new(&mut *transaction, team_row.series, team_row.event).await?.ok_or(IntoHtmlError::UnknownEvent)?;
    let mut creator = None;
    let mut my_role = None;
    let mut teammates = Vec::default();
    for member in sqlx::query!(r#"SELECT member AS "id: Id<Users>", status AS "status: SignupStatus", role AS "role: Role" FROM team_members WHERE team = $1"#, team_id as _).fetch_all(&mut **transaction).await? {
        if member.id == me.id {
            my_role = Some(member.role);
        } else {
            let is_confirmed = match member.status {
                SignupStatus::Created => {
                    creator = Some((User::from_id(&mut **transaction, member.id).await?.ok_or(IntoHtmlError::UnknownUser)?, member.role));
                    continue
                }
                SignupStatus::Confirmed => true,
                SignupStatus::Unconfirmed => false,
            };
            let user = User::from_id(&mut **transaction, member.id).await?.ok_or(IntoHtmlError::UnknownUser)?;
            teammates.push(html! {
                : user;
                : " (";
                @match event.team_config {
                    TeamConfig::Solo => @unreachable // team invite for solo event
                    TeamConfig::CoOp | TeamConfig::SlugOpen => {}
                    TeamConfig::TfbCoOp => {
                        : tfb::CoOpRole::try_from(member.role).expect("non-coop role in coop team");
                        : ", ";
                    }
                    TeamConfig::NightAndDay => {
                        : tfb::NightAndDayRole::try_from(member.role).expect("non-coop role in coop team");
                        : ", ";
                    }
                    TeamConfig::Pictionary => {
                        : pic::Role::try_from(member.role).expect("non-Pictionary role in Pictionary team");
                        : ", ";
                    }
                    TeamConfig::Multiworld => {
                        : mw::Role::try_from(member.role).expect("non-multiworld role in multiworld team");
                        : ", ";
                    }
                }
                @if is_confirmed {
                    : "confirmed)";
                } else {
                    : "unconfirmed)";
                }
            });
        }
    }
    let my_role = my_role.ok_or(IntoHtmlError::UnknownUser)?;
    Ok(html! {
        @match event.team_config {
            TeamConfig::Solo => {
                : "You have been invited to enter ";
                : event;
                : ".";
            }
            TeamConfig::CoOp | TeamConfig::SlugOpen => {
                @let (creator, _) = creator.ok_or(IntoHtmlError::UnknownUser)?;
                : creator;
                : " invited you to join ";
                : creator.possessive_determiner();
                : " team";
                @if let Some(team_name) = team_row.name {
                    : " “";
                    : team_name;
                    : "”";
                }
                : " for ";
                : event;
                @if let Some(teammates) = English.join_html_opt(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
            TeamConfig::TfbCoOp => {
                @let (creator, creator_role) = creator.ok_or(IntoHtmlError::UnknownUser)?;
                : creator;
                : " (";
                : tfb::CoOpRole::try_from(creator_role).expect("non-coop role in coop team");
                : ") invited you to join ";
                : creator.possessive_determiner();
                : " team";
                @if let Some(team_name) = team_row.name {
                    : " “";
                    : team_name;
                    : "”";
                }
                : " for ";
                : event;
                : " as ";
                : tfb::CoOpRole::try_from(my_role).expect("non-coop role in coop team");
                @if let Some(teammates) = English.join_html_opt(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
            TeamConfig::NightAndDay => {
                @let (creator, creator_role) = creator.ok_or(IntoHtmlError::UnknownUser)?;
                : creator;
                : " (";
                : tfb::NightAndDayRole::try_from(creator_role).expect("non-coop role in coop team");
                : ") invited you to join ";
                : creator.possessive_determiner();
                : " team";
                @if let Some(team_name) = team_row.name {
                    : " “";
                    : team_name;
                    : "”";
                }
                : " for ";
                : event;
                : " as ";
                : tfb::NightAndDayRole::try_from(my_role).expect("non-coop role in coop team");
                @if let Some(teammates) = English.join_html_opt(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
            TeamConfig::Pictionary => {
                @let (creator, creator_role) = creator.ok_or(IntoHtmlError::UnknownUser)?;
                : creator;
                : " (";
                : pic::Role::try_from(creator_role).expect("non-Pictionary role in Pictionary team");
                : ") invited you to join ";
                : creator.possessive_determiner();
                : " team";
                @if let Some(team_name) = team_row.name {
                    : " “";
                    : team_name;
                    : "”";
                }
                : " for ";
                : event;
                : " as ";
                : pic::Role::try_from(my_role).expect("non-Pictionary role in Pictionary team");
                @if let Some(teammates) = English.join_html_opt(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
            TeamConfig::Multiworld => {
                @let (creator, creator_role) = creator.ok_or(IntoHtmlError::UnknownUser)?;
                : creator;
                : " (";
                : mw::Role::try_from(creator_role).expect("non-multiworld role in multiworld team");
                : ") invited you to enter ";
                : event;
                : " as ";
                : mw::Role::try_from(my_role).expect("non-multiworld role in multiworld team");
                : " for team ";
                a(href = format!("https://{}/team/{}", racetime_host(), team_row.racetime_slug.expect("multiworld team without racetime slug"))) : team_row.name; //TODO use Team type
                @if let Some(teammates) = English.join_html_opt(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
        }
        @let (errors, accept_button) = if matches!(event.team_config, TeamConfig::Pictionary) && my_role == Role::Sheikah && me.racetime.is_none() {
            (html! {}, html! {
                a(class = "button", href = uri!(crate::auth::racetime_login(Some(uri!(notifications))))) : "Connect racetime.gg Account to Accept";
            })
        } else {
            button_form_ext(uri!(crate::event::confirm_signup(event.series, &*event.event, team_id)), csrf, errors, event::AcceptFormSource::from(source), "Accept")
        };
        : errors;
        @let (errors, decline_button) = button_form_ext(uri!(crate::event::resign_post(event.series, &*event.event, team_id)), csrf, Vec::default(), event::ResignFormSource::from(source), "Decline");
        : errors;
        div(class = "button-row") {
            : accept_button;
            : decline_button;
            //TODO options to block sender or event
        }
    })
}

pub(crate) async fn list(global: &GlobalState, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, ctx: Context<'_>) -> Result<RawHtml<String>, Error> {
    let mut transaction = global.db_pool.begin().await?;
    Ok(if let Some(me) = me {
        let mut chests_events = me.events_organized(&mut transaction, false).await?;
        chests_events.extend(me.events_participated(&mut transaction, false).await?);
        chests_events.sort_unstable_by(|e1, e2| e1.series.cmp(&e2.series).then_with(|| e1.event.cmp(&e2.event)));
        chests_events.dedup_by(|e1, e2| e1.series == e2.series && e1.event == e2.event);
        let chests_event = chests_events.choose(&mut rng());
        let chests = if let Some(event) = chests_event { event.chests().await? } else { ChestAppearances::random() };
        let mut notifications = Vec::default();
        for notification in Notification::get(&mut transaction, &global.http_client, &me).await? {
            notifications.push(notification.into_html(&mut transaction, global, &me, csrf, ctx.errors().collect_vec(), TeamInviteSource::Notifications).await?);
        }
        page(transaction, global, &Some(me), &uri, PageStyle { kind: PageKind::Notifications, ..PageStyle::new(chests) }, "Notifications — Mido's House", html! {
            h1 : "Notifications";
            @if notifications.is_empty() {
                p : "You have no notifications.";
            } else {
                ul {
                    @for notification in notifications {
                        li : notification;
                    }
                }
            }
        }).await?
    } else {
        page(transaction, global, &me, &uri, PageStyle { kind: PageKind::Notifications, ..PageStyle::new(ChestAppearances::random()) }, "Notifications — Mido's House", html! {
            p {
                a(href = uri!(auth::login(Some(uri!(notifications))))) : "Sign in or create a Mido's House account";
                : " to view your notifications.";
            }
        }).await?
    })
}

#[rocket::get("/notifications")]
pub(crate) async fn notifications(global: &GlobalState, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>) -> Result<RawHtml<String>, Error> {
    list(global, me, uri, csrf.as_ref(), Context::default()).await
}

#[rocket::post("/notifications/dismiss/<id>", data = "<form>")]
pub(crate) async fn dismiss(global: &GlobalState, me: User, uri: Origin<'_>, id: Id<Notifications>, csrf: Option<CsrfToken>, form: Form<Contextual<'_, EmptyForm>>) -> Result<RedirectOrContent, Error> {
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if form.value.is_some() {
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(list(global, Some(me), uri, csrf.as_ref(), form.context).await?)
        } else {
            sqlx::query!("DELETE FROM notifications WHERE id = $1 AND rcpt = $2", id as _, me.id as _).execute(&global.db_pool).await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(notifications)))
        }
    } else {
        RedirectOrContent::Content(list(global, Some(me), uri, csrf.as_ref(), form.context).await?)
    })
}

#[rocket::post("/notifications/confirm-restream/<id>", data = "<form>")]
pub(crate) async fn confirm_tsg_restream(global: &GlobalState, me: User, uri: Origin<'_>, id: Id<Races>, csrf: Option<CsrfToken>, form: Form<Contextual<'_, EmptyForm>>) -> Result<RedirectOrContent, Error> {
    let mut transaction = global.db_pool.begin().await?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if form.value.is_some() {
        let mut race = Race::from_id(&mut transaction, &global.http_client, id).await?;
        let event = race.event(&mut transaction).await?;
        if event.series != Series::Standard || event.event != "9cc" { //TODO roll out to other events after beta
            form.context.push_error(form::Error::validation("The new volunteer signup system is currently in beta and not yet enabled for this event."));
        }
        if race.is_ended() {
            form.context.push_error(form::Error::validation("This race has ended, so its volunteer assignments can no longer be edited."));
        }
        if !event.restream_coordinators(&mut transaction).await?.contains(&me) {
            form.context.push_error(form::Error::validation("You are not confirmed as a restreamer for this event. Please contact a tournament organizer to volunteer."));
        }
        if form.context.errors().next().is_some() {
            transaction.rollback().await?;
            RedirectOrContent::Content(list(global, Some(me), uri, csrf.as_ref(), form.context).await?)
        } else {
            race.video_urls.insert(English, "https://twitch.tv/thesilvergauntlets".parse().expect("failed to parse hardcoded URL"));
            race.save(&mut transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(notifications)))
        }
    } else {
        RedirectOrContent::Content(list(global, Some(me), uri, csrf.as_ref(), form.context).await?)
    })
}
