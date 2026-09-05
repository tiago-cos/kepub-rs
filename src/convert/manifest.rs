//! Registering the injected assets in the OPF package document's
//! `<manifest>`.
//!
//! This edits the OPF as text rather than parsing and reserializing it.

use std::fmt::Write;

use crate::error::KepubError;

const MANIFEST_CLOSE: &str = "</manifest>";

/// One `<item>` to add to the manifest.
pub struct ManifestItem<'a> {
    /// The item's `id` attribute. May be suffixed by
    /// [`add_manifest_items`] if it collides with an id already present.
    pub id: &'a str,
    /// The item's `href` attribute, relative to the OPF's own directory,
    /// not the archive root.
    pub href: &'a str,
    /// The item's `media-type` attribute.
    pub media_type: &'a str,
}

/// Inserts `items` immediately before the manifest's closing tag.
///
/// Items whose `href` is already present are skipped, so re-converting an
/// already-converted book doesn't accumulate duplicates. An `id` that
/// collides with one already in the document gets a numeric suffix rather
/// than producing an OPF with duplicate ids.
///
/// # Errors
///
/// Returns [`KepubError::InvalidEpub`] if `opf` has no `</manifest>` to
/// insert items before.
pub fn add_manifest_items(opf: &str, items: &[ManifestItem<'_>]) -> Result<String, KepubError> {
    let close_at = opf.rfind(MANIFEST_CLOSE).ok_or_else(|| {
        KepubError::InvalidEpub(
            "the OPF package document has no closing </manifest> tag to insert items before".into(),
        )
    })?;

    let mut insertion = String::new();
    for item in items {
        if opf.contains(&format!("href=\"{}\"", item.href)) {
            continue;
        }
        let id = unique_id(opf, &insertion, item.id);
        let _ = write!(
            insertion,
            r#"<item id="{}" href="{}" media-type="{}"/>"#,
            id, item.href, item.media_type
        );
    }

    if insertion.is_empty() {
        return Ok(opf.to_string());
    }

    let mut out = String::with_capacity(opf.len() + insertion.len());
    out.push_str(&opf[..close_at]);
    out.push_str(&insertion);
    out.push_str(&opf[close_at..]);
    Ok(out)
}

/// Finds an id not already used in `opf`, nor in `pending` (the items
/// being inserted alongside this one in the same call).
fn unique_id(opf: &str, pending: &str, preferred: &str) -> String {
    let taken = |candidate: &str| {
        let needle = format!("id=\"{candidate}\"");
        opf.contains(&needle) || pending.contains(&needle)
    };

    if !taken(preferred) {
        return preferred.to_string();
    }
    for n in 1u32.. {
        let candidate = format!("{preferred}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("the loop returns as soon as an unused id is found")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPF: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf">
<manifest>
<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
<item id="id001" href="001.html" media-type="application/xhtml+xml"/>
</manifest>
<spine toc="ncx"><itemref idref="id001"/></spine>
</package>"#;

    fn css_and_js<'a>() -> Vec<ManifestItem<'a>> {
        vec![
            ManifestItem {
                id: "js-kobo.js",
                href: "../js/kobo.js",
                media_type: "application/javascript",
            },
            ManifestItem {
                id: "css-kobo.css",
                href: "../css/kobo.css",
                media_type: "text/css",
            },
        ]
    }

    #[test]
    fn inserts_items_before_the_closing_tag() {
        let out = add_manifest_items(OPF, &css_and_js()).expect("should insert");

        assert!(out.contains(
            r#"<item id="js-kobo.js" href="../js/kobo.js" media-type="application/javascript"/>"#
        ));
        assert!(
            out.contains(
                r#"<item id="css-kobo.css" href="../css/kobo.css" media-type="text/css"/>"#
            )
        );

        let close = out
            .find("</manifest>")
            .expect("manifest closing tag should be present in output");
        let js_idx = out
            .find("js-kobo.js")
            .expect("js item should be present in output");
        let css_idx = out
            .find("css-kobo.css")
            .expect("css item should be present in output");

        assert!(js_idx < close);
        assert!(css_idx < close);
    }

    #[test]
    fn leaves_the_rest_of_the_document_untouched() {
        let out = add_manifest_items(OPF, &css_and_js()).expect("should insert");

        assert!(out.starts_with(r#"<?xml version="1.0"?>"#));
        assert!(out.contains(r#"<item id="ncx" href="toc.ncx""#));
        assert!(out.contains(r#"<spine toc="ncx"><itemref idref="id001"/></spine>"#));
        assert!(out.ends_with("</package>"));
    }

    #[test]
    fn is_idempotent() {
        let once = add_manifest_items(OPF, &css_and_js()).expect("first pass");
        let twice = add_manifest_items(&once, &css_and_js()).expect("second pass");
        assert_eq!(once, twice, "re-converting shouldn't duplicate items");
    }

    #[test]
    fn avoids_colliding_with_an_existing_id() {
        let opf = OPF.replace(r#"id="ncx""#, r#"id="css-kobo.css""#);
        let out = add_manifest_items(&opf, &css_and_js()).expect("should insert");

        assert!(
            out.contains(r#"<item id="css-kobo.css-1" href="../css/kobo.css""#),
            "expected a suffixed id, got: {out}"
        );
    }

    #[test]
    fn reports_a_missing_manifest() {
        let err = add_manifest_items("<package></package>", &css_and_js())
            .expect_err("should fail when manifest closing tag is missing");
        assert!(matches!(err, KepubError::InvalidEpub(m) if m.contains("</manifest>")));
    }
}
