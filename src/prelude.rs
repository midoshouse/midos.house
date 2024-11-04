pub(crate) use {
    std::{
        borrow::Cow,
        cmp::Ordering::{
            self,
            *,
        },
        collections::{
            HashSet,
            hash_map::{
                self,
                HashMap,
            },
        },
        convert::identity,
        fmt,
        hash::Hash,
        iter,
        marker::PhantomData,
        mem,
        num::NonZeroU8,
        path::{
            Path,
            PathBuf,
        },
        pin::{
            Pin,
            pin,
        },
        str::FromStr,
        sync::{
            Arc,
            LazyLock,
        },
        time::Duration,
    },
    async_trait::async_trait,
    chrono::{
        TimeDelta,
        prelude::*,
    },
    chrono_tz::{
        America,
        Europe,
        Tz,
    },
    collect_mac::collect,
    either::Either,
    enum_iterator::{
        Sequence,
        all,
    },
    futures::{
        future::{
            self,
            Future,
            FutureExt as _,
        },
        stream::{
            self,
            StreamExt as _,
            TryStreamExt as _,
        },
    },
    if_chain::if_chain,
    itertools::Itertools as _,
    lazy_regex::{
        regex_captures,
        regex_is_match,
    },
    log_lock::*,
    ootr_utils::{
        camc::ChestTexture,
        spoiler::{
            HashIcon,
            OcarinaNote,
            SpoilerLog,
        },
    },
    racetime::model::RaceData,
    rand::prelude::*,
    rocket::{
        FromForm,
        FromFormField,
        Responder,
        State,
        form::{
            self,
            Context,
            Contextual,
            Form,
        },
        http::{
            Status,
            ext::IntoOwned as _,
        },
        request::{
            self,
            FromParam,
            FromRequest,
            Request,
        },
        response::{
            Redirect,
            content::RawHtml,
        },
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        ContextualExt as _,
        CsrfForm,
        Origin,
        Suffix,
        ToHtml,
        html,
    },
    serde::{
        Deserialize,
        Deserializer,
        Serialize,
        de::Error as _,
    },
    serde_json::json,
    serde_with::serde_as,
    serenity::{
        all::{
            Context as DiscordCtx,
            MessageBuilder,
        },
        model::prelude::*,
        utils::EmbedMessageBuilding as _,
    },
    serenity_utils::{
        RwFuture,
        message::MessageBuilderExt as _,
    },
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
    tokio::{
        io,
        process::Command,
        select,
        sync::{
            mpsc,
            watch,
        },
        time::{
            Instant,
            sleep,
            sleep_until,
        },
    },
    tokio_util::io::StreamReader,
    typemap_rev::TypeMapKey,
    url::Url,
    uuid::Uuid,
    wheel::{
        fs::{
            self,
            File,
        },
        io_error_from_reqwest,
        traits::{
            IoResultExt as _,
            IsNetworkError,
            LocalResultExt as _,
            ReqwestResponseExt as _,
        },
    },
    crate::{
        Environment,
        auth,
        cal::{
            self,
            Entrant,
            Entrants,
            Race,
            RaceSchedule,
        },
        challonge,
        config::Config,
        discord_bot::{
            CommandIds,
            MessageBuilderExt as _,
            PgSnowflake,
        },
        draft::{
            self,
            Draft,
        },
        event::{
            self,
            AsyncKind,
            MatchSource,
            TeamConfig,
        },
        favicon::{
            self,
            ChestAppearances,
            ChestTextures,
        },
        form::*,
        http::{
            PageError,
            PageKind,
            PageResult,
            PageStyle,
            RedirectOrContent,
            StatusOrError,
            favicon,
            page,
            static_url,
        },
        id::{
            Id,
            Notifications,
            Races,
            Teams,
            Users,
        },
        lang::Language::{
            self,
            *,
        },
        macros::*,
        night_path,
        ootr_web,
        racetime_bot,
        racetime_host,
        seed,
        series::*,
        startgg,
        team::{
            self,
            Team,
        },
        time::*,
        user::{
            self,
            User,
        },
    },
};
#[cfg(unix)] pub(crate) use {
    async_proto::Protocol,
    xdg::BaseDirectories,
};
