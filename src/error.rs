use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Format(String),
    #[error("I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Png(#[from] png::EncodingError),
    #[error(transparent)]
    Xml(#[from] quick_xml::Error),
}

impl Error {
    pub fn format(message: impl Into<String>) -> Self {
        Self::Format(message.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        let path = path.into();
        self.map_err(|source| Error::Io { path, source })
    }
}
