// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A helper to build a tree of directory nodes.  It is useful in case when a nested tree is
//! desired, with specific nodes to be inserted as the leafs of this tree.  It is similar to the
//! functionality provided by the [`vfs_macros::pseudo_directory!`] macro, except that the macro
//! expects the tree structure to be defined at compile time, while this helper allows the tree
//! structure to be dynamic.

use crate::directory::entry::DirectoryEntry;
use crate::directory::helper::DirectlyMutable;
use crate::directory::immutable::Simple;

use flex_fuchsia_io as fio;
use itertools::Itertools;
use name::{Name, ParseNameError};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::marker::PhantomData;
use std::slice::Iter;
use std::sync::Arc;
use thiserror::Error;

/// Represents a paths provided to [`TreeBuilder::add_entry()`].  See [`TreeBuilder`] for details.
// I think it would be a bit more straightforward to have two different types that implement a
// `Path` trait, `OwnedPath` and `SharedPath`.  But, `add_entry` then needs two type variables: one
// for the type of the value passed in, and one for the type of the `Path` trait (either
// `OwnedPath` or `SharedPath`).  Type inference fails with two variables requiring explicit type
// annotation.  And that defeats the whole purpose of the overloading in the API.
//
//     pub fn add_entry<'path, 'components: 'path, F, P: 'path>(
//         &mut self,
//         path: F,
//         entry: Arc<dyn DirectoryEntry>,
//     ) -> Result<(), Error>
//
// Instead we capture the underlying implementation of the path in the `Impl` type and just wrap
// our type around it.  `'components` and `AsRef` constraints on the struct itself are not actually
// needed, but it makes it more the usage a bit easier to understand.
pub struct Path<'components, Impl>
where
    Impl: AsRef<[&'components str]>,
{
    path: Impl,
    _components: PhantomData<&'components str>,
}

impl<'components, Impl> Path<'components, Impl>
where
    Impl: AsRef<[&'components str]>,
{
    fn iter<'path>(&'path self) -> Iter<'path, &'components str>
    where
        'components: 'path,
    {
        self.path.as_ref().iter()
    }
}

impl<'component> From<&'component str> for Path<'component, Vec<&'component str>> {
    fn from(component: &'component str) -> Self {
        Path { path: vec![component], _components: PhantomData }
    }
}

impl<'components, Impl> From<Impl> for Path<'components, Impl>
where
    Impl: AsRef<[&'components str]>,
{
    fn from(path: Impl) -> Self {
        Path { path, _components: PhantomData }
    }
}

impl<'components, Impl> fmt::Display for Path<'components, Impl>
where
    Impl: AsRef<[&'components str]>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.iter().format("/"))
    }
}

pub enum TreeBuilder {
    Directory(HashMap<Name, TreeBuilder>, #[cfg(feature = "fdomain")] Arc<flex_client::Client>),
    Leaf(Arc<dyn DirectoryEntry>),
}

/// Collects a number of [`DirectoryEntry`] nodes and corresponding paths and the constructs a tree
/// of [`crate::directory::immutable::simple::Simple`] directories that hold these nodes.  This is a
/// companion tool, related to the [`vfs_macros::pseudo_directory!`] macro, except that it is
/// collecting the paths dynamically, while the [`vfs_macros::pseudo_directory!`] expects the tree
/// to be specified at compilation time.
///
/// Note that the final tree is build as a result of the [`Self::build()`] method that consumes the
/// builder.  You would need to use the [`crate::directory::helper::DirectlyMutable::add_entry()`]
/// interface to add any new nodes afterwards (a [`crate::directory::watchers::Controller`] APIs).
impl TreeBuilder {
    /// Constructs an empty builder.  It is always an empty [`crate::directory::immutable::Simple`]
    /// directory.
    pub fn empty_dir(#[cfg(feature = "fdomain")] client: Arc<flex_client::Client>) -> Self {
        TreeBuilder::Directory(
            HashMap::new(),
            #[cfg(feature = "fdomain")]
            client,
        )
    }

    /// Adds a [`DirectoryEntry`] at the specified path.  It can be either a file or a directory.
    /// In case it is a directory, this builder cannot add new child nodes inside of the added
    /// directory.  Any `entry` is treated as an opaque "leaf" as far as the builder is concerned.
    pub fn add_entry<'components, P: 'components, PathImpl>(
        &mut self,
        path: P,
        entry: Arc<dyn DirectoryEntry>,
    ) -> Result<(), Error>
    where
        P: Into<Path<'components, PathImpl>>,
        PathImpl: AsRef<[&'components str]>,
    {
        let path = path.into();
        let traversed = vec![];
        let mut rest = path.iter();
        match rest.next() {
            None => Err(Error::EmptyPath),
            Some(name) => self.add_path(
                &path,
                traversed,
                name,
                rest,
                |entries, name, full_path, _traversed| match entries
                    .insert(name, TreeBuilder::Leaf(entry))
                {
                    None => Ok(()),
                    Some(TreeBuilder::Directory(..)) => {
                        Err(Error::LeafOverDirectory { path: full_path.to_string() })
                    }
                    Some(TreeBuilder::Leaf(_)) => {
                        Err(Error::LeafOverLeaf { path: full_path.to_string() })
                    }
                },
            ),
        }
    }

    #[cfg(feature = "fdomain")]
    fn domain(&self) -> Option<Arc<flex_client::Client>> {
        match self {
            TreeBuilder::Directory(_, client) => Some(client.clone()),
            TreeBuilder::Leaf(_) => None,
        }
    }

    /// Adds an empty directory into the generated tree at the specified path.  The difference with
    /// the [`crate::directory::helper::DirectlyMutable::add_entry`] that adds an entry that is a directory is that the builder can can only
    /// add leaf nodes.  In other words, code like this will fail:
    ///
    /// ```should_panic
    /// use crate::{
    ///     directory::immutable::Simple,
    ///     file::vmo::read_only,
    /// };
    ///
    /// let mut tree = TreeBuilder::empty_dir();
    /// tree.add_entry(&["dir1"], Simple::new());
    /// tree.add_entry(&["dir1", "nested"], read_only(b"A file"));
    /// ```
    ///
    /// The problem is that the builder does not see "dir1" as a directory, but as a leaf node that
    /// it cannot descend into.
    ///
    /// If you use `add_empty_dir()` instead, it would work:
    ///
    /// ```
    /// use crate::file::vmo::read_only;
    ///
    /// let mut tree = TreeBuilder::empty_dir();
    /// tree.add_empty_dir(&["dir1"]);
    /// tree.add_entry(&["dir1", "nested"], read_only(b"A file"));
    /// ```
    pub fn add_empty_dir<'components, P: 'components, PathImpl>(
        &mut self,
        path: P,
    ) -> Result<(), Error>
    where
        P: Into<Path<'components, PathImpl>>,
        PathImpl: AsRef<[&'components str]>,
    {
        let path = path.into();
        let traversed = vec![];
        let mut rest = path.iter();
        #[cfg(feature = "fdomain")]
        let client = self.domain();
        match rest.next() {
            None => Err(Error::EmptyPath),
            Some(name) => self.add_path(
                &path,
                traversed,
                name,
                rest,
                |entries, name, full_path, traversed| match entries.entry(name).or_insert_with(
                    || {
                        TreeBuilder::Directory(
                            HashMap::new(),
                            #[cfg(feature = "fdomain")]
                            client.clone().unwrap(),
                        )
                    },
                ) {
                    TreeBuilder::Directory(..) => Ok(()),
                    TreeBuilder::Leaf(_) => Err(Error::EntryInsideLeaf {
                        path: full_path.to_string(),
                        traversed: traversed.iter().join("/"),
                    }),
                },
            ),
        }
    }

    fn add_path<'path, 'components: 'path, PathImpl, Inserter>(
        &mut self,
        full_path: &'path Path<'components, PathImpl>,
        mut traversed: Vec<&'components str>,
        name: &'components str,
        mut rest: Iter<'path, &'components str>,
        inserter: Inserter,
    ) -> Result<(), Error>
    where
        PathImpl: AsRef<[&'components str]>,
        Inserter: FnOnce(
            &mut HashMap<Name, TreeBuilder>,
            Name,
            &Path<'components, PathImpl>,
            Vec<&'components str>,
        ) -> Result<(), Error>,
    {
        let parsed_name =
            Name::try_from(name.to_string()).map_err(|error| Error::InvalidComponent {
                path: full_path.to_string(),
                component: name.to_string(),
                error,
            })?;

        #[cfg(feature = "fdomain")]
        let client = self.domain();
        match self {
            TreeBuilder::Directory(entries, ..) => match rest.next() {
                None => inserter(entries, parsed_name, full_path, traversed),
                Some(next_component) => {
                    traversed.push(name);
                    match entries.entry(parsed_name) {
                        Entry::Vacant(slot) => {
                            let mut child = TreeBuilder::Directory(
                                HashMap::new(),
                                #[cfg(feature = "fdomain")]
                                client.unwrap(),
                            );
                            child.add_path(full_path, traversed, next_component, rest, inserter)?;
                            slot.insert(child);
                            Ok(())
                        }
                        Entry::Occupied(mut slot) => slot.get_mut().add_path(
                            full_path,
                            traversed,
                            next_component,
                            rest,
                            inserter,
                        ),
                    }
                }
            },
            TreeBuilder::Leaf(_) => Err(Error::EntryInsideLeaf {
                path: full_path.to_string(),
                traversed: traversed.iter().join("/"),
            }),
        }
    }

    // Helper function for building a tree with a default inode generator. Use if you don't
    // care about directory inode values.
    pub fn build(self) -> Arc<Simple> {
        let mut generator = |_: &str| -> u64 { fio::INO_UNKNOWN };
        self.build_with_inode_generator(&mut generator)
    }

    /// Consumes the builder, producing a tree with all the nodes provided to
    /// [`crate::directory::helper::DirectlyMutable::add_entry()`] at their respective locations.
    /// The tree itself is built using [`crate::directory::immutable::Simple`]
    /// nodes, and the top level is a directory.
    pub fn build_with_inode_generator<F>(self, get_inode: &mut F) -> Arc<Simple>
    where
        F: for<'a> FnMut(&'a str) -> u64,
    {
        match self {
            TreeBuilder::Directory(mut entries, ..) => {
                let res = Simple::new_with_inode(get_inode("."));
                for (name, child) in entries.drain() {
                    let child = child.build_dyn(&name, get_inode);
                    res.add_entry_impl(name, child, /*overwrite=*/ false)
                        .map_err(|status| format!("Status: {}", status))
                        .expect(
                            "Internal error. We have already checked all the entry names. \
                             There should be no collisions.",
                        );
                }
                res
            }
            TreeBuilder::Leaf(_) => {
                panic!("Leaf nodes should not be buildable through the public API.")
            }
        }
    }

    fn build_dyn<F>(self, dir: &str, get_inode: &mut F) -> Arc<dyn DirectoryEntry>
    where
        F: for<'a> FnMut(&'a str) -> u64,
    {
        match self {
            TreeBuilder::Directory(mut entries, ..) => {
                let res = Simple::new_with_inode(get_inode(dir));
                for (name, child) in entries.drain() {
                    let child = child.build_dyn(&name, get_inode);
                    res.add_entry(name, child)
                        .map_err(|status| format!("Status: {}", status))
                        .expect(
                            "Internal error.  We have already checked all the entry names. \
                             There should be no collisions, nor overly long names.",
                        );
                }
                res
            }
            TreeBuilder::Leaf(entry) => entry,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("`add_entry` requires a non-empty path")]
    EmptyPath,

    #[error(
        "Path component is invalid.\n\
                   Path: {}\n\
                   Component: '{}'\n\
                   Error: '{}'",
        path,
        component,
        error
    )]
    InvalidComponent { path: String, component: String, error: ParseNameError },

    #[error(
        "Trying to insert a leaf over an existing directory.\n\
                   Path: {}",
        path
    )]
    LeafOverDirectory { path: String },

    #[error(
        "Trying to overwrite one leaf with another.\n\
                   Path: {}",
        path
    )]
    LeafOverLeaf { path: String },

    #[error(
        "Trying to insert an entry inside a leaf.\n\
                   Leaf path: {}\n\
                   Path been inserted: {}",
        path,
        traversed
    )]
    EntryInsideLeaf { path: String, traversed: String },
}

