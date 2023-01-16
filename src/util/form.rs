use {
    std::mem,
    rocket::{
        FromForm,
        form,
        response::content::RawHtml,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        ToHtml,
        html,
    },
};

/// A form that only holds a CSRF token
#[derive(FromForm)]
pub(crate) struct EmptyForm {
    csrf: String,
}

impl EmptyForm {
    pub(crate) fn verify(&self, token: &Option<CsrfToken>) -> Result<(), rocket_csrf::VerificationFailure> {
        if let Some(token) = token {
            token.verify(&self.csrf)
        } else {
            Err(rocket_csrf::VerificationFailure)
        }
    }
}

pub(crate) fn render_form_error(error: &form::Error<'_>) -> RawHtml<String> {
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
