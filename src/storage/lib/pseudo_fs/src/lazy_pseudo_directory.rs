// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::PseudoDirectory;
use fidl_fuchsia_io as fio;
use fuchsia_sync::{MappedMutexGuard, Mutex, MutexGuard};
use futures::lock::Mutex as AsyncMutex;
use std::sync::Arc;
use vfs::directory::entry::{
    DirectoryEntry, DirectoryEntryAsync, EntryInfo, GetEntryInfo, OpenRequest,
};
use zx_status::Status;

pub trait ToPseudoDirectory: Send + 'static {
    /// Constructs the `PseudoDirectory` the first time a request for the directory is received.
    ///
    /// The returned directory must not have an inode number.
    fn to_pseudo_directory(self) -> Arc<PseudoDirectory>;
}

pub trait ToPseudoDirectoryAsync: Send + 'static {
    /// Constructs the `PseudoDirectory` the first time a request for the directory is received.
    ///
    /// The returned directory must not have an inode number.
    fn to_pseudo_directory(self) -> impl Future<Output = Arc<PseudoDirectory>> + Send;
}

/// A pseudo directory that delays constructing a `PseudoDirectory` until a request is received.
///
/// The intended purpose of `LazyPseudoDirectory` is to save memory when presenting data in a
/// filesystem structure for debug purposes. The directory should not be accessed during regular
/// system usage otherwise no memory savings will occur.
pub struct LazyPseudoDirectory<T>(Mutex<Inner<T>>);

impl<T: ToPseudoDirectory> LazyPseudoDirectory<T> {
    pub fn new(data: T) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Inner::Data(data))))
    }

    /// Retrieves either the backing data or the directory depending on whether the directory has
    /// been accessed yet.
    pub fn state(&self) -> LazyPseudoDirectoryState<'_, T> {
        let inner = self.0.lock();
        match &*inner {
            Inner::Data(_) => LazyPseudoDirectoryState::Data(MutexGuard::map(inner, |inner| {
                let Inner::Data(data) = inner else { unreachable!() };
                data
            })),
            Inner::Directory(dir) => LazyPseudoDirectoryState::Directory(dir.clone()),
            Inner::Intermediate => unreachable!(),
        }
    }
}

pub enum LazyPseudoDirectoryState<'a, T> {
    Data(MappedMutexGuard<'a, T>),
    Directory(Arc<PseudoDirectory>),
}

impl<T> LazyPseudoDirectoryState<'_, T> {
    pub fn is_data(&self) -> bool {
        match self {
            Self::Data(_) => true,
            _ => false,
        }
    }

    pub fn is_directory(&self) -> bool {
        match self {
            Self::Directory(_) => true,
            _ => false,
        }
    }
}

enum Inner<T> {
    Data(T),
    Directory(Arc<PseudoDirectory>),

    /// An intermediate state used when converting from `Data` to `Directory`. A lock on `Inner` is
    /// held during the transition so this state is never be externally observable.
    Intermediate,
}

impl<T: ToPseudoDirectory> Inner<T> {
    fn get_or_init_directory(&mut self) -> Arc<PseudoDirectory> {
        if let Self::Directory(dir) = self {
            return dir.clone();
        }

        let Self::Data(data) = std::mem::replace(self, Self::Intermediate) else {
            unreachable!();
        };
        let dir = data.to_pseudo_directory();
        *self = Self::Directory(dir.clone());

        // Requiring that the directory does not have an inode number avoids creating the directory
        // when responding to `GetEntryInfo::entry_info` requests.
        debug_assert!(
            dir.entry_info().inode() == fio::INO_UNKNOWN,
            "The directory must not have an inode number"
        );
        dir
    }
}

impl<T> GetEntryInfo for LazyPseudoDirectory<T> {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl<T: ToPseudoDirectory> DirectoryEntry for LazyPseudoDirectory<T> {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        let mut this = self.0.lock();
        if let Inner::Directory(dir) = &*this {
            return dir.clone().open_entry(request);
        }
        if request.requires_event() || !request.path().is_empty() {
            this.get_or_init_directory().open_entry(request)
        } else {
            std::mem::drop(this);
            request.spawn(self);
            Ok(())
        }
    }
}

