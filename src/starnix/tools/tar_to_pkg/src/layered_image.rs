// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fs::create_dir_all;
use std::io::{Read, copy};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tar::{Archive, Entry};

/// Prefix for "whiteout" files, whose purpose is to hide files from lower layers.
///
/// Docker images use the same whiteout type as AUFS:
///  - A file called ".wh..wh..opq" hides all the files in the same directory from previous layers.
///  - Any other filename starting ".wh." simply hides the corresponding file from previous layers
///    (e.g. ".wh.example.txt" hides "example.txt").
static WHITEOUT_PREFIX: &[u8] = b".wh.";

/// A filename. We don't check/enforce a specific encoding.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Name(Vec<u8>);

impl Name {
    const EMPTY: Name = Name(Vec::new());

    fn is_empty(&self) -> bool {
        self.0 == b""
    }

    fn is_dot(&self) -> bool {
        self.0 == b"."
    }

    fn is_dotdot(&self) -> bool {
        self.0 == b".."
    }

    /// Returns true if this filename is meant to hide the contents of its parent directory.
    fn is_whiteout_opaque(&self) -> bool {
        // https://github.com/opencontainers/image-spec/issues/130 also mentions ".wh.__dir_opaque"
        // in addition to ".wh..wh..opq".
        self.0 == b".wh..wh..opq" || self.0 == b".wh.__dir_opaque"
    }

    /// Returns `Some(name)` if this filename is meant to hide `name`.
    ///
    /// WARNING: Always check is_whiteout_opaque() first to avoid false positives.
    fn strip_whiteout_prefix(&self) -> Option<Name> {
        self.0.strip_prefix(WHITEOUT_PREFIX).map(|slice| Name(slice.to_vec()))
    }
}

/// A sequential ID that is assigned to each added layer.
///
/// We use it to avoid removing entries from the same layer when an opaque whiteout is found.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct LayerIndex(usize);

/// Splits a path into (ancestors, basename).
///
/// Note:
/// - If the path points to the root directory, ([], Name::EMPTY) is returned.
/// - In any other case, ancestors always starts with Name::EMPTY.
fn parse_path(path: &[u8]) -> Result<(Vec<Name>, Name)> {
    // Split path at every '/'.
    let segments: Vec<Name> =
        path.split(|ch| *ch == b'/').map(|slice| Name(slice.to_vec())).collect();

    // We don't support .. in paths.
    if segments.iter().any(|segment| segment.is_dotdot()) {
        bail!("Found \"..\" in path");
    }

    // Ensure there is an empty element at the beginning (as a placeholder for the root directory),
    // but remove any other empty and '.' segments.
    let mut segments: Vec<Name> = [Name::EMPTY]
        .into_iter()
        .chain(segments.into_iter().filter(|segment| !segment.is_empty() && !segment.is_dot()))
        .collect();

    let name = segments.pop().unwrap();
    Ok((segments, name))
}

/// Helper struct to merge several filesystem layers into one.
pub struct LayeredImage {
    /// The root directory.
    root: Rc<Directory>,

    /// ID that will be assigned to the next layer.
    next_layer_index: LayerIndex,

    /// Where files' contents are stored.
    extract_dir: PathBuf,
}

impl LayeredImage {
    /// Create a blank image.
    ///
    /// It can later be populated by calling `add_layer` one or more times. Files added by such
    /// calls will be extracted into the directory given in `extract_dir`, which is created if
    /// non-existing and assumed to be initially empty.
    pub fn new(extract_dir: &Path) -> Result<LayeredImage> {
        create_dir_all(extract_dir)?;
        Ok(LayeredImage {
            root: Rc::new(Directory::default()),
            next_layer_index: LayerIndex(0),
            extract_dir: extract_dir.to_path_buf(),
        })
    }

