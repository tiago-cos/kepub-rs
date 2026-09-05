use thiserror::Error;

/// The error type for all fallible operations in this crate.
///
/// Covers failures across the full pipeline: reading and writing EPUB/KEPUB
/// archives, parsing structural XML, and translating between EPUB and Kobo
/// locations.
#[derive(Error, Debug)]
pub enum KepubError {
    /// An underlying I/O operation failed (e.g. reading a file from disk).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The ZIP archive (the EPUB/KEPUB container) could not be read or written.
    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// A structural XML file (e.g. the OPF or an XHTML content document)
    /// could not be parsed.
    #[error("XML parsing error in structural files: {0}")]
    XmlParse(#[from] roxmltree::Error),

    /// Serializing a document back to XHTML failed.
    #[error("Serialization error: {0}")]
    Serialize(String),

    /// The EPUB does not conform to the expected structure (e.g. a missing
    /// or malformed container/OPF file).
    #[error("Invalid EPUB format: {0}")]
    InvalidEpub(String),

    /// A required XML element was not found in a document.
    ///
    /// The wrapped value is the tag name of the missing element.
    #[error("required <{0}> element not found in document")]
    MissingElement(&'static str),

    /// A content document referenced by the EPUB manifest could not be
    /// found inside the archive.
    ///
    /// The wrapped value is the path of the missing content document.
    #[error("content document \"{0}\" not found in the archive")]
    ContentFileNotFound(String),

    /// No `koboSpan` element with the given `id` exists in the content
    /// document.
    #[error("no koboSpan found with id \"{0}\"")]
    SpanNotFound(String),

    /// A character offset fell outside the bounds of the text wrapped by a
    /// `koboSpan`.
    #[error(
        "offset {offset} is out of range for koboSpan \"{id}\" (text is {len} characters, so valid offsets are 0..={len})"
    )]
    InvalidSpanOffset {
        /// The `id` of the `koboSpan` the offset was checked against.
        id: String,
        /// The requested, out-of-range offset.
        offset: usize,
        /// The number of characters in the span's text (the valid range is
        /// `0..=len`).
        len: usize,
    },

    /// A location string could not be parsed into an [`EpubLocation`] or
    /// [`KoboLocation`].
    ///
    /// [`EpubLocation`]: crate::location::EpubLocation
    /// [`KoboLocation`]: crate::location::KoboLocation
    #[error("couldn't parse the location: {0}")]
    LocationParse(String),

    /// A location's element path and text index do not resolve to any node
    /// in the target document.
    #[error("location {element_path:?}/t{text_index} doesn't resolve: {detail}")]
    InvalidLocationPath {
        /// The 0-based path of element indices from the document root to
        /// the target element.
        element_path: Vec<usize>,
        /// The 0-based index of the text node within the target element.
        text_index: usize,
        /// A human-readable explanation of why the path failed to resolve.
        detail: String,
    },

    /// A character offset fell outside the bounds of the text run it was
    /// resolved against.
    #[error("character offset {offset} is out of range for text run t{text_index}")]
    InvalidLocationOffset {
        /// The requested, out-of-range character offset.
        offset: usize,
        /// The 0-based index of the text run the offset was checked
        /// against.
        text_index: usize,
    },

    /// A location points at text that Kobo does not wrap in a `koboSpan`
    /// (for example, text inside `<script>`, `<style>`, `<pre>`, or
    /// `<svg>`).
    #[error(
        "location {element_path:?}/t{text_index} points at text Kobo doesn't span (e.g. inside \
         <script>/<style>/<pre>, or inside an <svg>)"
    )]
    LocationNotSpanned {
        /// The 0-based path of element indices from the document root to
        /// the target element.
        element_path: Vec<usize>,
        /// The 0-based index of the text node within the target element.
        text_index: usize,
    },

    /// A location resolved to a `koboSpan` that wraps a whole element (an
    /// image or SVG) rather than text, so it has no character position
    /// inside it.
    #[error(
        "koboSpan \"{span_id}\" wraps an element (an image or SVG) at element path \
         {element_path:?}, which has no character position inside it"
    )]
    LocationIsElement {
        /// The 0-based path of element indices from the document root to
        /// the wrapped element.
        element_path: Vec<usize>,
        /// The `id` of the `koboSpan` wrapping the element.
        span_id: String,
    },
}