impl<T: ToPseudoDirectory> DirectoryEntryAsync for LazyPseudoDirectory<T> {
    async fn open_entry_async(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        if !request.wait_till_ready().await {
            // The channel was closed before any request was received.
            return Ok(());
        }
        let mut this = self.0.lock();
        this.get_or_init_directory().open_entry(request)
    }
}

/// A pseudo directory that delays constructing a `PseudoDirectory` until a request is received.
///
/// The intended purpose of `LazyPseudoDirectoryAsync` is to save memory when presenting data in a
/// filesystem structure for debug purposes. The directory should not be accessed during regular
/// system usage otherwise no memory savings will occur.
///
/// This is functionally identical to [`LazyPseudoDirectory`], except it is compatible with
/// [`ToPseudoDirectoryAsync`] (i.e. directories with asynchronous initializers).  As a tradeoff,
/// any time the contents are accessed, an async lock must be acquired, which will be less
/// performant.  As such, this should only be used when needed.
pub struct LazyPseudoDirectoryAsync<T>(AsyncMutex<InnerAsync<T>>);

enum InnerAsync<T> {
    Data(Option<T>),
    Directory(Arc<PseudoDirectory>),
}

impl<T: ToPseudoDirectoryAsync> LazyPseudoDirectoryAsync<T> {
    pub fn new(data: T) -> Arc<Self> {
        Arc::new(Self(AsyncMutex::new(InnerAsync::Data(Some(data)))))
    }

    pub async fn is_data(&self) -> bool {
        let inner = self.0.lock().await;
        matches!(&*inner, InnerAsync::Data(_))
    }

    pub async fn is_directory(&self) -> bool {
        let inner = self.0.lock().await;
        matches!(&*inner, InnerAsync::Directory(_))
    }
}

impl<T> GetEntryInfo for LazyPseudoDirectoryAsync<T> {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl<T: ToPseudoDirectoryAsync> DirectoryEntry for LazyPseudoDirectoryAsync<T> {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        if let Some(this) = self.0.try_lock() {
            if let InnerAsync::Directory(dir) = &*this {
                return dir.clone().open_entry(request);
            }
        }
        request.spawn(self);
        Ok(())
    }
}

impl<T: ToPseudoDirectoryAsync> DirectoryEntryAsync for LazyPseudoDirectoryAsync<T> {
    async fn open_entry_async(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        if !request.wait_till_ready().await {
            return Ok(());
        }

        let mut this = self.0.lock().await;

        let dir = match &mut *this {
            InnerAsync::Directory(dir) => dir.clone(),
            InnerAsync::Data(opt) => {
                let data = opt.take().expect("Data already taken");
                let dir = data.to_pseudo_directory().await;
                debug_assert!(
                    dir.entry_info().inode() == fio::INO_UNKNOWN,
                    "The directory must not have an inode number"
                );
                *this = InnerAsync::Directory(dir.clone());
                dir
            }
        };

        dir.open_entry(request)
    }
}

#[cfg(all(test))]
mod tests {
    use super::*;
    use crate::PseudoFile;
    use fidl::endpoints::create_proxy;
    use vfs::directory::helper::DirectlyMutable;
    use vfs::{ExecutionScope, Path, ToObjectRequest};

    #[cfg(target_os = "fuchsia")]
    use fuchsia_async::TestExecutor;

    struct MockData;

    fn open(
        dir: Arc<LazyPseudoDirectory<MockData>>,
        flags: fio::Flags,
        path: Path,
    ) -> fio::DirectoryProxy {
        let (client, server) = create_proxy::<fio::DirectoryMarker>();
        flags
            .to_object_request(server)
            .handle(|object_request| {
                dir.open_entry(OpenRequest::new(
                    ExecutionScope::new(),
                    flags,
                    path,
                    object_request,
                ))
                .unwrap();
                Ok(())
            })
            .unwrap();
        client
    }

    #[cfg(target_os = "fuchsia")]
    fn run_ready_tasks(executor: &mut TestExecutor) {
        let _ = executor.run_until_stalled(&mut std::future::pending::<()>());
    }

