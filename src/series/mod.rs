use {
    anyhow::anyhow,
    rocket::http::{
        impl_from_uri_param_identity,
        uri::{
            self,
            fmt::{
                Path,
                UriDisplay,
            },
        },
    },
    sqlx::{
        Decode,
        Encode,
        postgres::{
            PgArgumentBuffer,
            PgTypeInfo,
            PgValueRef,
        },
    },
    crate::prelude::*,
};

pub(crate) mod br;
pub(crate) mod fr;
pub(crate) mod league;
pub(crate) mod mp;
pub(crate) mod mw;
pub(crate) mod ndos;
pub(crate) mod pic;
pub(crate) mod rsl;
pub(crate) mod s;
pub(crate) mod scrubs;
pub(crate) mod sgl;
pub(crate) mod tfb;
pub(crate) mod wttbb;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Sequence)]
pub(crate) enum Series {
    CopaDoBrasil,
    League,
    MixedPools,
    Multiworld,
    NineDaysOfSaws,
    Pictionary,
    Rsl,
    Scrubs,
    SpeedGaming,
    Standard,
    TournoiFrancophone,
    TriforceBlitz,
    WeTryToBeBetter,
}

impl Series {
    pub(crate) fn to_str(&self) -> &'static str {
        match self {
            Self::CopaDoBrasil => "br",
            Self::League => "league",
            Self::MixedPools => "mp",
            Self::Multiworld => "mw",
            Self::NineDaysOfSaws => "9dos",
            Self::Pictionary => "pic",
            Self::Rsl => "rsl",
            Self::Scrubs => "scrubs",
            Self::SpeedGaming => "sgl",
            Self::Standard => "s",
            Self::TournoiFrancophone => "fr",
            Self::TriforceBlitz => "tfb",
            Self::WeTryToBeBetter => "wttbb",
        }
    }
}

impl FromStr for Series {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        all::<Self>().find(|series| series.to_str() == s).ok_or(())
    }
}

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.to_str(), f)
    }
}

impl<'r> Decode<'r, Postgres> for Series {
    fn decode(value: PgValueRef<'r>) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let series = <&str as Decode<Postgres>>::decode(value)?;
        series.parse().map_err(|()| anyhow!("unknown series: {series}").into())
    }
}

impl<'q> Encode<'q, Postgres> for Series {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> sqlx::encode::IsNull {
        Encode::<Postgres>::encode_by_ref(&self.to_str(), buf)
    }

    fn encode(self, buf: &mut PgArgumentBuffer) -> sqlx::encode::IsNull {
        Encode::<Postgres>::encode(self.to_str(), buf)
    }

    fn produces(&self) -> Option<PgTypeInfo> {
        Encode::<Postgres>::produces(&self.to_str())
    }

    fn size_hint(&self) -> usize {
        Encode::<Postgres>::size_hint(&self.to_str())
    }
}

impl sqlx::Type<Postgres> for Series {
    fn type_info() -> PgTypeInfo {
        <&str as sqlx::Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str as sqlx::Type<Postgres>>::compatible(ty)
    }
}

impl<'a> FromParam<'a> for Series {
    type Error = &'a str;

    fn from_param(param: &'a str) -> Result<Self, Self::Error> {
        param.parse().map_err(|()| param)
    }
}

impl UriDisplay<Path> for Series {
    fn fmt(&self, f: &mut uri::fmt::Formatter<'_, Path>) -> fmt::Result {
        UriDisplay::fmt(self.to_str(), f) // assume all series names are URI safe
    }
}

impl_from_uri_param_identity!([Path] Series);
