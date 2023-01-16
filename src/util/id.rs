use {
    std::str::FromStr,
    rand::prelude::*,
    rocket::{
        UriDisplayPath,
        UriDisplayQuery,
        form::{
            self,
            FromFormField,
        },
        request::FromParam,
    },
    serde::{
        Deserialize,
        Serialize,
    },
    sqlx::{
        Database,
        Decode,
        Encode,
        Postgres,
        Transaction,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize, UriDisplayPath, UriDisplayQuery)]
#[serde(transparent)]
pub(crate) struct Id(pub(crate) u64);

pub(crate) enum IdTable {
    Notifications,
    Races,
    Teams,
    Users,
}

impl Id {
    pub(crate) async fn new(transaction: &mut Transaction<'_, Postgres>, table: IdTable) -> sqlx::Result<Self> {
        Ok(loop {
            let id = Self(thread_rng().gen());
            let query = match table {
                IdTable::Notifications => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM notifications WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Races => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM races WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Teams => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Users => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, i64::from(id)),
            };
            if !query.fetch_one(&mut *transaction).await? { break id }
        })
    }
}

impl From<u64> for Id {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<i64> for Id {
    fn from(id: i64) -> Self {
        Self(id as u64)
    }
}

impl From<Id> for i64 {
    fn from(Id(id): Id) -> Self {
        id as Self
    }
}

impl FromStr for Id {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .or_else(|_| s.parse::<i64>().map(Self::from))
    }
}

impl<'r, DB: Database> Decode<'r, DB> for Id
where i64: Decode<'r, DB> {
    fn decode(value: <DB as sqlx::database::HasValueRef<'r>>::ValueRef) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        i64::decode(value).map(|id| Self(id as u64))
    }
}

impl<'q, DB: Database> Encode<'q, DB> for Id
where i64: Encode<'q, DB> {
    fn encode_by_ref(&self, buf: &mut <DB as sqlx::database::HasArguments<'q>>::ArgumentBuffer) -> sqlx::encode::IsNull {
        (self.0 as i64).encode(buf)
    }

    fn encode(self, buf: &mut <DB as sqlx::database::HasArguments<'q>>::ArgumentBuffer) -> sqlx::encode::IsNull {
        (self.0 as i64).encode(buf)
    }

    fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
        (self.0 as i64).produces()
    }

    fn size_hint(&self) -> usize {
        Encode::size_hint(&(self.0 as i64))
    }
}

impl<DB: Database> sqlx::Type<DB> for Id
where i64: sqlx::Type<DB> {
    fn type_info() -> <DB as Database>::TypeInfo {
        i64::type_info()
    }

    fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
        i64::compatible(ty)
    }
}

impl<'a> FromParam<'a> for Id {
    type Error = &'a str;

    fn from_param(param: &'a str) -> Result<Self, Self::Error> {
        u64::from_param(param)
            .map(Self)
            .or_else(|_| i64::from_param(param).map(Self::from))
    }
}

impl<'v> FromFormField<'v> for Id
where i64: FromFormField<'v>, u64: FromFormField<'v> {
    fn from_value(field: form::ValueField<'v>) -> form::Result<'v, Self> {
        u64::from_value(field.clone())
            .map(Self)
            .or_else(|_| i64::from_value(field).map(Self::from))
    }

    fn default() -> Option<Self> { None }
}
