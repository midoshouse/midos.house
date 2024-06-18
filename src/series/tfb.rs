use {
    racetime::model::*,
    crate::{
        event::{
            Data,
            InfoError,
        },
        prelude::*,
    },
};

#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct Score {
    pub(crate) pieces: u8,
    pub(crate) last_collection_time: Duration,
}

impl fmt::Display for Score {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pieces == 0 {
            write!(f, "0/3")
        } else {
            write!(f, "{}/3 in {}", self.pieces, English.format_duration(self.last_collection_time, false))
        }
    }
}

pub(crate) fn report_score_button(finish_time: Option<Duration>) -> (&'static str, ActionButton) {
    ("Report score", ActionButton::Message {
        message: format!("!score ${{pieces}} ${{last_collection_time}}"),
        help_text: Some(format!("Report your Triforce Blitz score for this race.")),
        survey: Some(vec![
            SurveyQuestion {
                name: format!("pieces"),
                label: format!("Pieces found"),
                default: Some(if let Some(finish_time) = finish_time {
                    if finish_time < Duration::from_secs(2 * 60 * 60) {
                        format!("3")
                    } else {
                        format!("1")
                    }
                } else {
                    format!("0")
                }),
                help_text: None,
                kind: SurveyQuestionKind::Radio,
                placeholder: None,
                options: vec![
                    (format!("0"), format!("0")),
                    (format!("1"), format!("1")),
                    (format!("2"), format!("2")),
                    (format!("3"), format!("3")),
                ],
            },
            SurveyQuestion {
                name: format!("last_collection_time"),
                label: format!("Most recent collection time"),
                default: finish_time.map(unparse_duration),
                help_text: Some(format!("Leave blank if you didn't collect any pieces.")),
                kind: SurveyQuestionKind::Input,
                placeholder: Some(format!("e.g. 1h23m45s")),
                options: Vec::default(),
            },
        ]),
        submit: Some(format!("Submit")),
    })
}

pub(crate) fn parse_seed_url(seed: &Url) -> Option<Uuid> {
    if_chain! {
        if let Some("triforceblitz.com" | "www.triforceblitz.com") = seed.host_str();
        if let Some(mut path_segments) = seed.path_segments();
        if path_segments.next() == Some("seed");
        if let Some(segment) = path_segments.next();
        if let Ok(uuid) = Uuid::parse_str(segment);
        if path_segments.next().is_none();
        then {
            Some(uuid)
        } else {
            None
        }
    }
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2" => Some(html! {
            article {
                p {
                    : "This is the 2nd season of the Triforce Blitz tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1p8HAwWsjsLW7tjfDl2SK-yQ35pVqbAS9GB72bkOIDFI/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        "3" => Some(html! {
            article {
                p {
                    : "This is the 3rd season of the Triforce Blitz tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1p8HAwWsjsLW7tjfDl2SK-yQ35pVqbAS9GB72bkOIDFI/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn qualifier_async_rules() -> RawHtml<String> {
    html! {
        p : "Rules:";
        ol {
            li : "You must start the seed within 15 minutes of obtaining it and submit your time within 10 minutes of finishing. Any additional time taken will be added to your final time. If technical difficulties arise with obtaining the seed/submitting your time, please DM one of the Triforce Blitz Tournament Organizers to get it sorted out. (Discord role “Triforce Blitz Organisation” for pings)";
            li : "If you obtain a seed but do not submit a finish time before submissions close, it will count as a forfeit.";
            li {
                : "Requesting the seed for async will make you ";
                strong : "ineligible";
                : " to participate in the respective live qualifier.";
            }
            li {
                : "To avoid accidental spoilers, the qualifier async ";
                strong : "CANNOT";
                : " be streamed. You must local record and upload to YouTube as an unlisted video.";
            }
            li {
                : "This should be run like an actual race. In the event of a technical issue, you are allowed to invoke the ";
                a(href = "https://docs.google.com/document/d/1BbvHJF8vtyrte76jpoCVQBTy9MYStpN3vr2PLdiCIMk/edit") : "Fair Play Agreement";
                : " and have up to a 15 minute time where you can try to catch back up. If you do this, you must fill out the appropriate field when submitting your time so it can be authenticated.";
            }
        }
    }
}
