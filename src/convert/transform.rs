//! Body-content mutation: head asset links, the book-columns/book-inner
//! wrapper, and koboSpan injection.
//!
//! Span injection is a single pass: [`Transform::walk_and_inject_spans`]
//! walks the tree in document order and mutates as it goes, wrapping each
//! spannable text run or element (`img`/`svg`) in a
//! `<span class="koboSpan" id="kobo.N.M">` immediately.

use indextree::{Node, NodeId};
use markup5ever::{Attribute, LocalName, QualName, local_name, ns};

use crate::dom::arena::{DocumentArena, NodeData};
use crate::dom::{find_body, find_head};
use crate::error::KepubError;

/// How a given element is treated while walking for span injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    /// Left completely untouched, not recursed into for span purposes.
    /// Still consumes a para ordinal at its parent's level like any
    /// element.
    SkipSubtree,
    /// Starts a new paragraph/segment counter scope (`p`, headings, `li`,
    /// ...).
    ParagraphBoundary,
    /// Wrapped whole in its own `<span class="koboSpan">`, sharing the
    /// same para sequence as text runs. Currently `img` and `svg`.
    VoidSpanned,
    /// Walked into, but doesn't itself start a new paragraph scope.
    Passthrough,
}

/// Configures and runs the EPUB-to-KEPUB body transformation.
///
/// Applying a `Transform` to a document injects Kobo's stylesheet/script
/// links into `<head>`, wraps the body in the `book-columns`/`book-inner`
/// divs, and wraps every spannable text run or element in a `koboSpan`.
/// `classify` and `segment` are exposed as swappable function pointers so
/// callers can override the element classification or sentence-segmentation
/// rules without forking the crate.
pub struct Transform<'a> {
    /// Href written into the injected `<link rel="stylesheet">`.
    pub css_href: &'a str,
    /// Src written into the injected `<script>`.
    pub js_href: &'a str,
    /// Decides how each element is treated during the span-injection
    /// walk. See [`ElementKind`].
    pub classify: fn(&str) -> ElementKind,
    /// Splits a text run into `(start, end)` byte-offset segments, each of
    /// which becomes its own `koboSpan`. Implementations must return
    /// segments that exactly partition the input (see
    /// [`assert_segments_cover`]).
    pub segment: fn(&str) -> Vec<(usize, usize)>,
}

impl Default for Transform<'_> {
    /// Returns a `Transform` using Kobo's conventional asset paths
    /// (`css/kobo.css`, `js/kobo.js`) and this crate's
    /// [`default_classify`]/[`default_segment`] rules.
    fn default() -> Self {
        Self {
            css_href: super::assets::KOBO_CSS_HREF,
            js_href: super::assets::KOBO_JS_HREF,
            classify: default_classify,
            segment: default_segment,
        }
    }
}

