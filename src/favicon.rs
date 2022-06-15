use {
    std::{
        fmt::{
            self,
            Write as _,
        },
        io,
    },
    derive_more::From,
    image::{
        GenericImage as _,
        RgbaImage,
        io::Reader as ImageReader,
    },
    itertools::Itertools as _,
    once_cell::sync::Lazy,
    rand::prelude::*,
    rocket::{
        fs::NamedFile,
        http::{
            impl_from_uri_param_identity,
            uri::{
                self,
                fmt::{
                    Path,
                    UriDisplay,
                },
            },
        },
        request::FromParam,
    },
    rocket_util::{
        Response,
        Suffix,
    },
    serde::Deserialize,
    crate::seed::SpoilerLog,
};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Bridge {
    Open,
    Vanilla,
    Stones,
    Medallions,
    Dungeons,
    Tokens,
    Hearts,
}

impl Default for Bridge {
    fn default() -> Self {
        Self::Medallions
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LacsCondition {
    Vanilla,
    Stones,
    Medallions,
    Dungeons,
    Tokens,
    Hearts,
}

impl Default for LacsCondition {
    fn default() -> Self {
        Self::Vanilla
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShuffleGanonBosskey {
    Remove,
    Vanilla,
    Dungeon,
    Overworld,
    AnyDungeon,
    Keysanity,
    OnLacs,
    Stones,
    Medallions,
    Dungeons,
    Tokens,
    Hearts,
}

impl Default for ShuffleGanonBosskey {
    fn default() -> Self {
        Self::Dungeon
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CorrectChestAppearances {
    Off,
    Classic,
    Textures,
    #[serde(alias = "sizes")]
    Both,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JsonItem {
    Simple(String),
    Complex {
        item: String,
        model: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(from = "JsonItem")]
struct Item {
    item: String,
    model: Option<String>,
}

impl From<JsonItem> for Item {
    fn from(item: JsonItem) -> Self {
        match item {
            JsonItem::Simple(item) => Self { item, model: None },
            JsonItem::Complex { item, model } => Self { item, model },
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct SpoilerLogLocations {
    #[serde(rename = "KF Midos Top Left Chest")] kf_midos_top_left_chest: Item,
    #[serde(rename = "KF Midos Top Right Chest")] kf_midos_top_right_chest: Item,
    #[serde(rename = "KF Midos Bottom Left Chest")] kf_midos_bottom_left_chest: Item,
    #[serde(rename = "KF Midos Bottom Right Chest")] kf_midos_bottom_right_chest: Item,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ChestTexture {
    Normal,
    OldMajor,
    Major,
    OldSmallKey,
    SmallKey,
    BossKey,
    Token,
}

impl TryFrom<char> for ChestTexture {
    type Error = char;

    fn try_from(c: char) -> Result<Self, char> {
        match c {
            'n' => Ok(Self::Normal),
            'm' => Ok(Self::OldMajor),
            'i' => Ok(Self::Major),
            'k' => Ok(Self::OldSmallKey),
            'y' => Ok(Self::SmallKey),
            'b' => Ok(Self::BossKey),
            's' => Ok(Self::Token),
            _ => Err(c),
        }
    }
}

impl From<ChestTexture> for char {
    fn from(texture: ChestTexture) -> Self {
        match texture {
            ChestTexture::Normal => 'n',
            ChestTexture::OldMajor => 'm',
            ChestTexture::Major => 'i',
            ChestTexture::OldSmallKey => 'k',
            ChestTexture::SmallKey => 'y',
            ChestTexture::BossKey => 'b',
            ChestTexture::Token => 's',
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
pub(crate) struct ChestAppearance {
    pub(crate) texture: ChestTexture,
    #[allow(unused)] big: bool,
}

impl ChestAppearance {
    const VANILLA: Self = Self {
        texture: ChestTexture::Normal,
        big: false,
    };

    fn from_item(camc_kind: CorrectChestAppearances, chus_in_logic: bool, token_wincon: bool, heart_wincon: bool, item: &Item) -> Self {
        if let CorrectChestAppearances::Off = camc_kind { return Self::VANILLA }
        let item_name = if item.item == "Ice Trap" {
            item.model.as_deref().expect("ice trap without model in CSMC")
        } else {
            &item.item
        };
        match item_name {
            "Bow" |
            "Slingshot" |
            "Boomerang" |
            "Progressive Hookshot" |
            "Lens of Truth" |
            "Zeldas Letter" |
            "Ocarina" |
            "Megaton Hammer" |
            "Cojiro" |
            "Bottle" |
            "Bottle with Red Potion" |
            "Bottle with Green Potion" |
            "Bottle with Blue Potion" |
            "Bottle with Fairy" |
            "Bottle with Milk" |
            "Rutos Letter" |
            "Skull Mask" |
            "Spooky Mask" |
            "Keaton Mask" |
            "Bunny Hood" |
            "Mask of Truth" |
            "Pocket Egg" |
            "Pocket Cucco" |
            "Odd Mushroom" |
            "Odd Potion" |
            "Poachers Saw" |
            "Broken Sword" |
            "Prescription" |
            "Eyeball Frog" |
            "Eyedrops" |
            "Claim Check" |
            "Kokiri Sword" |
            "Giants Knife" |
            "Mirror Shield" |
            "Goron Tunic" |
            "Zora Tunic" |
            "Iron Boots" |
            "Hover Boots" |
            "Bomb Bag" |
            "Progressive Strength Upgrade" |
            "Progressive Scale" |
            "Stone of Agony" |
            "Gerudo Membership Card" |
            "Progressive Wallet" |
            "Weird Egg" |
            "Goron Mask" |
            "Zora Mask" |
            "Gerudo Mask" |
            "Biggoron Sword" |
            "Fire Arrows" |
            "Ice Arrows" |
            "Light Arrows" |
            "Dins Fire" |
            "Farores Wind" |
            "Nayrus Love" |
            "Bombchus" |
            "Magic Meter" |
            "Bottle with Fish" |
            "Bottle with Blue Fire" |
            "Bottle with Bugs" |
            "Bottle with Big Poe" |
            "Bottle with Poe" |
            "Double Defense" |
            "Minuet of Forest" |
            "Bolero of Fire" |
            "Serenade of Water" |
            "Requiem of Spirit" |
            "Nocturne of Shadow" |
            "Prelude of Light" |
            "Zeldas Lullaby" |
            "Eponas Song" |
            "Sarias Song" |
            "Suns Song" |
            "Song of Time" |
            "Song of Storms" |
            "Magic Bean Pack" |
            "Triforce Piece" |
            "Easter Egg" |
            "Easter Egg (Pink)" |
            "Easter Egg (Orange)" |
            "Easter Egg (Green)" |
            "Easter Egg (Blue)" => match camc_kind {
                CorrectChestAppearances::Off => unreachable!(),
                CorrectChestAppearances::Classic => ChestAppearance { texture: ChestTexture::Normal, big: true },
                CorrectChestAppearances::Textures => ChestAppearance { texture: ChestTexture::Major, big: false },
                CorrectChestAppearances::Both => ChestAppearance { texture: ChestTexture::Major, big: true },
            },
            "Boss Key (Forest Temple)" |
            "Boss Key (Fire Temple)" |
            "Boss Key (Water Temple)" |
            "Boss Key (Spirit Temple)" |
            "Boss Key (Shadow Temple)" |
            "Boss Key (Ganons Castle)" => match camc_kind {
                CorrectChestAppearances::Off => unreachable!(),
                CorrectChestAppearances::Classic => ChestAppearance { texture: ChestTexture::BossKey, big: true },
                CorrectChestAppearances::Textures => ChestAppearance { texture: ChestTexture::BossKey, big: false },
                CorrectChestAppearances::Both => ChestAppearance { texture: ChestTexture::BossKey, big: true },
            },
            "Small Key (Forest Temple)" |
            "Small Key (Fire Temple)" |
            "Small Key (Water Temple)" |
            "Small Key (Spirit Temple)" |
            "Small Key (Shadow Temple)" |
            "Small Key (Bottom of the Well)" |
            "Small Key (Gerudo Training Ground)" |
            "Small Key (Thieves Hideout)" |
            "Small Key (Ganons Castle)" |
            "Small Key Ring (Forest Temple)" |
            "Small Key Ring (Fire Temple)" |
            "Small Key Ring (Water Temple)" |
            "Small Key Ring (Spirit Temple)" |
            "Small Key Ring (Shadow Temple)" |
            "Small Key Ring (Bottom of the Well)" |
            "Small Key Ring (Gerudo Training Ground)" |
            "Small Key Ring (Thieves Hideout)" |
            "Small Key Ring (Ganons Castle)" |
            "Silver Rupee (Dodongos Cavern Staircase)" |
            "Silver Rupee (Ice Cavern Spinning Scythe)" |
            "Silver Rupee (Ice Cavern Push Block)" |
            "Silver Rupee (Bottom of the Well Basement)" |
            "Silver Rupee (Shadow Temple Scythe Shortcut)" |
            "Silver Rupee (Shadow Temple Invisible Blades)" |
            "Silver Rupee (Shadow Temple Huge Pit)" |
            "Silver Rupee (Shadow Temple Invisible Spikes)" |
            "Silver Rupee (Gerudo Training Ground Slopes)" |
            "Silver Rupee (Gerudo Training Ground Lava)" |
            "Silver Rupee (Gerudo Training Ground Water)" |
            "Silver Rupee (Spirit Temple Child Early Torches)" |
            "Silver Rupee (Spirit Temple Adult Boulders)" |
            "Silver Rupee (Spirit Temple Lobby and Lower Adult)" |
            "Silver Rupee (Spirit Temple Sun Block)" |
            "Silver Rupee (Spirit Temple Adult Climb)" |
            "Silver Rupee (Ganons Castle Spirit Trial)" |
            "Silver Rupee (Ganons Castle Light Trial)" |
            "Silver Rupee (Ganons Castle Fire Trial)" |
            "Silver Rupee (Ganons Castle Shadow Trial)" |
            "Silver Rupee (Ganons Castle Water Trial)" |
            "Silver Rupee (Ganons Castle Forest Trial)" |
            "Silver Rupee Pouch (Dodongos Cavern Staircase)" |
            "Silver Rupee Pouch (Ice Cavern Spinning Scythe)" |
            "Silver Rupee Pouch (Ice Cavern Push Block)" |
            "Silver Rupee Pouch (Bottom of the Well Basement)" |
            "Silver Rupee Pouch (Shadow Temple Scythe Shortcut)" |
            "Silver Rupee Pouch (Shadow Temple Invisible Blades)" |
            "Silver Rupee Pouch (Shadow Temple Huge Pit)" |
            "Silver Rupee Pouch (Shadow Temple Invisible Spikes)" |
            "Silver Rupee Pouch (Gerudo Training Ground Slopes)" |
            "Silver Rupee Pouch (Gerudo Training Ground Lava)" |
            "Silver Rupee Pouch (Gerudo Training Ground Water)" |
            "Silver Rupee Pouch (Spirit Temple Child Early Torches)" |
            "Silver Rupee Pouch (Spirit Temple Adult Boulders)" |
            "Silver Rupee Pouch (Spirit Temple Lobby and Lower Adult)" |
            "Silver Rupee Pouch (Spirit Temple Sun Block)" |
            "Silver Rupee Pouch (Spirit Temple Adult Climb)" |
            "Silver Rupee Pouch (Ganons Castle Spirit Trial)" |
            "Silver Rupee Pouch (Ganons Castle Light Trial)" |
            "Silver Rupee Pouch (Ganons Castle Fire Trial)" |
            "Silver Rupee Pouch (Ganons Castle Shadow Trial)" |
            "Silver Rupee Pouch (Ganons Castle Water Trial)" |
            "Silver Rupee Pouch (Ganons Castle Forest Trial)" => match camc_kind {
                CorrectChestAppearances::Off => unreachable!(),
                CorrectChestAppearances::Classic => ChestAppearance { texture: ChestTexture::BossKey, big: false },
                CorrectChestAppearances::Textures => ChestAppearance { texture: ChestTexture::SmallKey, big: false },
                CorrectChestAppearances::Both => ChestAppearance { texture: ChestTexture::SmallKey, big: false },
            },
            "Ice Trap" => unreachable!(),
            "Bombchus (5)" |
            "Bombchus (10)" |
            "Bombchus (20)" => match camc_kind {
                CorrectChestAppearances::Off => unreachable!(),
                CorrectChestAppearances::Classic => ChestAppearance { texture: ChestTexture::Normal, big: chus_in_logic },
                CorrectChestAppearances::Textures => ChestAppearance { texture: if chus_in_logic { ChestTexture::Major } else { ChestTexture::Normal }, big: false },
                CorrectChestAppearances::Both => ChestAppearance { texture: if chus_in_logic { ChestTexture::Major } else { ChestTexture::Normal }, big: chus_in_logic },
            },
            "Gold Skulltula Token" => match camc_kind {
                CorrectChestAppearances::Off => unreachable!(),
                CorrectChestAppearances::Classic => ChestAppearance { texture: ChestTexture::Normal, big: token_wincon },
                CorrectChestAppearances::Textures => ChestAppearance { texture: ChestTexture::Token, big: false },
                CorrectChestAppearances::Both => ChestAppearance { texture: ChestTexture::Token, big: token_wincon },
            },
            "Heart Container" |
            "Piece of Heart" |
            "Piece of Heart (Treasure Chest Game)" => match camc_kind {
                CorrectChestAppearances::Off => unreachable!(),
                CorrectChestAppearances::Classic => ChestAppearance { texture: ChestTexture::Normal, big: heart_wincon },
                CorrectChestAppearances::Textures => ChestAppearance { texture: if heart_wincon { ChestTexture::Major } else { ChestTexture::Normal }, big: false },
                CorrectChestAppearances::Both => ChestAppearance { texture: if heart_wincon { ChestTexture::Major } else { ChestTexture::Normal }, big: heart_wincon },
            },
            "Bombs (5)" |
            "Deku Nuts (5)" |
            "Deku Stick (1)" |
            "Magic Bean" |
            "Deku Shield" |
            "Hylian Shield" |
            "Deku Seeds (5)" |
            "Compass (Deku Tree)" |
            "Compass (Dodongos Cavern)" |
            "Compass (Jabu Jabus Belly)" |
            "Compass (Forest Temple)" |
            "Compass (Fire Temple)" |
            "Compass (Water Temple)" |
            "Compass (Spirit Temple)" |
            "Compass (Shadow Temple)" |
            "Compass (Bottom of the Well)" |
            "Compass (Ice Cavern)" |
            "Map (Deku Tree)" |
            "Map (Dodongos Cavern)" |
            "Map (Jabu Jabus Belly)" |
            "Map (Forest Temple)" |
            "Map (Fire Temple)" |
            "Map (Water Temple)" |
            "Map (Spirit Temple)" |
            "Map (Shadow Temple)" |
            "Map (Bottom of the Well)" |
            "Map (Ice Cavern)" |
            "Recovery Heart" |
            "Arrows (5)" |
            "Arrows (10)" |
            "Arrows (30)" |
            "Rupee (1)" |
            "Rupees (5)" |
            "Rupees (20)" |
            "Heart Container (Boss)" |
            "Rupees (50)" |
            "Rupees (200)" |
            "Deku Sticks (5)" |
            "Deku Sticks (10)" |
            "Deku Nuts (10)" |
            "Bombs (10)" |
            "Bombs (20)" |
            "Deku Seeds (30)" |
            "Rupee (Treasure Chest Game)" |
            "Deku Stick Capacity" |
            "Deku Nut Capacity" => ChestAppearance { texture: ChestTexture::Normal, big: false },
            _ => unimplemented!(),
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(transparent)]
pub(crate) struct ChestAppearances(pub(crate) [ChestAppearance; 4]);

impl ChestAppearances {
    pub(crate) const VANILLA: Self = Self([ChestAppearance::VANILLA; 4]);

    pub(crate) fn random() -> Self {
        //TODO automatically keep up to date with the dev-fenhl branch of the RSL script
        static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../assets/chests-rsl-e09ce03.json")).expect("failed to parse chest weights"));

        WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
    }

    pub(crate) fn textures(self) -> ChestTextures {
        ChestTextures(self.0.map(|ChestAppearance { texture, .. }| texture))
    }
}

impl From<SpoilerLog> for ChestAppearances {
    fn from(SpoilerLog { settings, locations, .. }: SpoilerLog) -> Self {
        let camc_kind = settings.correct_chest_appearances
            .unwrap_or_else(|| if settings.correct_chest_sizes { CorrectChestAppearances::Classic } else { CorrectChestAppearances::Off });
        let token_wincon = matches!(settings.bridge, Bridge::Tokens) || matches!(settings.lacs_condition, LacsCondition::Tokens) || matches!(settings.shuffle_ganon_bosskey, ShuffleGanonBosskey::Tokens);
        let heart_wincon = matches!(settings.bridge, Bridge::Hearts) || matches!(settings.lacs_condition, LacsCondition::Hearts) || matches!(settings.shuffle_ganon_bosskey, ShuffleGanonBosskey::Hearts);
        let locations = locations.choose(&mut thread_rng()).expect("no worlds in location list");
        Self([
            ChestAppearance::from_item(camc_kind, settings.bombchus_in_logic, token_wincon, heart_wincon, &locations.kf_midos_top_left_chest),
            ChestAppearance::from_item(camc_kind, settings.bombchus_in_logic, token_wincon, heart_wincon, &locations.kf_midos_top_right_chest),
            ChestAppearance::from_item(camc_kind, settings.bombchus_in_logic, token_wincon, heart_wincon, &locations.kf_midos_bottom_left_chest),
            ChestAppearance::from_item(camc_kind, settings.bombchus_in_logic, token_wincon, heart_wincon, &locations.kf_midos_bottom_right_chest),
        ])
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(transparent)]
pub(crate) struct ChestTextures(pub(crate) [ChestTexture; 4]);

#[derive(Debug, From)]
pub(crate) enum ChestTexturesFromParamError {
    Len(Vec<ChestTexture>),
    Texture(char),
}

impl<'a> FromParam<'a> for ChestTextures {
    type Error = ChestTexturesFromParamError;

    fn from_param(param: &'a str) -> Result<Self, ChestTexturesFromParamError> {
        Ok(Self(param.chars().map(ChestTexture::try_from).try_collect::<_, Vec<_>, _>()?.try_into()?))
    }
}

impl UriDisplay<Path> for ChestTextures {
    fn fmt(&self, f: &mut uri::fmt::Formatter<'_, Path>) -> fmt::Result {
        write!(f, "{}", self.0.iter().copied().map_into::<char>().format(""))
    }
}

impl_from_uri_param_identity!([Path] ChestTextures);

#[rocket::get("/favicon.ico")]
pub(crate) async fn favicon_ico() -> io::Result<NamedFile> {
    //TODO random chest texture configurations based on current RSL weights?
    NamedFile::open("assets/favicon.ico").await
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum FaviconError {
    #[error(transparent)] Image(#[from] image::ImageError),
    #[error(transparent)] Io(#[from] io::Error),
    #[error("unsupported file extension")]
    UnsupportedSuffix,
}

#[rocket::get("/favicon/<textures>/<size_ext>")]
pub(crate) async fn favicon_png(textures: ChestTextures, size_ext: Suffix<'_, u32>) -> Result<Response<RgbaImage>, FaviconError> {
    let ChestTextures([top_left, top_right, bottom_left, bottom_right]) = textures;
    let Suffix(size, ext) = size_ext;
    if ext != "png" { return Err(FaviconError::UnsupportedSuffix) }
    let chest_size = size / 2;
    let mut buf = RgbaImage::new(size, size);
    buf.copy_from(&ImageReader::open(format!("assets/static/chest/{}{chest_size}.png", char::from(top_left)))?.decode()?, 0, 0)?;
    buf.copy_from(&ImageReader::open(format!("assets/static/chest/{}{chest_size}.png", char::from(top_right)))?.decode()?, chest_size, 0)?;
    buf.copy_from(&ImageReader::open(format!("assets/static/chest/{}{chest_size}.png", char::from(bottom_left)))?.decode()?, 0, chest_size)?;
    buf.copy_from(&ImageReader::open(format!("assets/static/chest/{}{chest_size}.png", char::from(bottom_right)))?.decode()?, chest_size, chest_size)?;
    Ok(Response(buf))
}
