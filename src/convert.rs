//! Top-level orchestration: read an EPUB, transform every spine content
//! document in parallel, write the result out as a KEPUB.

use std::collections::HashMap;
use std::io::{Read, Seek, Write};

use rayon::prelude::*;

use crate::archive::EpubArchive;
use crate::dom::serializer;
use crate::error::KepubError;
use transform::Transform;

mod assets;
mod manifest;
pub mod transform;

pub use transform::{ElementKind, default_classify, default_segment};

/// Configures and runs the full EPUB-to-KEPUB conversion pipeline.
///
/// [`Converter::default`] is the common case and needs no configuration.
/// Each field can be overridden individually with struct update syntax:
///
/// ```no_run
/// # use kepub_rs::Converter;
/// let converter = Converter {
///     css_contents: "/* my stylesheet */",
///     ..Converter::default()
/// };
/// ```
pub struct Converter<'a> {
    /// Contents of the stylesheet written into the output archive.
    pub css_contents: &'a str,
    /// Contents of the script written into the output archive.
    pub js_contents: &'a str,
    /// Decides how each element is treated during span injection. See
    /// [`ElementKind`], and [`default_classify`] to build on the default
    /// rules rather than replacing them wholesale.
    pub classify: fn(&str) -> ElementKind,
    /// Splits a text run into `(start, end)` byte-offset segments, each of
    /// which becomes its own koboSpan. Must return segments that exactly
    /// partition the input. See [`default_segment`].
    pub segment: fn(&str) -> Vec<(usize, usize)>,
}

impl Default for Converter<'_> {
    /// Returns a `Converter` using Kobo's default stylesheet and script
    /// (see [`assets::KOBO_CSS`]/[`assets::KOBO_JS`]).
    fn default() -> Self {
        let Transform {
            classify, segment, ..
        } = Transform::default();
        Self {
            css_contents: assets::KOBO_CSS,
            js_contents: assets::KOBO_JS,
            classify,
            segment,
        }
    }
}

