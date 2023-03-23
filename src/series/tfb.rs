use {
    std::iter,
    futures::stream,
    itertools::Itertools as _,
    ootr_utils::spoiler::HashIcon,
    rocket::{
        form::Context,
        response::content::RawHtml,
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::html,
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        event::{
            self,
            AsyncKind,
            Data,
            Error,
            InfoError,
            Series,
        },
        seed,
        util::{
            DateTimeFormat,
            Id,
            form_field,
            format_datetime,
            full_form,
            natjoin_html,
        },
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<RawHtml<String>, InfoError> {
    Ok(match &*data.event {
        "2" => html! {
            article {
                p {
                    : "This is the 2nd season of the Triforce Blitz tournament, organized by ";
                    : natjoin_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1p8HAwWsjsLW7tjfDl2SK-yQ35pVqbAS9GB72bkOIDFI/edit") : "the official document";
                    : " for details.";
                }
            }
        },
        _ => unimplemented!(),
    })
}

pub(crate) async fn status(transaction: &mut Transaction<'_, Postgres>, csrf: Option<CsrfToken>, data: &Data<'_>, team_id: Option<Id>, ctx: Context<'_>) -> Result<RawHtml<String>, Error> {
    Ok(if let Some(async_kind) = data.active_async(&mut *transaction, team_id).await? {
        let async_row = sqlx::query!(r#"SELECT tfb_uuid AS "tfb_uuid!", hash1 AS "hash1: HashIcon", hash2 AS "hash2: HashIcon", hash3 AS "hash3: HashIcon", hash4 AS "hash4: HashIcon", hash5 AS "hash5: HashIcon" FROM asyncs WHERE series = $1 AND event = $2 AND kind = $3"#, data.series as _, &data.event, async_kind as _).fetch_one(&mut *transaction).await?;
        let team_row = if let Some(team_id) = team_id {
            sqlx::query!(r#"SELECT requested AS "requested!", submitted FROM async_teams WHERE team = $1 AND KIND = $2 AND requested IS NOT NULL"#, i64::from(team_id), async_kind as _).fetch_optional(&mut *transaction).await?
        } else {
            None
        };
        if let Some(team_row) = team_row {
            if team_row.submitted.is_some() {
                if data.is_started(&mut *transaction).await? {
                    //TODO get this entrant's known matchup(s)
                    html! {
                        p : "Please schedule your matches using Discord threads in the scheduling channel.";
                    }
                } else {
                    //TODO if any vods are still missing, show form to add them
                    html! {
                        p : "Waiting for the start of the tournament and round 1 pairings. Keep an eye out for an announcement on Discord."; //TODO include start date?
                    }
                }
            } else {
                let seed = seed::Data {
                    file_hash: match (async_row.hash1, async_row.hash2, async_row.hash3, async_row.hash4, async_row.hash5) {
                        (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                        (None, None, None, None, None) => None,
                        _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
                    },
                    files: seed::Files::TriforceBlitz { uuid: async_row.tfb_uuid },
                };
                let seed_table = seed::table(stream::iter(iter::once(seed)), false).await?;
                let mut errors = ctx.errors().collect_vec();
                html! {
                    div(class = "info") {
                        p {
                            : "You requested the qualifier async on ";
                            : format_datetime(team_row.requested, DateTimeFormat { long: true, running_text: true });
                            : ".";
                        };
                        : seed_table;
                        p : "After playing the async, fill out the form below.";
                        : full_form(uri!(event::submit_async(data.series, &*data.event)), csrf, html! {
                            : form_field("pieces", &mut errors, html! {
                                label(for = "pieces") : "Number of Triforce Pieces found:";
                                input(type = "number", min = "0", max = "3", name = "pieces", value? = ctx.field_value("pieces"));
                            });
                            : form_field("time1", &mut errors, html! {
                                label(for = "time1") : "Time at which you found the most recent piece:";
                                input(type = "text", name = "time1", value? = ctx.field_value("time1")); //TODO h:m:s fields?
                                label(class = "help") : "(If you did not find any, leave this field blank.)";
                            });
                            : form_field("vod1", &mut errors, html! {
                                label(for = "vod1", class = "power") : "VoD:";
                                input(type = "text", name = "vod1", value? = ctx.field_value("vod1"));
                                label(class = "help") : "(You must submit a link to an unlisted YouTube video upload. The link to a YouTube video becomes available as soon as you begin the upload process.)";
                            });
                            : form_field("fpa", &mut errors, html! {
                                label(for = "fpa") {
                                    : "If you would like to invoke the ";
                                    a(href = "https://docs.google.com/document/d/1BbvHJF8vtyrte76jpoCVQBTy9MYStpN3vr2PLdiCIMk/edit") : "Fair Play Agreement";
                                    : ", describe the break(s) you took below. Include the reason, starting time, and duration.";
                                }
                                textarea(name = "fpa"); //TODO fill from form context
                            });
                        }, errors, "Submit");
                    }
                }
            }
        } else {
            html! {
                div(class = "info") {
                    @match async_kind {
                        AsyncKind::Qualifier => p {
                            : "To enter this tournament, play the qualifier, either live on ";
                            : format_datetime(data.start(&mut *transaction).await?.expect("missing start time for tfb/2"), DateTimeFormat { long: true, running_text: true });
                            : " or request it as an async using this form by ";
                            : format_datetime(data.start(&mut *transaction).await?.expect("missing start time for tfb/2") - chrono::Duration::hours(3), DateTimeFormat { long: true, running_text: true });
                            : ".";
                        }
                        _ => @unimplemented
                    }
                    p : "Rules:";
                    ol {
                        li : "You must start the seed within 15 minutes of obtaining it and submit your time within 10 minutes of finishing. Any additional time taken will be added to your final time. If technical difficulties arise with obtaining the seed/submitting your time, please DM one of the Triforce Blitz Tournament Organizers to get it sorted out. (Discord role “Triforce Blitz Organisation” for pings)";
                        li : "If you obtain a seed but do not submit a finish time before submissions close, it will count as a forfeit.";
                        li {
                            : "Requesting the seed for async will make you ";
                            strong : "ineligible";
                            : " to participate in the live qualifier on April 8th.";
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
                    @let mut errors = ctx.errors().collect_vec();
                    : full_form(uri!(event::request_async(data.series, &*data.event)), csrf, html! {
                        : form_field("confirm", &mut errors, html! {
                            input(type = "checkbox", id = "confirm", name = "confirm");
                            label(for = "confirm") : "I have read the above and am ready to play the seed";
                        });
                    }, errors, "Request Now");
                }
            }
        }
    } else {
        html! {
            p {
                : "To enter this tournament, play the qualifier, either live on ";
                : format_datetime(data.start(&mut *transaction).await?.expect("missing start time for tfb/2"), DateTimeFormat { long: true, running_text: true });
                : " or async starting on April 2.";
            }
        }
    })
}