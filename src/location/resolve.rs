//! Pure DOM-level translation between a finished kepub's koboSpan
//! structure and a simple structural location (element indices + text run
//! index + character offset). No zip I/O, no string parsing.
//!
//! A finished kepub is self-describing enough that this needs nothing
//! captured during conversion: a run of consecutive sibling `kobo.N.*`
//! spans sharing the same `N` reconstructs the one original text node they
//! were split from, and a span wrapping a single non-text element
//! (`img`/`svg`) is transparent — it stands in for that element directly,
//! not an extra nesting level. [`logical_children`] builds that
//! reconstructed view of a level's children on demand; everything else
//! here works against that view rather than the kepub's literal shape.
//!
//! Indices are plain and 0-based: element indices count only elements,
//! the text index counts only text runs, and they're separate sequences.
//!
//! Character offsets are in Unicode scalar values (Rust `char`s), not
//! bytes and not UTF-16 code units — unambiguous across languages, with no
//! surrogate-pair arithmetic and no chance of landing mid-character.

use indextree::{Node, NodeId};
use markup5ever::{Attribute, QualName};

use crate::dom::arena::{DocumentArena, NodeData};
use crate::dom::find_body;
use crate::error::KepubError;

/// One position among a level's *reconstructed* original children, after
/// collapsing consecutive same-`para` koboSpan runs back into the single
/// text node or wrapped element they represent.
#[derive(Debug, Clone)]
enum LogicalChild {
    /// One or more consecutive `kobo.N.*` sibling spans sharing the same
    /// `N`, standing in for the single original text node they were split
    /// from. `spans` is in seg order (document order).
    TextGroup { para: u32, spans: Vec<NodeId> },
    /// A single `kobo.N.1` span wrapping exactly one non-text element
    /// (`img`/`svg`), standing in for that element directly.
    WrappedElement { span: NodeId, inner: NodeId },
    /// Any other element, unchanged.
    Element(NodeId),
    /// A bare text node not wrapped in any span — shouldn't happen in a
    /// well-converted kepub, but handled rather than panicking.
    Text(NodeId),
}

impl LogicalChild {
    /// Returns whether this child occupies a slot in the element-index
    /// sequence (as opposed to the text-run sequence).
    const fn is_element_kind(&self) -> bool {
        matches!(self, Self::Element(_) | Self::WrappedElement { .. })
    }

    /// Returns whether this child occupies a slot in the text-run-index
    /// sequence (as opposed to the element sequence).
    const fn is_text_kind(&self) -> bool {
        matches!(self, Self::TextGroup { .. } | Self::Text(_))
    }

    /// The node to descend into for an element-kind child: a wrapped
    /// element's koboSpan is transparent, so descending goes straight to
    /// the element it wraps.
    const fn descend_target(&self) -> Option<NodeId> {
        match self {
            Self::Element(id) => Some(*id),
            Self::WrappedElement { inner, .. } => Some(*inner),
            _ => None,
        }
    }
}

