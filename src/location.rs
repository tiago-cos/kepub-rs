//! The public location layer: a compact, self-contained way to name a
//! character position inside an EPUB, and translation to and from Kobo's
//! span-based locations.
//!
//! # EPUB location format
//!
//! ```text
//! OEBPS/chapter-001.xhtml#0/2/t1:44
//!                         │ │  │  └─ character offset into that text run
//!                         │ │  └──── text run index (0-based) in that element
//!                         └─┴─────── element indices (0-based) from <body>
//! ```
//!
//! Everything is counted against the document's *original* structure:
//! koboSpans collapse back into the text nodes they were split from, and
//! the `book-columns`/`book-inner` wrapper that conversion adds is
//! skipped. The same string therefore addresses the same character in the
//! original EPUB and in the converted kepub.
//!
//! Element indices count only elements; the text index counts only text
//! runs; they're separate 0-based sequences. Character offsets are in
//! Unicode scalar values, so they can't land mid-character and need no
//! surrogate-pair arithmetic.
//!
//! An element path may be empty (`file.xhtml#t0:5`) for text sitting
//! directly in `<body>`.
//!
//! # Kobo location format
//!
//! ```text
//! OEBPS/chapter-001.xhtml#kobo.4.2:12
//!                         │      │  └─ character offset into that span's own text
//!                         └──────┴──── the koboSpan's id
//! ```
//!
//! The offset is into that one span's text, not the reconstructed original
//! text node — the same unit the EPUB format uses, so an offset never
//! changes meaning as it crosses between the two.

use std::fmt;
use std::io::{Read, Seek};
use std::str::FromStr;

use crate::archive::EpubArchive;
use crate::dom::arena::DocumentArena;
use crate::error::KepubError;

mod resolve;

/// A reading position as Kobo represents it: which content document, which
/// koboSpan within it, and how far into that span's own text.
///
/// `char_offset` names a position *between* characters, so a span of N
/// characters has N+1 valid offsets (0 through N).
#[derive(Debug, Clone, PartialEq, Eq)]
struct KoboLocation {
    /// Path of the content document inside the archive.
    content_path: String,
    /// The `N` in the span's `kobo.N.M` id — the paragraph/element ordinal
    /// this span belongs to.
    para: u32,
    /// The `M` in the span's `kobo.N.M` id — this segment's position
    /// within its `para` group.
    seg: u32,
    /// Character (Unicode scalar) offset into this span's own text.
    char_offset: usize,
}

impl KoboLocation {
    /// The koboSpan id this refers to, e.g. `kobo.8.2`.
    fn span_id(&self) -> String {
        format!("kobo.{}.{}", self.para, self.seg)
    }
}

/// A single character position inside an EPUB.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EpubLocation {
    /// Path of the content document inside the archive.
    content_path: String,
    /// 0-based element indices from `<body>` down to the element holding
    /// the text. Empty means the text sits directly in `<body>`.
    element_path: Vec<usize>,
    /// 0-based index of the text run within that element.
    text_index: usize,
    /// Character (Unicode scalar) offset into that text run, naming a
    /// position *between* characters.
    char_offset: usize,
}

impl fmt::Display for KoboLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}#{}:{}",
            self.content_path,
            self.span_id(),
            self.char_offset
        )
    }
}

impl FromStr for KoboLocation {
    type Err = KepubError;

    /// Parses a Kobo location string of the form
    /// `content_path#kobo.N.M:offset`.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::LocationParse`] if `s` has no `#` separator,
    /// an empty content path, or a span/offset part that doesn't match the
    /// expected grammar.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // rsplit so a '#' in a file path doesn't break parsing.
        let (content_path, span_part) = s.rsplit_once('#').ok_or_else(|| {
            KepubError::LocationParse(format!(
                "{s:?} is missing the \"#\" separating the file path from the span"
            ))
        })?;

        if content_path.is_empty() {
            return Err(KepubError::LocationParse(
                "the content document path is empty".into(),
            ));
        }

        let (span_id, offset_part) = span_part.rsplit_once(':').ok_or_else(|| {
            KepubError::LocationParse(format!(
                "{span_part:?} is missing the \":<char_offset>\" suffix"
            ))
        })?;

