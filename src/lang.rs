#![allow(unused_qualifications)] // in derive macro

use {
    std::{
        fmt,
        iter,
    },
    enum_iterator::Sequence,
    itertools::Itertools as _,
    rocket::{
        FromFormField,
        response::content::RawHtml,
    },
    rocket_util::{
        ToHtml,
        html,
    },
    self::Language::*,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Sequence, sqlx::Type, FromFormField)]
#[sqlx(type_name = "language")]
pub(crate) enum Language {
    #[sqlx(rename = "en")]
    #[field(value = "en")]
    English,
    #[sqlx(rename = "fr")]
    #[field(value = "fr")]
    French,
    #[sqlx(rename = "pt")]
    #[field(value = "pt")]
    Portuguese,
}

impl Language {
    pub(crate) fn short_code(&self) -> &'static str {
        match self {
            English => "en",
            French => "fr",
            Portuguese => "pt",
        }
    }

    pub(crate) fn join_html<T: ToHtml>(&self, elts: impl IntoIterator<Item = T>) -> Option<RawHtml<String>> {
        match self {
            French => {
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
                            : " et ";
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
            French => {
                let mut elts = elts.into_iter().fuse();
                match (elts.next(), elts.next()) {
                    (None, _) => None,
                    (Some(elt), None) => Some(elt.to_string()),
                    (Some(elt1), Some(elt2)) => {
                        let mut rest = iter::once(elt2).chain(elts).collect_vec();
                        let last = rest.pop().expect("rest contains at least elt2");
                        Some(format!("{elt1}{} et {last}", rest.into_iter().map(|elt| format!(", {elt}")).format("")))
                    }
                }
            }
            _ => {
                let mut elts = elts.into_iter().fuse();
                match (elts.next(), elts.next(), elts.next()) {
                    (None, _, _) => None,
                    (Some(elt), None, _) => Some(elt.to_string()),
                    (Some(elt1), Some(elt2), None) => Some(format!("{elt1} and {elt2}")),
                    (Some(elt1), Some(elt2), Some(elt3)) => {
                        let mut rest = [elt2, elt3].into_iter().chain(elts).collect_vec();
                        let last = rest.pop().expect("rest contains at least elt2 and elt3");
                        Some(format!("{elt1}, {}, and {last}", rest.into_iter().format(", ")))
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
