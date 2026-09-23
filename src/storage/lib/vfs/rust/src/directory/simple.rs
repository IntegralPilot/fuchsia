// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This is an implementation of "simple" pseudo directories.
//! Use [`crate::directory::immutable::Simple::new()`]
//! to construct actual instances.  See [`Simple`] for details.

use crate::ObjectRequestRef;
#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use crate::ToObjectRequest;
use crate::common::CreationMode;
use crate::directory::dirents_sink;
use crate::directory::entry::{DirectoryEntry, EntryInfo, OpenRequest, RequestFlags};
use crate::directory::entry_container::{Directory, DirectoryWatcher};
use crate::directory::helper::{AlreadyExists, DirectlyMutable, NotDirectory};
use crate::directory::immutable::connection::ImmutableConnection;
use crate::directory::traversal_position::TraversalPosition;
use crate::directory::watchers::Watchers;
use crate::directory::watchers::event_producers::{
    SingleNameEventProducer, StaticVecEventProducer,
};
use crate::execution_scope::ExecutionScope;
use crate::name::Name;
use crate::node::Node;
use crate::path::Path;
use crate::protocols::ProtocolsExt;
#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use flex_client::fidl::ServerEnd;
use flex_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::iter;
use std::sync::Arc;
use zx_status::Status;

use super::entry::GetEntryInfo;

/// An implementation of a "simple" pseudo directory.  This directory holds a set of entries,
/// allowing the server to add or remove entries via the
/// [`crate::directory::helper::DirectlyMutable::add_entry()`] and
/// [`crate::directory::helper::DirectlyMutable::remove_entry`] methods.
pub struct Simple {
    inner: Mutex<Inner>,

    // The inode for this directory. This should either be unique within this VFS, or INO_UNKNOWN.
    inode: u64,

    not_found_handler: Option<Box<dyn Fn(&str) + Send + Sync + 'static>>,
}

struct Inner {
    entries: BTreeMap<Name, Arc<dyn DirectoryEntry>>,

    watchers: Watchers,
}

impl Simple {
    pub fn new() -> Arc<Self> {
        Self::new_with_inode(fio::INO_UNKNOWN)
    }

    pub(crate) fn new_with_inode(inode: u64) -> Arc<Self> {
        Arc::new(Simple {
            inner: Mutex::new(Inner { entries: BTreeMap::new(), watchers: Watchers::new() }),
            inode,
            not_found_handler: None,
        })
    }

    /// Constructs a new pseudo directory with the provided entries.
    ///
    /// This function is marked `pub` only so the `pseudo_directory!` macro can use it. It should
    /// not be used directly outside of this crate.
    #[doc(hidden)]
    pub fn new_with_entries_and_inode(
        entries: BTreeMap<Name, Arc<dyn DirectoryEntry>>,
        inode: u64,
    ) -> Arc<Self> {
        Arc::new(Simple {
            inner: Mutex::new(Inner { entries, watchers: Watchers::new() }),
            inode,
            not_found_handler: None,
        })
    }