impl Transform<'_> {
    /// Runs the full pipeline on `doc` in place: injects head assets,
    /// injects koboSpans throughout the body, then wraps the body content
    /// in `book-columns`/`book-inner`.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::MissingElement`] if `doc` has no `<head>` or
    /// no `<body>` element.
    pub fn apply(&self, doc: &DocumentArena) -> Result<(), KepubError> {
        self.inject_head_assets(doc)?;

        let body = find_body(doc)?;

        let mut para: u32 = 0;
        self.walk_and_inject_spans(doc, body, &mut para);

        Self::wrap_body(doc, body);

        Ok(())
    }

    /// Appends the Kobo stylesheet `<link>` and script `<script>` elements
    /// to `doc`'s `<head>`.
    ///
    /// # Errors
    ///
    /// Returns [`KepubError::MissingElement`] if `doc` has no `<head>`
    /// element.
    fn inject_head_assets(&self, doc: &DocumentArena) -> Result<(), KepubError> {
        let head = find_head(doc)?;

        let link = doc.new_node(NodeData::Element {
            name: QualName::new(None, ns!(html), local_name!("link")),
            attrs: vec![
                attr("rel", "stylesheet"),
                attr("type", "text/css"),
                attr("href", self.css_href),
            ],
        });
        doc.append_child(head, link);

        let script = doc.new_node(NodeData::Element {
            name: QualName::new(None, ns!(html), local_name!("script")),
            attrs: vec![attr("type", "text/javascript"), attr("src", self.js_href)],
        });
        doc.append_child(head, script);

        Ok(())
    }

    /// Moves body's existing children into a new `div#book-inner`, wrapped
    /// in turn by `div#book-columns`, which becomes body's sole child.
    fn wrap_body(doc: &DocumentArena, body: NodeId) {
        let existing: Vec<NodeId> = {
            let arena = doc.arena.borrow();
            body.children(&arena).collect()
        };

        let book_inner = doc.new_node(NodeData::Element {
            name: QualName::new(None, ns!(html), local_name!("div")),
            attrs: vec![attr("id", "book-inner")],
        });
        for child in existing {
            {
                let mut arena = doc.arena.borrow_mut();
                child.detach(&mut *arena);
            }
            doc.append_child(book_inner, child);
        }

        let book_columns = doc.new_node(NodeData::Element {
            name: QualName::new(None, ns!(html), local_name!("div")),
            attrs: vec![attr("id", "book-columns")],
        });
        doc.append_child(book_columns, book_inner);
        doc.append_child(body, book_columns);
    }

    /// Walks `parent`'s subtree in document order, mutating as it goes:
    /// every spannable text run gets sentence-split and spliced into one
    /// or more koboSpans, every [`ElementKind::VoidSpanned`] element gets
    /// wrapped whole.
    ///
    /// `para` is threaded through the whole walk by mutable reference,
    /// since it's a single running counter shared across the entire
    /// document, not scoped to any particular container.
    fn walk_and_inject_spans(&self, doc: &DocumentArena, parent: NodeId, para: &mut u32) {
        let parent_local: Option<String> = {
            let arena = doc.arena.borrow();
            match arena.get(parent).map(Node::get) {
                Some(NodeData::Element { name, .. }) => Some(name.local.to_string()),
                _ => None,
            }
        };

        let children: Vec<NodeId> = {
            let arena = doc.arena.borrow();
            parent.children(&arena).collect()
        };

        for child in children {
            let data = {
                let arena = doc.arena.borrow();
                arena
                    .get(child)
                    .expect("child NodeId vanished from arena during the walk")
                    .get()
                    .clone()
            };

            match data {
                NodeData::Document => {
                    unreachable!("a Document node cannot be the child of another node")
                }
                NodeData::Comment(_)
                | NodeData::ProcessingInstruction { .. }
                | NodeData::Doctype { .. } => {}
                NodeData::Text(text) => {
                    let whitespace_only = text.trim().is_empty();
                    let keep = !whitespace_only || parent_local.as_deref() == Some("p");
                    if keep {
                        *para += 1;
                        self.inject_text_spans(doc, child, &text, *para);
                    }
                }
                NodeData::Element { name, .. } => match (self.classify)(name.local.as_ref()) {
                    ElementKind::SkipSubtree => {}
                    ElementKind::VoidSpanned => {
                        *para += 1;
                        let id = format!("kobo.{para}.1");
                        wrap_in_span(doc, child, &id);
                    }
                    ElementKind::ParagraphBoundary | ElementKind::Passthrough => {
                        self.walk_and_inject_spans(doc, child, para);
                    }
                },
            }
        }
    }

    /// Splits one text node's content into segments and splices a koboSpan
    /// in for each, then detaches the now-redundant original text node.
    ///
    /// # Panics
    ///
    /// Panics (via [`assert_segments_cover`]) if `self.segment` returns
    /// segments that don't exactly partition `text`, or if `text` has more
    /// than `u32::MAX` segments.
    fn inject_text_spans(&self, doc: &DocumentArena, node: NodeId, text: &str, para: u32) {
        let segments = (self.segment)(text);
        assert_segments_cover(text, &segments);

        for (seg_index, (start, end)) in segments.into_iter().enumerate() {
            let seg = u32::try_from(seg_index)
                .expect("paragraph cannot contain more than u32::MAX segments")
                + 1;
            let id = format!("kobo.{para}.{seg}");
            let span = make_span(doc, &id, &text[start..end]);

            let mut arena = doc.arena.borrow_mut();
            node.insert_before(span, &mut *arena);
        }

        let mut arena = doc.arena.borrow_mut();
        node.detach(&mut *arena);
    }
}

