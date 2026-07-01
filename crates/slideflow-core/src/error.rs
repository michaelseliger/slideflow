use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("not a valid PPTX package: {0}")]
    InvalidPackage(String),

    #[error("missing part in package: {0}")]
    MissingPart(String),

    #[error("XML error in {part}: {message}")]
    Xml { part: String, message: String },

    #[error("slide {index} out of range (deck has {count} slides)")]
    SlideOutOfRange { index: usize, count: usize },

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("watch error: {0}")]
    Watch(String),

    #[error("compose error: {0}")]
    Compose(String),

    #[error("render error: {0}")]
    Render(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path: path.into(), source }
    }

    pub fn xml(part: impl Into<String>, message: impl std::fmt::Display) -> Self {
        Error::Xml { part: part.into(), message: message.to_string() }
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        Error::InvalidPackage(e.to_string())
    }
}
