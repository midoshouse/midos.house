use {
    futures::stream::{
        self,
        StreamExt as _,
        TryStreamExt as _,
    },
    horrorshow::{
        RenderBox,
        box_html,
        html,
        owned_html,
        rocket::TemplateExt as _,
    },
    rocket::{
        State,
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
        PageKind,
        PageStyle,
        User,
        auth,
        event::{
            Role,
            SignupStatus,
        },
        page,
        util::{
            Id,
            natjoin,
        },
    },
};

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum Error {
    #[error(transparent)] Horrorshow(#[from] horrorshow::Error),
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
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

    async fn into_html(self, pool: &PgPool, me: &User) -> Result<Html<String>, Error> {
        Ok(match self {
            Self::Simple(id) => {
                let text = match sqlx::query_scalar!(r#"SELECT kind AS "kind: SimpleNotificationKind" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await? {
                    SimpleNotificationKind::Accept => {
                        let sender = sqlx::query_scalar!(r#"SELECT sender AS "sender!: Id" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await?;
                        let sender = User::from_id(pool, sender).await?.ok_or(Error::UnknownUser)?;
                        (box_html! {
                            : sender.into_html();
                            : " accepted your invitation to join a team for ";
                            a(href = uri!(crate::event::pictionary_random_settings).to_string()) : "the 1st Random Settings Pictionary Spoiler Log Race"; //TODO don't hardcode event
                            : ".";
                        }) as Box<dyn RenderBox>
                    }
                    SimpleNotificationKind::Decline => {
                        let sender = sqlx::query_scalar!(r#"SELECT sender AS "sender!: Id" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await?;
                        let sender = User::from_id(pool, sender).await?.ok_or(Error::UnknownUser)?;
                        box_html! {
                            : sender.into_html();
                            : " declined your invitation to form a team for ";
                            a(href = uri!(crate::event::pictionary_random_settings).to_string()) : "the 1st Random Settings Pictionary Spoiler Log Race"; //TODO don't hardcode event
                            : ".";
                        }
                    }
                    SimpleNotificationKind::Resign => {
                        let sender = sqlx::query_scalar!(r#"SELECT sender AS "sender!: Id" FROM notifications WHERE id = $1"#, i64::from(id)).fetch_one(pool).await?;
                        let sender = User::from_id(pool, sender).await?.ok_or(Error::UnknownUser)?;
                        box_html! {
                            : sender.into_html();
                            : " resigned your team from ";
                            a(href = uri!(crate::event::pictionary_random_settings).to_string()) : "the 1st Random Settings Pictionary Spoiler Log Race"; //TODO don't hardcode event
                            : ".";
                        }
                    }
                };
                html! {
                    : text;
                    div(class = "button-row") {
                        form(action = uri!(dismiss(id)).to_string(), method = "post") {
                            input(type = "submit", value = "Dismiss");
                        }
                    }
                }.write_to_html()?
            }
            Self::TeamInvite(team_id) => {
                let team_name = sqlx::query_scalar!("SELECT name FROM teams WHERE id = $1", i64::from(team_id)).fetch_one(pool).await?;
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
                        teammates.push(owned_html! {
                            : user.into_html();
                            : " (";
                            : member.role.to_html();
                            @if is_confirmed {
                                : ", confirmed)";
                            } else {
                                : ", unconfirmed)";
                            }
                        });
                    }
                }
                let (creator, creator_role) = creator.ok_or(Error::UnknownUser)?;
                let my_role = my_role.ok_or(Error::UnknownUser)?;
                html! {
                    : creator.into_html();
                    : " (";
                    : creator_role.to_html();
                    : ") invited you to join their team"; //TODO adjust pronouns based on racetime.gg user data?
                    @if let Some(team_name) = team_name {
                        : " “";
                        : team_name;
                        : "”";
                    }
                    : " for ";
                    a(href = uri!(crate::event::pictionary_random_settings).to_string()) : "the 1st Random Settings Pictionary Spoiler Log Race"; //TODO don't hardcode event
                    : " as ";
                    : my_role.to_html();
                    @if let Some(teammates) = natjoin(teammates) {
                        : " together with ";
                        : teammates;
                    }
                    : ".";
                    div(class = "button-row") {
                        @if my_role == Role::Sheikah && me.racetime_id.is_none() {
                            a(class = "button", href = uri!(crate::auth::racetime_login).to_string()) : "Connect racetime.gg Account to Accept";
                        } else {
                            form(action = uri!(crate::event::pictionary_random_settings_confirm_signup(team_id)).to_string(), method = "post") {
                                input(type = "submit", value = "Accept");
                            }
                        }
                        form(action = uri!(crate::event::pictionary_random_settings_resign(team_id)).to_string(), method = "post") {
                            input(type = "submit", value = "Decline");
                        }
                        //TODO block sender, block event
                    }
                }.write_to_html()?
            }
        })
    }
}

#[rocket::get("/notifications")]
pub(crate) async fn notifications(pool: &State<PgPool>, me: Option<User>) -> Result<Html<String>, Error> {
    if let Some(me) = me {
        let notifications = stream::iter(Notification::get(pool, &me).await?)
            .then(|notification| notification.into_html(pool, &me))
            .try_collect::<Vec<_>>().await?;
        page(pool, &Some(me), PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications — Mido's House", html! {
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
        }).await.map_err(Error::from)
    } else {
        page(&pool, &me, PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications — Mido's House", html! {
            p {
                a(href = uri!(auth::login).to_string()) : "Sign in or create a Mido's House account";
                : " to view your notifications.";
            }
        }).await.map_err(Error::from)
    }
}

#[rocket::post("/notifications/dismiss/<id>")]
pub(crate) async fn dismiss(pool: &State<PgPool>, me: User, id: Id) -> Result<Redirect, Debug<sqlx::Error>> {
    sqlx::query!("DELETE FROM notifications WHERE id = $1 AND rcpt = $2", i64::from(id), i64::from(me.id)).execute(&**pool).await?;
    Ok(Redirect::to(uri!(notifications)))
}
