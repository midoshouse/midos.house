use {
    base64::engine::{
        Engine as _,
        general_purpose::STANDARD as BASE64,
    },
    rocket::{
        Rocket,
        config::SecretKey,
        fs::FileServer,
        response::content::RawText,
    },
    rocket_oauth2::{
        OAuth2,
        OAuthConfig,
    },
    rocket_util::Doctype,
    crate::{
        api,
        notification::{
            self,
            Notification,
        },
        racetime_bot::SeedMetadata,
        prelude::*,
    },
};

include!(concat!(env!("OUT_DIR"), "/static_files.rs"));

pub(crate) use static_url;

#[derive(Responder)]
pub(crate) enum RedirectOrContent {
    Redirect(Redirect),
    Content(RawHtml<String>),
}

#[derive(Responder)]
pub(crate) enum StatusOrError<E> {
    Status(Status),
    Err(E),
}

pub(crate) fn favicon(url: &Url) -> RawHtml<String> {
    match url.host_str() {
        Some("multistre.am") => html! {
            img(class = "favicon", alt = "external link (multistre.am)", src = static_url!("multistream-favicon.jpg"));
        },
        Some("youtu.be") => html! {
            img(class = "favicon", alt = "external link (youtu.be)", srcset = "https://www.youtube.com/s/desktop/435d54f2/img/favicon.ico 16w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_32x32.png 32w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_48x48.png 48w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_96x96.png 96w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_144x144.png 144w");
        },
        Some("challonge.com" | "www.challonge.com") => html! {
            img(class = "favicon", alt = "external link (challonge.com)", srcset = "https://assets.challonge.com/favicon-16x16.png 16w, https://assets.challonge.com/favicon-32x32.png 32w");
        },
        Some("docs.google.com") if url.path_segments().into_iter().flatten().next() == Some("document") => html! {
            img(class = "favicon", alt = "external link (docs.google.com/document)", src = "https://ssl.gstatic.com/docs/documents/images/kix-favicon7.ico");
        },
        Some("docs.google.com") if url.path_segments().into_iter().flatten().next() == Some("forms") => html! {
            img(class = "favicon", alt = "external link (docs.google.com/forms)", srcset = "https://ssl.gstatic.com/docs/spreadsheets/forms/favicon_qp2.png 16w, https://ssl.gstatic.com/docs/forms/device_home/android_192.png 192w");
        },
        Some("docs.google.com") if url.path_segments().into_iter().flatten().next() == Some("spreadsheets") => html! {
            img(class = "favicon", alt = "external link (docs.google.com/spreadsheets)", src = "https://ssl.gstatic.com/docs/spreadsheets/favicon3.ico");
        },
        Some("drive.google.com") => html! {
            img(class = "favicon", alt = "external link (drive.google.com)", src = "https://ssl.gstatic.com/docs/doclist/images/drive_2022q3_32dp.png");
        },
        Some("ootrandomizer.com" | "league.ootrandomizer.com") => html! {
            img(class = "favicon", alt = "external link (ootrandomizer.com)", srcset = "https://ootrandomizer.com/img/favicon-16x16.png 16w, https://ootrandomizer.com/img/favicon-32x32.png 32w");
        },
        Some("tiltify.com") => html! {
            img(class = "favicon", alt = "external link (tiltify.com)", srcset = "https://assets.tiltify.com/favicons/favicon-16x16.png 16w, https://assets.tiltify.com/favicons/favicon-32x32.png 32w, https://assets.tiltify.com/favicons/favicon-48x48.png 48w, https://assets.tiltify.com/favicons/favicon-64x64.png 64w, https://assets.tiltify.com/favicons/favicon-96x96.png 96w, https://assets.tiltify.com/favicons/favicon-128x128.png 128w, https://assets.tiltify.com/favicons/favicon-256x256.png 256w");
        },
        Some("youtube.com" | "www.youtube.com") => html! {
            img(class = "favicon", alt = "external link (youtube.com)", srcset = "https://www.youtube.com/s/desktop/435d54f2/img/favicon.ico 16w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_32x32.png 32w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_48x48.png 48w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_96x96.png 96w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_144x144.png 144w");
        },
        Some("zeldaspeedruns.com" | "www.zeldaspeedruns.com") => html! {
            img(class = "favicon", alt = "external link (zeldaspeedruns.com)", srcset = "https://www.zeldaspeedruns.com/favicon-16x16.png 16w, https://www.zeldaspeedruns.com/favicon-32x32.png 32w, https://www.zeldaspeedruns.com/favicon-96x96.png 96w, https://www.zeldaspeedruns.com/android-chrome-192x192.png 192w, https://www.zeldaspeedruns.com/favicon-194x194.png 194w");
        },
        Some("discord.gg") => html! {
            img(class = "favicon", alt = "external link (discord.gg)", src = static_url!("discord-favicon.ico"));
        },
        Some("racetime.gg" | "racetime.midos.house") => html! {
            img(class = "favicon", alt = "external link (racetime.gg)", src = static_url!("racetimeGG-favicon.svg"));
        },
        Some("start.gg" | "www.start.gg") => html! {
            img(class = "favicon", alt = "external link (start.gg)", src = "https://www.start.gg/__static/images/favicon/favicon.ico");
        },
        Some("twitch.tv" | "www.twitch.tv") => html! {
            img(class = "favicon", alt = "external link (twitch.tv)", srcset = "https://static.twitchcdn.net/assets/favicon-16-52e571ffea063af7a7f4.png 16w, https://static.twitchcdn.net/assets/favicon-32-e29e246c157142c94346.png 32w");
        },
        _ => html! {
            : "🌐";
        },
    }
}

