use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "5" => Some(html! {
            article {
                p {
                    : "Season 5 of the Scrubs tournament is organized by Aughoti, Froppy, Oakishi, picks, ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". ";
                    a(href = "https://discord.gg/hpHngEY") : "Join the Discord server";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}
