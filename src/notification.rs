use crate::{
    event::{
        Role,
        SignupStatus,
    },
    prelude::*,
};

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("unknown event")]
    UnknownEvent,
    #[error("unknown user")]
    UnknownUser,
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
}

impl Notification {
    pub(crate) async fn get(transaction: &mut Transaction<'_, Postgres>, me: &User) -> Result<Vec<Self>, event::DataError> {
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
        Ok(notifications)
    }

    async fn into_html(self, transaction: &mut Transaction<'_, Postgres>, me: &User, csrf: Option<&CsrfToken>) -> Result<RawHtml<String>, Error> {
        Ok(match self {
            Self::Simple(id) => {
                let text = match sqlx::query_scalar!(r#"SELECT kind AS "kind: SimpleNotificationKind" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await? {
                    SimpleNotificationKind::Accept => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id<Users>", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
                        let sender = User::from_id(&mut **transaction, row.sender).await?.ok_or(Error::UnknownUser)?;
                        let event = event::Data::new(&mut *transaction, row.series, row.event).await?.ok_or(Error::UnknownEvent)?;
                        html! {
                            : sender;
                            : " accepted your invitation to join a team for ";
                            : event;
                            : ".";
                        }
                    }
                    SimpleNotificationKind::Decline => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id<Users>", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
                        let sender = User::from_id(&mut **transaction, row.sender).await?.ok_or(Error::UnknownUser)?;
                        let event = event::Data::new(&mut *transaction, row.series, row.event).await?.ok_or(Error::UnknownEvent)?;
                        html! {
                            : sender;
                            : " declined your invitation to form a team for ";
                            : event;
                            : ".";
                        }
                    }
                    SimpleNotificationKind::Resign => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id<Users>", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, id as _).fetch_one(&mut **transaction).await?;
                        let sender = User::from_id(&mut **transaction, row.sender).await?.ok_or(Error::UnknownUser)?;
                        let event = event::Data::new(&mut *transaction, row.series, row.event).await?.ok_or(Error::UnknownEvent)?;
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
                    div(class = "button-row") {
                        form(action = uri!(dismiss(id)).to_string(), method = "post") {
                            : csrf;
                            input(type = "submit", value = "Dismiss Notification");
                        }
                    }
                }
            }
            Self::TeamInvite(team_id) => team_invite(transaction, me, csrf, team_id).await?,
        })
    }
}