/// Translates a kobo location (which span, plus a character offset into
/// that span's own text) into a structural location: the element index
/// path from `<body>`, the text run index within the final element, and a
/// character offset into the *reconstructed original* text node.
///
/// # Errors
///
/// Returns [`KepubError::SpanNotFound`] if no `kobo.{para}.{seg}` span
/// exists, [`KepubError::InvalidSpanOffset`] if `offset_in_span` exceeds
/// the span's own text length, [`KepubError::LocationIsElement`] if the
/// span wraps an element (an image or SVG) rather than text, or
/// [`KepubError::MissingElement`]/[`KepubError::InvalidEpub`] if `doc`'s
/// structure can't be walked as expected.
pub fn kobo_to_location(
    doc: &DocumentArena,
    para: u32,
    seg: u32,
    offset_in_span: usize,
) -> Result<(Vec<usize>, usize, usize), KepubError> {
    let span_id = format!("kobo.{para}.{seg}");
    let target_span =
        find_span_by_id(doc, &span_id).ok_or_else(|| KepubError::SpanNotFound(span_id.clone()))?;

    let span_text = span_text_content(doc, target_span);
    let span_len = span_text.chars().count();
    if offset_in_span > span_len {
        return Err(KepubError::InvalidSpanOffset {
            id: span_id,
            offset: offset_in_span,
            len: span_len,
        });
    }

    let root = body_content_root(doc)?;

    let mut chain = vec![target_span];
    loop {
        let current = *chain.last().expect("chain is never empty");
        if current == root {
            break;
        }
        let parent = {
            let arena = doc.arena.borrow();
            arena.get(current).and_then(Node::parent)
        }
        .ok_or_else(|| {
            KepubError::InvalidEpub(format!(
                "reached the document root while walking up from koboSpan \"{span_id}\" \
                 without finding the expected body content container"
            ))
        })?;
        chain.push(parent);
    }
    chain.pop();
    chain.reverse();

    let mut element_path = Vec::new();
    let mut current_parent = root;

    for &next in chain.iter().take(chain.len().saturating_sub(1)) {
        let logical = logical_children(doc, current_parent);
        let index = logical
            .iter()
            .filter(|lc| lc.is_element_kind())
            .position(|lc| lc.descend_target() == Some(next))
            .ok_or_else(|| {
                KepubError::InvalidEpub(format!(
                    "couldn't locate an ancestor element while walking toward \"{span_id}\""
                ))
            })?;
        element_path.push(index);
        current_parent = next;
    }

    let logical = logical_children(doc, current_parent);
    let text_index = logical
        .iter()
        .filter(|lc| lc.is_text_kind())
        .position(|lc| match lc {
            LogicalChild::TextGroup { spans, .. } => spans.contains(&target_span),
            _ => false,
        });

    let Some(text_index) = text_index else {
        let index = logical
            .iter()
            .filter(|lc| lc.is_element_kind())
            .position(|lc| matches!(lc, LogicalChild::WrappedElement { span, .. } if *span == target_span))
            .ok_or_else(|| KepubError::SpanNotFound(span_id.clone()))?;
        element_path.push(index);
        return Err(KepubError::LocationIsElement {
            element_path,
            span_id,
        });
    };

    let group = same_para_group(doc, target_span, para);
    let mut char_offset = 0usize;
    for span in group {
        if span == target_span {
            char_offset += offset_in_span;
            break;
        }
        char_offset += span_text_content(doc, span).chars().count();
    }

    Ok((element_path, text_index, char_offset))
}

/// Translates a structural location back into a kobo location: which
/// `(para, seg)` span the position falls in, and a character offset into
/// that span's own text.
///
/// # Errors
///
/// Returns [`KepubError::InvalidLocationPath`] if `element_path` or
/// `text_index` don't resolve to a real position in `doc`,
/// [`KepubError::LocationNotSpanned`] if the addressed text run is a bare
/// text node Kobo never wraps in a span, or
/// [`KepubError::InvalidLocationOffset`] if `char_offset` exceeds the
/// text run's length. Also propagates [`KepubError::MissingElement`] if
/// `doc` has no `<body>`.
pub fn location_to_kobo(
    doc: &DocumentArena,
    element_path: &[usize],
    text_index: usize,
    char_offset: usize,
) -> Result<(u32, u32, usize), KepubError> {
    let mut current = body_content_root(doc)?;

    for (depth, &index) in element_path.iter().enumerate() {
        let logical = logical_children(doc, current);
        let target = logical
            .iter()
            .filter(|lc| lc.is_element_kind())
            .nth(index)
            .and_then(LogicalChild::descend_target)
            .ok_or_else(|| KepubError::InvalidLocationPath {
                element_path: element_path.to_vec(),
                text_index,
                detail: format!("no element at index {index} (depth {depth})"),
            })?;
        current = target;
    }

    let logical = logical_children(doc, current);
    let text_run = logical
        .iter()
        .filter(|lc| lc.is_text_kind())
        .nth(text_index)
        .ok_or_else(|| KepubError::InvalidLocationPath {
            element_path: element_path.to_vec(),
            text_index,
            detail: format!("no text run at index {text_index} in the addressed element"),
        })?;

    match text_run {
        LogicalChild::TextGroup { para, spans } => {
            resolve_group_offset(doc, *para, spans, char_offset).ok_or({
                KepubError::InvalidLocationOffset {
                    offset: char_offset,
                    text_index,
                }
            })
        }
        _ => Err(KepubError::LocationNotSpanned {
            element_path: element_path.to_vec(),
            text_index,
        }),
    }
}