pub(crate) enum PageKind {
    Index,
    Banner,
    Center,
    Login,
    MyProfile,
    Notifications,
    Other,
}

pub(crate) struct PageStyle {
    pub(crate) kind: PageKind,
    pub(crate) chests: ChestAppearances,
}

impl Default for PageStyle {
    fn default() -> Self {
        Self {
            kind: PageKind::Other,
            chests: ChestAppearances::random(),
        }
    }
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum PageError {
    #[error(transparent)] Event(#[from] event::DataError),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("missing user data for Fenhl")]
    FenhlUserData,
    #[error("missing user data for Xopar")]
    XoparUserData,
}

impl<E: Into<PageError>> From<E> for StatusOrError<PageError> {
    fn from(e: E) -> Self {
        Self::Err(e.into())
    }
}

impl IsNetworkError for PageError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Event(_) => false,
            Self::Sql(_) => false,
            Self::Wheel(e) => e.is_network_error(),
            Self::FenhlUserData => false,
            Self::XoparUserData => false,
        }
    }
}

pub(crate) type PageResult = Result<RawHtml<String>, PageError>;

pub(crate) async fn page(mut transaction: Transaction<'_, Postgres>, me: &Option<User>, uri: &Origin<'_>, style: PageStyle, title: &str, content: impl ToHtml) -> PageResult {
    let notifications = if let Some(me) = me {
        if let PageKind::Notifications = style.kind {
            Vec::default()
        } else {
            Notification::get(&mut transaction, me).await?
        }
    } else {
        Vec::default()
    };
    let (banner_content, content) = if let PageKind::Banner = style.kind {
        (Some(content), None)
    } else {
        (None, Some(content))
    };
    let fenhl = User::from_id(&mut *transaction, Id::from(14571800683221815449_u64)).await?.ok_or(PageError::FenhlUserData)?;
    let xopar = User::from_id(&mut *transaction, Id::from(17762941071474623984_u64)).await?.ok_or(PageError::XoparUserData)?;
    transaction.commit().await?;
    Ok(html! {
        : Doctype;
        html {
            head {
                meta(charset = "utf-8");
                title : title;
                meta(name = "viewport", content = "width=device-width, initial-scale=1, shrink-to-fit=no");
                link(rel = "icon", sizes = "1024x1024", type = "image/png", href = uri!(favicon::favicon_png(Suffix(style.chests.textures(), "png"))).to_string());
                link(rel = "stylesheet", href = static_url!("common.css"));
                script(defer, src = static_url!("common.js"));
            }
            body(class = matches!(style.kind, PageKind::Banner).then(|| "fullscreen")) {
                div {
                    nav(class? = matches!(style.kind, PageKind::Index).then(|| "index")) {
                        a(class = "nav", href? = (!matches!(style.kind, PageKind::Index)).then(|| uri!(index).to_string())) {
                            div(class = "logo") {
                                @for chest in style.chests.0 {
                                    img(class = format!("chest chest-{}", char::from(chest.texture)), src = match chest.texture {
                                        ChestTexture::Normal => static_url!("chest/n.png"),
                                        ChestTexture::OldMajor => static_url!("chest/m.png"),
                                        ChestTexture::Major => static_url!("chest/i.png"),
                                        ChestTexture::SmallKeyOld => static_url!("chest/k.png"),
                                        ChestTexture::SmallKey1500 => static_url!("chest/y.png"),
                                        ChestTexture::SmallKey1751 => static_url!("chest/a.png"),
                                        ChestTexture::BossKey => static_url!("chest/b.png"),
                                        ChestTexture::Token => static_url!("chest/s.png"),
                                        ChestTexture::Invisible => static_url!("chest/d.png"),
                                        ChestTexture::Heart => static_url!("chest/h.png"),
                                        ChestTexture::Bombchu => static_url!("chest/c.png"),
                                    });
                                }
                            }
                            h1 : "Mido's House";
                        }
                        div(id = "login") {
                            @if !matches!(style.kind, PageKind::Login) {
                                @if let Some(me) = me {
                                    : "signed in as ";
                                    @if let PageKind::MyProfile = style.kind {
                                        bdi : me.display_name();
                                    } else {
                                        : me;
                                    }
                                    br;
                                    //TODO link to preferences
                                    a(href = uri!(auth::logout(Some(uri))).to_string()) : "Sign out";
                                } else {
                                    a(href = uri!(auth::login(Some(uri))).to_string()) : "Sign in / Create account";
                                }
                                @if !notifications.is_empty() {
                                    br;
                                }
                            }
                            @if !notifications.is_empty() {
                                a(href = uri!(notification::notifications).to_string()) {
                                    : notifications.len().to_string();
                                    @if notifications.len() == 1 {
                                        : " notification";
                                    } else {
                                        : " notifications";
                                    }
                                }
                            }
                        }
                    }
                    @if let Some(content) = content {
                        main(class? = matches!(style.kind, PageKind::Center).then(|| "center")) {
                            : content;
                        }
                    }
                }
                : banner_content;
                footer {
                    p {
                        : "hosted by ";
                        : fenhl;
                        : " • ";
                        a(href = "https://fenhl.net/disc") : "disclaimer";
                        : " • ";
                        a(href = "https://github.com/midoshouse/midos.house") : "source code";
                    }
                    p {
                        : "Special thanks to Maplestar for some of the chest icons used in the logo, and to ";
                        : xopar;
                        : " and shirosoluna for some of the seed hash icons!";
                    }
                }
            }
        }
    })
}