    /// Apply a new layer on top of the previous ones.
    ///
    /// If `handle_whiteouts` is true, whiteout files will hide the corresponding files from
    /// previous layers. If false, whiteout files are processed as regular files with no special
    /// meaning.
    pub fn add_layer<R: Read>(
        mut self,
        mut archive: Archive<R>,
        handle_whiteouts: bool,
    ) -> Result<LayeredImage> {
        let current_layer_index = self.next_layer_index;
        self.next_layer_index = LayerIndex(current_layer_index.0 + 1);

        // Mapping from paths to inserted nodes, to resolve hard links.
        let mut path_to_node: HashMap<Vec<u8>, NodeRef> = HashMap::new();

        for (i, entry) in archive.entries()?.enumerate() {
            let mut entry = entry?;

            let (ancestors, name) = parse_path(&entry.path_bytes())?;
            if name.is_empty() {
                assert!(ancestors.is_empty(), "Entry must be the root dir");

                // Keep the root dir's entries but rebuild its metadata.
                let entries = self.root.entries.take();
                self.root = Rc::new(Directory {
                    metadata: Metadata::from_entry(&mut entry)?,
                    entries: RefCell::new(entries),
                });
                continue;
            }

            assert!(ancestors.first() == Some(&Name::EMPTY), "Entry must be within root dir");
            let parent = self.get_or_create_directory(&ancestors, current_layer_index)?;
            let mut parent_entries = parent.entries.borrow_mut();

            // If requested, handle the special meaning of whiteout files.
            if handle_whiteouts {
                if name.is_whiteout_opaque() {
                    // Drop all the entries from previous layers.
                    parent_entries
                        .retain(|_, (_, layer_index)| *layer_index == current_layer_index);
                    continue;
                }
                if let Some(name) = name.strip_whiteout_prefix() {
                    // Drop only the entry with the specific name.
                    parent_entries.remove(&name);
                    continue;
                }
            }

            let node = match entry.header().entry_type() {
                tar::EntryType::Regular => {
                    // Generate a unique filename and extract the contents into it.
                    let extracted_name = format!("{}-{}", current_layer_index.0, i);
                    let extracted_path = self.extract_dir.join(&extracted_name);
                    copy(&mut entry, &mut std::fs::File::create(&extracted_path)?)?;

                    let file = File {
                        metadata: Metadata::from_entry(&mut entry)?,
                        data_file_path: extracted_path,
                    };
                    NodeRef::File(Rc::new(file))
                }
                tar::EntryType::Link => {
                    let link_path = entry.link_name_bytes().unwrap().to_vec();
                    let Some(node) = path_to_node.get(&link_path) else {
                        bail!("Hard link does not refer to an already-seen file");
                    };
                    node.clone()
                }
                tar::EntryType::Symlink => {
                    let link_path = entry.link_name_bytes().unwrap().to_vec();
                    let symlink = Symlink {
                        metadata: Metadata::from_entry(&mut entry)?,
                        target: Name(link_path),
                    };
                    NodeRef::Symlink(Rc::new(symlink))
                }
                tar::EntryType::Directory => {
                    // If a directory with the same name already exists, preserve its entries.
                    let entries = match parent_entries.get(&name) {
                        Some((NodeRef::Directory(directory), _)) => directory.entries.take(),
                        _ => HashMap::new(),
                    };

                    let directory = Directory {
                        metadata: Metadata::from_entry(&mut entry)?,
                        entries: RefCell::new(entries),
                    };
                    NodeRef::Directory(Rc::new(directory))
                }
                _ => {
                    unimplemented!("Tar entry type: {:?}", entry.header().entry_type());
                }
            };

            path_to_node.insert(entry.path_bytes().to_vec(), node.clone());
            parent_entries.insert(name, (node, current_layer_index));
        }

        Ok(self)
    }

    /// Ensures that the given path exists as a directory, creating it if necessary.
    pub fn ensure_directory_exists(mut self, path: &str) -> Result<LayeredImage> {
        let current_layer_index = self.next_layer_index;
        self.next_layer_index = LayerIndex(current_layer_index.0 + 1);

        let segments: Vec<Name> =
            path.split("/").map(|substr| Name(substr.as_bytes().to_vec())).collect();
        self.get_or_create_directory(&segments, current_layer_index)?;

        Ok(self)
    }

    /// Seals the file system hierarchy, assigns inode numbers, and returns the resulting root directory.
    pub fn finalize(self, inode_num_generator: &mut dyn FnMut() -> u64) -> Directory {
        let root_dir = Rc::try_unwrap(self.root)
            .map_err(|_| ())
            .expect("No entries should point to the root directory");

        // Execute a visit to assign inode numbers.
        let mut visitor = AssignInodeNumVisitor { inode_num_generator };
        visitor.visit_directory(b"", &root_dir);

        root_dir
    }

    /// Resolves the given path, whose last segment is assumed to be adirectory, creating
    /// intermediate directories in the process, if they don't exist yet.
    fn get_or_create_directory(
        &self,
        segments: &[Name],
        current_layer_index: LayerIndex,
    ) -> Result<Rc<Directory>> {
        let mut it = segments.iter();

        // Start from the root directory.
        assert!(it.next() == Some(&Name::EMPTY), "Path must start from the root dir");
        let mut cur = self.root.clone();

        while let Some(segment) = it.next() {
            let next = {
                let mut entries = cur.entries.borrow_mut();
                let entry = entries.entry(segment.clone()).or_insert_with(|| {
                    (NodeRef::Directory(Rc::new(Directory::default())), current_layer_index)
                });

                match entry {
                    (NodeRef::Directory(next), layer_index) => {
                        // Record the fact that this layer acknowledges the existence of this dir,
                        // to prevent it from being discarded by a whiteout in the same layer.
                        *layer_index = current_layer_index;
                        next.clone()
                    }
                    (node, layer_index) if *layer_index != current_layer_index => {
                        // A previous layer had a non-directory with the same name, replace it.
                        let new_dir = Rc::new(Directory::default());
                        *node = NodeRef::Directory(new_dir.clone());
                        *layer_index = current_layer_index;
                        new_dir
                    }
                    _ => {
                        bail!(
                            "The same layer references both a directory and a non-directory with the same name"
                        );
                    }
                }
            };
            cur = next;
        }

        Ok(cur)
    }
}

