use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(Some(html! {
        article {
            p {
                : "This is OoTR League season ";
                : data.event;
                : ", organized by ";
                : English.join_html(data.organizers(transaction).await?);
                : ". See ";
                a(href = "https://league.ootrandomizer.com/") : "the official website";
                : " for details.";
            }
        }
    }))
}
