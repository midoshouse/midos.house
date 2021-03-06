use {
    futures::stream::{
        self,
        StreamExt as _,
        TryStreamExt as _,
    },
    rocket::{
        State,
        form::Form,
        response::{
            Redirect,
            content::RawHtml,
        },
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        Origin,
        html,
    },
    sqlx::PgPool,
    crate::{
        auth,
        event::{
            self,
            Role,
            Series,
            SignupStatus,
            TeamConfig,
            mw,
            pic,
        },
        http::{
            PageError,
            PageKind,
            PageStyle,
            page,
        },
        user::User,
        util::{
            EmptyForm,
            Id,
            natjoin,
        },
    },
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
    Simple(Id),
    TeamInvite(Id),
}

impl Notification {
    pub(crate) async fn get(pool: &PgPool, me: &User) -> sqlx::Result<Vec<Self>> {
        let mut notifications = sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM notifications WHERE rcpt = $1"#, i64::from(me.id))
            .fetch(pool)
            .map_ok(Self::Simple)
            .try_collect::<Vec<_>>().await?;
        notifications.extend(sqlx::query_scalar!(r#"SELECT team AS "team: Id" FROM team_members WHERE member = $1 AND status = 'unconfirmed'"#, i64::from(me.id))
            .fetch(pool)
            .map_ok(Self::TeamInvite)
            .try_collect::<Vec<_>>().await?);
        Ok(notifications)
    }

    async fn into_html(self, pool: &PgPool, me: &User, csrf: &Option<CsrfToken>) -> Result<RawHtml<String>, Error> {
        Ok(match self {
            Self::Simple(id) => {
                let text = match sqlx::query_scalar!(r#"SELECT kind AS "kind: SimpleNotificationKind" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await? {
                    SimpleNotificationKind::Accept => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await?;
                        let sender = User::from_id(pool, row.sender).await?.ok_or(Error::UnknownUser)?;
                        let event = event::Data::new(pool.clone(), row.series, row.event).await?.ok_or(Error::UnknownEvent)?;
                        html! {
                            : sender;
                            : " accepted your invitation to join a team for ";
                            : event;
                            : ".";
                        }
                    }
                    SimpleNotificationKind::Decline => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await?;
                        let sender = User::from_id(pool, row.sender).await?.ok_or(Error::UnknownUser)?;
                        let event = event::Data::new(pool.clone(), row.series, row.event).await?.ok_or(Error::UnknownEvent)?;
                        html! {
                            : sender;
                            : " declined your invitation to form a team for ";
                            : event;
                            : ".";
                        }
                    }
                    SimpleNotificationKind::Resign => {
                        let row = sqlx::query!(r#"SELECT sender AS "sender!: Id", series AS "series!: Series", event AS "event!" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await?;
                        let sender = User::from_id(pool, row.sender).await?.ok_or(Error::UnknownUser)?;
                        let event = event::Data::new(pool.clone(), row.series, row.event).await?.ok_or(Error::UnknownEvent)?;
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
                            input(type = "submit", value = "Dismiss");
                        }
                    }
                }
            }
            Self::TeamInvite(team_id) => {
                let team_row = sqlx::query!(r#"SELECT series AS "series!: Series", event, name, racetime_slug FROM teams WHERE id = $1"#, i64::from(team_id)).fetch_one(pool).await?;
                let event = event::Data::new(pool.clone(), team_row.series, team_row.event).await?.ok_or(Error::UnknownEvent)?;
                let mut creator = None;
                let mut my_role = None;
                let mut teammates = Vec::default();
                let mut members = sqlx::query!(r#"SELECT member AS "id: Id", status AS "status: SignupStatus", role AS "role: Role" FROM team_members WHERE team = $1"#, i64::from(team_id)).fetch(pool);
                while let Some(member) = members.try_next().await? {
                    if member.id == me.id {
                        my_role = Some(member.role);
                    } else {
                        let is_confirmed = match member.status {
                            SignupStatus::Created => {
                                creator = Some((User::from_id(pool, member.id).await?.ok_or(Error::UnknownUser)?, member.role));
                                continue
                            }
                            SignupStatus::Confirmed => true,
                            SignupStatus::Unconfirmed => false,
                        };
                        let user = User::from_id(pool, member.id).await?.ok_or(Error::UnknownUser)?;
                        teammates.push(html! {
                            : user;
                            : " (";
                            @match event.team_config() {
                                TeamConfig::Multiworld => {
                                    : mw::Role::try_from(member.role).expect("non-multiworld role in multiworld team");
                                    : ", ";
                                }
                                TeamConfig::Pictionary => {
                                    : pic::Role::try_from(member.role).expect("non-Pictionary role in Pictionary team");
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
                let (creator, creator_role) = creator.ok_or(Error::UnknownUser)?;
                let my_role = my_role.ok_or(Error::UnknownUser)?;
                html! {
                    @match event.team_config() {
                        TeamConfig::Multiworld => {
                            : creator;
                            : " (";
                            : mw::Role::try_from(creator_role).expect("non-multiworld role in multiworld team");
                            : ") invited you to enter ";
                            : event;
                            : " as ";
                            : mw::Role::try_from(my_role).expect("non-multiworld role in multiworld team");
                            : " for team ";
                            a(href = format!("https://racetime.gg/team/{}", team_row.racetime_slug.expect("multiworld team without racetime slug"))) : team_row.name;
                            @if let Some(teammates) = natjoin(teammates) {
                                : " together with ";
                                : teammates;
                            }
                            : ".";
                        }
                        TeamConfig::Pictionary => {
                            : creator;
                            : " (";
                            : pic::Role::try_from(creator_role).expect("non-Pictionary role in Pictionary team");
                            : ") invited you to join their team"; //TODO adjust pronouns based on racetime.gg user data?
                            @if let Some(team_name) = team_row.name {
                                : " ???";
                                : team_name;
                                : "???";
                            }
                            : " for ";
                            : event;
                            : " as ";
                            : pic::Role::try_from(my_role).expect("non-Pictionary role in Pictionary team");
                            @if let Some(teammates) = natjoin(teammates) {
                                : " together with ";
                                : teammates;
                            }
                            : ".";
                        }
                    }
                    div(class = "button-row") {
                        @if matches!(event.team_config(), TeamConfig::Pictionary) && my_role == Role::Sheikah && me.racetime_id.is_none() {
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
                }
            }
        })
    }
}

#[rocket::get("/notifications")]
pub(crate) async fn notifications(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>) -> Result<RawHtml<String>, Error> {
    Ok(if let Some(me) = me {
        let notifications = stream::iter(Notification::get(pool, &me).await?)
            .then(|notification| notification.into_html(pool, &me, &csrf))
            .try_collect::<Vec<_>>().await?;
        page(pool, &Some(me), &uri, PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications ??? Mido's House", html! {
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
        page(pool, &me, &uri, PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications ??? Mido's House", html! {
            p {
                a(href = uri!(auth::login(Some(uri!(notifications)))).to_string()) : "Sign in or create a Mido's House account";
                : " to view your notifications.";
            }
        }).await?
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum DismissError {
    #[error(transparent)] Csrf(#[from] rocket_csrf::VerificationFailure),
    #[error(transparent)] Sql(#[from] sqlx::Error),
}

#[rocket::post("/notifications/dismiss/<id>", data = "<form>")]
pub(crate) async fn dismiss(pool: &State<PgPool>, me: User, id: Id, csrf: Option<CsrfToken>, form: Form<EmptyForm>) -> Result<Redirect, DismissError> {
    form.verify(&csrf)?; //TODO option to resubmit on error page (with some ???are you sure???? wording)
    sqlx::query!("DELETE FROM notifications WHERE id = $1 AND rcpt = $2", i64::from(id), i64::from(me.id)).execute(&**pool).await?;
    Ok(Redirect::to(uri!(notifications)))
}
