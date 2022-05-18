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
            natjoin,
        },
    },
};

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
