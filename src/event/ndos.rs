use {
    futures::stream::{
        self,
        StreamExt as _,
        TryStreamExt as _,
    },
    rocket::response::content::RawHtml,
    rocket_util::html,
    sqlx::PgPool,
    crate::{
        event::InfoError,
        user::User,
        util::{
            Id,
            natjoin_html,
        },
    },
};

pub(super) async fn info(pool: &PgPool, event: &str) -> Result<RawHtml<String>, InfoError> {
    let organizers = stream::iter([
        5246396495391975113, // Kofca
    ])
        .map(Id)
        .then(|id| async move { User::from_id(pool, id).await?.ok_or(InfoError::OrganizerUserData) })
        .try_collect::<Vec<_>>().await?;
    Ok(html! {
        article {
            p {
                : "This is a placeholder for day ";
                : event;
                : " of the 9 Days of SAWS event, organized by ";
                : natjoin_html(organizers);
                : ". More infos coming soon.";
            }
        }
    })
}