/// Validates that `segments` exactly partitions `text`'s byte range: no
/// gaps, no overlaps, sorted in order, and every cut falls on a UTF-8
/// character boundary.
///
/// A `segment` function that violates this would silently drop or
/// duplicate characters or panic later, less clearly, when
/// `run.text[start..end]` hits a byte index that isn't a valid char
/// boundary. Catching it here, right after the segmenter runs, gives a
/// much clearer error than either of those.
///
/// # Panics
///
/// Panics if `segments` has a gap, an overlap, an inverted range, a cut
/// that doesn't land on a character boundary, or doesn't cover all of
/// `text`.
fn assert_segments_cover(text: &str, segments: &[(usize, usize)]) {
    let mut expected_start = 0usize;

    for &(start, end) in segments {
        assert_eq!(
            start, expected_start,
            "segmenter produced a gap or overlap: expected the next segment to \
             start at byte {expected_start}, got {start} (text: {text:?})"
        );
        assert!(
            end >= start,
            "segmenter produced an inverted range ({start}, {end}) (text: {text:?})"
        );
        assert!(
            text.is_char_boundary(start) && text.is_char_boundary(end),
            "segmenter produced range ({start}, {end}) that doesn't fall on a \
             UTF-8 character boundary (text: {text:?})"
        );
        expected_start = end;
    }

    assert_eq!(
        expected_start,
        text.len(),
        "segmenter didn't cover the whole run: segments reached byte \
         {expected_start}, but the text is {} bytes long (text: {text:?})",
        text.len()
    );
}

/// Creates a `<span class="koboSpan" id="...">` containing a single text
/// node with `text`, and adds it (unattached) to the arena.
fn make_span(doc: &DocumentArena, id: &str, text: &str) -> NodeId {
    let span = doc.new_node(NodeData::Element {
        name: QualName::new(None, ns!(html), local_name!("span")),
        attrs: vec![attr("class", "koboSpan"), attr("id", id)],
    });
    let text_node = doc.new_node(NodeData::Text(text.to_string()));
    doc.append_child(span, text_node);
    span
}

/// Wraps an existing node (`img`/`svg`) in a new
/// `<span class="koboSpan" id="...">`, preserving its position among
/// siblings: the span takes the node's old spot, and the node becomes the
/// span's sole child.
fn wrap_in_span(doc: &DocumentArena, node: NodeId, id: &str) {
    let span = doc.new_node(NodeData::Element {
        name: QualName::new(None, ns!(html), local_name!("span")),
        attrs: vec![attr("class", "koboSpan"), attr("id", id)],
    });

    {
        let mut arena = doc.arena.borrow_mut();
        node.insert_before(span, &mut *arena);
        node.detach(&mut *arena);
    }
    doc.append_child(span, node);
}

/// Builds an unprefixed attribute with `name` and `value`.
///
/// Attribute names here are runtime strings (`id` values, configured
/// hrefs), so this uses `LocalName::from` rather than the `local_name!`
/// macro, which only accepts literals known at compile time.
fn attr(name: &str, value: &str) -> Attribute {
    Attribute {
        name: QualName::new(None, ns!(), LocalName::from(name)),
        value: value.into(),
    }
}

/// Classifies an HTML element by its local name according to the element
/// handling rules used when transforming a document.
///
/// Elements are classified as either skipped subtrees, void elements that
/// receive spans, paragraph boundaries, or passthrough elements.
///
/// See [`ElementKind`] for the meaning of each classification.
#[must_use]
pub fn default_classify(local_name: &str) -> ElementKind {
    match local_name {
        "script" | "style" | "pre" | "audio" | "video" | "math" => ElementKind::SkipSubtree,
        "img" | "svg" => ElementKind::VoidSpanned,
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "td" | "th" | "blockquote"
        | "figcaption" => ElementKind::ParagraphBoundary,
        _ => ElementKind::Passthrough,
    }
}

