use thiserror::Error;


#[derive(Error, Debug, Clone)]
#[error("{0}")]
pub struct ReadError(pub String);

#[derive(Error, Debug, Clone)]
pub enum BundleError {
    #[error("invalid bundle: {0}")]
    InvalidBytes(#[from] ReadError),
    #[error("unsupported compression: {0}")]
    UnsupportedCompression(String),
}

#[derive(Error, Debug, Clone)]
pub enum MetadataError {
    #[error("invalid metadata: {0}")]
    InvalidBytes(#[from] ReadError),
}

#[derive(Error, Debug, Clone)]
pub enum TextureCacheError {
    #[error("invalid texture cache: {0}")]
    InvalidBytes(#[from] ReadError),
}

#[derive(Error, Debug, Clone)]
pub enum ConversionError {
    #[error("unrecognized container; expected a bundle, metadata.store, or texture.cache")]
    UnknownContainer,
    #[error(transparent)]
    Bundle(#[from] BundleError),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    #[error(transparent)]
    TextureCache(#[from] TextureCacheError),
}
