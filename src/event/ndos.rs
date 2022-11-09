use rocket::response::content::RawHtml;

pub(super) fn info(event: &str) -> RawHtml<String> {
    match event {
        _ => unimplemented!(), //TODO
    }
}