    /// Creates a new directory with the provided function that will be called whenever this VFS
    /// receives an open request for a path that is not present in the directory. The handler is
    /// invoked with the full path of the missing entry, relative to the root.
    pub fn new_with_not_found_handler(handler: impl Fn(&str) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Simple {
            inner: Mutex::new(Inner { entries: BTreeMap::new(), watchers: Watchers::new() }),
            inode: fio::INO_UNKNOWN,
            not_found_handler: Some(Box::new(handler)),
        })
    }

    /// Returns the entry identified by `name`.
    pub fn get_entry(&self, name: &str) -> Result<Arc<dyn DirectoryEntry>, Status> {
        crate::name::validate_name(name)?;

        let this = self.inner.lock();
        match this.entries.get(name) {
            Some(entry) => Ok(entry.clone()),
            None => Err(Status::NOT_FOUND),
        }
    }

    /// Gets or inserts an entry (as supplied by the callback `f`).
    pub fn get_or_insert<T: DirectoryEntry>(
        &self,
        name: Name,
        f: impl FnOnce() -> Arc<T>,
    ) -> Arc<dyn DirectoryEntry> {
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        match inner.entries.entry(name) {
            Entry::Vacant(slot) => {
                inner.watchers.send_event(&mut SingleNameEventProducer::added(slot.key()));
                slot.insert(f()).clone()
            }
            Entry::Occupied(entry) => entry.get().clone(),
        }
    }

    /// Removes all entries from the directory.
    pub fn remove_all_entries(&self) {
        let mut inner = self.inner.lock();
        if !inner.entries.is_empty() {
            let names = std::mem::take(&mut inner.entries)
                .into_keys()
                .map(String::from)
                .collect::<Vec<String>>();
            inner.watchers.send_event(&mut StaticVecEventProducer::removed(names));
        }
    }

    fn open_impl<'a, P: ProtocolsExt + ToRequestFlags>(
        self: Arc<Self>,
        mut scope: ExecutionScope,
        mut path: Path,
        protocols: P,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        // See if the path has a next segment, if so we want to traverse down the directory.
        // Otherwise we've arrived at the right directory.
        let (name, path_ref) = match path.next_with_ref() {
            (path_ref, Some(name)) => (name, path_ref),
            (_, None) => {
                if protocols.create_unnamed_temporary_in_directory_path() {
                    // Creating an entry is not supported.
                    return Err(Status::NOT_SUPPORTED);
                }
                object_request
                    .take()
                    .create_connection_sync::<ImmutableConnection<_>, _>(scope, self, protocols);
                return Ok(());
            }
        };

        // Don't hold the inner lock while opening the entry in case the directory contains itself.
        let _guard;
        let entry = match self.inner.lock().entries.get(name) {
            Some(entry) => {
                // Whilst we are holding the lock, see if an alternative scope should be used.
                if let Some(s) = entry.scope() {
                    // Make sure we can get an active guard.
                    let Some(g) = s.try_active_guard() else {
                        return Err(Status::PEER_CLOSED);
                    };
                    scope = s;
                    _guard = g;
                }
                Some(entry.clone())
            }
            None => None,
        };

        match (entry, path_ref.is_empty(), protocols.creation_mode()) {
            (None, false, _) | (None, true, CreationMode::Never) => {
                // Either:
                //   - we're at an intermediate directory and the next entry doesn't exist, or
                //   - we're at the last directory and the next entry doesn't exist and creating the
                //     entry wasn't requested.
                if let Some(not_found_handler) = &self.not_found_handler {
                    not_found_handler(path_ref.as_str());
                }
                Err(Status::NOT_FOUND)
            }
            (
                None,
                true,
                CreationMode::Always
                | CreationMode::AllowExisting
                | CreationMode::UnnamedTemporary
                | CreationMode::UnlinkableUnnamedTemporary,
            ) => {
                // We're at the last directory and the entry doesn't exist and creating the entry
                // was requested which isn't supported.
                Err(Status::NOT_SUPPORTED)
            }
            (
                Some(_),
                true,
                CreationMode::UnnamedTemporary | CreationMode::UnlinkableUnnamedTemporary,
            ) => {
                // We're at the last directory and the entry exists and it was requested to create
                // an unnamed temporary object in this entry (this is not supported for simple
                // pseudo directory).
                Err(Status::NOT_SUPPORTED)
            }
            (Some(_), true, CreationMode::Always) => {
                // We're at the last directory and the entry exists but creating the entry is
                // required.
                Err(Status::ALREADY_EXISTS)
            }
            (Some(entry), _, _) => entry.open_entry(OpenRequest::new(
                scope,
                protocols.to_request_flags(),
                path,
                object_request,
            )),
        }
    }
}

impl GetEntryInfo for Simple {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.inode, fio::DirentType::Directory)
    }
}

impl DirectoryEntry for Simple {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_dir(self)
    }
}

impl Node for Simple {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::ENUMERATE
                    | fio::Operations::TRAVERSE,
                id: self.inode,
            }
        ))
    }
}

impl Directory for Simple {
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn deprecated_open(
        self: Arc<Self>,
        scope: ExecutionScope,
        flags: fio::OpenFlags,
        path: Path,
        server_end: ServerEnd<fio::NodeMarker>,
    ) {
        flags
            .to_object_request(server_end)
            .handle(|object_request| self.open_impl(scope, path, flags, object_request));
    }

    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        self.open_impl(scope, path, flags, object_request)
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
        use dirents_sink::AppendResult;

        let this = self.inner.lock();

        let (mut sink, entries_iter) = match pos {
            TraversalPosition::Start => {
                match sink.append(&EntryInfo::new(self.inode, fio::DirentType::Directory), ".") {
                    AppendResult::Ok(sink) => (sink, this.entries.range::<Name, _>(..)),
                    AppendResult::Sealed(sealed) => {
                        return Ok((TraversalPosition::Start, sealed));
                    }
                }
            }

            TraversalPosition::Name(next_name) => {
                // The only way to get a `TraversalPosition::Name` is if we returned it in the
                // `AppendResult::Sealed` code path below. Therefore, the conversion from
                // `next_name` to `Name` will never fail in practice.
                let next: Name = next_name.to_owned().try_into().unwrap();
                (sink, this.entries.range::<Name, _>(next..))
            }

            TraversalPosition::Bytes(_) | TraversalPosition::Index(_) => unreachable!(),

            TraversalPosition::End => return Ok((TraversalPosition::End, sink.seal())),
        };