pub(crate) async fn team_invite(transaction: &mut Transaction<'_, Postgres>, me: &User, csrf: Option<&CsrfToken>, team_id: Id<Teams>) -> Result<RawHtml<String>, Error> {
    let team_row = sqlx::query!(r#"SELECT series AS "series: Series", event, name, racetime_slug FROM teams WHERE id = $1"#, team_id as _).fetch_one(&mut **transaction).await?;
    let event = event::Data::new(&mut *transaction, team_row.series, team_row.event).await?.ok_or(Error::UnknownEvent)?;
    let mut creator = None;
    let mut my_role = None;
    let mut teammates = Vec::default();
    for member in sqlx::query!(r#"SELECT member AS "id: Id<Users>", status AS "status: SignupStatus", role AS "role: Role" FROM team_members WHERE team = $1"#, team_id as _).fetch_all(&mut **transaction).await? {
        if member.id == me.id {
            my_role = Some(member.role);
        } else {
            let is_confirmed = match member.status {
                SignupStatus::Created => {
                    creator = Some((User::from_id(&mut **transaction, member.id).await?.ok_or(Error::UnknownUser)?, member.role));
                    continue
                }
                SignupStatus::Confirmed => true,
                SignupStatus::Unconfirmed => false,
            };
            let user = User::from_id(&mut **transaction, member.id).await?.ok_or(Error::UnknownUser)?;
            teammates.push(html! {
                : user;
                : " (";
                @match event.team_config {
                    TeamConfig::Solo => @unreachable // team invite for solo event
                    TeamConfig::CoOp => {}
                    TeamConfig::TfbCoOp => {
                        : tfb::CoOpRole::try_from(member.role).expect("non-coop role in coop team");
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
    let my_role = my_role.ok_or(Error::UnknownUser)?;
    Ok(html! {
        @match event.team_config {
            TeamConfig::Solo => {
                : "You have been invited to enter ";
                : event;
                : ".";
            }
            TeamConfig::CoOp => {
                @let (creator, _) = creator.ok_or(Error::UnknownUser)?;
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
                @if let Some(teammates) = English.join_html(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
            TeamConfig::TfbCoOp => {
                @let (creator, creator_role) = creator.ok_or(Error::UnknownUser)?;
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
                @if let Some(teammates) = English.join_html(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
            TeamConfig::Pictionary => {
                @let (creator, creator_role) = creator.ok_or(Error::UnknownUser)?;
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
                @if let Some(teammates) = English.join_html(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
            TeamConfig::Multiworld => {
                @let (creator, creator_role) = creator.ok_or(Error::UnknownUser)?;
                : creator;
                : " (";
                : mw::Role::try_from(creator_role).expect("non-multiworld role in multiworld team");
                : ") invited you to enter ";
                : event;
                : " as ";
                : mw::Role::try_from(my_role).expect("non-multiworld role in multiworld team");
                : " for team ";
                a(href = format!("https://{}/team/{}", racetime_host(), team_row.racetime_slug.expect("multiworld team without racetime slug"))) : team_row.name; //TODO use Team type
                @if let Some(teammates) = English.join_html(teammates) {
                    : " together with ";
                    : teammates;
                }
                : ".";
            }
        }
        div(class = "button-row") {
            @if matches!(event.team_config, TeamConfig::Pictionary) && my_role == Role::Sheikah && me.racetime.is_none() {
                a(class = "button", href = uri!(crate::auth::racetime_login(Some(uri!(notifications)))).to_string()) : "Connect racetime.gg Account to Accept";
            } else {
                form(action = uri!(crate::event::confirm_signup(event.series, &*event.event, team_id)).to_string(), method = "post") {
                    : csrf;
                    input(type = "submit", value = "Accept");
                }
            }
            form(action = uri!(crate::event::resign_post(event.series, &*event.event, team_id)).to_string(), method = "post") {
                : csrf;
                input(type = "submit", value = "Decline");
            }
            //TODO options to block sender or event
        }
    })
}

#[rocket::get("/notifications")]
pub(crate) async fn notifications(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>) -> Result<RawHtml<String>, Error> {
    let mut transaction = pool.begin().await?;
    Ok(if let Some(me) = me {
        let mut notifications = Vec::default();
        for notification in Notification::get(&mut transaction, &me).await? {
            notifications.push(notification.into_html(&mut transaction, &me, csrf.as_ref()).await?);
        }
        page(transaction, &Some(me), &uri, PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications — Mido's House", html! {
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
        page(transaction, &me, &uri, PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications — Mido's House", html! {
            p {
                a(href = uri!(auth::login(Some(uri!(notifications)))).to_string()) : "Sign in or create a Mido's House account";
                : " to view your notifications.";
            }
        }).await?
    })
}

#[rocket::post("/notifications/dismiss/<id>", data = "<form>")]
pub(crate) async fn dismiss(pool: &State<PgPool>, me: User, id: Id<Notifications>, csrf: Option<CsrfToken>, form: Form<Contextual<'_, EmptyForm>>) -> Result<Redirect, rocket_util::Error<sqlx::Error>> {
    let mut form = form.into_inner();
    form.verify(&csrf);
    if form.context.errors().next().is_none() {
        sqlx::query!("DELETE FROM notifications WHERE id = $1 AND rcpt = $2", id as _, me.id as _).execute(&**pool).await?;
    }
    Ok(Redirect::to(uri!(notifications)))
}
