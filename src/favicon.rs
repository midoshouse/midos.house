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
};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ChestTexture {
    Normal,
    Major,
    SmallKey,
    BossKey,
    Token,
}

impl TryFrom<char> for ChestTexture {
    type Error = char;

    fn try_from(c: char) -> Result<Self, char> {
        match c {
            'n' => Ok(Self::Normal),
            'm' => Ok(Self::Major),
            'k' => Ok(Self::SmallKey),
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
            ChestTexture::Major => 'm',
            ChestTexture::SmallKey => 'k',
            ChestTexture::BossKey => 'b',
            ChestTexture::Token => 's',
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
pub(crate) struct ChestAppearance {
    pub(crate) texture: ChestTexture,
    pub(crate) big: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(transparent)]
pub(crate) struct ChestAppearances(pub(crate) [ChestAppearance; 4]);

impl ChestAppearances {
    pub(crate) fn random() -> Self {
        static WEIGHTS: Lazy<Vec<(ChestAppearances, usize)>> = Lazy::new(|| serde_json::from_str(include_str!("../assets/chests-rsl-da4dae5.json")).expect("failed to parse chest weights"));

        WEIGHTS.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
    }

    pub(crate) fn textures(self) -> ChestTextures {
        ChestTextures(self.0.map(|ChestAppearance { texture, .. }| texture))
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
    //TODO random chest configurations based on current RSL weights except CSMC is replaced with CTMC?
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
