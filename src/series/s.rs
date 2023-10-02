use crate::prelude::*;

pub(crate) fn info(event: &str) -> Option<RawHtml<String>> {
    match event {
        "6" => Some(html! {
            article {
                p {
                    : "This is the 6th season of the main Ocarina of Time randomizer tournament. See ";
                    a(href = "https://docs.google.com/document/d/1Hkrh2A_szTUTgPqkzrqjSF0YWTtU34diLaypX9pyzUI/edit") : "the official document";
                    : " for details.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://challonge.com/ChallengeCupSeason6") : "Challenge Cup groups and bracket";
                    }
                }
            }
        }),
        "7" => Some(html! {
            article {
                p {
                    : "This is the 7th season of the main Ocarina of Time randomizer tournament. See ";
                    a(href = "https://docs.google.com/document/d/1iN1q3NArRoQhean5W0qfoTSM2xLlj9QjuWkzDO0xME0/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    }
}

pub(crate) fn enter_form() -> RawHtml<String> {
    html! {
        article {
            p : "Play in the qualifiers to enter this tournament.";
            p {
                : "Note: This page is not official. See ";
                a(href = "https://docs.google.com/document/d/1iN1q3NArRoQhean5W0qfoTSM2xLlj9QjuWkzDO0xME0/edit") : "the official document";
                : " for details.";
            }
        }
    }
}