impl Converter<'_> {
    /// Reads an EPUB from `input`, converts every spine content document in
    /// parallel, and writes the resulting KEPUB to `output`.
    ///
    /// Also writes `css_contents`/`js_contents` into the output archive and
    /// registers both in the OPF manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if the input EPUB cannot be read or parsed, if a
    /// spine content document cannot be transformed, if the OPF manifest
    /// cannot be updated, or if the resulting KEPUB cannot be written.
    pub fn convert<R, W>(&self, input: R, output: W) -> Result<(), KepubError>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        let mut archive = EpubArchive::new(input)?;
        let spine = archive.get_spine_xhtml()?;

        let results: Vec<(String, Vec<u8>)> = spine
            .into_par_iter()
            .map(|(path, bytes)| -> Result<_, KepubError> {
                let xhtml = self.transform_one(&bytes, &path)?;
                Ok((path, xhtml.into_bytes()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut replacements: HashMap<String, Vec<u8>> = results.into_iter().collect();

        let (opf_path, opf_bytes) = archive.get_opf()?;
        let opf = String::from_utf8(opf_bytes).map_err(|e| {
            KepubError::InvalidEpub(format!("the OPF package document isn't valid UTF-8: {e}"))
        })?;

        let css_from_opf = Self::get_relative_href(&opf_path, assets::KOBO_CSS_HREF);
        let js_from_opf = Self::get_relative_href(&opf_path, assets::KOBO_JS_HREF);
        let updated_opf = manifest::add_manifest_items(
            &opf,
            &[
                manifest::ManifestItem {
                    id: "js-kobo.js",
                    href: &js_from_opf,
                    media_type: "application/javascript",
                },
                manifest::ManifestItem {
                    id: "css-kobo.css",
                    href: &css_from_opf,
                    media_type: "text/css",
                },
            ],
        )?;

        replacements.insert(opf_path, updated_opf.into_bytes());

        let additions: HashMap<String, Vec<u8>> = HashMap::from([
            (
                assets::KOBO_CSS_HREF.to_string(),
                self.css_contents.as_bytes().to_vec(),
            ),
            (
                assets::KOBO_JS_HREF.to_string(),
                self.js_contents.as_bytes().to_vec(),
            ),
        ]);

        archive.write_kepub(output, replacements, additions)?;

        Ok(())
    }

    /// Parses, mutates, and serializes a single content document at
    /// `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if `bytes` cannot be parsed as XHTML, if the
    /// transform cannot be applied (e.g. a missing `<head>` or `<body>`),
    /// or if the result cannot be serialized.
    fn transform_one(&self, bytes: &[u8], path: &str) -> Result<String, KepubError> {
        let doc = crate::dom::parse(bytes)?;

        let dynamic_css = Self::get_relative_href(path, assets::KOBO_CSS_HREF);
        let dynamic_js = Self::get_relative_href(path, assets::KOBO_JS_HREF);

        let mutator = Transform {
            css_href: &dynamic_css,
            js_href: &dynamic_js,
            classify: self.classify,
            segment: self.segment,
        };
        mutator.apply(&doc)?;

        serializer::serialize(&doc)
    }

    /// Computes `target`'s path relative to `file_path`'s directory,
    /// assuming both are rooted at the archive root.
    ///
    /// `target` is treated as archive-root-relative; one `../` is
    /// prepended per path segment in `file_path` to reach back up to the
    /// archive root before descending into `target`.
    fn get_relative_href(file_path: &str, target: &str) -> String {
        let depth = file_path.chars().filter(|&c| c == '/').count();

        if depth == 0 {
            target.to_string()
        } else {
            let prefix = "../".repeat(depth);
            format!("{prefix}{target}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn mock_epub(spine_xhtml: &str) -> Cursor<Vec<u8>> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

            zip.start_file("mimetype", options)
                .expect("should start mimetype file in mock epub");
            zip.write_all(b"application/epub+zip")
                .expect("should write mimetype contents");

            zip.start_file("META-INF/container.xml", options)
                .expect("should start container.xml in mock epub");
            zip.write_all(
                br#"<?xml version="1.0"?>
                <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
                    <rootfiles>
                        <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
                    </rootfiles>
                </container>"#,
            )
            .expect("should write container.xml contents");

            zip.start_file("OEBPS/content.opf", options)
                .expect("should start content.opf in mock epub");
            zip.write_all(
                br#"<?xml version="1.0"?>
                <package xmlns="http://www.idpf.org/2007/opf">
                    <manifest>
                        <item id="ch1" href="text/chapter1.xhtml" media-type="application/xhtml+xml"/>
                    </manifest>
                    <spine><itemref idref="ch1"/></spine>
                </package>"#,
            )
            .expect("should write content.opf contents");

            zip.start_file("OEBPS/text/chapter1.xhtml", options)
                .expect("should start chapter1.xhtml in mock epub");
            zip.write_all(spine_xhtml.as_bytes())
                .expect("should write chapter1.xhtml contents");

            zip.start_file("OEBPS/cover.jpg", options)
                .expect("should start cover.jpg in mock epub");
            zip.write_all(b"not-really-a-jpeg")
                .expect("should write mock cover contents");

            zip.finish().expect("should finalize mock epub zip writer");
        }
        buf.set_position(0);
        buf
    }

    #[test]
    fn convert_injects_spans_and_preserves_other_files() {
        let input = mock_epub("<html><head></head><body><p>Hello world</p></body></html>");
        let mut output = Cursor::new(Vec::new());

        Converter::default()
            .convert(input, &mut output)
            .expect("conversion should succeed");

        output.set_position(0);
        let mut result = zip::ZipArchive::new(output).expect("output should be a valid zip");

        let mut chapter = String::new();
        result
            .by_name("OEBPS/text/chapter1.xhtml")
            .expect("converted chapter1.xhtml should exist in output archive")
            .read_to_string(&mut chapter)
            .expect("chapter1.xhtml should be readable as valid string");
        assert!(chapter.contains("koboSpan"), "got: {chapter}");
        assert!(chapter.contains("book-columns"), "got: {chapter}");

        let mut cover = Vec::new();
        result
            .by_name("OEBPS/cover.jpg")
            .expect("non-content files should be preserved unchanged")
            .read_to_end(&mut cover)
            .expect("cover.jpg bytes should be readable");
        assert_eq!(cover, b"not-really-a-jpeg");
    }

    #[test]
    fn convert_writes_assets_and_registers_them_in_the_opf() {
        let input = mock_epub("<html><head></head><body><p>Hello world</p></body></html>");
        let mut output = Cursor::new(Vec::new());

        Converter::default()
            .convert(input, &mut output)
            .expect("conversion should succeed");

        output.set_position(0);
        let mut result = zip::ZipArchive::new(output).expect("output should be a valid zip");

        let mut css = String::new();
        result
            .by_name("css/kobo.css")
            .expect("kobo.css should be written into the archive")
            .read_to_string(&mut css)
            .expect("kobo.css should be readable as valid string");

        assert!(css.contains("height: 100%"), "got: {css}");

        result
            .by_name("js/kobo.js")
            .expect("kobo.js should be written into the archive, even when empty");

        let mut opf = String::new();
        result
            .by_name("OEBPS/content.opf")
            .expect("content.opf should exist in output archive")
            .read_to_string(&mut opf)
            .expect("content.opf should be readable as valid string");

        assert!(
            opf.contains(r#"href="../css/kobo.css""#),
            "manifest should register the stylesheet relative to the OPF, got: {opf}"
        );
        assert!(
            opf.contains(r#"href="../js/kobo.js""#),
            "manifest should register the script relative to the OPF, got: {opf}"
        );

        let css_pos = opf
            .find("kobo.css")
            .expect("kobo.css should be present in manifest");
        let manifest_close_pos = opf
            .find("</manifest>")
            .expect("manifest closing tag should be present in OPF");
        assert!(
            css_pos < manifest_close_pos,
            "items must land inside <manifest>"
        );
    }

    #[test]
    fn converting_twice_does_not_duplicate_manifest_items() {
        let input = mock_epub("<html><head></head><body><p>Hello world</p></body></html>");
        let mut once = Cursor::new(Vec::new());
        Converter::default()
            .convert(input, &mut once)
            .expect("first conversion should succeed");

        once.set_position(0);
        let mut twice = Cursor::new(Vec::new());
        Converter::default()
            .convert(once, &mut twice)
            .expect("re-converting an already-converted book should succeed");

        twice.set_position(0);
        let mut result = zip::ZipArchive::new(twice).expect("output should be a valid zip");
        let mut opf = String::new();
        result
            .by_name("OEBPS/content.opf")
            .expect("content.opf should exist in re-converted archive")
            .read_to_string(&mut opf)
            .expect("content.opf should be readable as valid string");

        assert_eq!(
            opf.matches(r#"href="../css/kobo.css""#).count(),
            1,
            "got: {opf}"
        );
    }
}
