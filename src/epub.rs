use std::collections::HashMap;
use std::io::{Read, Seek};
use zip::ZipArchive;

use crate::error::KepubError;

pub struct EpubArchive<R: Read + Seek> {
    archive: ZipArchive<R>,
}

impl<R: Read + Seek> EpubArchive<R> {
    /// Initialize the EPUB archive without extracting files yet.
    pub fn new(reader: R) -> Result<Self, KepubError> {
        let archive = ZipArchive::new(reader)?;
        Ok(Self { archive })
    }

    /// Reads a specific file from the ZIP archive into a String.
    fn read_file_as_string(&mut self, path: &str) -> Result<String, KepubError> {
        let mut file = self.archive.by_name(path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Ok(content)
    }

    /// Reads a specific file from the ZIP archive into a raw byte vector.
    fn read_file_as_bytes(&mut self, path: &str) -> Result<Vec<u8>, KepubError> {
        let mut file = self.archive.by_name(path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        Ok(content)
    }

    /// Finds the path to the .opf rootfile by parsing META-INF/container.xml
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

    /// Extracts all XHTML documents sequentially as defined by the EPUB spine.
    /// Returns a Vec of tuples: (``file_path``, ``raw_file_bytes``)
    pub fn get_spine_xhtml(&mut self) -> Result<Vec<(String, Vec<u8>)>, KepubError> {
        let opf_path = self.get_rootfile_path()?;

        let base_dir = opf_path
            .rsplit_once('/')
            .map(|(dir, _)| format!("{dir}/"))
            .unwrap_or_default();

        let opf_xml = self.read_file_as_string(&opf_path)?;
        let doc = roxmltree::Document::parse(&opf_xml)?;

        // 1. Parse the Manifest (maps id -> relative href)
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

        // 2. Parse the Spine to get the reading order
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    /// Helper function to create an in-memory ZIP archive from a list of (`file_path`, content)
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
}