/// Splits `text` into sentence-like segments, returning each as a
/// `(start, end)` byte-offset pair that together exactly partition `text`.
///
/// A cut is made after a run of terminal punctuation (`.`, `?`, `!`),
/// optionally followed by a closing quote, provided that run is then
/// followed by whitespace or the end of the text; any trailing whitespace
/// is folded into the segment that just ended. Punctuation immediately
/// followed by more non-whitespace content (`"3.14"`, `"Mr.Smith"`) is not
/// treated as a boundary. Confirmed against real Kobo-converted samples —
/// see this module's tests.
#[must_use]
pub fn default_segment(text: &str) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return Vec::new();
    }

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut seg_start = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i].1;

        if is_terminal_punct(c) {
            let mut j = i + 1;
            while j < chars.len() && is_terminal_punct(chars[j].1) {
                j += 1;
            }
            while j < chars.len() && is_closing_quote(chars[j].1) {
                j += 1;
            }
            let mut k = j;
            while k < chars.len() && chars[k].1.is_whitespace() {
                k += 1;
            }

            let found_whitespace = k > j;
            let at_end = k == chars.len();

            if found_whitespace || at_end {
                let end_byte = if k < chars.len() {
                    chars[k].0
                } else {
                    text.len()
                };
                out.push((seg_start, end_byte));
                seg_start = end_byte;
                i = k;
                continue;
            }

            i = j;
            continue;
        }

        i += 1;
    }

    if seg_start < text.len() {
        out.push((seg_start, text.len()));
    }

    out
}

/// Returns whether `c` is one of the terminal punctuation marks
/// (`.`, `?`, `!`) [`default_segment`] treats as a possible sentence
/// boundary.
const fn is_terminal_punct(c: char) -> bool {
    matches!(c, '.' | '?' | '!')
}

/// Returns whether `c` is a closing-quote character [`default_segment`]
/// folds into the end of a sentence immediately after terminal
/// punctuation.
const fn is_closing_quote(c: char) -> bool {
    matches!(
        c,
        '"' | '\'' | '\u{201D}' | '\u{2019}' | '\u{00BB}' | '\u{203A}'
    )
}

#[cfg(test)]
mod tests {
    use crate::dom::find_element;

    use super::*;

    fn parse(xml: &str) -> DocumentArena {
        crate::dom::parse(xml.as_bytes()).expect("failed to parse test XML")
    }

    fn text_of(doc: &DocumentArena, node: NodeId) -> String {
        let data = {
            let arena = doc.arena.borrow();
            arena.get(node).expect("node not found").get().clone()
        };
        match data {
            NodeData::Text(t) => t,
            NodeData::Element { .. } => {
                let children: Vec<NodeId> = {
                    let arena = doc.arena.borrow();
                    node.children(&arena).collect()
                };
                children.iter().map(|c| text_of(doc, *c)).collect()
            }
            _ => String::new(),
        }
    }

    fn element_at(doc: &DocumentArena, node: NodeId) -> (String, Vec<(String, String)>) {
        let arena = doc.arena.borrow();
        match arena.get(node).expect("node not found").get() {
            NodeData::Element { name, attrs } => (
                name.local.as_ref().to_string(),
                attrs
                    .iter()
                    .map(|a| (a.name.local.as_ref().to_string(), a.value.to_string()))
                    .collect(),
            ),
            other => panic!("expected an element, found {other:?}"),
        }
    }

    fn children_of(doc: &DocumentArena, node: NodeId) -> Vec<NodeId> {
        let arena = doc.arena.borrow();
        node.children(&arena).collect()
    }

    fn kobo_span_ids(doc: &DocumentArena, node: NodeId) -> Vec<String> {
        let mut out = Vec::new();
        collect_kobo_span_ids(doc, node, &mut out);
        out
    }

