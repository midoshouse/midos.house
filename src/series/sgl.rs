use {
    rocket::response::content::RawHtml,
    rocket_util::html,
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        event::{
            Data,
            InfoError,
        },
        lang::Language::English,
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2023onl" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2023 SpeedGaming Live online OoTR tournament, organized by ";
                        : English.join_html(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://docs.google.com/document/d/1EACqBl8ZOreD6xT5jQ2HrdLOnpBpKyjS3FUYK8XFeqg/edit") : "Rules document";
                            }
                            li {
                                a(href = "https://sglive.speedgaming.org/") : "Main SGL event page";
                            }
                            li {
                                a(href = "https://discord.gg/YGzQsUp") : "Main SGL Discord";
                            }
                            li {
                                a(href = "https://discord.gg/UjPaKk5b2w") : "OoTR SGL Discord";
                            }
                        }
                    }
                }
            })
        }
        "2023live" => {
            Some(html! {
                article {
                    p {
                        : "Welcome to the 2023 SpeedGaming Live in-person OoTR tournament, organized by ";
                        : English.join_html(data.organizers(&mut *transaction).await?);
                        : ".";
                        h2 : "See also";
                        ul {
                            li {
                                a(href = "https://docs.google.com/document/d/1EACqBl8ZOreD6xT5jQ2HrdLOnpBpKyjS3FUYK8XFeqg/edit") : "Rules document";
                            }
                            li {
                                a(href = "https://sglive.speedgaming.org/") : "Main SGL event page";
                            }
                            li {
                                a(href = "https://matcherino.com/t/sglive23") : "Matcherino";
                            }
                            li {
                                a(href = "https://discord.gg/YGzQsUp") : "Main SGL Discord";
                            }
                            li {
                                a(href = "https://discord.gg/UjPaKk5b2w") : "OoTR SGL Discord";
                            }
                        }
                    }
                }
            })
        }
        _ => None,
    })
}
