use std::borrow::Cow;

use indextree::NodeId;
use markup5ever::interface::{ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::{Attribute, ExpandedName, LocalName, Namespace, QualName};
use tendril::StrTendril;

use crate::dom::arena::{DocumentArena, NodeData};

/// An owned wrapper around [`QualName`] implementing [`ElemName`].
///
/// `TreeSink::ElemName` must be borrowable from `&self`, but our
/// [`NodeData::Element`] stores its `name` behind a [`std::cell::RefCell`],
/// so we can't hand back a borrow with the right lifetime. Cloning into
/// this owned wrapper sidesteps that.
#[derive(Debug)]
pub struct OwnedElemName(pub QualName);

impl ElemName for OwnedElemName {
    fn ns(&self) -> &Namespace {
        &self.0.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.0.local
    }

    fn expanded(&self) -> ExpandedName<'_> {
        self.0.expanded()
    }
}

impl TreeSink for DocumentArena {
    type Handle = NodeId;
    type Output = Self;
    type ElemName<'a> = OwnedElemName;

    /// Returns the arena itself once parsing is complete.
    fn finish(self) -> Self::Output {
        self
    }

    /// Silently ignores a non-fatal parse error.
    ///
    /// Parsing continues regardless, per the HTML/XML parsing algorithms'
    /// error-recovery model.
    fn parse_error(&self, _msg: Cow<'static, str>) {}

    /// Returns the handle to the document root node.
    fn get_document(&self) -> Self::Handle {
        self.root
    }

    /// Returns the qualified name of the element at `target`.
    ///
    /// # Panics
    ///
    /// Panics if `target` is not present in the arena, or if it does not
    /// refer to a [`NodeData::Element`].
    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        let arena = self.arena.borrow();
        match arena.get(*target).expect("Node not found").get() {
            NodeData::Element { name, .. } => OwnedElemName(name.clone()),
            _ => panic!("Not an element!"),
        }
    }

    /// Returns `target` unchanged.
    ///
    /// We don't model `<template>` contents separately from the rest of
    /// the tree, so the template element itself doubles as its own
    /// contents handle.
    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        *target
    }

    /// Returns whether `x` and `y` refer to the same node.
    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    /// No-op: this crate doesn't track quirks mode.
    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    /// Creates a new element node with `name` and `attrs` and adds it to
    /// the arena (without attaching it to the tree).
    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> Self::Handle {
        self.new_node(NodeData::Element { name, attrs })
    }

    /// Creates a new comment node and adds it to the arena.
    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        self.new_node(NodeData::Comment(text.to_string()))
    }

    /// Creates a new processing-instruction node and adds it to the arena.
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        self.new_node(NodeData::ProcessingInstruction {
            target: target.to_string(),
            data: data.to_string(),
        })
    }

    /// Appends `child` as the last child of `parent`.
    ///
    /// If `child` is text and the parent's last child is already a text
    /// node, the new text is merged into it instead of creating a new
    /// node, matching the DOM's text-node coalescing behavior.
    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(node_id) => {
                self.append_child(*parent, node_id);
            }
            NodeOrText::AppendText(text) => {
                let last_child = parent.children(&*self.arena.borrow()).next_back();

                if let Some(child_id) = last_child {
                    let mut arena = self.arena.borrow_mut();
                    if let NodeData::Text(existing_text) = arena
                        .get_mut(child_id)
                        .expect("child handle not found in arena")
                        .get_mut()
                    {
                        existing_text.push_str(&text);
                        return;
                    }
                }

                let text_id = self.new_node(NodeData::Text(text.to_string()));
                self.append_child(*parent, text_id);
            }
        }
    }

    /// Inserts `child` immediately before `sibling` in the tree.
    ///
    /// If `child` is text and `sibling`'s previous sibling is already a
    /// text node, the new text is merged into it instead of creating a
    /// new node.
    fn append_before_sibling(&self, sibling: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(node_id) => {
                sibling.insert_before(node_id, &mut *self.arena.borrow_mut());
            }
            NodeOrText::AppendText(text) => {
                let prev_sibling = self
                    .arena
                    .borrow()
                    .get(*sibling)
                    .expect("sibling handle not found in arena")
                    .previous_sibling();

                if let Some(prev_id) = prev_sibling {
                    let mut arena = self.arena.borrow_mut();
                    if let NodeData::Text(existing_text) = arena
                        .get_mut(prev_id)
                        .expect("previous sibling handle not found in arena")
                        .get_mut()
                    {
                        existing_text.push_str(&text);
                        return;
                    }
                }

                let text_id = self.new_node(NodeData::Text(text.to_string()));
                sibling.insert_before(text_id, &mut *self.arena.borrow_mut());
            }
        }
    }

    /// Appends `child` under `element`, ignoring `_prev_element`.
    ///
    /// This crate doesn't special-case the foster-parenting behavior XML
    /// parsers can request via this hook (relevant mainly to HTML table
    /// parsing), so it just delegates to [`append`](Self::append).
    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        _prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        self.append(element, child);
    }

    /// Creates a doctype node from `name`, `public_id`, and `system_id`
    /// and appends it to the document root.
    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let doctype_id = self.new_node(NodeData::Doctype {
            name: name.to_string(),
            public_id: public_id.to_string(),
            system_id: system_id.to_string(),
        });
        self.append_child(self.root, doctype_id);
    }

    /// Adds each attribute in `new_attrs` to `target` that isn't already
    /// present (by name), leaving existing attributes untouched.
    ///
    /// If `target` is not an element, this is a no-op.
    fn add_attrs_if_missing(&self, target: &Self::Handle, new_attrs: Vec<Attribute>) {
        let mut arena = self.arena.borrow_mut();
        if let NodeData::Element { attrs, .. } = arena
            .get_mut(*target)
            .expect("element handle not found in arena")
            .get_mut()
        {
            for new_attr in new_attrs {
                if !attrs.iter().any(|a| a.name == new_attr.name) {
                    attrs.push(new_attr);
                }
            }
        }
    }

    /// Detaches `target` from its parent, removing it (and its subtree)
    /// from the tree.
    fn remove_from_parent(&self, target: &Self::Handle) {
        target.detach(&mut *self.arena.borrow_mut());
    }

    /// Moves all children of `node` to become children of `new_parent`,
    /// preserving their order.
    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        let children: Vec<NodeId> = node.children(&*self.arena.borrow()).collect();
        for child in children {
            self.append_child(*new_parent, child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xml5ever::driver::{XmlParseOpts, parse_document};
    use xml5ever::tendril::TendrilSink;

    #[test]
    fn test_xml_parsing_to_arena() {
        let xml = r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <html xmlns="http://www.w3.org/1999/xhtml">
                <head><title>Test</title></head>
                <body>
                    <p id="first">Hello <b>World</b>!</p>
                </body>
            </html>
        "#;

        let arena = DocumentArena::new();

        let document = parse_document(arena, XmlParseOpts::default())
            .from_utf8()
            .read_from(&mut xml.as_bytes())
            .expect("Failed to parse XML");

        let root = document.root;

        let arena_ref = document.arena.borrow();

        let html_node = root
            .children(&*arena_ref)
            .next_back()
            .expect("Document should have a child");

        let node_data = arena_ref
            .get(html_node)
            .expect("failed to get node data from index tree")
            .get();

        if let NodeData::Element { name, .. } = node_data {
            assert_eq!(name.local.as_ref(), "html");
        } else {
            panic!("Expected html element");
        }
    }
}