    fn collect_kobo_span_ids(doc: &DocumentArena, node: NodeId, out: &mut Vec<String>) {
        let is_element = {
            let arena = doc.arena.borrow();
            matches!(
                arena.get(node).map(Node::get),
                Some(NodeData::Element { .. })
            )
        };

        if is_element {
            let (tag, attrs) = element_at(doc, node);
            if tag == "span"
                && attrs.contains(&("class".to_string(), "koboSpan".to_string()))
                && let Some((_, id)) = attrs.iter().find(|(k, _)| k == "id")
            {
                out.push(id.clone());
            }
        }

        let children: Vec<NodeId> = {
            let arena = doc.arena.borrow();
            node.children(&arena).collect()
        };
        for child in children {
            collect_kobo_span_ids(doc, child, out);
        }
    }

    #[test]
    fn missing_body_returns_error() {
        let doc = parse("<html><head></head></html>");
        let err = Transform::default()
            .apply(&doc)
            .expect_err("mutator should return an error when the body is missing");
        assert!(matches!(err, KepubError::MissingElement("body")));
    }

    #[test]
    fn missing_head_returns_error() {
        let doc = parse("<html><body><p>hi</p></body></html>");
        let err = Transform::default()
            .apply(&doc)
            .expect_err("mutator should return an error when the head is missing");
        assert!(matches!(err, KepubError::MissingElement("head")));
    }

    #[test]
    fn head_assets_are_injected() {
        let doc = parse("<html><head></head><body><p>Hi</p></body></html>");
        let mutator = Transform {
            css_href: "../Styles/test.css",
            js_href: "../Scripts/test.js",
            ..Transform::default()
        };
        mutator.apply(&doc).expect("apply should succeed");

        let head = find_element(&doc, doc.root, "head").expect("head should exist");
        let children = children_of(&doc, head);
        assert_eq!(children.len(), 2, "head should gain exactly link + script");

        let (tag, attrs) = element_at(&doc, children[0]);
        assert_eq!(tag, "link");
        assert!(attrs.contains(&("rel".into(), "stylesheet".into())));
        assert!(attrs.contains(&("href".into(), "../Styles/test.css".into())));

        let (tag, attrs) = element_at(&doc, children[1]);
        assert_eq!(tag, "script");
        assert!(attrs.contains(&("src".into(), "../Scripts/test.js".into())));
    }

    #[test]
    fn body_is_wrapped_in_book_columns_and_book_inner() {
        let doc = parse("<html><head></head><body><p>One</p><p>Two</p></body></html>");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        let body = find_element(&doc, doc.root, "body").expect("body should exist");
        let body_children = children_of(&doc, body);
        assert_eq!(body_children.len(), 1, "body should have exactly one child");

        let (tag, attrs) = element_at(&doc, body_children[0]);
        assert_eq!(tag, "div");
        assert!(attrs.contains(&("id".into(), "book-columns".into())));

        let columns_children = children_of(&doc, body_children[0]);
        assert_eq!(columns_children.len(), 1);

        let (tag, attrs) = element_at(&doc, columns_children[0]);
        assert_eq!(tag, "div");
        assert!(attrs.contains(&("id".into(), "book-inner".into())));

        let inner_children = children_of(&doc, columns_children[0]);
        assert_eq!(
            inner_children.len(),
            2,
            "book-inner should hold both original <p>s"
        );
        assert_eq!(element_at(&doc, inner_children[0]).0, "p");
        assert_eq!(element_at(&doc, inner_children[1]).0, "p");
    }

    #[test]
    fn single_paragraph_gets_one_span() {
        let doc = parse("<html><head></head><body><p>Hello world</p></body></html>");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(kobo_span_ids(&doc, doc.root), vec!["kobo.1.1"]);

        let p = find_element(&doc, doc.root, "p").expect("p should still exist");
        let p_children = children_of(&doc, p);
        assert_eq!(
            p_children.len(),
            1,
            "the text node should be replaced by one span"
        );
        assert_eq!(text_of(&doc, p_children[0]), "Hello world");
    }