#[rocket::get("/")]
async fn index(discord_ctx: &State<RwFuture<DiscordCtx>>, pool: &State<PgPool>, http_client: &State<reqwest::Client>, me: Option<User>, uri: Origin<'_>) -> Result<RawHtml<String>, event::Error> {
    let mut transaction = pool.begin().await?;
    let mut upcoming_events = Vec::default();
    let mut races = Vec::default();
    for row in sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE listed AND (end_time IS NULL OR end_time > NOW()) ORDER BY start ASC NULLS LAST"#).fetch_all(&mut *transaction).await? {
        let event = event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during transaction");
        races.extend(Race::for_event(&mut transaction, http_client, &event).await?.into_iter().filter(|race| match race.schedule {
            RaceSchedule::Unscheduled => false,
            RaceSchedule::Live { end, .. } => end.is_none(),
            RaceSchedule::Async { start1, start2, end1, end2, .. } => start1.is_some() && start2.is_some() && (end1.is_none() || end2.is_none()), // second half scheduled and not ended
        }));
        upcoming_events.push(event);
    }
    races.sort_unstable_by(|race1, race2| {
        let start1 = match race1.schedule {
            RaceSchedule::Unscheduled => None,
            RaceSchedule::Live { start, .. } => Some(start),
            RaceSchedule::Async { start1, start2, .. } => start1.max(start2),
        };
        let start2 = match race2.schedule {
            RaceSchedule::Unscheduled => None,
            RaceSchedule::Live { start, .. } => Some(start),
            RaceSchedule::Async { start1, start2, .. } => start1.max(start2),
        };
        start1.cmp(&start2)
            .then_with(|| race1.series.to_str().cmp(race2.series.to_str()))
            .then_with(|| race1.event.cmp(&race2.event))
            .then_with(|| race1.phase.cmp(&race2.phase))
            .then_with(|| race1.round.cmp(&race2.round))
            .then_with(|| race1.source.cmp(&race2.source))
            .then_with(|| race1.game.cmp(&race2.game))
            .then_with(|| race1.id.cmp(&race2.id))
    });
    let chests_event = upcoming_events.choose(&mut thread_rng());
    let chests = if let Some(event) = chests_event { event.chests().await? } else { ChestAppearances::random() };
    let mut ongoing_events = Vec::default();
    for event in upcoming_events.drain(..).collect_vec() {
        if event.series != Series::Standard || event.event != "w" { // the weeklies are a perpetual event so we avoid always listing them
            if event.is_started(&mut transaction).await? { &mut ongoing_events } else { &mut upcoming_events }.push(event);
        }
    }
    let page_content = html! {
        p {
            : "Mido's House is a platform where ";
            a(href = "https://ootrandomizer.com/") : "Ocarina of Time randomizer";
            : " events like tournaments or community races can be organized. You may also be looking for the ";
            a(href = uri!(mw).to_string()) : "Mido's House Multiworld";
            : " app.";
        }
        div(class = "section-list") {
            div {
                h1 : "Ongoing events";
                ul {
                    @if ongoing_events.is_empty() {
                        i : "(none currently)";
                    } else {
                        @for event in ongoing_events {
                            li : event;
                        }
                    }
                }
            }
            div {
                h1 : "Upcoming events";
                ul {
                    @if upcoming_events.is_empty() {
                        i : "(none currently)";
                    } else {
                        @for event in upcoming_events {
                            li {
                                : event;
                                @if let Some(start) = event.start(&mut transaction).await? {
                                    : " — ";
                                    : format_datetime(start, DateTimeFormat { long: false, running_text: false });
                                }
                            }
                        }
                    }
                }
            }
        }
        p {
            a(href = uri!(archive).to_string()) : "Past events";
            : " • ";
            a(href = uri!(new_event).to_string()) : "Planning an event?";
        }
        h1 : "Ongoing/upcoming races";
        p {
            span(class = "timezone-wrapper") {
                : "Times shown in your timezone (detected as ";
                span(class = "timezone") : "[unknown]";
                : ") • ";
            }
            a(href = uri!(cal::index_help).to_string()) : "Add to calendar";
        }
        @if races.is_empty() {
            i : "(none currently)";
        } else {
            : cal::race_table(&mut transaction, &*discord_ctx.read().await, http_client, &uri, None, cal::RaceTableOptions { game_count: false, show_multistreams: true, can_create: false, can_edit: me.as_ref().is_some_and(|me| me.is_archivist), show_restream_consent: false, challonge_import_ctx: None }, &races).await?;
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { kind: PageKind::Index, chests, ..PageStyle::default() }, "Mido's House", page_content).await?)
}