/// Metadata about a given inode.
pub struct Metadata {
    mode: u16,
    uid: u16,
    gid: u16,
    xattrs: BTreeMap<Box<[u8]>, Box<[u8]>>,

    // Assigned by `AssignInodeNumberVisitor` when `LayeredImage::finalize` is called.
    inode_num: Cell<Option<u64>>,
}

impl Metadata {
    fn from_entry<R: Read>(entry: &mut Entry<'_, R>) -> Result<Metadata> {
        let header = entry.header();
        let mode = (header.mode()? & 0o7777).try_into().unwrap();
        let uid = header.uid()?.try_into().context("uid")?;
        let gid = header.gid()?.try_into().context("gid")?;

        let mut xattrs = BTreeMap::new();
        if let Some(extensions) = entry.pax_extensions()? {
            for extension in extensions {
                let extension = extension?;
                if let Some(key) = extension.key_bytes().strip_prefix(b"SCHILY.xattr.") {
                    xattrs.insert(key.into(), extension.value_bytes().into());
                }
            }
        }

        Ok(Metadata { mode, uid, gid, xattrs, inode_num: Cell::new(None) })
    }

    pub fn mode(&self) -> u16 {
        self.mode
    }

    pub fn uid(&self) -> u16 {
        self.uid
    }

    pub fn gid(&self) -> u16 {
        self.gid
    }

    pub fn extended_attributes(&self) -> &BTreeMap<Box<[u8]>, Box<[u8]>> {
        &self.xattrs
    }

    pub fn inode_num(&self) -> u64 {
        self.inode_num.get().expect("this method is never called before assigning inode numbers")
    }
}

#[derive(Clone)]
enum NodeRef {
    File(Rc<File>),
    Symlink(Rc<Symlink>),
    Directory(Rc<Directory>),
}

pub struct File {
    metadata: Metadata,
    data_file_path: PathBuf,
}

impl File {
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns the path of the local file with the contents of this file.
    pub fn data_file_path(&self) -> &Path {
        &self.data_file_path
    }
}

pub struct Symlink {
    metadata: Metadata,
    target: Name,
}

impl Symlink {
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns the target of this symbolik link.
    pub fn target(&self) -> &[u8] {
        &self.target.0
    }
}

pub struct Directory {
    metadata: Metadata,

    /// Children of this directory, with the index of the layer that added them.
    entries: RefCell<HashMap<Name, (NodeRef, LayerIndex)>>,
}

impl Default for Directory {
    /// Creates an empty directory with "normal" metadata.
    ///
    /// It is used for directory that are implicitly referenced in paths as ancestors but never
    /// explicitly listed in the source archive.
    fn default() -> Self {
        Self {
            metadata: Metadata {
                mode: 0o755,
                uid: 0,
                gid: 0,
                xattrs: BTreeMap::new(),
                inode_num: Cell::new(None),
            },
            entries: RefCell::new(HashMap::new()),
        }
    }
}

impl Directory {
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn visit(&self, visitor: &mut dyn DirectoryVisitor) {
        // Sort entries to avoid nondeterminism in the visit order.
        let mut entries: Vec<_> = self
            .entries
            .borrow()
            .iter()
            .map(|(name, (node, _))| (name.clone(), node.clone()))
            .collect();
        entries.sort_by(|(name_a, _), (name_b, _)| name_a.cmp(name_b));

        // Run visitor on each entry.
        for (Name(name), node) in &entries {
            match node {
                NodeRef::File(file) => visitor.visit_file(name, file),
                NodeRef::Symlink(symlink) => visitor.visit_symlink(name, symlink),
                NodeRef::Directory(directory) => visitor.visit_directory(name, directory),
            };
        }
    }
}

pub trait DirectoryVisitor {
    fn visit_file(&mut self, name: &[u8], file: &File);
    fn visit_symlink(&mut self, name: &[u8], symlink: &Symlink);
    fn visit_directory(&mut self, name: &[u8], directory: &Directory);
}

/// A directory visitor that assigns inode numbers to all reachable nodes.
struct AssignInodeNumVisitor<'a> {
    inode_num_generator: &'a mut dyn FnMut() -> u64,
}

impl AssignInodeNumVisitor<'_> {
    fn visit_metadata(&mut self, metadata: &Metadata) -> bool {
        if metadata.inode_num.get().is_none() {
            metadata.inode_num.set(Some((self.inode_num_generator)()));
            true
        } else {
            false
        }
    }
}

impl DirectoryVisitor for AssignInodeNumVisitor<'_> {
    fn visit_file(&mut self, _name: &[u8], file: &File) {
        self.visit_metadata(file.metadata());
    }

    fn visit_symlink(&mut self, _name: &[u8], symlink: &Symlink) {
        self.visit_metadata(symlink.metadata());
    }

    fn visit_directory(&mut self, _name: &[u8], directory: &Directory) {
        if self.visit_metadata(directory.metadata()) {
            directory.visit(self);
        }
    }
}
