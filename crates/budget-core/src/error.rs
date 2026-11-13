use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(msg.into()))
}
