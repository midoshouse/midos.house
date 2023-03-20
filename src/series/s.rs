use {
    rocket::response::content::RawHtml,
    rocket_util::html,
};

pub(crate) fn info(event: &str) -> RawHtml<String> {
    match event {
        "6" => html! {
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
        },
        _ => unimplemented!(),
    }
}
