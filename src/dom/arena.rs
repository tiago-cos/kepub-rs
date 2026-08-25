use std::cell::RefCell;

use indextree::{Arena, NodeId};
use markup5ever::{Attribute, QualName};

/// Represents the specific data held by a single node in our DOM tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeData {
    /// The root of the entire tree
    Document,

    /// The <!DOCTYPE html> declaration
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },

    /// Standard text content
    Text(String),

    /// <!-- Comments -->
    Comment(String),

    /// An XHTML element like <p> or <div class="book-inner">
    Element {
        name: QualName,
        attrs: Vec<Attribute>,
    },

    /// <?xml-stylesheet ... ?>
    ProcessingInstruction { target: String, data: String },
}

/// The Arena containing all nodes for a single parsed document.
pub struct DocumentArena {
    pub arena: RefCell<Arena<NodeData>>,
    pub root: NodeId,
}

impl DocumentArena {
    /// Initializes a new, empty document arena with a root Document node.
    pub fn new() -> Self {
        let mut arena = Arena::new();
        let root = arena.new_node(NodeData::Document);

        Self {
            arena: RefCell::new(arena),
            root,
        }
    }

    /// Helper function to create a new node and add it to the arena
    pub fn new_node(&self, data: NodeData) -> NodeId {
        self.arena.borrow_mut().new_node(data)
    }

    /// Appends a child to a parent node safely.
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

        // Ensure root is a Document
        assert_eq!(
            *doc.arena
                .borrow()
                .get(doc.root)
                .expect("failed to get document root node")
                .get(),
            NodeData::Document
        );

        // Create an element: <div id="book-inner">
        let div_data = NodeData::Element {
            name: QualName::new(None, ns!(html), local_name!("div")),
            attrs: vec![Attribute {
                name: QualName::new(None, ns!(), local_name!("id")),
                value: "book-inner".into(),
            }],
        };
        let div_id = doc.new_node(div_data);

        // Create a text node: "Hello World"
        let text_id = doc.new_node(NodeData::Text("Hello World".to_string()));

        // Structure the tree: Document -> div -> text
        doc.append_child(doc.root, div_id);
        doc.append_child(div_id, text_id);

        // Verify the structure using indextree iterators
        let arena = doc.arena.borrow();

        let children: Vec<NodeId> = doc.root.children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], div_id);

        let div_children: Vec<NodeId> = div_id.children(&arena).collect();
        assert_eq!(div_children.len(), 1);
        assert_eq!(div_children[0], text_id);
    }
}
