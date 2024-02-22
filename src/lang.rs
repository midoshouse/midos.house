#![allow(unused_qualifications)] // in derive macro

use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Sequence, Deserialize, sqlx::Type, FromFormField)]
#[sqlx(type_name = "language")]
pub(crate) enum Language {
    #[serde(rename = "en")]
    #[sqlx(rename = "en")]
    #[field(value = "en")]
    English,
    #[serde(rename = "fr")]
    #[sqlx(rename = "fr")]
    #[field(value = "fr")]
    French,
    #[serde(rename = "de")]
    #[sqlx(rename = "de")]
    #[field(value = "de")]
    German,
    #[serde(rename = "pt")]
    #[sqlx(rename = "pt")]
    #[field(value = "pt")]
    Portuguese,
}

impl Language {
    pub(crate) fn short_code(&self) -> &'static str {
        match self {
            English => "en",
            French => "fr",
            German => "de",
            Portuguese => "pt",
        }
    }

    pub(crate) fn format_duration(&self, duration: Duration, running_text: bool) -> String {
        let secs = duration.as_secs();
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;
        if running_text {
            match self {
                French => {
                    let parts = (hours > 0).then(|| format!("{hours} heure{}", if hours == 1 { "" } else { "s" })).into_iter()
                        .chain((mins > 0).then(|| format!("{mins} minute{}", if mins == 1 { "" } else { "s" })))
                        .chain((secs > 0).then(|| format!("{secs} seconde{}", if secs == 1 { "" } else { "s" })));
                    French.join_str(parts).unwrap_or_else(|| format!("0 secondes"))
                }
                _ => {
                    let parts = (hours > 0).then(|| format!("{hours} hour{}", if hours == 1 { "" } else { "s" })).into_iter()
                        .chain((mins > 0).then(|| format!("{mins} minute{}", if mins == 1 { "" } else { "s" })))
                        .chain((secs > 0).then(|| format!("{secs} second{}", if secs == 1 { "" } else { "s" })));
                    English.join_str(parts).unwrap_or_else(|| format!("0 seconds"))
                }
            }
        } else {
            format!("{hours}:{mins:02}:{secs:02}")
        }
    }

    pub(crate) fn join_html<T: ToHtml>(&self, elts: impl IntoIterator<Item = T>) -> Option<RawHtml<String>> {
        match self {
            French | Portuguese => {
                let mut elts = elts.into_iter().fuse();
                match (elts.next(), elts.next()) {
                    (None, _) => None,
                    (Some(elt), None) => Some(html! {
                        : elt;
                    }),
                    (Some(elt1), Some(elt2)) => {
                        let mut rest = iter::once(elt2).chain(elts).collect_vec();
                        let last = rest.pop().expect("rest contains at least elt3");
                        Some(html! {
                            : elt1;
                            @for elt in rest {
                                : ", ";
                                : elt;
                            }
                            @match self {
                                French => : " et ";
                                Portuguese => : " e ";
                                _ => @unreachable
                            }
                            : last;
                        })
                    }
                }
            }
            _ => {
                let mut elts = elts.into_iter().fuse();
                match (elts.next(), elts.next(), elts.next()) {
                    (None, _, _) => None,
                    (Some(elt), None, _) => Some(html! {
                        : elt;
                    }),
                    (Some(elt1), Some(elt2), None) => Some(html! {
                        : elt1;
                        : " and ";
                        : elt2;
                    }),
                    (Some(elt1), Some(elt2), Some(elt3)) => {
                        let mut rest = iter::once(elt3).chain(elts).collect_vec();
                        let last = rest.pop().expect("rest contains at least elt3");
                        Some(html! {
                            : elt1;
                            : ", ";
                            : elt2;
                            @for elt in rest {
                                : ", ";
                                : elt;
                            }
                            : ", and ";
                            : last;
                        })
                    }
                }
            }
        }
    }

    pub(crate) fn join_str<T: fmt::Display>(&self, elts: impl IntoIterator<Item = T>) -> Option<String> {
        match self {
            French => French.join_str_with("et", elts),
            _ => English.join_str_with("and", elts),
        }
    }

    pub(crate) fn join_str_with<T: fmt::Display>(&self, conjunction: &str, elts: impl IntoIterator<Item = T>) -> Option<String> {
        match self {
            French => {
                let mut elts = elts.into_iter().fuse();
                match (elts.next(), elts.next()) {
                    (None, _) => None,
                    (Some(elt), None) => Some(elt.to_string()),
                    (Some(elt1), Some(elt2)) => {
                        let mut rest = iter::once(elt2).chain(elts).collect_vec();
                        let last = rest.pop().expect("rest contains at least elt2");
                        Some(format!("{elt1}{} {conjunction} {last}", rest.into_iter().map(|elt| format!(", {elt}")).format("")))
                    }
                }
            }
            _ => {
                let mut elts = elts.into_iter().fuse();
                match (elts.next(), elts.next(), elts.next()) {
                    (None, _, _) => None,
                    (Some(elt), None, _) => Some(elt.to_string()),
                    (Some(elt1), Some(elt2), None) => Some(format!("{elt1} {conjunction} {elt2}")),
                    (Some(elt1), Some(elt2), Some(elt3)) => {
                        let mut rest = [elt2, elt3].into_iter().chain(elts).collect_vec();
                        let last = rest.pop().expect("rest contains at least elt2 and elt3");
                        Some(format!("{elt1}, {}, {conjunction} {last}", rest.into_iter().format(", ")))
                    }
                }
            }
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            English => write!(f, "English"),
            French => write!(f, "French"),
            German => write!(f, "German"),
            Portuguese => write!(f, "Portuguese"),
        }
    }
}

impl ToHtml for Language {
    fn to_html(&self) -> RawHtml<String> {
        html! {
            : self.to_string();
        }
    }
}