        for (name, entry) in entries_iter {
            match sink.append(&entry.entry_info(), &name) {
                AppendResult::Ok(new_sink) => sink = new_sink,
                AppendResult::Sealed(sealed) => {
                    return Ok((TraversalPosition::Name(name.clone().into()), sealed));
                }
            }
        }

        Ok((TraversalPosition::End, sink.seal()))
    }

    fn register_watcher(
        self: Arc<Self>,
        scope: ExecutionScope,
        mask: fio::WatchMask,
        watcher: DirectoryWatcher,
    ) -> Result<(), Status> {
        let mut this = self.inner.lock();

        let mut names = StaticVecEventProducer::existing({
            let entry_names = this.entries.keys();
            iter::once(".".to_string()).chain(entry_names.map(|x| x.to_owned().into())).collect()
        });

        let controller = this.watchers.add(scope, self.clone(), mask, watcher);
        controller.send_event(&mut names);
        controller.send_event(&mut SingleNameEventProducer::idle());

        Ok(())
    }

    fn unregister_watcher(self: Arc<Self>, key: usize) {
        let mut this = self.inner.lock();
        this.watchers.remove(key);
    }
}

impl DirectlyMutable for Simple {
    fn add_entry_impl(
        &self,
        name: Name,
        entry: Arc<dyn DirectoryEntry>,
        overwrite: bool,
    ) -> Result<(), AlreadyExists> {
        let mut this = self.inner.lock();

        if !overwrite && this.entries.contains_key(&name) {
            return Err(AlreadyExists);
        }

        this.watchers.send_event(&mut SingleNameEventProducer::added(&name));

        let _ = this.entries.insert(name, entry);
        Ok(())
    }

    fn remove_entry_impl(
        &self,
        name: Name,
        must_be_directory: bool,
    ) -> Result<Option<Arc<dyn DirectoryEntry>>, NotDirectory> {
        let mut this = self.inner.lock();

        match this.entries.entry(name) {
            Entry::Vacant(_) => Ok(None),
            Entry::Occupied(occupied) => {
                if must_be_directory
                    && occupied.get().entry_info().type_() != fio::DirentType::Directory
                {
                    Err(NotDirectory)
                } else {
                    let (key, value) = occupied.remove_entry();
                    this.watchers.send_event(&mut SingleNameEventProducer::removed(&key));
                    Ok(Some(value))
                }
            }
        }
    }
}

trait ToRequestFlags {
    fn to_request_flags(&self) -> RequestFlags;
}

#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
impl ToRequestFlags for fio::OpenFlags {
    fn to_request_flags(&self) -> RequestFlags {
        RequestFlags::Open1(*self)
    }
}

impl ToRequestFlags for fio::Flags {
    fn to_request_flags(&self) -> RequestFlags {
        RequestFlags::Open3(*self)
    }
}

#[doc(hidden)]
pub mod __private {
    pub const INO_UNKNOWN: u64 = flex_fuchsia_io::INO_UNKNOWN;
}

