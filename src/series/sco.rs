use crate::prelude::*;

#[derive(Sequence)]
pub(crate) enum Format {
    League,
    Sgl,
    Saws,
    Bingo,
    Ice,
    Mixed,
    Franco,
    Triforce,
}

impl Format {
    pub(crate) fn for_race(race: &Race) -> Option<Self> {
        if let Series::SlugOpen = race.series {
            race.draft.as_ref().and_then(|draft| draft.settings.get("format")).map(|s| s.parse().expect("unexpected SlugCentral Open format"))
        } else {
            None
        }
    }

    pub(crate) fn default_race_duration(&self) -> TimeDelta {
        match self {
            Self::Ice => TimeDelta::minutes(30),
            Self::Sgl | Self::Bingo /*TODO verify */ | Self::Mixed => TimeDelta::hours(3),
            Self::League | Self::Saws | Self::Franco | Self::Triforce /*TODO verify */ => TimeDelta::hours(3) + TimeDelta::minutes(30),
        }
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "league" => Ok(Self::League),
            "sgl" => Ok(Self::Sgl),
            "saws" => Ok(Self::Saws),
            "bingo" => Ok(Self::Bingo),
            "ice" => Ok(Self::Ice),
            "mixed" => Ok(Self::Mixed),
            "franco" => Ok(Self::Franco),
            "triforce" => Ok(Self::Triforce),
            _ => Err(s.to_owned()),
        }
    }
}
