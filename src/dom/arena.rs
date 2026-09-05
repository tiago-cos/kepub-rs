use std::cell::RefCell;

use indextree::{Arena, NodeId};
use markup5ever::{Attribute, QualName};

/// Represents the specific data held by a single node in our DOM tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeData {
    /// The root of the entire tree.
    Document,
    /// A `<!DOCTYPE html>` declaration.
    Doctype {
        /// The doctype name (e.g. `"html"`).
        name: String,
        /// The public identifier, if any.
        public_id: String,
        /// The system identifier, if any.
        system_id: String,
    },
    /// Standard text content.
    Text(String),
    /// An `<!-- ... -->` comment.
    Comment(String),
    /// An XHTML element such as `<p>` or `<div class="book-inner">`.
    Element {
        /// The element's fully-qualified tag name (including namespace).
        name: QualName,
        /// The element's attributes, in document order.
        attrs: Vec<Attribute>,
    },
    /// A processing instruction such as `<?xml-stylesheet ... ?>`.
    ProcessingInstruction {
        /// The processing instruction's target.
        target: String,
        /// The processing instruction's data.
        data: String,
    },
}

/// The arena containing all nodes for a single parsed document.
///
/// Nodes are stored in an [`indextree::Arena`] and referenced by
/// [`NodeId`], giving cheap, `Copy`-able handles into the tree without
/// borrow-checker fights. The arena itself is wrapped in a [`RefCell`] so
/// nodes can be mutated (e.g. appended) through a shared `&DocumentArena`.
pub struct DocumentArena {
    /// The underlying node storage.
    pub arena: RefCell<Arena<NodeData>>,
    /// The [`NodeId`] of the tree's root [`NodeData::Document`] node.
    pub root: NodeId,
}

impl DocumentArena {
    /// Initializes a new, empty document arena with a root
    /// [`NodeData::Document`] node.
    pub fn new() -> Self {
        let mut arena = Arena::new();
        let root = arena.new_node(NodeData::Document);
        Self {
            arena: RefCell::new(arena),
            root,
        }
    }

    /// Creates a new node holding `data` and adds it to the arena.
    ///
    /// The returned [`NodeId`] is not yet attached to the tree; use
    /// [`append_child`](Self::append_child) to place it.
    pub fn new_node(&self, data: NodeData) -> NodeId {
        self.arena.borrow_mut().new_node(data)
    }

    /// Appends `child` as the last child of `parent`.
    pub fn append_child(&self, parent: NodeId, child: NodeId) {
        parent.append(child, &mut *self.arena.borrow_mut());
    }
}

impl Default for DocumentArena {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markup5ever::{local_name, ns};

    #[test]
    fn test_arena_creation_and_appending() {
        let doc = DocumentArena::new();

        assert_eq!(
            *doc.arena
                .borrow()
                .get(doc.root)
                .expect("failed to get document root node")
                .get(),
            NodeData::Document
        );

        let div_data = NodeData::Element {
            name: QualName::new(None, ns!(html), local_name!("div")),
            attrs: vec![Attribute {
                name: QualName::new(None, ns!(), local_name!("id")),
                value: "book-inner".into(),
            }],
        };
        let div_id = doc.new_node(div_data);

        let text_id = doc.new_node(NodeData::Text("Hello World".to_string()));

        doc.append_child(doc.root, div_id);
        doc.append_child(div_id, text_id);

        let arena = doc.arena.borrow();
        let children: Vec<NodeId> = doc.root.children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], div_id);

        let div_children: Vec<NodeId> = div_id.children(&arena).collect();
        assert_eq!(div_children.len(), 1);
        assert_eq!(div_children[0], text_id);
    }
}
