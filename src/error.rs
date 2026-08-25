use thiserror::Error;

#[derive(Error, Debug)]
pub enum KepubError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("XML parsing error in structural files: {0}")]
    XmlParse(#[from] roxmltree::Error),

    #[error("Invalid EPUB format: {0}")]
    InvalidEpub(String),
}