#[rocket::get("/archive")]
async fn archive(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> Result<RawHtml<String>, event::Error> {
    let mut transaction = pool.begin().await?;
    let mut past_events = Vec::default();
    for row in sqlx::query!(r#"SELECT series AS "series: Series", event FROM events WHERE listed AND end_time IS NOT NULL AND end_time <= NOW() ORDER BY end_time DESC"#).fetch_all(&mut *transaction).await? {
        past_events.push(event::Data::new(&mut transaction, row.series, row.event).await?.expect("event deleted during transaction"));
    }
    let chests_event = past_events.choose(&mut thread_rng());
    let chests = if let Some(event) = chests_event { event.chests().await? } else { ChestAppearances::random() };
    let page_content = html! {
        h1 : "Past events";
        ul {
            @if past_events.is_empty() {
                i : "(none currently)";
            } else {
                @for event in past_events {
                    li {
                        : event;
                        : " — ";
                        : format_date_range(event.start(&mut transaction).await?.expect("ended event with no start date"), event.end.expect("checked above"));
                    };
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests, ..PageStyle::default() }, "Event Archive — Mido's House", page_content).await?)
}

#[rocket::get("/new")]
async fn new_event(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> PageResult {
    let mut transaction = pool.begin().await?;
    let fenhl = User::from_id(&mut *transaction, Id::from(14571800683221815449_u64)).await?.ok_or(PageError::FenhlUserData)?;
    page(transaction, &me, &uri, PageStyle::default(), "New Event — Mido's House", html! {
        p {
            : "If you are planning a tournament, community race, or other event for the Ocarina of Time randomizer community, or if you would like Mido's House to archive data about a past event you organized, please contact ";
            : fenhl;
            : " to determine the specific needs of the event.";
        }
    }).await
}

#[rocket::get("/mw")]
async fn mw(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> PageResult {
    let transaction = pool.begin().await?;
    page(transaction, &me, &uri, PageStyle { kind: PageKind::Center, ..PageStyle::default() }, "Mido's House Multiworld", html! {
        h1 : "Mido's House Multiworld";
        img(class = "banner icon", src = static_url!("mw.png"));
        p {
            : "Mido's House Multiworld is a tool that can be used to play ";
            a(href = "https://wiki.ootrandomizer.com/index.php?title=Multiworld") : "multiworld";
            : " seeds of the ";
            a(href = "https://ootrandomizer.com/") : "Ocarina of Time randomizer";
            : ". It supports cross-platform play between ";
            a(href = uri!(mw_platforms).to_string()) : "different platforms";
            : ", and does not require port forwarding.";
        }
        div(class = "button-row large-button-row") {
            a(class = "button", href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer.exe") {
                : "Download for Windows";
                br;
                small : "supports EverDrive*, BizHawk, and Project64";
            }
            a(class = "button", href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer-linux") {
                : "Download for Linux";
                br;
                small : "supports EverDrive* and BizHawk";
            }
        }
        p {
            : "*EverDrive support is currently experimental and requires ";
            a(href = "https://ootrandomizer.com/generatorDev?version=devFenhl_") : "Fenhl's branch of the randomizer";
            : ".";
        }
        p {
            : "If you need help, please ask in ";
            a(href = "https://discord.gg/BGRrKKn") : "#setup-support on the OoTR Discord";
            : " (feel free to ping @fenhl) or ";
            a(href = "https://github.com/midoshouse/ootr-multiworld/issues/new") : "open an issue";
            : ".";
        }
        p {
            a(href = "https://github.com/midoshouse/ootr-multiworld") : "The source code for Mido's House Multiworld";
            : " is available on GitHub.";
        }
    }).await
}

#[rocket::get("/mw/platforms")]
async fn mw_platforms(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>) -> PageResult {
    let transaction = pool.begin().await?;
    page(transaction, &me, &uri, PageStyle { kind: PageKind::Center, ..PageStyle::default() }, "platform support — Mido's House Multiworld", html! {
        h1 {
            a(href = uri!(mw).to_string()) : "Mido's House Multiworld";
            : " platform support status";
        }
        table {
            tr {
                th;
                th : "Windows";
                th : "Linux";
                th : "macOS";
            }
            tr {
                th : "EverDrive";
                td {
                    : "✓ (";
                    a(href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer.exe") : "download";
                    : ") *";
                }
                td {
                    : "✓ (";
                    a(href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer-linux") : "download";
                    : ") *";
                }
                td {
                    a(href = "https://github.com/midoshouse/ootr-multiworld/issues/41") : "Planned";
                }
            }
            tr {
                th : "Wii Virtual Console";
                td(colspan = "3") : "Would require a modification to Virtual Console itself. The “Multiworld 2.0” project claims to have solved this issue but has not shared any details out of concerns for competitive integrity.";
            }
            tr {
                th : "BizHawk";
                td {
                    : "✓ (";
                    a(href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer.exe") : "download";
                    : ")";
                }
                td {
                    : "✓ (";
                    a(href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer-linux") : "download";
                    : ")";
                }
                td {
                    a(href = "https://github.com/tasemulators/bizHawk#macos-legacy-bizhawk") : "Not supported by BizHawk itself";
                }
            }
            tr {
                th : "Project64";
                td {
                    : "✓ (";
                    a(href = "https://github.com/midoshouse/ootr-multiworld/releases/latest/download/multiworld-installer.exe") : "download";
                    : ")";
                }
                td(colspan = "2") : "Not supported by Project64 itself";
            }
            tr {
                th : "Project64-EM";
                td(colspan = "3") {
                    : "Not planned. Project64-EM is a modified version of Project64 created by the ";
                    a(href = "https://ootmm.com/") : "OoTMM";
                    : " community which removes the plugin system used by Mido's House Multiworld and replaces it with a different one. Note that Mido's House Multiworld does not support OoTMM — please follow ";
                    a(href = "https://ootmm.com/multiworld") : "the OoTMM multiworld setup guide";
                    : " instead.";
                }
            }
            tr {
                th : "RetroArch";
                td(colspan = "3") {
                    a(href = "https://github.com/midoshouse/ootr-multiworld/issues/25") : "Planned";
                }
            }
        }
        p {
            : "*EverDrive support is currently experimental and requires ";
            a(href = "https://ootrandomizer.com/generatorDev?version=devFenhl_") : "Fenhl's branch of the randomizer";
            : ".";
        }
        p {
            : "If your operating system, console, or emulator is not listed here, please ";
            a(href = "https://github.com/midoshouse/ootr-multiworld/issues/new") : "open an issue";
            : " to request support.";
        }
    }).await
}

#[rocket::get("/robots.txt")]
async fn robots_txt() -> RawText<&'static str> {
    RawText("User-agent: *\nDisallow: /seed/\nDisallow: /static/\n")
}

#[rocket::catch(400)]
async fn bad_request(request: &Request<'_>) -> PageResult {
    eprintln!("responding with 400 Bad Request to request {request:?}");
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    let uri = request.guard::<Origin<'_>>().await.succeeded().unwrap_or_else(|| Origin(uri!(index)));
    page(pool.begin().await?, &me, &uri, PageStyle { chests: ChestAppearances::SMALL_KEYS, ..PageStyle::default() }, "Bad Request — Mido's House", html! {
        h1 : "Error 400: Bad Request";
        p : "Login failed. If you need help, contact Fenhl on Discord.";
    }).await
}

#[rocket::catch(404)]
async fn not_found(request: &Request<'_>) -> PageResult {
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    let uri = request.guard::<Origin<'_>>().await.succeeded().unwrap_or_else(|| Origin(uri!(index)));
    page(pool.begin().await?, &me, &uri, PageStyle { kind: PageKind::Banner, chests: ChestAppearances::INVISIBLE, ..PageStyle::default() }, "Not Found — Mido's House", html! {
        div(style = "flex-grow: 0;") {
            h1 : "Error 404: Not Found";
        }
        img(style = "flex-grow: 1;", class = "banner nearest-neighbor", src = "https://i.imgur.com/i4lJkiq.png");
    }).await
}

#[rocket::catch(500)]
async fn internal_server_error(request: &Request<'_>) -> PageResult {
    if let Environment::Production = Environment::default() {
        wheel::night_report(&format!("{}/error", night_path()), Some("internal server error")).await?;
    }
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    let uri = request.guard::<Origin<'_>>().await.succeeded().unwrap_or_else(|| Origin(uri!(index)));
    page(pool.begin().await?, &me, &uri, PageStyle { chests: ChestAppearances::TOKENS, ..PageStyle::default() }, "Internal Server Error — Mido's House", html! {
        h1 : "Error 500: Internal Server Error";
        p : "Sorry, something went wrong. Please notify Fenhl on Discord.";
    }).await
}

#[rocket::catch(default)]
async fn fallback_catcher(status: Status, request: &Request<'_>) -> PageResult {
    eprintln!("responding with unexpected HTTP status code {} {} to request {request:?}", status.code, status.reason_lossy());
    if let Environment::Production = Environment::default() {
        wheel::night_report(&format!("{}/error", night_path()), Some(&format!("responding with unexpected HTTP status code: {} {}", status.code, status.reason_lossy()))).await?;
    }
    let pool = request.guard::<&State<PgPool>>().await.expect("missing database pool");
    let me = request.guard::<User>().await.succeeded();
    let uri = request.guard::<Origin<'_>>().await.succeeded().unwrap_or_else(|| Origin(uri!(index)));
    page(pool.begin().await?, &me, &uri, PageStyle { chests: ChestAppearances::TOKENS, ..PageStyle::default() }, &format!("{} — Mido's House", status.reason_lossy()), html! {
        h1 {
            : "Error ";
            : status.code;
            : ": ";
            : status.reason_lossy();
        }
        p : "Sorry, something went wrong. Please notify Fenhl on Discord.";
    }).await
}

pub(crate) async fn rocket(pool: PgPool, discord_ctx: RwFuture<DiscordCtx>, http_client: reqwest::Client, config: Config, port: u16, seed_metadata: Arc<RwLock<HashMap<String, SeedMetadata>>>) -> Result<Rocket<rocket::Ignite>, crate::Error> {
    let discord_config = if Environment::default().is_dev() { &config.discord_dev } else { &config.discord_production };
    let racetime_config = if Environment::default().is_dev() { &config.racetime_oauth_dev } else { &config.racetime_oauth_production };
    Ok(rocket::custom(rocket::Config::figment().merge(rocket::Config {
        secret_key: SecretKey::from(&BASE64.decode(&config.secret_key)?),
        log_level: Some(rocket::config::Level::ERROR),
        ..rocket::Config::default()
    }).merge(("port", port))) //TODO report issue for lack of typed interface to set port, see https://github.com/rwf2/Rocket/commit/fd294049c784cb52680a423616fadc29d57fa25b
    .mount("/", rocket::routes![
        index,
        archive,
        new_event,
        mw,
        mw_platforms,
        robots_txt,
        api::graphql_request,
        api::graphql_query,
        api::graphql_playground,
        api::entrants_csv,
        auth::racetime_callback,
        auth::discord_callback,
        auth::challonge_callback,
        auth::startgg_callback,
        auth::login,
        auth::logout,
        auth::racetime_login,
        auth::discord_login,
        auth::challonge_login,
        auth::startgg_login,
        auth::register_racetime,
        auth::register_discord,
        auth::merge_accounts,
        cal::index_help,
        cal::index,
        cal::for_series,
        cal::for_event,
        cal::create_race,
        cal::create_race_post,
        cal::import_races,
        cal::import_races_post,
        cal::edit_race,
        cal::edit_race_post,
        cal::add_file_hash,
        cal::add_file_hash_post,
        event::info,
        event::races,
        event::status,
        event::status_post,
        event::find_team,
        event::find_team_post,
        event::confirm_signup,
        event::resign,
        event::resign_post,
        event::opt_out,
        event::opt_out_post,
        event::request_async,
        event::submit_async,
        event::enter::get,
        event::enter::post,
        event::teams::get,
        event::volunteer,
        event::configure::get,
        event::configure::post,
        favicon::favicon_ico,
        favicon::favicon_png,
        notification::notifications,
        notification::dismiss,
        seed::get,
        user::profile,
    ])
    .mount("/static", FileServer::without_index("assets/static"))
    .register("/", rocket::catchers![
        bad_request,
        not_found,
        internal_server_error,
        fallback_catcher,
    ])
    .attach(rocket_csrf::Fairing::default())
    .attach(OAuth2::<auth::RaceTime>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider {
            auth_uri: format!("https://{}/o/authorize", racetime_host()).into(),
            token_uri: format!("https://{}/o/token", racetime_host()).into(),
        },
        racetime_config.client_id.clone(),
        racetime_config.client_secret.clone(),
        Some(match Environment::default() {
            Environment::Local => uri!("http://localhost:24814", auth::racetime_callback),
            Environment::Dev => uri!("https://dev.midos.house", auth::racetime_callback),
            Environment::Production => uri!("https://midos.house", auth::racetime_callback),
        }.to_string()),
    )))
    .attach(OAuth2::<auth::Discord>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider::Discord,
        discord_config.client_id.to_string(),
        discord_config.client_secret.to_string(),
        Some(match Environment::default() {
            Environment::Local => uri!("http://localhost:24814", auth::discord_callback),
            Environment::Dev => uri!("https://dev.midos.house", auth::discord_callback),
            Environment::Production => uri!("https://midos.house", auth::discord_callback),
        }.to_string()),
    )))
    .attach(OAuth2::<auth::Challonge>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider {
            auth_uri: "https://api.challonge.com/oauth/authorize".into(),
            token_uri: "https://api.challonge.com/oauth/token".into(),
        },
        config.challonge.client_id.to_string(),
        config.challonge.client_secret.to_string(),
        Some(match Environment::default() {
            Environment::Local => uri!("http://localhost:24814", auth::challonge_callback),
            Environment::Dev => uri!("https://dev.midos.house", auth::challonge_callback),
            Environment::Production => uri!("https://midos.house", auth::challonge_callback),
        }.to_string()),
    )))
    .attach(OAuth2::<auth::StartGG>::custom(rocket_oauth2::HyperRustlsAdapter::default(), OAuthConfig::new(
        rocket_oauth2::StaticProvider {
            auth_uri: "https://start.gg/oauth/authorize".into(),
            token_uri: "https://api.start.gg/oauth/access_token".into(),
        },
        config.startgg_oauth.client_id.to_string(),
        config.startgg_oauth.client_secret.to_string(),
        Some(match Environment::default() {
            Environment::Local => uri!("http://localhost:24814", auth::startgg_callback),
            Environment::Dev => uri!("https://dev.midos.house", auth::startgg_callback),
            Environment::Production => uri!("https://midos.house", auth::startgg_callback),
        }.to_string()),
    )))
    .manage(config)
    .manage(pool.clone())
    .manage(discord_ctx)
    .manage(http_client)
    .manage(api::schema(pool))
    .manage(seed_metadata)
    .ignite().await?)
}
