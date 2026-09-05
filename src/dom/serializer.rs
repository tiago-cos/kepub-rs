//! Serializes a [`DocumentArena`] back into XHTML text.
//!
//! Deliberately not a general-purpose XML/HTML serializer — it only has to
//! round-trip what our own parser ([`crate::dom::sink`]) can produce and
//! what [`crate::convert::transform`] inserts, and it has to produce output
//! that's simultaneously valid as HTML5 and as well-formed XML ("polyglot"
//! markup).

use indextree::{Node, NodeId};
use markup5ever::{Attribute, Namespace, QualName, ns};

use crate::dom::arena::{DocumentArena, NodeData};
use crate::error::KepubError;

/// HTML5's void elements — the only tags where self-closing syntax
/// (`<tag/>`) parses identically under both XML and HTML5 rules. Every
/// other element must get an explicit closing tag, even if empty.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Serializes `doc` to a UTF-8 XHTML string, prefixed with a canonical XML
/// declaration.
///
/// The result is plain text; convert with `.into_bytes()` when writing it
/// into the output zip.
///
/// # Errors
///
/// Returns [`KepubError::Serialize`] if the document contains content that
/// cannot be represented as valid XML, specifically:
/// - a comment whose text contains `"--"` or ends with `-` (disallowed by
///   XML's comment grammar),
/// - a void element (e.g. `<img>`) that unexpectedly has children, or
/// - text or attribute content containing a character XML 1.0 disallows
///   (most C0 control characters other than tab, LF, and CR).
pub fn serialize(doc: &DocumentArena) -> Result<String, KepubError> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");

    let children: Vec<NodeId> = {
        let arena = doc.arena.borrow();
        doc.root.children(&arena).collect()
    };
    for child in children {
        if is_xml_declaration(doc, child) {
            continue;
        }

        write_node(doc, child, &mut out, &ns!())?;
    }

    Ok(out)
}

/// Returns whether `node` is the source document's own `<?xml ... ?>`
/// declaration, so [`serialize`] can skip it (it emits its own canonical
/// declaration instead of copying the source's).
fn is_xml_declaration(doc: &DocumentArena, node: NodeId) -> bool {
    let arena = doc.arena.borrow();
    matches!(
        arena.get(node).map(Node::get),
        Some(NodeData::ProcessingInstruction { target, .. }) if target.eq_ignore_ascii_case("xml")
    )
}

/// Serializes a single node (and, for elements, its subtree) into `out`.
///
/// `parent_ns` is the namespace URI in scope from the enclosing element,
/// used by [`write_element`] to decide whether a namespace declaration
/// needs to be (re)emitted.
///
/// # Errors
///
/// Returns [`KepubError::Serialize`] under the same conditions as
/// [`serialize`].
///
/// # Panics
///
/// Panics if `node` is not present in `doc`'s arena.
fn write_node(
    doc: &DocumentArena,
    node: NodeId,
    out: &mut String,
    parent_ns: &Namespace,
) -> Result<(), KepubError> {
    let node_data = {
        let arena = doc.arena.borrow();
        arena
            .get(node)
            .expect("NodeId not found in its own arena")
            .get()
            .clone()
    };

    match node_data {
        NodeData::Document => unreachable!("a Document node cannot appear below the root"),
        NodeData::Doctype {
            name,
            public_id,
            system_id,
        } => write_doctype(&name, &public_id, &system_id, out),
        NodeData::Text(text) => write_text(&text, out)?,
        NodeData::Comment(text) => write_comment(&text, out)?,
        NodeData::ProcessingInstruction { target, data } => write_pi(&target, &data, out),
        NodeData::Element { name, attrs } => {
            write_element(doc, node, &name, &attrs, out, parent_ns)?;
        }
    }

    Ok(())
}