#[cfg(test)]
mod tests {
    use super::{Error, Simple, TreeBuilder};

    // Macros are exported into the root of the crate.
    use crate::directory::serve;
    use crate::{assert_close, assert_read, file, pseudo_directory};

    use flex_fuchsia_io as fio;
    #[cfg(not(feature = "fdomain"))]
    use fuchsia_fs::directory::{DirEntry, DirentKind, open_directory, open_file, readdir};
    #[cfg(feature = "fdomain")]
    use fuchsia_fs_fdomain::directory::{DirEntry, DirentKind, open_directory, open_file, readdir};

    #[cfg(feature = "fdomain")]
    fn empty_dir() -> TreeBuilder {
        TreeBuilder::empty_dir(flex_local::local_client_empty())
    }

    #[cfg(not(feature = "fdomain"))]
    fn empty_dir() -> TreeBuilder {
        TreeBuilder::empty_dir()
    }

    async fn assert_open_file_contents(
        root: &fio::DirectoryProxy,
        path: &str,
        flags: fio::Flags,
        expected_contents: &str,
    ) {
        let file = open_file(&root, path, flags).await.unwrap();
        assert_read!(file, expected_contents);
        assert_close!(file);
    }

    async fn get_id_of_path(
        root: &fio::DirectoryProxy,
        path: &str,
        #[cfg(feature = "fdomain")] client: &std::sync::Arc<flex_client::Client>,
    ) -> u64 {
        #[cfg(feature = "fdomain")]
        let (proxy, server) = client.create_proxy::<fio::NodeMarker>();
        #[cfg(not(feature = "fdomain"))]
        let (proxy, server) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
        root.open(path, fio::PERM_READABLE, &Default::default(), server.into_channel())
            .expect("failed to call open");
        let (_, immutable_attrs) = proxy
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .expect("FIDL call failed")
            .expect("GetAttributes failed");
        immutable_attrs.id.expect("ID missing from GetAttributes response")
    }

