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
    #[error("invalid bundle: {0}")]
    InvalidBytes(#[from] ReadError),
}