/// Checks that a structural location actually addresses a character
/// position in `doc`, without translating it to anything.
///
/// Unlike [`location_to_kobo`], this works against an **unconverted** EPUB
/// as well as a kepub: it never looks up koboSpan ids, only walks the
/// logical structure. [`logical_children`] reports a plain, unspanned text
/// node as [`LogicalChild::Text`] and a converted one as
/// [`LogicalChild::TextGroup`], and both are accepted here — which is what
/// makes the same location string checkable against either form of the
/// document.
///
/// # Errors
///
/// Returns [`KepubError::InvalidLocationPath`] if `element_path` or
/// `text_index` don't resolve, or [`KepubError::InvalidLocationOffset`] if
/// `char_offset` exceeds the addressed text run's length. Also propagates
/// [`KepubError::MissingElement`] if `doc` has no `<body>`.
pub fn validate_location(
    doc: &DocumentArena,
    element_path: &[usize],
    text_index: usize,
    char_offset: usize,
) -> Result<(), KepubError> {
    let mut current = body_content_root(doc)?;

    for (depth, &index) in element_path.iter().enumerate() {
        let logical = logical_children(doc, current);
        let target = logical
            .iter()
            .filter(|lc| lc.is_element_kind())
            .nth(index)
            .and_then(LogicalChild::descend_target)
            .ok_or_else(|| KepubError::InvalidLocationPath {
                element_path: element_path.to_vec(),
                text_index,
                detail: format!("no element at index {index} (depth {depth})"),
            })?;
        current = target;
    }

    let logical = logical_children(doc, current);
    let text_run = logical
        .iter()
        .filter(|lc| lc.is_text_kind())
        .nth(text_index)
        .ok_or_else(|| KepubError::InvalidLocationPath {
            element_path: element_path.to_vec(),
            text_index,
            detail: format!("no text run at index {text_index} in the addressed element"),
        })?;

    let text = logical_text_content(doc, text_run);
    let len = text.chars().count();
    if char_offset > len {
        return Err(KepubError::InvalidLocationOffset {
            offset: char_offset,
            text_index,
        });
    }

    Ok(())
}

/// Checks that a kobo location addresses a real position in `doc`: the
/// span exists, and the offset is within that span's own text.
///
/// This one is kepub-only by nature — an unconverted EPUB has no koboSpans
/// to address in the first place, and will report
/// [`KepubError::SpanNotFound`].
///
/// # Errors
///
/// Returns [`KepubError::SpanNotFound`] if no `kobo.{para}.{seg}` span
/// exists, or [`KepubError::InvalidSpanOffset`] if `offset_in_span`
/// exceeds the span's own text length.
pub fn validate_kobo_span(
    doc: &DocumentArena,
    para: u32,
    seg: u32,
    offset_in_span: usize,
) -> Result<(), KepubError> {
    let span_id = format!("kobo.{para}.{seg}");
    let span =
        find_span_by_id(doc, &span_id).ok_or_else(|| KepubError::SpanNotFound(span_id.clone()))?;

    let len = span_text_content(doc, span).chars().count();
    if offset_in_span > len {
        return Err(KepubError::InvalidSpanOffset {
            id: span_id,
            offset: offset_in_span,
            len,
        });
    }

    Ok(())
}

