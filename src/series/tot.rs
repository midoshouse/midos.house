use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2" => Some(html! {
            article {
                p {
                    : "This is the 2nd season of the Tournament of Truth, organized by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1YNCm4XUCeWlz9UHPz5lwTIRUVasnAIjP8aXC7r5djWc/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}
