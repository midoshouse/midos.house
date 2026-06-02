use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2026" => Some(html! {
            article {
                p {
                    : "This is the first OOTR Spoiler Log Tournament, organized by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1FuTuwsDtguuxaF5sDmReWpt6o8MqyVwRov0osVoeiYA/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}