        let char_offset: usize = offset_part.parse().map_err(|_| {
            KepubError::LocationParse(format!("{offset_part:?} isn't a valid character offset"))
        })?;

        let malformed_span = || {
            KepubError::LocationParse(format!(
                "expected a span id like \"kobo.4.2\", got {span_id:?}"
            ))
        };

        let ordinals = span_id.strip_prefix("kobo.").ok_or_else(malformed_span)?;
        let (para_str, seg_str) = ordinals.split_once('.').ok_or_else(malformed_span)?;

        let para: u32 = para_str.parse().map_err(|_| {
            KepubError::LocationParse(format!("{para_str:?} isn't a valid paragraph ordinal"))
        })?;
        let seg: u32 = seg_str.parse().map_err(|_| {
            KepubError::LocationParse(format!("{seg_str:?} isn't a valid segment ordinal"))
        })?;

        Ok(Self {
            content_path: content_path.to_string(),
            para,
            seg,
            char_offset,
        })
    }
}

impl fmt::Display for EpubLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.content_path, self.node_part())
    }
}

impl EpubLocation {
    /// The part after the `#`, without the content path — e.g.
    /// `0/2/t1:44`.
    fn node_part(&self) -> String {
        let mut out = String::new();
        for index in &self.element_path {
            out.push_str(&index.to_string());
            out.push('/');
        }
        out.push('t');
        out.push_str(&self.text_index.to_string());
        out.push(':');
        out.push_str(&self.char_offset.to_string());
        out
    }

    /// Parses the part after the `#` (e.g. `0/2/t1:44`) into an
    /// `EpubLocation`, combined with the already-separated `content_path`.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::LocationParse`] if `node_part` doesn't match
    /// the `<element>/.../t<text_index>:<char_offset>` grammar.
    fn parse_node_part(content_path: &str, node_part: &str) -> Result<Self, KepubError> {
        let (path_part, offset_part) = node_part.rsplit_once(':').ok_or_else(|| {
            KepubError::LocationParse(format!(
                "{node_part:?} is missing the \":<char_offset>\" suffix"
            ))
        })?;

        let char_offset: usize = offset_part.parse().map_err(|_| {
            KepubError::LocationParse(format!("{offset_part:?} isn't a valid character offset"))
        })?;

        let mut segments: Vec<&str> = path_part.split('/').collect();

        let text_segment = segments.pop().ok_or_else(|| {
            KepubError::LocationParse(format!("{node_part:?} has no text run index"))
        })?;
        let text_index_str = text_segment.strip_prefix('t').ok_or_else(|| {
            KepubError::LocationParse(format!(
                "expected the last path segment to be a text run index like \"t0\", got \
                 {text_segment:?}"
            ))
        })?;
        let text_index: usize = text_index_str.parse().map_err(|_| {
            KepubError::LocationParse(format!("{text_index_str:?} isn't a valid text run index"))
        })?;

        let mut element_path = Vec::with_capacity(segments.len());
        for segment in segments {
            if segment.is_empty() {
                return Err(KepubError::LocationParse(format!(
                    "{node_part:?} has an empty element index (check for a doubled \"/\")"
                )));
            }
            let index: usize = segment.parse().map_err(|_| {
                KepubError::LocationParse(format!("{segment:?} isn't a valid element index"))
            })?;
            element_path.push(index);
        }

        Ok(Self {
            content_path: content_path.to_string(),
            element_path,
            text_index,
            char_offset,
        })
    }
}

impl FromStr for EpubLocation {
    type Err = KepubError;

    /// Parses a location string of the form
    /// `content_path#element/.../tN:offset`.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::LocationParse`] if `s` has no `#` separator,
    /// an empty content path, or a position part that doesn't match the
    /// expected grammar.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (content_path, node_part) = s.rsplit_once('#').ok_or_else(|| {
            KepubError::LocationParse(format!(
                "{s:?} is missing the \"#\" separating the file path from the position"
            ))
        })?;

        if content_path.is_empty() {
            return Err(KepubError::LocationParse(
                "the content document path is empty".into(),
            ));
        }

        Self::parse_node_part(content_path, node_part)
    }
}

