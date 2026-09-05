//! Generic XHTML machinery: an arena-backed DOM, the xml5ever `TreeSink`
//! that populates it, a polyglot XHTML serializer, and the small set of
//! queries both features need.
//!
//! Nothing in here knows anything about Kobo, kepubs, or koboSpans. The
//! Kobo-specific work lives in [`crate::convert`] (transforming a document)
//! and [`crate::location`] (addressing a position in one), both of which
//! build on this.

pub mod arena;
pub mod serializer;
mod sink;

use indextree::{Node, NodeId};
use xml5ever::driver::{XmlParseOpts, parse_document};
use xml5ever::tendril::TendrilSink;

use crate::error::KepubError;
use arena::{DocumentArena, NodeData};

/// Parses XHTML bytes into a fresh [`DocumentArena`].
///
/// Every caller needs the same xml5ever incantation, so it lives here once
/// rather than being repeated at each call site.
///
/// # Errors
///
/// Returns [`KepubError::XmlParse`] if `bytes` is not well-formed XML.
pub fn parse(bytes: &[u8]) -> Result<DocumentArena, KepubError> {
    let mut reader = bytes;
    let doc = parse_document(DocumentArena::new(), XmlParseOpts::default())
        .from_utf8()
        .read_from(&mut reader)?;
    Ok(doc)
}

/// Depth-first search for the first element with the given local name.
///
/// Callers that care about scope should pass a specific `start` rather
/// than the document root — searching from the root finds the first match
/// in document order, which for `script` or `style` may well be one this
/// crate injected into `<head>` rather than the one in the body.
pub fn find_element(doc: &DocumentArena, start: NodeId, local_name: &str) -> Option<NodeId> {
    let is_match = {
        let arena = doc.arena.borrow();
        matches!(
            arena.get(start).map(Node::get),
            Some(NodeData::Element { name, .. }) if name.local.as_ref() == local_name
        )
    };
    if is_match {
        return Some(start);
    }
    let children: Vec<NodeId> = {
        let arena = doc.arena.borrow();
        start.children(&arena).collect()
    };
    for child in children {
        if let Some(found) = find_element(doc, child, local_name) {
            return Some(found);
        }
    }
    None
}

/// Finds `doc`'s `<head>` element.
///
/// # Errors
///
/// Returns [`KepubError::MissingElement`] if `doc` has no `<head>`
/// element.
pub fn find_head(doc: &DocumentArena) -> Result<NodeId, KepubError> {
    find_element(doc, doc.root, "head").ok_or(KepubError::MissingElement("head"))
}

/// Finds `doc`'s `<body>` element.
///
/// # Errors
///
/// Returns [`KepubError::MissingElement`] if `doc` has no `<body>`
/// element.
pub fn find_body(doc: &DocumentArena) -> Result<NodeId, KepubError> {
    find_element(doc, doc.root, "body").ok_or(KepubError::MissingElement("body"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_name_of(doc: &DocumentArena, node: NodeId) -> String {
        let arena = doc.arena.borrow();
        match arena.get(node).map(Node::get) {
            Some(NodeData::Element { name, .. }) => name.local.as_ref().to_string(),
            other => panic!("expected an element, found {other:?}"),
        }
    }

    #[test]
    fn parses_well_formed_xhtml() {
        let doc = parse(b"<html><head></head><body><p>hi</p></body></html>").expect("should parse");
        assert!(find_body(&doc).is_ok());
    }

    #[test]
    fn find_element_locates_nested_target() {
        let doc =
            parse(b"<html><head><title>T</title></head><body><div><p>x</p></div></body></html>")
                .expect("should parse");
        let p = find_element(&doc, doc.root, "p").expect("should find nested p");
        assert_eq!(local_name_of(&doc, p), "p");
    }

    #[test]
    fn find_element_returns_none_when_absent() {
        let doc = parse(b"<html><head></head><body></body></html>").expect("should parse");
        assert!(find_element(&doc, doc.root, "nonexistent").is_none());
    }

    #[test]
    fn missing_body_and_head_are_reported() {
        let doc = parse(b"<html><head></head></html>").expect("should parse");
        assert!(matches!(
            find_body(&doc),
            Err(KepubError::MissingElement("body"))
        ));
        let doc = parse(b"<html><body></body></html>").expect("should parse");
        assert!(matches!(
            find_head(&doc),
            Err(KepubError::MissingElement("head"))
        ));
    }
}
