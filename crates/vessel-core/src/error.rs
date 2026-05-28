use miette::Diagnostic;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, VesselError>;

#[derive(Debug, Error, Diagnostic)]
pub enum VesselError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("extractor error: {0}")]
    Extractor(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}