/// Translates a Kobo location string into an EPUB location string.
///
/// `location` is a Kobo location such as
/// `OEBPS/chapter-001.xhtml#kobo.4.2:12`; the result is an EPUB location
/// such as `OEBPS/chapter-001.xhtml#0/2/t1:44`. `input` must be a
/// converted kepub, since an unconverted EPUB has no koboSpans to address.
///
/// # Errors
///
/// Returns [`KepubError::LocationParse`] if `location` isn't a valid Kobo
/// location string, [`KepubError::ContentFileNotFound`] if the content
/// document it names isn't in the archive, [`KepubError::SpanNotFound`] if
/// no matching koboSpan exists, [`KepubError::InvalidSpanOffset`] if the
/// offset is out of range for that span, or
/// [`KepubError::LocationIsElement`] if the span wraps an image or SVG
/// rather than text. Also propagates any error from opening the archive or
/// parsing the content document.
pub fn kobo_to_epub_location<R: Read + Seek>(
    input: R,
    location: &str,
) -> Result<String, KepubError> {
    let kobo: KoboLocation = location.parse()?;
    let doc = load_content_document(input, &kobo.content_path)?;

    let (element_path, text_index, char_offset) =
        resolve::kobo_to_location(&doc, kobo.para, kobo.seg, kobo.char_offset)?;

    Ok(EpubLocation {
        content_path: kobo.content_path,
        element_path,
        text_index,
        char_offset,
    }
    .to_string())
}

/// Translates an EPUB location string into a Kobo location string.
///
/// `location` is an EPUB location such as
/// `OEBPS/chapter-001.xhtml#0/2/t1:44`; the result is a Kobo location such
/// as `OEBPS/chapter-001.xhtml#kobo.4.2:12`. `input` must be a converted
/// kepub, since the result names a koboSpan.
///
/// # Errors
///
/// Returns [`KepubError::LocationParse`] if `location` isn't a valid EPUB
/// location string, [`KepubError::ContentFileNotFound`] if the content
/// document it names isn't in the archive,
/// [`KepubError::InvalidLocationPath`] if the element path or text index
/// don't resolve, [`KepubError::LocationNotSpanned`] if the addressed text
/// isn't wrapped in a koboSpan, or [`KepubError::InvalidLocationOffset`]
/// if the offset is out of range. Also propagates any error from opening
/// the archive or parsing the content document.
pub fn epub_to_kobo_location<R: Read + Seek>(
    input: R,
    location: &str,
) -> Result<String, KepubError> {
    let epub: EpubLocation = location.parse()?;
    let doc = load_content_document(input, &epub.content_path)?;

    let (para, seg, offset_in_span) =
        resolve::location_to_kobo(&doc, &epub.element_path, epub.text_index, epub.char_offset)?;

    Ok(KoboLocation {
        content_path: epub.content_path,
        para,
        seg,
        char_offset: offset_in_span,
    }
    .to_string())
}

/// Checks that an EPUB location string addresses a real character
/// position, without translating it.
///
/// This works against an **unconverted** EPUB as well as a kepub, since it
/// walks the document's logical structure rather than looking up koboSpan
/// ids — so the same location string can be checked against the original
/// book and the converted one.
///
/// Returns `Ok(())` when valid; the error says specifically what didn't
/// resolve. Use `.is_ok()` if a plain boolean is all you need.
///
/// # Errors
///
/// Returns [`KepubError::LocationParse`] if `location` isn't a valid EPUB
/// location string, [`KepubError::ContentFileNotFound`] if the content
/// document it names isn't in the archive,
/// [`KepubError::InvalidLocationPath`] if the element path or text index
/// don't resolve, or [`KepubError::InvalidLocationOffset`] if the offset
/// exceeds the addressed text run's length. Also propagates any error from
/// opening the archive or parsing the content document.
pub fn validate_epub_location<R: Read + Seek>(input: R, location: &str) -> Result<(), KepubError> {
    let epub: EpubLocation = location.parse()?;
    let doc = load_content_document(input, &epub.content_path)?;

    resolve::validate_location(&doc, &epub.element_path, epub.text_index, epub.char_offset)
}

