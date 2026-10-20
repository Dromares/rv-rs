//! Naming a node of the tree.
//!
//! The tree has no parent pointers and the fixer adds and removes nodes while it works, so it
//! never holds a reference across a step. A node is named by the way down to it: the type and
//! name of every node from the first child of the root. Names do not change while fixing (the
//! DAT's name is `name`, the on-disk case is `file_name`), so a path stays valid until its node
//! is removed.

use std::path::PathBuf;

use rv_core::{FileType, RvFile, Settings, TreeSelect};

/// One step of a [`NodePath`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Seg {
    pub file_type: FileType,
    pub name: String,
}

/// The way down from the root of the tree to a node (the root itself is the empty path).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodePath(pub Vec<Seg>);

impl NodePath {
    pub fn root() -> NodePath {
        NodePath(Vec::new())
    }

    pub fn of(node: &RvFile) -> Seg {
        Seg {
            file_type: node.file_type,
            name: node.name.clone(),
        }
    }

    pub fn child(&self, file_type: FileType, name: &str) -> NodePath {
        let mut v = self.0.clone();
        v.push(Seg {
            file_type,
            name: name.to_owned(),
        });
        NodePath(v)
    }

    /// The path of the parent; the root is its own parent.
    pub fn parent(&self) -> NodePath {
        let mut v = self.0.clone();
        v.pop();
        NodePath(v)
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.0.len()
    }

    pub fn last(&self) -> Option<&Seg> {
        self.0.last()
    }

    /// True when `self` is `other` or below it.
    pub fn starts_with(&self, other: &NodePath) -> bool {
        self.0.len() >= other.0.len() && self.0[..other.0.len()] == other.0[..]
    }

    /// `TreeFullName`: the names joined with `/`.
    pub fn tree_name(&self) -> String {
        let names: Vec<&str> = self.0.iter().map(|s| s.name.as_str()).collect();
        names.join("/")
    }

    /// `TreeFullName` of the parent.
    pub fn parent_tree_name(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        let names: Vec<&str> = self.0[..self.0.len() - 1]
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        names.join("/")
    }
}

impl std::fmt::Display for NodePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.tree_name())
    }
}

/// The node at `path`.
pub fn get<'a>(root: &'a RvFile, path: &NodePath) -> Option<&'a RvFile> {
    let mut node = root;
    for seg in &path.0 {
        let (found, i) = node.child_name_search(seg.file_type, &seg.name);
        if !found {
            return None;
        }
        node = &node.children[i];
    }
    Some(node)
}

/// The node at `path`, mutable.
pub fn get_mut<'a>(root: &'a mut RvFile, path: &NodePath) -> Option<&'a mut RvFile> {
    let mut node = root;
    for seg in &path.0 {
        let (found, i) = node.child_name_search(seg.file_type, &seg.name);
        if !found {
            return None;
        }
        node = &mut node.children[i];
    }
    Some(node)
}

/// The tree check state that governs `path`: the row of the nearest node on the way down that
/// has one, the node itself included (`RvFile.treeType`, `rvFile.cs:276-286`). A path outside any
/// row (not possible below the root's children) reads as unselected.
pub fn tree_select(root: &RvFile, path: &NodePath) -> TreeSelect {
    let mut node = root;
    let mut select = TreeSelect::UN_SELECTED;
    for seg in &path.0 {
        let (found, i) = node.child_name_search(seg.file_type, &seg.name);
        if !found {
            break;
        }
        node = &node.children[i];
        if let Some(row) = node.tree {
            select = row.checked;
        }
    }
    select
}

/// `GetPhysicalPath` of the item at `path`: the tree path with the on-disk case of every name,
/// mapped through the directory mappings. For an archive entry this is the path the entry would
/// have inside the archive's directory, which is not a file; use [`container_path`].
pub fn physical(root: &RvFile, settings: &Settings, path: &NodePath) -> PathBuf {
    let mut node = root;
    let mut case_names: Vec<&str> = Vec::new();
    for seg in &path.0 {
        let (found, i) = node.child_name_search(seg.file_type, &seg.name);
        if !found {
            case_names.push(seg.name.as_str());
            continue;
        }
        node = &node.children[i];
        case_names.push(node.name_case());
    }
    PathBuf::from(settings.physical_path(&case_names.join("/")))
}

/// The file on disk that holds the item at `path`: the item itself for a file or an archive, its
/// archive for an archive entry.
pub fn container_path(root: &RvFile, settings: &Settings, path: &NodePath) -> PathBuf {
    match path.last() {
        Some(seg) if is_entry(seg.file_type) => physical(root, settings, &path.parent()),
        _ => physical(root, settings, path),
    }
}

pub fn is_entry(t: FileType) -> bool {
    t == FileType::FILE_ZIP || t == FileType::FILE_SEVEN_ZIP
}
