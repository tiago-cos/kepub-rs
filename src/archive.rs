use std::collections::HashMap;
use std::io::{Read, Seek, Write};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::error::KepubError;

/// A wrapper around a ZIP-backed EPUB/KEPUB archive.
///
/// Wraps any `R: Read + Seek` (a file, an in-memory buffer, etc.) and
/// provides EPUB-aware reads (spine extraction, OPF lookup) plus a
/// copy-and-patch write path for producing a modified archive.
pub struct EpubArchive<R: Read + Seek> {
    archive: ZipArchive<R>,
}

impl<R: Read + Seek> EpubArchive<R> {
    /// Opens `reader` as a ZIP archive without extracting any files yet.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::Zip`] if `reader` is not a valid ZIP archive.
    pub fn new(reader: R) -> Result<Self, KepubError> {
        let archive = ZipArchive::new(reader)?;
        Ok(Self { archive })
    }

    /// Reads the file at `path` into a `String`.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::Zip`] if `path` doesn't exist in the archive,
    /// or [`KepubError::Io`] if its contents aren't valid UTF-8.
    fn read_file_as_string(&mut self, path: &str) -> Result<String, KepubError> {
        let mut file = self.archive.by_name(path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Ok(content)
    }

    /// Reads a content document by `path`.
    ///
    /// Unlike [`read_file_as_bytes`](Self::read_file_as_bytes), a missing
    /// file is reported as [`KepubError::ContentFileNotFound`] rather than
    /// a raw zip error — `path` comes from the caller here, not from the
    /// archive's own manifest, so "you asked for something that isn't
    /// there" is a meaningfully different failure than a corrupt archive.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::ContentFileNotFound`] if `path` doesn't exist
    /// in the archive, or [`KepubError::Zip`] for any other archive read
    /// failure.
    pub fn read_content_document(&mut self, path: &str) -> Result<Vec<u8>, KepubError> {
        match self.archive.by_name(path) {
            Ok(mut file) => {
                let mut content = Vec::new();
                file.read_to_end(&mut content)?;
                Ok(content)
            }
            Err(zip::result::ZipError::FileNotFound) => {
                Err(KepubError::ContentFileNotFound(path.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Reads the file at `path` into a raw byte vector.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::Zip`] if `path` doesn't exist in the archive.
    fn read_file_as_bytes(&mut self, path: &str) -> Result<Vec<u8>, KepubError> {
        let mut file = self.archive.by_name(path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        Ok(content)
    }

    /// Returns the OPF package document's path inside the archive and its
    /// raw bytes.
    ///
    /// The path matters as much as the contents: manifest hrefs are
    /// relative to the OPF's own directory, so callers need it to compute
    /// correct relative paths.
    ///
    /// # Errors
    ///
    /// Returns an error if `META-INF/container.xml` is missing or
    /// malformed, or if the OPF it points at can't be read.
    pub fn get_opf(&mut self) -> Result<(String, Vec<u8>), KepubError> {
        let path = self.get_rootfile_path()?;
        let bytes = self.read_file_as_bytes(&path)?;
        Ok((path, bytes))
    }

    /// Finds the path to the `.opf` rootfile by parsing
    /// `META-INF/container.xml`.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::Zip`] if `container.xml` is missing,
    /// [`KepubError::XmlParse`] if it's malformed, or
    /// [`KepubError::InvalidEpub`] if it has no `<rootfile>` element or the
    /// element is missing its `full-path` attribute.
    fn get_rootfile_path(&mut self) -> Result<String, KepubError> {
        let container_xml = self.read_file_as_string("META-INF/container.xml")?;
        let doc = roxmltree::Document::parse(&container_xml)?;

        let rootfile_node = doc
            .descendants()
            .find(|n| n.has_tag_name("rootfile"))
            .ok_or_else(|| {
                KepubError::InvalidEpub("No <rootfile> found in container.xml".into())
            })?;

        let full_path = rootfile_node.attribute("full-path").ok_or_else(|| {
            KepubError::InvalidEpub("<rootfile> missing 'full-path' attribute".into())
        })?;

        Ok(full_path.to_string())
    }

    /// Extracts all XHTML content documents in spine (reading) order.
    ///
    /// Returns a `Vec` of `(file_path, raw_file_bytes)` pairs, one per
    /// spine entry.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::InvalidEpub`] if the OPF's spine references
    /// no XHTML content documents, or propagates any error from reading or
    /// parsing the container/OPF/spine files.
    pub fn get_spine_xhtml(&mut self) -> Result<Vec<(String, Vec<u8>)>, KepubError> {
        let opf_path = self.get_rootfile_path()?;

        let base_dir = opf_path
            .rsplit_once('/')
            .map(|(dir, _)| format!("{dir}/"))
            .unwrap_or_default();

        let opf_xml = self.read_file_as_string(&opf_path)?;
        let doc = roxmltree::Document::parse(&opf_xml)?;

        let mut manifest_map = HashMap::new();
        for node in doc.descendants().filter(|n| n.has_tag_name("item")) {
            if let (Some(id), Some(href), Some(media_type)) = (
                node.attribute("id"),
                node.attribute("href"),
                node.attribute("media-type"),
            ) && media_type == "application/xhtml+xml"
            {
                manifest_map.insert(id.to_string(), format!("{base_dir}{href}"));
            }
        }

        let mut spine_files = Vec::new();
        for node in doc.descendants().filter(|n| n.has_tag_name("itemref")) {
            if let Some(idref) = node.attribute("idref")
                && let Some(full_path) = manifest_map.get(idref)
            {
                let bytes = self.read_file_as_bytes(full_path)?;
                spine_files.push((full_path.clone(), bytes));
            }
        }

        if spine_files.is_empty() {
            return Err(KepubError::InvalidEpub(
                "No XHTML files found in the spine".into(),
            ));
        }

        Ok(spine_files)
    }

    /// Writes a new EPUB/KEPUB archive to `output`.
    ///
    /// Every entry from the original archive is copied over verbatim,
    /// except that any path present as a key in `replacements` gets those
    /// bytes instead of the original ones. `replacements` is drained as
    /// it's consumed; if anything is left in it afterward, that means a
    /// path was given that doesn't actually exist in the source archive,
    /// which is treated as an error rather than silently ignored. Any path
    /// in `additions` that doesn't already exist in the source archive is
    /// appended as a new file; a path present in both `replacements` and
    /// `additions` is resolved in favor of `replacements`.
    ///
    /// `mimetype`, if present, is always written first and stored
    /// uncompressed, regardless of where it appears in the source archive
    /// — the OCF packaging spec requires this so a file can be identified
    /// as an EPUB by inspecting just its first bytes. Getting this wrong
    /// can make some readers refuse the file even though it's a perfectly
    /// valid zip otherwise.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::InvalidEpub`] if `replacements` contains a
    /// path not present in the source archive. Propagates
    /// [`KepubError::Zip`] or [`KepubError::Io`] for any underlying read
    /// or write failure.
    pub fn write_kepub<W: Write + Seek>(
        &mut self,
        output: W,
        mut replacements: HashMap<String, Vec<u8>>,
        mut additions: HashMap<String, Vec<u8>>,
    ) -> Result<(), KepubError> {
        let mut writer = ZipWriter::new(output);

        let mut entries = Vec::with_capacity(self.archive.len());
        for i in 0..self.archive.len() {
            let file = self.archive.by_index(i)?;
            entries.push((file.name().to_string(), file.unix_mode()));
        }

        let mimetype = self.read_file_as_bytes("mimetype")?;

        let mimetype_mode = entries
            .iter()
            .find(|(name, _)| name == "mimetype")
            .and_then(|(_, mode)| *mode)
            .unwrap_or(0o644);

        let mimetype_opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(mimetype_mode);

        writer.start_file("mimetype", mimetype_opts)?;
        writer.write_all(&mimetype)?;

        for (name, mode) in entries {
            if name == "mimetype" {
                continue;
            }

            let bytes = match replacements
                .remove(&name)
                .or_else(|| additions.remove(&name))
            {
                Some(supplied) => supplied,
                None => self.read_file_as_bytes(&name)?,
            };

            let mut options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

            if let Some(mode) = mode {
                options = options.unix_permissions(mode);
            } else if name.ends_with('/') {
                options = options.unix_permissions(0o755);
            } else {
                options = options.unix_permissions(0o644);
            }

            writer.start_file(&name, options)?;
            writer.write_all(&bytes)?;
        }

        if !replacements.is_empty() {
            let mut unmatched: Vec<String> = replacements.into_keys().collect();
            unmatched.sort();

            return Err(KepubError::InvalidEpub(format!(
                "replacement(s) given for path(s) not found in the original archive: {unmatched:?}"
            )));
        }

        let mut new_files: Vec<(String, Vec<u8>)> = additions.into_iter().collect();
        new_files.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, bytes) in new_files {
            let mut options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

            if name.ends_with('/') {
                options = options.unix_permissions(0o755);
            } else {
                options = options.unix_permissions(0o644);
            }

            writer.start_file(&name, options)?;
            writer.write_all(&bytes)?;
        }

        writer.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    /// Creates an in-memory ZIP archive from a list of `(file_path, content)`
    /// pairs.
    fn create_mock_epub(files: &[(&str, &str)]) -> Cursor<Vec<u8>> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

            for (path, content) in files {
                zip.start_file(*path, options)
                    .expect("failed to add mock EPUB file");
                zip.write_all(content.as_bytes())
                    .expect("failed to write mock EPUB file");
            }
            zip.finish().expect("failed to finish mock EPUB archive");
        }

        buf.set_position(0);
        buf
    }

    #[test]
    fn test_valid_epub_spine_extraction() {
        let container_xml = r#"<?xml version="1.0"?>
            <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
                <rootfiles>
                    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
                </rootfiles>
            </container>"#;

        let opf_xml = r#"<?xml version="1.0"?>
            <package version="2.0" xmlns="http://www.idpf.org/2007/opf">
                <manifest>
                    <item id="ch1" href="text/chapter1.xhtml" media-type="application/xhtml+xml"/>
                    <item id="ch2" href="text/chapter2.xhtml" media-type="application/xhtml+xml"/>
                    <item id="css" href="styles.css" media-type="text/css"/>
                </manifest>
                <spine>
                    <itemref idref="ch1"/>
                    <itemref idref="ch2"/>
                </spine>
            </package>"#;

        let ch1_content = "<html><body>Chapter 1</body></html>";
        let ch2_content = "<html><body>Chapter 2</body></html>";

        let zip_buf = create_mock_epub(&[
            ("META-INF/container.xml", container_xml),
            ("OEBPS/content.opf", opf_xml),
            ("OEBPS/text/chapter1.xhtml", ch1_content),
            ("OEBPS/text/chapter2.xhtml", ch2_content),
            ("OEBPS/styles.css", "body { color: black; }"),
        ]);

        let mut archive = EpubArchive::new(zip_buf).expect("Failed to open valid zip");
        let spine = archive.get_spine_xhtml().expect("Failed to extract spine");

        assert_eq!(spine.len(), 2);

        assert_eq!(spine[0].0, "OEBPS/text/chapter1.xhtml");
        assert_eq!(spine[0].1, ch1_content.as_bytes());

        assert_eq!(spine[1].0, "OEBPS/text/chapter2.xhtml");
        assert_eq!(spine[1].1, ch2_content.as_bytes());
    }

    #[test]
    fn test_missing_container_xml() {
        let zip_buf = create_mock_epub(&[("OEBPS/content.opf", "<package></package>")]);

        let mut archive = EpubArchive::new(zip_buf).expect("failed to open test ZIP");
        let result = archive.get_spine_xhtml();

        assert!(matches!(result, Err(KepubError::Zip(_))));
    }

    #[test]
    fn test_invalid_container_xml() {
        let bad_container = r#"<?xml version="1.0"?>
            <container version="1.0">
                <rootfiles>
                    <!-- Missing full-path attribute -->
                    <rootfile media-type="application/oebps-package+xml"/>
                </rootfiles>
            </container>"#;

        let zip_buf = create_mock_epub(&[("META-INF/container.xml", bad_container)]);

        let mut archive = EpubArchive::new(zip_buf).expect("failed to open test ZIP");
        let result = archive.get_spine_xhtml();

        assert!(
            matches!(result, Err(KepubError::InvalidEpub(msg)) if msg.contains("missing 'full-path'"))
        );
    }

    #[test]
    fn test_missing_spine_files() {
        let container_xml =
            r#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#;
        let opf_xml = r#"
            <package>
                <manifest>
                    <item id="ch1" href="missing.xhtml" media-type="application/xhtml+xml"/>
                </manifest>
                <spine><itemref idref="ch1"/></spine>
            </package>"#;

        let zip_buf = create_mock_epub(&[
            ("META-INF/container.xml", container_xml),
            ("content.opf", opf_xml),
        ]);

        let mut archive = EpubArchive::new(zip_buf).expect("failed to open test ZIP");
        let result = archive.get_spine_xhtml();

        assert!(result.is_err());
    }

    #[test]
    fn write_kepub_copies_unreplaced_files_and_applies_replacements() {
        let zip_buf = create_mock_epub(&[
            ("mimetype", "application/epub+zip"),
            ("META-INF/container.xml", "<container/>"),
            ("OEBPS/text/chapter1.xhtml", "<html>original</html>"),
            ("OEBPS/styles.css", "body{}"),
        ]);
        let mut archive = EpubArchive::new(zip_buf).expect("failed to open test ZIP");

        let mut replacements = HashMap::new();
        replacements.insert(
            "OEBPS/text/chapter1.xhtml".to_string(),
            b"<html>replaced</html>".to_vec(),
        );

        let mut out = Cursor::new(Vec::new());
        archive
            .write_kepub(&mut out, replacements, HashMap::new())
            .expect("write_kepub should succeed");

        out.set_position(0);
        let mut result = ZipArchive::new(out).expect("output should be a valid zip");

        let mut chapter = String::new();
        result
            .by_name("OEBPS/text/chapter1.xhtml")
            .expect("chapter1 should exist in output")
            .read_to_string(&mut chapter)
            .expect("chapter1 should contain valid UTF-8 text");
        assert_eq!(chapter, "<html>replaced</html>");

        let mut css = String::new();
        result
            .by_name("OEBPS/styles.css")
            .expect("styles.css should exist, copied unchanged")
            .read_to_string(&mut css)
            .expect("styles.css should contain valid UTF-8 text");
        assert_eq!(css, "body{}");
    }

    #[test]
    fn write_kepub_writes_mimetype_first_and_stored() {
        let zip_buf = create_mock_epub(&[
            ("META-INF/container.xml", "<container/>"),
            ("mimetype", "application/epub+zip"),
        ]);
        let mut archive = EpubArchive::new(zip_buf).expect("failed to open test ZIP");

        let mut out = Cursor::new(Vec::new());
        archive
            .write_kepub(&mut out, HashMap::new(), HashMap::new())
            .expect("write_kepub should succeed");

        out.set_position(0);
        let mut result = ZipArchive::new(out).expect("output should be a valid zip");
        let first_entry = result
            .by_index(0)
            .expect("output should have at least one entry");
        assert_eq!(first_entry.name(), "mimetype");
        assert_eq!(first_entry.compression(), zip::CompressionMethod::Stored);
    }

    #[test]
    fn write_kepub_appends_additions_as_new_files() {
        let zip_buf = create_mock_epub(&[
            ("mimetype", "application/epub+zip"),
            ("OEBPS/content.opf", "<package/>"),
        ]);
        let mut archive = EpubArchive::new(zip_buf).expect("failed to open test ZIP");

        let mut additions = HashMap::new();
        additions.insert("css/kobo.css".to_string(), b"body{}".to_vec());

        let mut out = Cursor::new(Vec::new());
        archive
            .write_kepub(&mut out, HashMap::new(), additions)
            .expect("write_kepub should succeed");

        out.set_position(0);
        let mut result = ZipArchive::new(out).expect("output should be a valid zip");
        let mut css = String::new();
        result
            .by_name("css/kobo.css")
            .expect("the added file should exist in the output")
            .read_to_string(&mut css)
            .expect("the added CSS file should contain valid UTF-8 text");
        assert_eq!(css, "body{}");
    }

    #[test]
    fn write_kepub_rejects_replacement_for_unknown_path() {
        let zip_buf = create_mock_epub(&[("mimetype", "application/epub+zip")]);
        let mut archive = EpubArchive::new(zip_buf).expect("failed to open test ZIP");

        let mut replacements = HashMap::new();
        replacements.insert("does/not/exist.xhtml".to_string(), b"x".to_vec());

        let mut out = Cursor::new(Vec::new());
        let result = archive.write_kepub(&mut out, replacements, HashMap::new());
        assert!(matches!(result, Err(KepubError::InvalidEpub(_))));
    }
}
