use crate::{
    event::{
        Data,
        Error,
        Tab,
        enter,
    },
    prelude::*,
    series::pic::EnterFormDefaults,
};

#[derive(Clone, Copy, PartialEq, Eq, FromFormField, UriDisplayQuery)]
pub(crate) enum Role {
    #[field(value = "sheikah")]
    Sheikah,
    #[field(value = "gerudo")]
    Gerudo,
}

impl ToHtml for Role {
    fn to_html(&self) -> RawHtml<String> {
        match self {
            Self::Sheikah => html! {
                span(class = "sheikah") : "mentor";
            },
            Self::Gerudo => html! {
                span(class = "gerudo") : "mentee";
            },
        }
    }
}

impl TryFrom<event::Role> for Role {
    type Error = ();

    fn try_from(role: event::Role) -> Result<Self, ()> {
        match role {
            event::Role::Sheikah => Ok(Self::Sheikah),
            event::Role::Gerudo => Ok(Self::Gerudo),
            _ => Err(()),
        }
    }
}

impl From<Role> for event::Role {
    fn from(role: Role) -> Self {
        match role {
            Role::Sheikah => Self::Sheikah,
            Role::Gerudo => Self::Gerudo,
        }
    }
}

pub(crate) async fn enter_form(mut transaction: Transaction<'_, Postgres>, global: &GlobalState, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, data: Data<'_>, defaults: EnterFormDefaults<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, global, me.as_ref(), csrf, Tab::Enter, false).await?;
    Ok(page(transaction, global, &me, &uri, PageStyle::new(data.chests().await?), &format!("Enter — {}", data.display_name), if me.is_some() {
        let mut errors = defaults.errors();
        html! {
            : header;
            : full_form(uri!(enter::post(data.series, &*data.event)), csrf, html! {
                legend {
                    : "Fill out this form to enter the event as a team. Your teammate will receive an invitation they have to accept to confirm the signup. If you don't have a team yet, you can ";
                    @if let Some(ref find_team_url) = data.find_team_url {
                        a(href = find_team_url) : "look for a teammate";
                    } else {
                        a(href = uri!(event::find_team(data.series, &*data.event))) : "look for a teammate";
                    }
                    : " instead.";
                }
                : form_field("team_name", &mut errors, html! {
                    label(for = "team_name") : "Team Name:";
                    input(type = "text", name = "team_name", value? = defaults.team_name());
                    label(class = "help") : "(Optional unless you want to be on restream. Can be changed later. Organizers may remove inappropriate team names.)";
                });
                : form_field("my_role", &mut errors, html! {
                    label(for = "my_role") : "My Role:";
                    input(id = "my_role-sheikah", class = "sheikah", type = "radio", name = "my_role", value = "sheikah", checked? = defaults.my_role() == Some(pic::Role::Sheikah));
                    label(class = "sheikah", for = "my_role-sheikah") : "Mentor";
                    input(id = "my_role-gerudo", class = "gerudo", type = "radio", name = "my_role", value = "gerudo", checked? = defaults.my_role() == Some(pic::Role::Gerudo));
                    label(class = "gerudo", for = "my_role-gerudo") : "Mentee";
                });
                : form_field("teammate", &mut errors, html! {
                    label(for = "teammate") : "Teammate:";
                    input(type = "text", name = "teammate", value? = defaults.teammate_text().as_deref());
                    label(class = "help") : "(Enter your teammate's Mido's House user ID. It can be found on their profile page.)"; //TODO add JS-based user search?
                });
            }, errors, "Enter");
        }
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(enter::get(data.series, &*data.event, defaults.my_role(), defaults.teammate())))))) : "Sign in or create a Mido's House account";
                    : " to enter this event.";
                }
            }
        }
    }).await?)
}