    #[test]
    fn separate_paragraphs_get_separate_para_counters() {
        let doc = parse("<html><head></head><body><p>First</p><p>Second</p></body></html>");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(kobo_span_ids(&doc, doc.root), vec!["kobo.1.1", "kobo.2.1"]);
    }

    #[test]
    fn script_content_is_left_untouched() {
        let doc =
            parse("<html><head></head><body><script>var a = 1;</script><p>Text</p></body></html>");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(
            kobo_span_ids(&doc, doc.root),
            vec!["kobo.1.1"],
            "only <p>'s text should produce a span"
        );

        let body = find_element(&doc, doc.root, "body").expect("body should exist");
        let script =
            find_element(&doc, body, "script").expect("script should still exist under body");
        let script_children = children_of(&doc, script);
        assert_eq!(
            script_children.len(),
            1,
            "script's content should be untouched"
        );

        let data = {
            let arena = doc.arena.borrow();
            arena
                .get(script_children[0])
                .expect("script child node should exist in arena")
                .get()
                .clone()
        };
        assert!(matches!(data, NodeData::Text(t) if t == "var a = 1;"));
    }

    #[test]
    fn whitespace_only_text_outside_p_is_not_spanned() {
        let doc = parse("<html><head></head><body>\n  <p>Text</p>\n</body></html>");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(
            kobo_span_ids(&doc, doc.root),
            vec!["kobo.1.1"],
            "surrounding whitespace shouldn't get its own span"
        );

        let p = find_element(&doc, doc.root, "p").expect("p should exist");
        assert_eq!(text_of(&doc, p), "Text");
    }

    #[test]
    fn whitespace_only_text_inside_p_is_spanned() {
        let doc = parse("<html><head></head><body><p> </p></body></html>");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");
        assert_eq!(kobo_span_ids(&doc, doc.root), vec!["kobo.1.1"]);
    }

    #[test]
    fn span_text_round_trips_to_original_content() {
        let doc = parse(
            "<html><head></head><body><p>Hello <em>world</em>, how are you?</p></body></html>",
        );
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        let p = find_element(&doc, doc.root, "p").expect("p should exist");
        assert_eq!(text_of(&doc, p), "Hello world, how are you?");
    }

    #[test]
    fn multibyte_utf8_text_is_spanned_correctly() {
        let doc = parse("<html><head></head><body><p>café</p></body></html>");
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(kobo_span_ids(&doc, doc.root), vec!["kobo.1.1"]);

        let p = find_element(&doc, doc.root, "p").expect("p should exist");
        assert_eq!(text_of(&doc, p), "café");
    }

    #[test]
    fn custom_segmenter_is_used() {
        fn split_on_space(text: &str) -> Vec<(usize, usize)> {
            let mut out = Vec::new();
            let mut start = 0;
            for (i, c) in text.char_indices() {
                if c == ' ' {
                    let end = i + c.len_utf8();
                    out.push((start, end));
                    start = end;
                }
            }
            if start < text.len() {
                out.push((start, text.len()));
            }
            out
        }

        let doc = parse("<html><head></head><body><p>Hello world</p></body></html>");
        let mutator = Transform {
            segment: split_on_space,
            ..Transform::default()
        };
        mutator.apply(&doc).expect("apply should succeed");

        assert_eq!(
            kobo_span_ids(&doc, doc.root),
            vec!["kobo.1.1", "kobo.1.2"],
            "custom segmenter should split into two words"
        );

        let p = find_element(&doc, doc.root, "p")
            .expect("test document should contain the <p> element");
        assert_eq!(text_of(&doc, p), "Hello world");
    }

    #[test]
    fn default_classify_categorizes_known_elements() {
        assert_eq!(default_classify("p"), ElementKind::ParagraphBoundary);
        assert_eq!(default_classify("h2"), ElementKind::ParagraphBoundary);
        assert_eq!(default_classify("script"), ElementKind::SkipSubtree);
        assert_eq!(default_classify("style"), ElementKind::SkipSubtree);
        assert_eq!(default_classify("img"), ElementKind::VoidSpanned);
        assert_eq!(default_classify("svg"), ElementKind::VoidSpanned);
        assert_eq!(default_classify("span"), ElementKind::Passthrough);
        assert_eq!(default_classify("em"), ElementKind::Passthrough);
    }