    #[fuchsia::test]
    async fn vfs_with_custom_inodes() {
        let mut tree = empty_dir();
        tree.add_entry(&["a", "b", "file"], file::read_only(b"A content")).unwrap();
        tree.add_entry(&["a", "c", "file"], file::read_only(b"B content")).unwrap();

        let mut get_inode = |name: &str| -> u64 {
            match name {
                "a" => 1,
                "b" => 2,
                "c" => 3,
                _ => fio::INO_UNKNOWN,
            }
        };
        let root = tree.build_with_inode_generator(&mut get_inode);
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);
        assert_eq!(
            get_id_of_path(
                &root,
                "a",
                #[cfg(feature = "fdomain")]
                &scope.domain()
            )
            .await,
            1
        );
        assert_eq!(
            get_id_of_path(
                &root,
                "a/b",
                #[cfg(feature = "fdomain")]
                &scope.domain()
            )
            .await,
            2
        );
        assert_eq!(
            get_id_of_path(
                &root,
                "a/c",
                #[cfg(feature = "fdomain")]
                &scope.domain()
            )
            .await,
            3
        );
    }

    #[fuchsia::test]
    async fn two_files() {
        let mut tree = empty_dir();
        tree.add_entry("a", file::read_only(b"A content")).unwrap();
        tree.add_entry("b", file::read_only(b"B content")).unwrap();

        let root = tree.build();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);

        assert_eq!(
            readdir(&root).await.unwrap(),
            vec![
                DirEntry { name: String::from("a"), kind: DirentKind::File },
                DirEntry { name: String::from("b"), kind: DirentKind::File },
            ]
        );
        assert_open_file_contents(&root, "a", fio::PERM_READABLE, "A content").await;
        assert_open_file_contents(&root, "b", fio::PERM_READABLE, "B content").await;

        assert_close!(root);
    }

    #[fuchsia::test]
    async fn overlapping_paths() {
        let mut tree = empty_dir();
        tree.add_entry(&["one", "two"], file::read_only(b"A")).unwrap();
        tree.add_entry(&["one", "three"], file::read_only(b"B")).unwrap();
        tree.add_entry("four", file::read_only(b"C")).unwrap();

        let root = tree.build();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);

        assert_eq!(
            readdir(&root).await.unwrap(),
            vec![
                DirEntry { name: String::from("four"), kind: DirentKind::File },
                DirEntry { name: String::from("one"), kind: DirentKind::Directory },
            ]
        );
        let one_dir = open_directory(&root, "one", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&one_dir).await.unwrap(),
            vec![
                DirEntry { name: String::from("three"), kind: DirentKind::File },
                DirEntry { name: String::from("two"), kind: DirentKind::File },
            ]
        );
        assert_close!(one_dir);

        assert_open_file_contents(&root, "one/two", fio::PERM_READABLE, "A").await;
        assert_open_file_contents(&root, "one/three", fio::PERM_READABLE, "B").await;
        assert_open_file_contents(&root, "four", fio::PERM_READABLE, "C").await;

        assert_close!(root);
    }

    #[fuchsia::test]
    async fn directory_leaf() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        #[cfg(feature = "fdomain")]
        let _dummy_handle = client.create_proxy::<fio::NodeMarker>();
        #[cfg(not(feature = "fdomain"))]
        let client = flex_client::fidl::ZirconClient;
        let _ = &client;
        let etc = pseudo_directory! {
            "fstab" => file::read_only(b"/dev/fs /"),
            "ssh" => pseudo_directory! {
                "sshd_config" => file::read_only(b"# Empty"),
            },
        };

        let mut tree = empty_dir();
        tree.add_entry("etc", etc).unwrap();
        tree.add_entry("uname", file::read_only(b"Fuchsia")).unwrap();

        let root = tree.build();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);

        assert_eq!(
            readdir(&root).await.unwrap(),
            vec![
                DirEntry { name: String::from("etc"), kind: DirentKind::Directory },
                DirEntry { name: String::from("uname"), kind: DirentKind::File },
            ]
        );
        let etc_dir = open_directory(&root, "etc", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&etc_dir).await.unwrap(),
            vec![
                DirEntry { name: String::from("fstab"), kind: DirentKind::File },
                DirEntry { name: String::from("ssh"), kind: DirentKind::Directory },
            ]
        );
        assert_close!(etc_dir);
        let ssh_dir = open_directory(&root, "etc/ssh", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&ssh_dir).await.unwrap(),
            vec![DirEntry { name: String::from("sshd_config"), kind: DirentKind::File }]
        );
        assert_close!(ssh_dir);

        assert_open_file_contents(&root, "etc/fstab", fio::PERM_READABLE, "/dev/fs /").await;
        assert_open_file_contents(&root, "etc/ssh/sshd_config", fio::PERM_READABLE, "# Empty")
            .await;
        assert_open_file_contents(&root, "uname", fio::PERM_READABLE, "Fuchsia").await;

        assert_close!(root);
    }

    #[fuchsia::test]
    async fn add_empty_dir_populate_later() {
        let mut tree = empty_dir();
        tree.add_empty_dir(&["one", "two"]).unwrap();
        tree.add_entry(&["one", "two", "three"], file::read_only(b"B")).unwrap();

        let root = tree.build();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);

        assert_eq!(
            readdir(&root).await.unwrap(),
            vec![DirEntry { name: String::from("one"), kind: DirentKind::Directory }]
        );
        let one_dir = open_directory(&root, "one", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&one_dir).await.unwrap(),
            vec![DirEntry { name: String::from("two"), kind: DirentKind::Directory }]
        );
        assert_close!(one_dir);
        let two_dir = open_directory(&root, "one/two", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&two_dir).await.unwrap(),
            vec![DirEntry { name: String::from("three"), kind: DirentKind::File }]
        );
        assert_close!(two_dir);

        assert_open_file_contents(&root, "one/two/three", fio::PERM_READABLE, "B").await;

        assert_close!(root);
    }

    #[fuchsia::test]
    async fn add_empty_dir_already_exists() {
        let mut tree = empty_dir();
        tree.add_entry(&["one", "two", "three"], file::read_only(b"B")).unwrap();
        tree.add_empty_dir(&["one", "two"]).unwrap();

        let root = tree.build();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);

        assert_eq!(
            readdir(&root).await.unwrap(),
            vec![DirEntry { name: String::from("one"), kind: DirentKind::Directory }]
        );

        let one_dir = open_directory(&root, "one", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&one_dir).await.unwrap(),
            vec![DirEntry { name: String::from("two"), kind: DirentKind::Directory }]
        );
        assert_close!(one_dir);

        let two_dir = open_directory(&root, "one/two", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&two_dir).await.unwrap(),
            vec![DirEntry { name: String::from("three"), kind: DirentKind::File }]
        );
        assert_close!(two_dir);

        assert_open_file_contents(&root, "one/two/three", fio::PERM_READABLE, "B").await;

        assert_close!(root);
    }

    #[fuchsia::test]
    async fn lone_add_empty_dir() {
        let mut tree = empty_dir();
        tree.add_empty_dir(&["just-me"]).unwrap();

        let root = tree.build();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);

        assert_eq!(
            readdir(&root).await.unwrap(),
            vec![DirEntry { name: String::from("just-me"), kind: DirentKind::Directory }]
        );
        let just_me_dir = open_directory(&root, "just-me", fio::PERM_READABLE).await.unwrap();
        assert_eq!(readdir(&just_me_dir).await.unwrap(), Vec::new());

        assert_close!(just_me_dir);
        assert_close!(root);
    }

    #[fuchsia::test]
    async fn add_empty_dir_inside_add_empty_dir() {
        let mut tree = empty_dir();
        tree.add_empty_dir(&["container"]).unwrap();
        tree.add_empty_dir(&["container", "nested"]).unwrap();

        let root = tree.build();
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        let root = serve(root, scope.clone(), fio::PERM_READABLE);

        assert_eq!(
            readdir(&root).await.unwrap(),
            vec![DirEntry { name: String::from("container"), kind: DirentKind::Directory }]
        );

        let container_dir = open_directory(&root, "container", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            readdir(&container_dir).await.unwrap(),
            vec![DirEntry { name: String::from("nested"), kind: DirentKind::Directory }]
        );
        assert_close!(container_dir);

        let nested_dir =
            open_directory(&root, "container/nested", fio::PERM_READABLE).await.unwrap();
        assert_eq!(readdir(&nested_dir).await.unwrap(), Vec::new());
        assert_close!(nested_dir);

        assert_close!(root);
    }

    #[fuchsia::test]
    async fn error_empty_path_in_add_entry() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry(&[], file::read_only(b"Invalid"))
            .expect_err("Empty paths are not allowed.");
        assert_eq!(err, Error::EmptyPath);
    }

    #[fuchsia::test]
    async fn error_empty_first_component() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry(&[""], file::read_only(b"Invalid"))
            .expect_err("Empty paths are not allowed.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: String::new(),
                component: String::new(),
                error: name::ParseNameError::Empty
            }
        );
    }

    #[fuchsia::test]
    async fn error_empty_component() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry(&["a", "", "c"], file::read_only(b"Invalid"))
            .expect_err("Empty paths are not allowed.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: "a//c".to_string(),
                component: String::new(),
                error: name::ParseNameError::Empty
            }
        );
    }

    #[fuchsia::test]
    async fn error_slash_in_component() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry("a/b", file::read_only(b"Invalid"))
            .expect_err("Slash in path component name.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: "a/b".to_string(),
                component: "a/b".to_string(),
                error: name::ParseNameError::Slash
            }
        );
    }

    #[fuchsia::test]
    async fn error_slash_in_second_component() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry(&["a", "b/c"], file::read_only(b"Invalid"))
            .expect_err("Slash in path component name.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: "a/b/c".to_string(),
                component: "b/c".to_string(),
                error: name::ParseNameError::Slash
            }
        );
    }

    #[fuchsia::test]
    async fn error_component_name_too_long() {
        let mut tree = empty_dir();

        let long_component = "abcdefghij".repeat(fio::MAX_NAME_LENGTH as usize / 10 + 1);

        let path: &[&str] = &["a", &long_component, "b"];
        let err = tree
            .add_entry(path, file::read_only(b"Invalid"))
            .expect_err("Individual component names may not exceed MAX_FILENAME bytes.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: format!("a/{}/b", long_component),
                component: long_component.clone(),
                error: name::ParseNameError::TooLong
            }
        );
    }

    #[fuchsia::test]
    async fn error_dot_in_component() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry(&["a", "."], file::read_only(b"Invalid"))
            .expect_err("Dot in path component name.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: "a/.".to_string(),
                component: ".".to_string(),
                error: name::ParseNameError::Dot
            }
        );
    }

    #[fuchsia::test]
    async fn error_dot_dot_in_component() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry(&["a", ".."], file::read_only(b"Invalid"))
            .expect_err("Dot dot in path component name.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: "a/..".to_string(),
                component: "..".to_string(),
                error: name::ParseNameError::DotDot
            }
        );
    }

    #[fuchsia::test]
    async fn error_null_in_component() {
        let mut tree = empty_dir();
        let err = tree
            .add_entry(&["a", "foo\0bar"], file::read_only(b"Invalid"))
            .expect_err("Embedded null in component.");
        assert_eq!(
            err,
            Error::InvalidComponent {
                path: "a/foo\0bar".to_string(),
                component: "foo\0bar".to_string(),
                error: name::ParseNameError::EmbeddedNul
            }
        );
    }

    #[fuchsia::test]
    async fn error_leaf_over_directory() {
        let mut tree = empty_dir();

        tree.add_entry(&["top", "nested", "file"], file::read_only(b"Content")).unwrap();
        let err = tree
            .add_entry(&["top", "nested"], file::read_only(b"Invalid"))
            .expect_err("A leaf may not be constructed over a directory.");
        assert_eq!(err, Error::LeafOverDirectory { path: "top/nested".to_string() });
    }

    #[fuchsia::test]
    async fn error_leaf_over_leaf() {
        let mut tree = empty_dir();

        tree.add_entry(&["top", "nested", "file"], file::read_only(b"Content")).unwrap();
        let err = tree
            .add_entry(&["top", "nested", "file"], file::read_only(b"Invalid"))
            .expect_err("A leaf may not be constructed over another leaf.");
        assert_eq!(err, Error::LeafOverLeaf { path: "top/nested/file".to_string() });
    }

    #[fuchsia::test]
    async fn error_entry_inside_leaf() {
        let mut tree = empty_dir();

        tree.add_entry(&["top", "file"], file::read_only(b"Content")).unwrap();
        let err = tree
            .add_entry(&["top", "file", "nested"], file::read_only(b"Invalid"))
            .expect_err("A leaf may not be constructed over another leaf.");
        assert_eq!(
            err,
            Error::EntryInsideLeaf {
                path: "top/file/nested".to_string(),
                traversed: "top/file".to_string()
            }
        );
    }

    #[fuchsia::test]
    async fn error_entry_inside_leaf_directory() {
        let mut tree = empty_dir();

        // Even when a leaf is itself a directory the tree builder cannot insert a nested entry.
        tree.add_entry(&["top", "file"], Simple::new()).unwrap();
        let err = tree
            .add_entry(&["top", "file", "nested"], file::read_only(b"Invalid"))
            .expect_err("A leaf may not be constructed over another leaf.");
        assert_eq!(
            err,
            Error::EntryInsideLeaf {
                path: "top/file/nested".to_string(),
                traversed: "top/file".to_string()
            }
        );
    }
}