/// Builds a pseudo directory using a simple DSL. The directory entry names must be static strings
/// (`&'static str`).
///
/// # Examples
///
/// This will construct a small tree of read-only files:
/// ```
/// let root = pseudo_directory! {
///     "etc" => pseudo_directory! {
///         "fstab" => read_only(b"/dev/fs /"),
///         "passwd" => read_only(b"[redacted]"),
///         "shells" => read_only(b"/bin/bash"),
///         "ssh" => pseudo_directory! {
///           "sshd_config" => read_only(b"# Empty"),
///         },
///     },
///     "uname" => read_only(b"Fuchsia"),
/// };
/// ```
///
/// # Panics
///
/// This macro will panic if there are duplicate entries or any of the entry names are invalid. See
/// [`name::validate_name`] for the restrictions.
#[macro_export]
macro_rules! pseudo_directory {
    ( $( $name:expr => $entry:expr ),* $(,)? ) => {{
        let entries = ::std::collections::BTreeMap::from([
            $(
                (
                    $crate::name::Name::from_static($name),
                    $entry as ::std::sync::Arc<dyn $crate::directory::entry::DirectoryEntry>,
                ),
            )*
        ]);
        // Check for duplicate entries by comparing the length of the constructed map with the
        // number of entries passed in.
        ::std::assert_eq!(
            entries.len(),
            <[()]>::len(&[ $( $crate::__replace_with_unit_type!($name) ),* ]),
            "Duplicate entries in pseudo_directory!"
        );
        $crate::directory::immutable::Simple::new_with_entries_and_inode(
            entries,
            $crate::directory::simple::__private::INO_UNKNOWN,
        )
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __replace_with_unit_type {
    ($_t:tt) => {
        ()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::immutable::Simple;
    use crate::file;
    use crate::object_request::ObjectRequest;

    #[test]
    fn add_entry_success() {
        let dir = Simple::new();
        assert_eq!(
            dir.add_entry("path_without_separators", file::read_only(b"test")),
            Ok(()),
            "add entry with valid filename should succeed"
        );
    }

    #[test]
    fn add_entry_error_name_with_path_separator() {
        let dir = Simple::new();
        let status = dir
            .add_entry("path/with/separators", file::read_only(b"test"))
            .expect_err("add entry with path separator should fail");
        assert_eq!(status, Status::INVALID_ARGS);
    }

    #[test]
    fn add_entry_error_name_too_long() {
        let dir = Simple::new();
        let status = dir
            .add_entry("a".repeat(10000), file::read_only(b"test"))
            .expect_err("add entry whose name is too long should fail");
        assert_eq!(status, Status::BAD_PATH);
    }

    #[fuchsia::test]
    async fn not_found_handler() {
        let path_mutex = Arc::new(Mutex::new(None));
        let path_mutex_clone = path_mutex.clone();
        let dir = Simple::new_with_not_found_handler(move |path| {
            *path_mutex_clone.lock() = Some(path.to_string());
        });

        let path_mutex_clone = path_mutex.clone();
        let sub_dir = Simple::new_with_not_found_handler(move |path| {
            *path_mutex_clone.lock() = Some(path.to_string());
        });
        dir.add_entry("dir", sub_dir).expect("add entry with valid filename should succeed");

        dir.add_entry("file", file::read_only(b"test"))
            .expect("add entry with valid filename should succeed");

        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();

        for (path, expectation) in vec![
            (".", None),
            ("does-not-exist", Some("does-not-exist".to_string())),
            ("file", None),
            ("dir", None),
            ("dir/does-not-exist", Some("dir/does-not-exist".to_string())),
        ] {
            log::info!("{path}");
            #[cfg(feature = "fdomain")]
            let (_proxy, server_end) = {
                let client = scope.domain();
                client.create_proxy::<fio::NodeMarker>()
            };
            #[cfg(not(feature = "fdomain"))]
            let (_proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
            let flags = fio::Flags::PROTOCOL_NODE | fio::Flags::FLAG_SEND_REPRESENTATION;
            let path = Path::validate_and_split(path).unwrap();
            ObjectRequest::new(flags, &fio::Options::default(), server_end.into_channel().into())
                .handle(|request| dir.clone().open(scope.clone(), path, flags, request));

            assert_eq!(expectation, path_mutex.lock().take());
        }
    }

    #[test]
    fn remove_all_entries() {
        let dir = Simple::new();

        dir.add_entry("file", file::read_only(""))
            .expect("add entry with valid filename should succeed");

        dir.remove_all_entries();
        assert_eq!(
            dir.get_entry("file").err().expect("file should no longer exist"),
            Status::NOT_FOUND
        );
    }

    #[fuchsia::test]
    async fn test_alternate_scope() {
        struct MockEntry(ExecutionScope);

        impl DirectoryEntry for MockEntry {
            fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
                assert_eq!(request.scope(), &self.0);
                Ok(())
            }

            fn scope(&self) -> Option<ExecutionScope> {
                Some(self.0.clone())
            }
        }

        impl GetEntryInfo for MockEntry {
            fn entry_info(&self) -> EntryInfo {
                EntryInfo::new(1, fio::DirentType::Directory)
            }
        }

        let dir = Simple::new();

        #[cfg(feature = "fdomain")]
        let dummy_scope =
            crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let dummy_scope = crate::execution_scope::ExecutionScope::new();

        dir.add_entry("foo", Arc::new(MockEntry(dummy_scope))).expect("add_entry failed");

        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        #[cfg(feature = "fdomain")]
        let (_proxy, server) = scope.domain().create_proxy::<fio::DirectoryMarker>();
        #[cfg(not(feature = "fdomain"))]
        let (_client, server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
        let mut request = ObjectRequest::new(
            fio::Flags::empty(),
            &fio::Options::default(),
            server.into_channel().into(),
        );
        dir.open(scope, Path::dot(), fio::Flags::empty(), &mut request).expect("open succeeded");
    }
}
