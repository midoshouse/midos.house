use {
    std::mem,
    rocket::{
        FromForm,
        form,
        http::uri::Origin,
        response::content::RawHtml,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        CsrfForm,
        ToHtml,
        html,
    },
};

/// A form that only holds a CSRF token
#[derive(FromForm, CsrfForm)]
pub(crate) struct EmptyForm {
    #[field(default = String::new())]
    csrf: String,
}

fn render_form_error(error: &form::Error<'_>) -> RawHtml<String> {
    html! {
        p(class = "error") : error.to_string();
    }
}

pub(crate) fn form_field(name: &str, errors: &mut Vec<&form::Error<'_>>, content: impl ToHtml) -> RawHtml<String> {
    let field_errors;
    (field_errors, *errors) = mem::take(errors).into_iter().partition(|error| error.is_for(name));
    html! {
        fieldset(class? = (!field_errors.is_empty()).then(|| "error")) {
            @for error in field_errors {
                : render_form_error(error);
            }
            : content;
        }
    }
}

pub(crate) fn full_form(uri: Origin<'_>, csrf: Option<&CsrfToken>, content: impl ToHtml, errors: Vec<&form::Error<'_>>, submit_text: &str) -> RawHtml<String> {
    html! {
        form(action = uri.to_string(), method = "post") {
            : csrf;
            @for error in errors {
                : render_form_error(error);
            }
            : content;
            fieldset {
                input(type = "submit", value = submit_text);
            }
        }
    }
}