/// Writes a `<!DOCTYPE ...>` declaration for `name`, `public_id`, and
/// `system_id` into `out`.
///
/// Renders a bare `<!DOCTYPE name>` when both IDs are empty, a `PUBLIC`
/// form when `public_id` is set, or a `SYSTEM` form when only `system_id`
/// is set.
fn write_doctype(name: &str, public_id: &str, system_id: &str, out: &mut String) {
    out.push_str("<!DOCTYPE ");
    out.push_str(name);
    if !public_id.is_empty() {
        out.push_str(" PUBLIC \"");
        out.push_str(public_id);
        out.push('"');
        if !system_id.is_empty() {
            out.push_str(" \"");
            out.push_str(system_id);
            out.push('"');
        }
    } else if !system_id.is_empty() {
        out.push_str(" SYSTEM \"");
        out.push_str(system_id);
        out.push('"');
    }
    out.push_str(">\n");
}

/// Writes a `<?target data?>` processing instruction into `out`.
fn write_pi(target: &str, data: &str, out: &mut String) {
    out.push_str("<?");
    out.push_str(target);
    if !data.is_empty() {
        out.push(' ');
        out.push_str(data);
    }
    out.push_str("?>");
}

/// Writes an XML comment (`<!--...-->`) into `out`.
///
/// # Errors
///
/// Returns [`KepubError::Serialize`] if `text` contains `"--"` or ends
/// with `-`, both of which XML disallows in comment content. Failing here
/// keeps the problem visible at the source instead of surfacing as a
/// mysterious parse failure on whatever reads the file later.
fn write_comment(text: &str, out: &mut String) -> Result<(), KepubError> {
    if text.contains("--") || text.ends_with('-') {
        return Err(KepubError::Serialize(format!(
            "comment {text:?} contains \"--\" or ends with \"-\", which XML disallows in comments"
        )));
    }
    out.push_str("<!--");
    out.push_str(text);
    out.push_str("-->");
    Ok(())
}

/// Writes an element and its subtree into `out`, reconstructing any
/// namespace declarations its tag or attributes require.
///
/// Emits an `xmlns="..."` declaration when `name`'s namespace differs from
/// `parent_ns`, and an `xmlns:prefix="..."` binding for any attribute prefix
/// (e.g. `epub:type`) not already spelled out in `attrs`. Recurses into
/// children with `name.ns` as their new `parent_ns`.
///
/// # Errors
///
/// Returns [`KepubError::Serialize`] if:
/// - this is a void element (per [`VOID_ELEMENTS`]) that unexpectedly has
///   children — XML itself has no notion of void elements, so nothing
///   prevents a caller from constructing this shape, but no correctly
///   authored document should have one, or
/// - any attribute or namespace value contains a character XML 1.0
///   disallows.
fn write_element(
    doc: &DocumentArena,
    node: NodeId,
    name: &QualName,
    attrs: &[Attribute],
    out: &mut String,
    parent_ns: &Namespace,
) -> Result<(), KepubError> {
    let tag = qualified_name(name);

    out.push('<');
    out.push_str(&tag);

    let declares_default = attrs
        .iter()
        .any(|a| a.name.prefix.is_none() && a.name.local.as_ref() == "xmlns");
    if !declares_default && name.prefix.is_none() && name.ns != *parent_ns {
        out.push_str(" xmlns=\"");
        write_attr_value(&name.ns, out)?;
        out.push('"');
    }

    let mut declared: Vec<&str> = Vec::new();
    for a in attrs {
        if a.name.prefix.is_none() && a.name.local.as_ref() == "xmlns" {
            continue;
        }
        let Some(prefix) = a.name.prefix.as_ref() else {
            continue;
        };
        let prefix = prefix.as_ref();
        if prefix == "xmlns" || prefix == "xml" || a.name.ns.is_empty() {
            continue;
        }
        let already_spelled_out = attrs.iter().any(|other| {
            other.name.prefix.as_ref().map(AsRef::as_ref) == Some("xmlns")
                && other.name.local.as_ref() == prefix
        });
        if already_spelled_out || declared.contains(&prefix) {
            continue;
        }
        declared.push(prefix);
        out.push_str(" xmlns:");
        out.push_str(prefix);
        out.push_str("=\"");
        write_attr_value(&a.name.ns, out)?;
        out.push('"');
    }

    for a in attrs {
        out.push(' ');
        out.push_str(&qualified_name(&a.name));
        out.push_str("=\"");
        write_attr_value(&a.value, out)?;
        out.push('"');
    }

    let children: Vec<NodeId> = {
        let arena = doc.arena.borrow();
        node.children(&arena).collect()
    };

    let is_void = name.prefix.is_none() && VOID_ELEMENTS.contains(&name.local.as_ref());

    if is_void {
        if !children.is_empty() {
            return Err(KepubError::Serialize(format!(
                "void element <{tag}> unexpectedly has children"
            )));
        }
        out.push_str("/>");
    } else {
        out.push('>');
        for child in children {
            write_node(doc, child, out, &name.ns)?;
        }
        out.push_str("</");
        out.push_str(&tag);
        out.push('>');
    }

    Ok(())
}

