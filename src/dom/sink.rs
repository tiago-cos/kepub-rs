use indextree::NodeId;
use markup5ever::interface::{ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::{Attribute, ExpandedName, LocalName, Namespace, QualName};
use std::borrow::Cow;
use tendril::StrTendril;

use crate::dom::arena::{DocumentArena, NodeData};

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

    fn finish(self) -> Self::Output {
        self
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        eprintln!("XML Parse Error: {msg}");
    }

    fn get_document(&self) -> Self::Handle {
        self.root
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        let arena = self.arena.borrow();
        match arena.get(*target).expect("Node not found").get() {
            NodeData::Element { name, .. } => OwnedElemName(name.clone()),
            _ => panic!("Not an element!"),
        }
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        *target
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> Self::Handle {
        self.new_node(NodeData::Element { name, attrs })
    }

    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        self.new_node(NodeData::Comment(text.to_string()))
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        self.new_node(NodeData::ProcessingInstruction {
            target: target.to_string(),
            data: data.to_string(),
        })
    }

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

    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        _prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        self.append(element, child);
    }

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

    fn remove_from_parent(&self, target: &Self::Handle) {
        target.detach(&mut *self.arena.borrow_mut());
    }

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