    #[test]
    fn default_segment_treats_whole_run_as_one_segment() {
        assert_eq!(default_segment("Hello world."), vec![(0, 12)]);
        assert_eq!(default_segment(""), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn default_segment_matches_real_sample_text_before_inline() {
        let text = "\u{201C}Like it matters where the library is? The point is the schedule is \
                     tight, and so is it.\u{201D} Her eyes find mine in the mirror before \
                     going back to her magazine. She always did have a way with words, but \
                     that doesn\u{2019}t mean what she\u{2019}s saying is any less true. I ";
        let segments = default_segment(text);
        let pieces: Vec<&str> = segments.iter().map(|&(s, e)| &text[s..e]).collect();

        assert_eq!(
            pieces,
            vec![
                "\u{201C}Like it matters where the library is? ",
                "The point is the schedule is tight, and so is it.\u{201D} ",
                "Her eyes find mine in the mirror before going back to her magazine. ",
                "She always did have a way with words, but that doesn\u{2019}t mean what \
                 she\u{2019}s saying is any less true. ",
                "I ",
            ]
        );
    }

    #[test]
    fn default_segment_matches_real_sample_text_after_inline() {
        let text = " freaking out. I\u{2019}m about thirty seconds away from a full-blown \
                     panic attack.";
        let segments = default_segment(text);
        let pieces: Vec<&str> = segments.iter().map(|&(s, e)| &text[s..e]).collect();

        assert_eq!(
            pieces,
            vec![
                " freaking out. ",
                "I\u{2019}m about thirty seconds away from a full-blown panic attack.",
            ]
        );
    }

    #[test]
    fn default_segment_leaves_run_with_no_terminal_punctuation_whole() {
        assert_eq!(default_segment("am"), vec![(0, 2)]);
    }

    #[test]
    fn default_segment_does_not_split_on_internal_apostrophes() {
        let text = "doesn\u{2019}t mean what she\u{2019}s saying.";
        assert_eq!(
            default_segment(text).len(),
            1,
            "no terminal punctuation until the final period"
        );
    }

    #[test]
    fn default_segment_does_not_split_mid_number() {
        let text = "The value is 3.14 exactly.";
        let pieces: Vec<&str> = default_segment(text)
            .iter()
            .map(|&(s, e)| &text[s..e])
            .collect();
        assert_eq!(pieces, vec!["The value is 3.14 exactly."]);
    }

    #[test]
    fn default_segment_does_not_split_on_commas_in_a_long_sentence() {
        let text = "A primeira, claro, no presente, com o monumento que a gente sempre quis \
                     ver ali diante dos olhos, o tempero da comida diferente provada pela \
                     primeira vez, a temperatura da \u{e1}gua no mergulho. ";
        assert_eq!(default_segment(text).len(), 1);
    }

    #[test]
    fn matches_real_kobo_conversion_sample_end_to_end() {
        let doc = parse(
            "<html><head></head><body><p class=\"subsq\">\u{201C}Like it matters where the \
             library is? The point is the schedule is tight, and so is it.\u{201D} Her eyes find \
             mine in the mirror before going back to her magazine. She always did have a way \
             with words, but that doesn\u{2019}t mean what she\u{2019}s saying is any less \
             true. I <i>am</i> freaking out. I\u{2019}m about thirty seconds away from a \
             full-blown panic attack.</p></body></html>",
        );
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(
            kobo_span_ids(&doc, doc.root),
            vec![
                "kobo.1.1", "kobo.1.2", "kobo.1.3", "kobo.1.4", "kobo.1.5", "kobo.2.1", "kobo.3.1",
                "kobo.3.2",
            ],
            "5 + 1 + 2 segments across the three text runs, matching the real sample's shape"
        );

        let p = find_element(&doc, doc.root, "p").expect("p should exist");
        let full_text = text_of(&doc, p);
        assert_eq!(
            full_text,
            "\u{201C}Like it matters where the library is? The point is the schedule is tight, and \
             so is it.\u{201D} Her eyes find mine in the mirror before going back to her \
             magazine. She always did have a way with words, but that doesn\u{2019}t mean what \
             she\u{2019}s saying is any less true. I am freaking out. I\u{2019}m about thirty \
             seconds away from a full-blown panic attack.",
            "round trip should reconstruct the original text exactly, got: {full_text}"
        );
    }

    #[test]
    fn images_are_wrapped_and_share_the_para_sequence_with_text() {
        let doc = parse(
            "<html><head></head><body>\
             <div><img src=\"left.png\" alt=\"\"/></div>\
             <div><img src=\"right.png\" alt=\"\"/></div>\
             <div><div><img src=\"above.png\" alt=\"\"/></div>\
             <h1><a id=\"anchor\"/>Title</h1>\
             <div><img src=\"below.png\" alt=\"\"/></div></div>\
             </body></html>",
        );
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(
            kobo_span_ids(&doc, doc.root),
            vec!["kobo.1.1", "kobo.2.1", "kobo.3.1", "kobo.4.1", "kobo.5.1"],
            "4 images + 1 heading text run, sharing one document-order sequence"
        );

        let img = find_element(&doc, doc.root, "img").expect("first img should still exist");
        let img_parent = {
            let arena = doc.arena.borrow();
            arena
                .get(img)
                .expect("img node should exist")
                .parent()
                .expect("img should have a parent")
        };
        let (parent_tag, parent_attrs) = element_at(&doc, img_parent);
        assert_eq!(parent_tag, "span");
        assert!(parent_attrs.contains(&("class".into(), "koboSpan".into())));
        assert!(parent_attrs.contains(&("id".into(), "kobo.1.1".into())));
    }

    #[test]
    fn svg_is_wrapped_like_img() {
        let doc = parse(
            r#"<html><head></head><body><div><svg width="10" height="10"><circle r="5"/></svg></div></body></html>"#,
        );
        Transform::default()
            .apply(&doc)
            .expect("apply should succeed");

        assert_eq!(
            kobo_span_ids(&doc, doc.root),
            vec!["kobo.1.1"],
            "the whole <svg> is one wrapped unit, not walked into"
        );

        let svg = find_element(&doc, doc.root, "svg").expect("svg should still exist");
        let svg_parent = {
            let arena = doc.arena.borrow();
            arena
                .get(svg)
                .expect("svg node should exist")
                .parent()
                .expect("svg should have a parent")
        };
        assert_eq!(element_at(&doc, svg_parent).0, "span");

        let circle = find_element(&doc, doc.root, "circle").expect("circle should still exist");
        let circle_parent = {
            let arena = doc.arena.borrow();
            arena
                .get(circle)
                .expect("circle node should exist")
                .parent()
                .expect("circle should have a parent")
        };
        assert_eq!(element_at(&doc, circle_parent).0, "svg");
    }

    #[test]
    fn assert_segments_cover_accepts_full_coverage() {
        assert_segments_cover("Hello world", &[(0, 6), (6, 11)]);
        assert_segments_cover("", &[]);
    }

    #[test]
    #[should_panic(expected = "gap or overlap")]
    fn assert_segments_cover_rejects_a_gap() {
        assert_segments_cover("Hello world", &[(0, 5), (6, 11)]);
    }

    #[test]
    #[should_panic(expected = "gap or overlap")]
    fn assert_segments_cover_rejects_an_overlap() {
        assert_segments_cover("Hello world", &[(0, 6), (4, 11)]);
    }

    #[test]
    #[should_panic(expected = "didn't cover the whole run")]
    fn assert_segments_cover_rejects_a_trailing_gap() {
        assert_segments_cover("Hello world", &[(0, 6)]);
    }

    #[test]
    #[should_panic(expected = "character boundary")]
    fn assert_segments_cover_rejects_a_mid_character_split() {
        assert_segments_cover("é", &[(0, 1), (1, 2)]);
    }
}