/// Formats `name` as `prefix:local`, or just `local` when there's no
/// prefix.
fn qualified_name(name: &QualName) -> String {
    name.prefix.as_ref().map_or_else(
        || name.local.to_string(),
        |prefix| format!("{}:{}", prefix.as_ref(), name.local.as_ref()),
    )
}

/// Writes `text` into `out` as escaped XML text content (`&`, `<`, and `>`
/// are escaped; `>` is escaped defensively though not strictly required by
/// XML).
///
/// # Errors
///
/// Returns [`KepubError::Serialize`] if `text` contains a character XML
/// 1.0 disallows (see [`is_disallowed_xml_char`]).
fn write_text(text: &str, out: &mut String) -> Result<(), KepubError> {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if is_disallowed_xml_char(c) => return Err(control_char_error(c)),
            _ => out.push(c),
        }
    }
    Ok(())
}

/// Writes `value` into `out` as an escaped, double-quoted XML attribute
/// value (`&`, `<`, and `"` are escaped).
///
/// # Errors
///
/// Returns [`KepubError::Serialize`] if `value` contains a character XML
/// 1.0 disallows (see [`is_disallowed_xml_char`]).
fn write_attr_value(value: &str, out: &mut String) -> Result<(), KepubError> {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            c if is_disallowed_xml_char(c) => return Err(control_char_error(c)),
            _ => out.push(c),
        }
    }
    Ok(())
}

/// Returns whether `c` is a character XML 1.0's `Char` production
/// disallows.
///
/// This excludes most C0 control characters (tab, LF, and CR are the
/// exceptions XML permits). Well-formed source shouldn't contain these —
/// this check exists to fail loudly rather than silently emit invalid XML
/// if one ever ends up in a string assembled at runtime (an attribute
/// value built from user data, say) rather than one that came straight
/// from parsing.
const fn is_disallowed_xml_char(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}')
}

