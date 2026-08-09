use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => Some(html! {
            article {
                p {
                    : "This event is organised by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/14Omq_KhFKurXweqkp5MHN9kxsJKOdV_M4CA2bexvEG4/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}