/// Checks that a Kobo location string addresses a real position in a
/// kepub: the content document exists, the span exists, and the offset is
/// within that span's text.
///
/// Unlike [`validate_epub_location`], this requires a *converted* kepub —
/// an unconverted EPUB has no koboSpans, so every location will report
/// [`KepubError::SpanNotFound`].
///
/// Returns `Ok(())` when valid; the error says specifically what didn't
/// resolve. Use `.is_ok()` if a plain boolean is all you need.
///
/// # Errors
///
/// Returns [`KepubError::LocationParse`] if `location` isn't a valid Kobo
/// location string, [`KepubError::ContentFileNotFound`] if the content
/// document it names isn't in the archive, [`KepubError::SpanNotFound`] if
/// no matching koboSpan exists, or [`KepubError::InvalidSpanOffset`] if
/// the offset is out of range for that span. Also propagates any error
/// from opening the archive or parsing the content document.
pub fn validate_kobo_location<R: Read + Seek>(input: R, location: &str) -> Result<(), KepubError> {
    let kobo: KoboLocation = location.parse()?;
    let doc = load_content_document(input, &kobo.content_path)?;

    resolve::validate_kobo_span(&doc, kobo.para, kobo.seg, kobo.char_offset)
}

/// Opens `input` as an archive and reads and parses the content document
/// at `path`.
///
/// # Errors
///
/// Returns [`KepubError::Zip`] if `input` isn't a valid archive,
/// [`KepubError::ContentFileNotFound`] if `path` isn't in it, or
/// [`KepubError::XmlParse`] if its contents aren't well-formed XML.
fn load_content_document<R: Read + Seek>(
    input: R,
    path: &str,
) -> Result<DocumentArena, KepubError> {
    let mut archive = EpubArchive::new(input)?;
    let bytes = archive.read_content_document(path)?;
    crate::dom::parse(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Converter;
    use std::io::{Cursor, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    const PATH: &str = "OEBPS/chapter1.xhtml";

    fn raw_epub(body_xhtml: &str) -> Cursor<Vec<u8>> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

            zip.start_file("mimetype", options)
                .expect("should create mimetype entry");
            zip.write_all(b"application/epub+zip")
                .expect("should write mimetype content");

            zip.start_file("META-INF/container.xml", options)
                .expect("should create container.xml entry");
            zip.write_all(
                br#"<?xml version="1.0"?>
                <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
                    <rootfiles>
                        <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
                    </rootfiles>
                </container>"#,
            )
            .expect("should write container.xml content");

            zip.start_file("OEBPS/content.opf", options)
                .expect("should create content.opf entry");
            zip.write_all(
                br#"<?xml version="1.0"?>
                <package xmlns="http://www.idpf.org/2007/opf">
                    <manifest>
                        <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
                    </manifest>
                    <spine><itemref idref="ch1"/></spine>
                </package>"#,
            )
            .expect("should write content.opf content");

            zip.start_file("OEBPS/chapter1.xhtml", options)
                .expect("should create chapter1.xhtml entry");
            zip.write_all(
                format!("<html><head></head><body>{body_xhtml}</body></html>").as_bytes(),
            )
            .expect("should write chapter1.xhtml content");

            zip.finish().expect("should finish writing the EPUB ZIP");
        }
        buf.set_position(0);
        buf
    }

    fn kepub(body_xhtml: &str) -> Cursor<Vec<u8>> {
        let mut converted = Cursor::new(Vec::new());
        Converter::default()
            .convert(raw_epub(body_xhtml), &mut converted)
            .expect("conversion should succeed");
        converted.set_position(0);
        converted
    }

    #[test]
    fn formats_and_parses_an_epub_location() {
        let location = EpubLocation {
            content_path: "OEBPS/chapter-001.xhtml".into(),
            element_path: vec![0, 2],
            text_index: 1,
            char_offset: 44,
        };
        let s = location.to_string();
        assert_eq!(s, "OEBPS/chapter-001.xhtml#0/2/t1:44");
        assert_eq!(
            s.parse::<EpubLocation>()
                .expect("should parse the formatted location"),
            location
        );
    }

    #[test]
    fn formats_and_parses_an_empty_element_path() {
        let location = EpubLocation {
            content_path: "a.xhtml".into(),
            element_path: vec![],
            text_index: 0,
            char_offset: 5,
        };
        assert_eq!(location.to_string(), "a.xhtml#t0:5");
        assert_eq!(
            "a.xhtml#t0:5"
                .parse::<EpubLocation>()
                .expect("should parse the formatted location"),
            location
        );
    }

    #[test]
    fn formats_and_parses_a_kobo_location() {
        let location = KoboLocation {
            content_path: "OEBPS/chapter-001.xhtml".into(),
            para: 4,
            seg: 2,
            char_offset: 12,
        };
        let s = location.to_string();
        assert_eq!(s, "OEBPS/chapter-001.xhtml#kobo.4.2:12");
        assert_eq!(
            s.parse::<KoboLocation>()
                .expect("should parse the formatted location"),
            location
        );
    }

    #[test]
    fn rejects_malformed_epub_locations() {
        assert!("no-hash".parse::<EpubLocation>().is_err());
        assert!(
            "a.xhtml#0/2:5".parse::<EpubLocation>().is_err(),
            "missing t prefix"
        );
        assert!(
            "a.xhtml#0/t1".parse::<EpubLocation>().is_err(),
            "missing offset"
        );
        assert!(
            "a.xhtml#0//t1:5".parse::<EpubLocation>().is_err(),
            "doubled slash"
        );
        assert!(
            "a.xhtml#x/t1:5".parse::<EpubLocation>().is_err(),
            "non-numeric index"
        );
        assert!("#t0:5".parse::<EpubLocation>().is_err(), "empty path");
    }

    #[test]
    fn rejects_malformed_kobo_locations() {
        assert!("no-hash".parse::<KoboLocation>().is_err());
        assert!(
            "a.xhtml#kobo.4.2".parse::<KoboLocation>().is_err(),
            "missing offset"
        );
        assert!(
            "a.xhtml#4.2:0".parse::<KoboLocation>().is_err(),
            "missing kobo prefix"
        );
        assert!(
            "a.xhtml#kobo.4:0".parse::<KoboLocation>().is_err(),
            "missing seg ordinal"
        );
        assert!(
            "a.xhtml#kobo.x.2:0".parse::<KoboLocation>().is_err(),
            "non-numeric para"
        );
        assert!("#kobo.1.1:0".parse::<KoboLocation>().is_err(), "empty path");
    }

    #[test]
    fn round_trips_through_an_archive() {
        let kobo = "OEBPS/chapter1.xhtml#kobo.1.1:6";

        let epub =
            kobo_to_epub_location(kepub("<p>Hello world</p>"), kobo).expect("should translate");
        assert_eq!(epub, "OEBPS/chapter1.xhtml#0/t0:6");

        let back = epub_to_kobo_location(kepub("<p>Hello world</p>"), &epub)
            .expect("should translate back");
        assert_eq!(back, kobo);
    }

    #[test]
    fn round_trips_a_multi_segment_paragraph() {
        let body = "<p>First sentence. Second sentence.</p>";
        let kobo = "OEBPS/chapter1.xhtml#kobo.1.2:3";

        let epub = kobo_to_epub_location(kepub(body), kobo).expect("should translate");
        let back = epub_to_kobo_location(kepub(body), &epub).expect("should translate back");
        assert_eq!(back, kobo);
    }

    #[test]
    fn a_malformed_location_string_is_reported() {
        let err = kobo_to_epub_location(kepub("<p>Hello</p>"), "not-a-location")
            .expect_err("a malformed location string should produce an error");
        assert!(matches!(err, KepubError::LocationParse(_)));
    }

    #[test]
    fn missing_content_file_is_reported() {
        let err = kobo_to_epub_location(kepub("<p>Hello</p>"), "OEBPS/nope.xhtml#kobo.1.1:0")
            .expect_err("missing content file should produce an error");
        assert!(matches!(err, KepubError::ContentFileNotFound(p) if p == "OEBPS/nope.xhtml"));
    }

    #[test]
    fn unknown_span_cascades() {
        let err = kobo_to_epub_location(kepub("<p>Hello</p>"), "OEBPS/chapter1.xhtml#kobo.99.1:0")
            .expect_err("unknown Kobo span should produce an error");
        assert!(matches!(err, KepubError::SpanNotFound(id) if id == "kobo.99.1"));
    }

    #[test]
    fn bad_element_path_cascades() {
        let err = epub_to_kobo_location(kepub("<p>Hello</p>"), "OEBPS/chapter1.xhtml#40/t0:0")
            .expect_err("invalid element path should produce an error");
        assert!(matches!(err, KepubError::InvalidLocationPath { .. }));
    }

    #[test]
    fn a_valid_location_validates_against_both_epub_and_kepub() {
        let location = "OEBPS/chapter1.xhtml#0/t0:6";

        validate_epub_location(raw_epub("<p>Hello world</p>"), location)
            .expect("valid against the unconverted EPUB");
        validate_epub_location(kepub("<p>Hello world</p>"), location)
            .expect("valid against the kepub");
    }

    #[test]
    fn offset_at_the_very_end_of_a_run_is_valid() {
        validate_epub_location(raw_epub("<p>Hello</p>"), "OEBPS/chapter1.xhtml#0/t0:5")
            .expect("one-past-the-end is a valid position");
    }

    #[test]
    fn validation_rejects_a_bad_element_index() {
        let err = validate_epub_location(raw_epub("<p>Hello</p>"), "OEBPS/chapter1.xhtml#40/t0:0")
            .expect_err("invalid element index should produce an error");
        assert!(matches!(err, KepubError::InvalidLocationPath { .. }));
    }

    #[test]
    fn validation_rejects_a_bad_text_index() {
        let err = validate_epub_location(raw_epub("<p>Hello</p>"), "OEBPS/chapter1.xhtml#0/t9:0")
            .expect_err("invalid text index should produce an error");
        assert!(matches!(err, KepubError::InvalidLocationPath { .. }));
    }

    #[test]
    fn validation_rejects_an_out_of_range_offset() {
        let err = validate_epub_location(raw_epub("<p>Hello</p>"), "OEBPS/chapter1.xhtml#0/t0:99")
            .expect_err("out-of-range offset should produce an error");
        assert!(matches!(
            err,
            KepubError::InvalidLocationOffset { offset: 99, .. }
        ));
    }

    #[test]
    fn validation_rejects_a_missing_content_file() {
        let err = validate_epub_location(raw_epub("<p>Hello</p>"), "OEBPS/nope.xhtml#0/t0:0")
            .expect_err("missing content file should produce an error");
        assert!(matches!(err, KepubError::ContentFileNotFound(_)));
    }

    #[test]
    fn a_valid_kobo_location_validates() {
        validate_kobo_location(
            kepub("<p>Hello world</p>"),
            "OEBPS/chapter1.xhtml#kobo.1.1:6",
        )
        .expect("should be valid");
    }

    #[test]
    fn kobo_validation_rejects_an_unknown_span() {
        let err = validate_kobo_location(kepub("<p>Hello</p>"), "OEBPS/chapter1.xhtml#kobo.99.1:0")
            .expect_err("unknown Kobo span should produce an error");
        assert!(matches!(err, KepubError::SpanNotFound(id) if id == "kobo.99.1"));
    }

    #[test]
    fn kobo_offsets_count_characters_not_bytes() {
        let body = "<p>Hi \u{1F600} there.</p>";

        validate_kobo_location(kepub(body), "OEBPS/chapter1.xhtml#kobo.1.1:11")
            .expect("11 is the position just past the end");

        let err = validate_kobo_location(kepub(body), "OEBPS/chapter1.xhtml#kobo.1.1:12")
            .expect_err("offset past the end of the span should produce an error");
        assert!(matches!(
            err,
            KepubError::InvalidSpanOffset {
                offset: 12,
                len: 11,
                ..
            }
        ));
    }

    #[test]
    fn kobo_validation_fails_against_an_unconverted_epub() {
        let err = validate_kobo_location(
            raw_epub("<p>Hello world</p>"),
            "OEBPS/chapter1.xhtml#kobo.1.1:0",
        )
        .expect_err("Kobo location should not validate against an unconverted EPUB");
        assert!(matches!(err, KepubError::SpanNotFound(_)));
    }

    #[test]
    fn the_test_path_constant_matches_the_mock_archive() {
        assert!(validate_epub_location(raw_epub("<p>Hi</p>"), &format!("{PATH}#0/t0:0")).is_ok());
    }
}