/// Builds the [`KepubError::Serialize`] error for a disallowed XML
/// character `c`.
fn control_char_error(c: char) -> KepubError {
    KepubError::Serialize(format!(
        "content contains U+{:04X}, a character XML 1.0 disallows",
        c as u32
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> DocumentArena {
        crate::dom::parse(xml.as_bytes()).expect("failed to parse test XML")
    }

    #[test]
    fn void_element_self_closes() {
        let doc = parse(r#"<html><body><img src="a.png"/></body></html>"#);
        let out = serialize(&doc).expect("serialize should succeed");
        assert!(out.contains(r#"<img src="a.png"/>"#), "got: {out}");
        assert!(!out.contains("</img>"), "got: {out}");
    }

    #[test]
    fn empty_non_void_element_gets_explicit_close_tag() {
        let doc = parse("<html><body><div></div></body></html>");
        let out = serialize(&doc).expect("serialize should succeed");
        assert!(out.contains("<div></div>"), "got: {out}");
        assert!(!out.contains("<div/>"), "got: {out}");
    }

    #[test]
    fn text_content_is_escaped() {
        let doc = parse("<html><body><p>A &amp; B &lt;3</p></body></html>");
        let out = serialize(&doc).expect("serialize should succeed");
        assert!(out.contains("A &amp; B &lt;3"), "got: {out}");
    }

    #[test]
    fn attribute_values_are_escaped() {
        let doc = parse(r#"<html><body><p title="a &quot;b&quot; c &amp; d">x</p></body></html>"#);
        let out = serialize(&doc).expect("serialize should succeed");
        assert!(
            out.contains(r#"title="a &quot;b&quot; c &amp; d""#),
            "got: {out}"
        );
    }

    #[test]
    fn namespaced_attribute_preserves_prefix() {
        let doc = parse(
            r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><p epub:type="pagebreak">x</p></body></html>"#,
        );
        let out = serialize(&doc).expect("serialize should succeed");
        assert!(out.contains(r#"epub:type="pagebreak""#), "got: {out}");
        assert!(
            out.contains(r#"xmlns:epub="http://www.idpf.org/2007/ops""#),
            "got: {out}"
        );
    }

    #[test]
    fn comment_with_double_hyphen_is_rejected() {
        let doc = DocumentArena::new();
        let comment = doc.new_node(NodeData::Comment("a -- b".to_string()));
        doc.append_child(doc.root, comment);

        let result = serialize(&doc);
        assert!(matches!(result, Err(KepubError::Serialize(_))));
    }

    #[test]
    fn void_element_with_children_is_an_error() {
        let doc = parse(r#"<html><body><img src="a.png">oops</img></body></html>"#);
        let result = serialize(&doc);
        assert!(matches!(result, Err(KepubError::Serialize(_))));
    }

    #[test]
    fn doctype_with_no_public_or_system_id_is_rendered_plainly() {
        let doc = DocumentArena::new();
        let doctype = doc.new_node(NodeData::Doctype {
            name: "html".to_string(),
            public_id: String::new(),
            system_id: String::new(),
        });
        doc.append_child(doc.root, doctype);

        let out = serialize(&doc).expect("serialize should succeed");
        assert!(out.contains("<!DOCTYPE html>"), "got: {out}");
    }

    #[test]
    fn root_element_keeps_its_default_namespace() {
        let doc = parse(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head></head><body><p>hi</p></body></html>"#,
        );
        let out = serialize(&doc).expect("serialize should succeed");
        assert!(
            out.contains(r#"<html xmlns="http://www.w3.org/1999/xhtml""#),
            "got: {out}"
        );
    }

    #[test]
    fn descendants_inherit_rather_than_redeclaring() {
        let doc = parse(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head></head><body><p>hi</p></body></html>"#,
        );
        let out = serialize(&doc).expect("serialize should succeed");
        assert_eq!(
            out.matches(r#"xmlns="http://www.w3.org/1999/xhtml""#)
                .count(),
            1,
            "got: {out}"
        );
    }

    #[test]
    fn an_element_in_a_different_namespace_declares_its_own() {
        let doc = parse(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><svg xmlns="http://www.w3.org/2000/svg"><circle/></svg></body></html>"#,
        );
        let out = serialize(&doc).expect("serialize should succeed");
        assert!(
            out.contains(r#"xmlns="http://www.w3.org/2000/svg""#),
            "got: {out}"
        );
        assert_eq!(
            out.matches(r#"xmlns="http://www.w3.org/2000/svg""#).count(),
            1,
            "got: {out}"
        );
    }

    #[test]
    fn transformed_document_still_carries_the_namespace() {
        let doc = parse(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head></head><body><p>Hello world</p></body></html>"#,
        );
        crate::convert::transform::Transform::default()
            .apply(&doc)
            .expect("transform should succeed");
        let out = serialize(&doc).expect("serialize should succeed");

        assert!(out.contains("koboSpan"), "sanity: spans were injected");
        assert!(
            out.contains(r#"xmlns="http://www.w3.org/1999/xhtml""#),
            "the namespace must survive transformation, got: {out}"
        );
    }

    #[test]
    fn source_xml_declaration_is_not_duplicated() {
        let doc = parse(r#"<?xml version="1.0" encoding="UTF-8"?><html><body/></html>"#);
        let out = serialize(&doc).expect("serialize should succeed");

        let count = out.matches("<?xml").count();
        assert_eq!(count, 1, "expected exactly one XML declaration, got: {out}");
        assert!(
            out.starts_with(r#"<?xml version="1.0" encoding="utf-8"?>"#),
            "got: {out}"
        );
    }
}
