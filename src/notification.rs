use {
    horrorshow::{
        RenderBox,
        html,
    },
    rocket::{
        State,
        uri,
    },
    sqlx::PgPool,
    crate::{
        PageKind,
        PageResult,
        PageStyle,
        User,
        auth,
        page,
    },
};

pub(crate) enum Notification {}

impl Notification {
    pub(crate) async fn get(_: &PgPool, _: &User) -> sqlx::Result<Vec<Self>> {
        Ok(Vec::default()) //TODO
    }

    fn to_html(&self) -> Box<dyn RenderBox> {
        match *self {}
    }
}

#[rocket::get("/notifications")]
pub(crate) async fn notifications(pool: &State<PgPool>, me: Option<User>) -> PageResult {
    if let Some(me) = me {
        let notifications = Notification::get(&pool, &me).await?;
        page(&pool, &Some(me), PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications — Mido's House", html! {
            h1 : "Notifications";
            @if notifications.is_empty() {
                p : "You have no notifications.";
            } else {
                ul {
                    @for notification in notifications {
                        li : notification.to_html();
                    }
                }
            }
        }).await
    } else {
        page(&pool, &me, PageStyle { kind: PageKind::Notifications, ..PageStyle::default() }, "Notifications — Mido's House", html! {
            p {
                a(href = uri!(auth::login).to_string()) : "Sign in or create a Mido's House account";
                : " to view your notifications.";
            }
        }).await
    }
}