/// The text a logical child stands for: a converted `TextGroup`'s spans
/// concatenated back together, or a plain text node's own content.
fn logical_text_content(doc: &DocumentArena, lc: &LogicalChild) -> String {
    match lc {
        LogicalChild::TextGroup { spans, .. } => {
            spans.iter().map(|&s| span_text_content(doc, s)).collect()
        }
        LogicalChild::Text(id) => {
            let arena = doc.arena.borrow();
            match arena.get(*id).map(Node::get) {
                Some(NodeData::Text(t)) => t.clone(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    }
}

/// Groups `parent`'s actual children back into what they represent in the
/// document's original (pre-kepub) structure.
///
/// Walks `parent`'s children in document order, collapsing any run of
/// consecutive sibling koboSpans sharing the same `para` into a single
/// [`LogicalChild::TextGroup`] (or a [`LogicalChild::WrappedElement`] when
/// a lone span wraps exactly one non-text element), and passing through
/// any other element or bare text node unchanged. This is the single place
/// that understands the mapping between a kepub's literal span structure
/// and the document's original shape; every public function above builds
/// on it rather than re-deriving it.
fn logical_children(doc: &DocumentArena, parent: NodeId) -> Vec<LogicalChild> {
    let children: Vec<(NodeId, NodeData)> = {
        let arena = doc.arena.borrow();
        parent
            .children(&arena)
            .map(|id| {
                let data = arena
                    .get(id)
                    .expect("child NodeId vanished from arena")
                    .get()
                    .clone();
                (id, data)
            })
            .collect()
    };

    let mut out = Vec::new();
    let mut i = 0;
    while i < children.len() {
        let (id, ref data) = children[i];

        match data {
            NodeData::Text(_) => {
                out.push(LogicalChild::Text(id));
                i += 1;
            }
            NodeData::Element { name, attrs }
                if is_kobo_span(name, attrs) && kobo_span_para(attrs).is_some() =>
            {
                let para = kobo_span_para(attrs).expect("checked by the match guard above");

                let mut group = vec![id];
                let mut j = i + 1;
                while j < children.len() {
                    let (jid, ref jdata) = children[j];
                    let continues = matches!(
                        jdata,
                        NodeData::Element { name, attrs }
                            if is_kobo_span(name, attrs) && kobo_span_para(attrs) == Some(para)
                    );
                    if !continues {
                        break;
                    }
                    group.push(jid);
                    j += 1;
                }

                let inner: Vec<NodeId> = {
                    let arena = doc.arena.borrow();
                    id.children(&arena).collect()
                };
                let wraps_element = group.len() == 1
                    && inner.len() == 1
                    && matches!(
                        {
                            let arena = doc.arena.borrow();
                            arena.get(inner[0]).map(|n| n.get().clone())
                        },
                        Some(NodeData::Element { .. })
                    );

                if wraps_element {
                    out.push(LogicalChild::WrappedElement {
                        span: id,
                        inner: inner[0],
                    });
                } else {
                    out.push(LogicalChild::TextGroup { para, spans: group });
                }

                i = j;
            }
            NodeData::Element { .. } => {
                out.push(LogicalChild::Element(id));
                i += 1;
            }
            _ => {
                // Comments, PIs, doctype: not addressable.
                i += 1;
            }
        }
    }

    out
}

/// Returns whether `name`/`attrs` describe a `<span class="koboSpan">`.
fn is_kobo_span(name: &QualName, attrs: &[Attribute]) -> bool {
    name.local.as_ref() == "span"
        && attrs
            .iter()
            .any(|a| a.name.local.as_ref() == "class" && a.value.to_string() == "koboSpan")
}

/// Extracts the `N` from a koboSpan's `id="kobo.N.M"` attribute, or `None`
/// if `attrs` has no `id` matching that format.
fn kobo_span_para(attrs: &[Attribute]) -> Option<u32> {
    let id = attrs
        .iter()
        .find(|a| a.name.local.as_ref() == "id")?
        .value
        .to_string();
    let rest = id.strip_prefix("kobo.")?;
    let (para_str, _seg) = rest.split_once('.')?;
    para_str.parse().ok()
}

/// Depth-first search for a koboSpan element with the given `id`
/// attribute, starting from `doc`'s root.
fn find_span_by_id(doc: &DocumentArena, id: &str) -> Option<NodeId> {
    find_span_by_id_from(doc, doc.root, id)
}

/// Depth-first search for a koboSpan element with the given `id`
/// attribute, starting from `start`.
fn find_span_by_id_from(doc: &DocumentArena, start: NodeId, id: &str) -> Option<NodeId> {
    let is_match = {
        let arena = doc.arena.borrow();
        match arena.get(start).map(Node::get) {
            Some(NodeData::Element { name, attrs }) => {
                is_kobo_span(name, attrs)
                    && attrs
                        .iter()
                        .any(|a| a.name.local.as_ref() == "id" && a.value.to_string() == id)
            }
            _ => false,
        }
    };
    if is_match {
        return Some(start);
    }

    let children: Vec<NodeId> = {
        let arena = doc.arena.borrow();
        start.children(&arena).collect()
    };
    for child in children {
        if let Some(found) = find_span_by_id_from(doc, child, id) {
            return Some(found);
        }
    }
    None
}

/// Concatenates the text of `span`'s direct text-node children.
fn span_text_content(doc: &DocumentArena, span: NodeId) -> String {
    let children: Vec<NodeId> = {
        let arena = doc.arena.borrow();
        span.children(&arena).collect()
    };

    let mut out = String::new();
    for child in children {
        let data = {
            let arena = doc.arena.borrow();
            arena.get(child).map(|n| n.get().clone())
        };
        if let Some(NodeData::Text(t)) = data {
            out.push_str(&t);
        }
    }
    out
}

/// All spans sharing `span`'s parent and `para` value, in document order.
fn same_para_group(doc: &DocumentArena, span: NodeId, para: u32) -> Vec<NodeId> {
    let parent = {
        let arena = doc.arena.borrow();
        arena.get(span).and_then(Node::parent)
    };
    let Some(parent) = parent else {
        return vec![span];
    };

    let siblings: Vec<NodeId> = {
        let arena = doc.arena.borrow();
        parent.children(&arena).collect()
    };

    siblings
        .into_iter()
        .filter(|&s| {
            let arena = doc.arena.borrow();
            match arena.get(s).map(Node::get) {
                Some(NodeData::Element { name, attrs }) => {
                    is_kobo_span(name, attrs) && kobo_span_para(attrs) == Some(para)
                }
                _ => false,
            }
        })
        .collect()
}

/// Given a text-run group and a character offset into the reconstructed
/// original text node, finds which span the offset falls in and rebases
/// the offset to be relative to that span's own text.
///
/// An offset landing exactly on a boundary between two spans resolves to
/// the *start* of the next span rather than the end of the current one,
/// except at the very last span in the group, which has no next span to
/// hand off to. Returns `None` if `char_offset` exceeds the group's total
/// length.
fn resolve_group_offset(
    doc: &DocumentArena,
    para: u32,
    spans: &[NodeId],
    char_offset: usize,
) -> Option<(u32, u32, usize)> {
    let mut remaining = char_offset;

    for (i, &span) in spans.iter().enumerate() {
        let text = span_text_content(doc, span);
        let len = text.chars().count();
        let is_last = i == spans.len() - 1;

        if remaining < len || (is_last && remaining == len) {
            let span_idx = u32::try_from(i + 1).ok()?;
            return Some((para, span_idx, remaining));
        }

        remaining -= len;
    }

    None
}

/// Returns the value of `node`'s `id` attribute, if it's an element and
/// has one.
fn element_id_attr(doc: &DocumentArena, node: NodeId) -> Option<String> {
    let arena = doc.arena.borrow();
    match arena.get(node).map(Node::get) {
        Some(NodeData::Element { attrs, .. }) => attrs
            .iter()
            .find(|a| a.name.local.as_ref() == "id")
            .map(|a| a.value.to_string()),
        _ => None,
    }
}

/// Body's real content sits two levels deeper than `<body>` after
/// conversion (`body > div#book-columns > div#book-inner > ...`), and that
/// wrapper never existed in the original document — indices must skip it.
///
/// Falls back to `body` itself if the wrapper isn't there, so the same
/// location addresses the same character in the original EPUB too.
///
/// # Errors
///
/// Returns [`KepubError::MissingElement`] if `doc` has no `<body>`.
fn body_content_root(doc: &DocumentArena) -> Result<NodeId, KepubError> {
    let body = find_body(doc)?;

    let body_children: Vec<NodeId> = {
        let arena = doc.arena.borrow();
        body.children(&arena).collect()
    };

    if body_children.len() == 1
        && element_id_attr(doc, body_children[0]).as_deref() == Some("book-columns")
    {
        let inner: Vec<NodeId> = {
            let arena = doc.arena.borrow();
            body_children[0].children(&arena).collect()
        };
        if inner.len() == 1 && element_id_attr(doc, inner[0]).as_deref() == Some("book-inner") {
            return Ok(inner[0]);
        }
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::transform::Transform;

    fn kepub_from(xml: &str) -> DocumentArena {
        let doc = crate::dom::parse(xml.as_bytes()).expect("failed to parse test XML");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");
        doc
    }

    fn assert_all_spans_round_trip(doc: &DocumentArena, expected_spans: usize) {
        let mut checked = 0;
        let max_para = u32::try_from(expected_spans)
            .expect("expected_spans exceeds u32::MAX")
            .saturating_add(5);

        for para in 1..=max_para {
            for seg in 1..=10u32 {
                let id = format!("kobo.{para}.{seg}");
                if find_span_by_id(doc, &id).is_none() {
                    continue;
                }

                let text = span_text_content(
                    doc,
                    find_span_by_id(doc, &id).expect("span should exist after confirming its ID"),
                );
                let span_len = text.chars().count();

                for offset in 0..=span_len {
                    let Ok((path, text_index, char_offset)) =
                        kobo_to_location(doc, para, seg, offset)
                    else {
                        continue;
                    };

                    let (rt_para, rt_seg, rt_offset) =
                        location_to_kobo(doc, &path, text_index, char_offset).unwrap_or_else(|e| {
                            panic!("{id} @ {offset} -> {path:?}/t{text_index}:{char_offset} failed to come back: {e}")
                        });

                    if (para, seg, offset) != (rt_para, rt_seg, rt_offset) {
                        assert!(
                            offset == span_len,
                            "{id} @ {offset} came back as kobo.{rt_para}.{rt_seg} @ {rt_offset}"
                        );
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "no spans were actually checked");
    }

    #[test]
    fn round_trips_a_simple_span() {
        let doc = kepub_from("<html><head></head><body><p>Hello world</p></body></html>");

        let (path, text_index, char_offset) =
            kobo_to_location(&doc, 1, 1, 6).expect("should translate");
        assert_eq!(path, vec![0]);
        assert_eq!(text_index, 0);
        assert_eq!(char_offset, 6);

        let back = location_to_kobo(&doc, &path, text_index, char_offset).expect("should reverse");
        assert_eq!(back, (1, 1, 6));
    }

    #[test]
    fn round_trips_across_an_inline_element() {
        let doc = kepub_from("<html><head></head><body><p>Hello <em>world</em></p></body></html>");

        let (path, text_index, char_offset) =
            kobo_to_location(&doc, 2, 1, 2).expect("should translate");
        assert_eq!(path, vec![0, 0]);
        assert_eq!(text_index, 0);
        assert_eq!(char_offset, 2);

        let back = location_to_kobo(&doc, &path, text_index, char_offset).expect("should reverse");
        assert_eq!(back, (2, 1, 2));
    }

    #[test]
    fn round_trips_multi_segment_paragraph() {
        let doc = kepub_from(
            "<html><head></head><body><p>First sentence. Second sentence.</p></body></html>",
        );

        let (path, text_index, char_offset) =
            kobo_to_location(&doc, 1, 2, 0).expect("should translate");
        assert_eq!(char_offset, 16, "'First sentence. ' is 16 chars");

        let back = location_to_kobo(&doc, &path, text_index, char_offset).expect("should reverse");
        assert_eq!(back, (1, 2, 0));

        assert_eq!(
            location_to_kobo(&doc, &path, text_index, 5)
                .expect("should resolve an offset inside segment 1"),
            (1, 1, 5)
        );
    }

    #[test]
    fn offsets_are_characters_on_both_sides() {
        let doc = kepub_from("<html><head></head><body><p>Hi \u{1F600} there.</p></body></html>");

        let (path, text_index, char_offset) =
            kobo_to_location(&doc, 1, 1, 4).expect("should translate");
        assert_eq!(
            char_offset, 4,
            "single-span run: offsets should match exactly"
        );

        let back = location_to_kobo(&doc, &path, text_index, char_offset).expect("should reverse");
        assert_eq!(back, (1, 1, 4));
    }

    #[test]
    fn the_position_just_past_the_last_character_is_valid() {
        let doc = kepub_from("<html><head></head><body><p>Hello</p></body></html>");

        let (path, text_index, char_offset) =
            kobo_to_location(&doc, 1, 1, 5).expect("end-of-run should be a valid position");
        assert_eq!(char_offset, 5);

        let back = location_to_kobo(&doc, &path, text_index, char_offset).expect("should reverse");
        assert_eq!(back, (1, 1, 5));

        assert!(kobo_to_location(&doc, 1, 1, 6).is_err());
    }

    #[test]
    fn every_span_round_trips_in_a_deeply_nested_document() {
        let doc = kepub_from(
            "<html><head></head><body>\
             <div class=\"outer\">\
             <h1>Chapter One</h1>\
             <div><img src=\"deco.png\" alt=\"\"/></div>\
             <p>First sentence. Second one here. Third!</p>\
             <p>Text with <em>emphasis</em> and <strong>strength</strong> inside.</p>\
             <ul><li>Item one.</li><li>Item <em>two</em>.</li></ul>\
             </div>\
             <p>A trailing paragraph.</p>\
             </body></html>",
        );
        assert_all_spans_round_trip(&doc, 20);
    }

    #[test]
    fn unknown_span_is_reported() {
        let doc = kepub_from("<html><head></head><body><p>Hello</p></body></html>");
        let err = kobo_to_location(&doc, 99, 1, 0)
            .expect_err("unknown span should be reported as an error");
        assert!(matches!(err, KepubError::SpanNotFound(id) if id == "kobo.99.1"));
    }

    #[test]
    fn out_of_range_offset_is_reported() {
        let doc = kepub_from("<html><head></head><body><p>Hi</p></body></html>");
        let err = kobo_to_location(&doc, 1, 1, 99)
            .expect_err("out-of-range span offset should be reported as an error");
        assert!(matches!(
            err,
            KepubError::InvalidSpanOffset {
                offset: 99,
                len: 2,
                ..
            }
        ));
    }

    #[test]
    fn bad_element_index_is_reported() {
        let doc = kepub_from("<html><head></head><body><p>Hello</p></body></html>");
        let err = location_to_kobo(&doc, &[40], 0, 0)
            .expect_err("invalid element index should be reported as an error");
        assert!(matches!(err, KepubError::InvalidLocationPath { .. }));
    }

    #[test]
    fn bad_text_index_is_reported() {
        let doc = kepub_from("<html><head></head><body><p>Hello</p></body></html>");
        let err = location_to_kobo(&doc, &[0], 40, 0)
            .expect_err("invalid text index should be reported as an error");
        assert!(matches!(err, KepubError::InvalidLocationPath { .. }));
    }

    #[test]
    fn out_of_range_char_offset_is_reported() {
        let doc = kepub_from("<html><head></head><body><p>Hi</p></body></html>");
        let err = location_to_kobo(&doc, &[0], 0, 99)
            .expect_err("out-of-range character offset should be reported as an error");
        assert!(matches!(
            err,
            KepubError::InvalidLocationOffset { offset: 99, .. }
        ));
    }

    #[test]
    fn a_wrapped_image_reports_it_is_an_element_not_text() {
        let doc = kepub_from(
            r#"<html><head></head><body><div><img src="a.png" alt=""/></div></body></html>"#,
        );
        let err = kobo_to_location(&doc, 1, 1, 0)
            .expect_err("wrapped image should report that the location is an element");
        assert!(matches!(err, KepubError::LocationIsElement { .. }));
    }
}
