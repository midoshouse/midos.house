use {
    std::fmt::Write as _,
    derive_more::From,
    image::{
        GenericImage as _,
        RgbaImage,
        io::Reader as ImageReader,
    },
    ootr_utils::{
        camc::ChestAppearance,
        spoiler::Item,
    },
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
    rocket_util::Response,
    crate::prelude::*,
};

#[derive(Clone, Copy, Deserialize)]
#[serde(transparent)]
pub(crate) struct ChestAppearances(pub(crate) [ChestAppearance; 4]);

impl ChestAppearances {
    pub(crate) const VANILLA: Self = Self([ChestAppearance { texture: ChestTexture::Normal, big: false }; 4]);
    pub(crate) const INVISIBLE: Self = Self([ChestAppearance { texture: ChestTexture::Invisible, big: false }; 4]);
    pub(crate) const SMALL_KEYS: Self = Self([ChestAppearance { texture: ChestTexture::SmallKey1751, big: false }; 4]);
    pub(crate) const TOKENS: Self = Self([ChestAppearance { texture: ChestTexture::Token, big: false }; 4]);

    pub(crate) fn random() -> Self {
        //TODO automatically keep up to date with the dev-fenhl branch of the RSL script
        static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../assets/chests-rsl-9c9e7ca.json")).expect("failed to parse chest weights"));

        WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
    }

    pub(crate) fn textures(self) -> ChestTextures {
        ChestTextures(self.0.map(|ChestAppearance { texture, .. }| texture))
    }
}

impl From<SpoilerLog> for ChestAppearances {
    fn from(spoiler: SpoilerLog) -> Self {
        let locations = spoiler.locations.choose(&mut thread_rng()).expect("no worlds in location list");
        Self([
            ChestAppearance::from_item(&spoiler, ChestAppearance { texture: ChestTexture::Normal, big: false }, locations.get("KF Midos Top Left Chest").or_else(|| locations.get("Mido Chest Top Left")).cloned().unwrap_or_else(|| Item { item: format!("Rupees (5)"), model: None, player: NonZeroU8::new(1).unwrap() })),
            ChestAppearance::from_item(&spoiler, ChestAppearance { texture: ChestTexture::Normal, big: false }, locations.get("KF Midos Top Right Chest").or_else(|| locations.get("Mido Chest Top Right")).cloned().unwrap_or_else(|| Item { item: format!("Rupees (5)"), model: None, player: NonZeroU8::new(1).unwrap() })),
            ChestAppearance::from_item(&spoiler, ChestAppearance { texture: ChestTexture::Normal, big: false }, locations.get("KF Midos Bottom Left Chest").or_else(|| locations.get("Mido Chest Bottom Left")).cloned().unwrap_or_else(|| Item { item: format!("Rupee (1)"), model: None, player: NonZeroU8::new(1).unwrap() })),
            ChestAppearance::from_item(&spoiler, ChestAppearance { texture: ChestTexture::Normal, big: false }, locations.get("KF Midos Bottom Right Chest").or_else(|| locations.get("Mido Chest Bottom Right")).cloned().unwrap_or_else(|| Item { item: format!("Recovery Heart"), model: None, player: NonZeroU8::new(1).unwrap() })),
        ])
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ChestTextures(pub(crate) [ChestTexture; 4]);

#[derive(Debug, thiserror::Error, From)]
pub(crate) enum ChestTexturesFromParamError {
    #[error("expected 4 characters, got {}", .0.len())]
    Len(Vec<ChestTexture>),
    #[error("unknown chest texture abbreviation: {0}")]
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