    impl ToPseudoDirectory for MockData {
        fn to_pseudo_directory(self) -> Arc<PseudoDirectory> {
            let inner = PseudoDirectory::new();
            inner.add_entry("file", PseudoFile::from_data("1234")).unwrap();
            let dir = PseudoDirectory::new();
            dir.add_entry("inner", inner).unwrap();
            dir
        }
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn test_open_entry_with_no_request_does_not_create_directory() {
        let mut exec = TestExecutor::new();
        let lazy_dir = LazyPseudoDirectory::new(MockData);
        let _client = open(lazy_dir.clone(), fio::PERM_READABLE, Path::dot());
        run_ready_tasks(&mut exec);
        assert!(lazy_dir.state().is_data());
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn test_open_entry_with_representation_creates_directory() {
        let mut exec = TestExecutor::new();
        let lazy_dir = LazyPseudoDirectory::new(MockData);
        let _client = open(
            lazy_dir.clone(),
            fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
            Path::dot(),
        );
        run_ready_tasks(&mut exec);
        assert!(lazy_dir.state().is_directory());
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn test_open_entry_with_path_creates_directory() {
        let mut exec = TestExecutor::new();
        let lazy_dir = LazyPseudoDirectory::new(MockData);
        let _client = open(lazy_dir.clone(), fio::PERM_READABLE, "inner".try_into().unwrap());
        run_ready_tasks(&mut exec);
        assert!(lazy_dir.state().is_directory());
    }

    #[fuchsia::test]
    async fn test_create_directory_on_request() {
        let lazy_dir = LazyPseudoDirectory::new(MockData);
        let client = open(lazy_dir.clone(), fio::PERM_READABLE, Path::dot());
        assert!(lazy_dir.state().is_data());
        client.get_flags().await.unwrap().unwrap();
        assert!(lazy_dir.state().is_directory());
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn test_peer_closed_does_not_create_directory() {
        let mut exec = TestExecutor::new();
        let lazy_dir = LazyPseudoDirectory::new(MockData);
        let client = open(lazy_dir.clone(), fio::PERM_READABLE, Path::dot());
        assert!(lazy_dir.state().is_data());

        // Close the channel and wait for the peer closed signal to be received.
        std::mem::drop(client);
        run_ready_tasks(&mut exec);

        assert!(lazy_dir.state().is_data());
    }

    #[fuchsia::test]
    async fn test_read_inner_file() {
        let lazy_dir = LazyPseudoDirectory::new(MockData);
        let client = open(lazy_dir.clone(), fio::PERM_READABLE, Path::dot());
        assert_eq!(
            fuchsia_fs::directory::read_file_to_string(&client, "inner/file")
                .await
                .expect("failed to read file"),
            "1234"
        );
    }
    impl ToPseudoDirectoryAsync for MockData {
        async fn to_pseudo_directory(self) -> Arc<PseudoDirectory> {
            let inner = PseudoDirectory::new();
            inner.add_entry("file", PseudoFile::from_data("1234")).unwrap();
            let dir = PseudoDirectory::new();
            dir.add_entry("inner", inner).unwrap();
            dir
        }
    }

    fn open_async_dir(
        dir: Arc<LazyPseudoDirectoryAsync<MockData>>,
        flags: fio::Flags,
        path: Path,
    ) -> fio::DirectoryProxy {
        let (client, server) = create_proxy::<fio::DirectoryMarker>();
        flags
            .to_object_request(server)
            .handle(|object_request| {
                dir.open_entry(OpenRequest::new(
                    ExecutionScope::new(),
                    flags,
                    path,
                    object_request,
                ))
                .unwrap();
                Ok(())
            })
            .unwrap();
        client
    }

    #[fuchsia::test]
    async fn test_async_create_directory_on_request() {
        let lazy_dir = LazyPseudoDirectoryAsync::new(MockData);
        let client = open_async_dir(lazy_dir.clone(), fio::PERM_READABLE, Path::dot());
        assert!(lazy_dir.is_data().await);
        client.get_flags().await.unwrap().unwrap();
        assert!(lazy_dir.is_directory().await);
    }

    #[fuchsia::test]
    async fn test_async_read_inner_file() {
        let lazy_dir = LazyPseudoDirectoryAsync::new(MockData);
        let client = open_async_dir(lazy_dir.clone(), fio::PERM_READABLE, Path::dot());
        assert_eq!(
            fuchsia_fs::directory::read_file_to_string(&client, "inner/file")
                .await
                .expect("failed to read file"),
            "1234"
        );
    }
}
