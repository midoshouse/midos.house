use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "3" => Some(html! {
            article {
                p {
                    : "This is the 3rd co-op tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1hzTrwpKKfgCxtMnRC32xaF390zkAnT01Fr-jS5ummR0/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}
