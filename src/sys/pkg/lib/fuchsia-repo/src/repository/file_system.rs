// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::range::{ContentRange, Range};
use crate::repository::{Error, RepoProvider, RepoStorage, Resource};
use crate::util::file_stream;
use anyhow::{Context as _, Result, anyhow};
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use delivery_blob::DeliveryBlobType;
use fuchsia_async as fasync;
use fuchsia_merkle::Hash;
use futures::future::BoxFuture;
use futures::{AsyncRead, FutureExt as _};
use log::warn;
use std::collections::BTreeSet;
use std::fs::{self, DirBuilder};
use std::io::{Seek as _, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::time::SystemTime;
use tempfile::{NamedTempFile, TempPath};
use tuf::metadata::{MetadataPath, MetadataVersion, TargetPath};
use tuf::pouf::Pouf1;
use tuf::repository::{
    FileSystemRepository as TufFileSystemRepository,
    FileSystemRepositoryBuilder as TufFileSystemRepositoryBuilder,
    RepositoryProvider as TufRepositoryProvider, RepositoryStorage as TufRepositoryStorage,
};

#[cfg(not(target_os = "fuchsia"))]
use {
    crate::repository::RepositorySpec,
    futures::{Stream, StreamExt as _, stream::BoxStream},
    notify::{RecursiveMode, Watcher as _, recommended_watcher},
    std::{
        ffi::OsStr,
        pin::Pin,
        task::{Context, Poll},
    },
};

/// Describes how package blobs should be copied into the repository.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CopyMode {
    /// Copy package blobs into the repository. This will skip copying the blob if it already exists
    /// in the repository.
    ///
    /// This will create a Copy-on-Write (reflink) on file systems that support it.
    #[default]
    Copy,

    /// Copy package blobs into the repository. This will overwrite a blob if it already exists in
    /// the repository.
    ///
    /// This will create a Copy-on-Write (reflink) on file systems that support it.
    CopyOverwrite,

    /// Create hard links from the package blobs into the repository.
    HardLink,
}

/// A builder to create a repository contained on the local file system.
pub struct FileSystemRepositoryBuilder {
    metadata_repo_path: Utf8PathBuf,
    blob_repo_path: Utf8PathBuf,
    copy_mode: CopyMode,
    aliases: BTreeSet<String>,
    delivery_blob_type: DeliveryBlobType,
}

impl FileSystemRepositoryBuilder {
    /// Creates a [FileSystemRepositoryBuilder] where the TUF metadata is stored in
    /// `metadata_repo_path`, and the blobs are stored in `blob_repo_path`.
    pub fn new(metadata_repo_path: Utf8PathBuf, blob_repo_path: Utf8PathBuf) -> Self {
        FileSystemRepositoryBuilder {
            metadata_repo_path,
            blob_repo_path,
            copy_mode: CopyMode::Copy,
            aliases: BTreeSet::new(),
            delivery_blob_type: DeliveryBlobType::Type1,
        }
    }

    /// Select which [CopyMode] to use when copying files into the repository.
    pub fn copy_mode(mut self, copy_mode: CopyMode) -> Self {
        self.copy_mode = copy_mode;
        self
    }

    /// alias this repository to this name when this repository is registered on a target.
    pub fn alias(mut self, alias: String) -> Self {
        self.aliases.insert(alias);
        self
    }

    /// alias this repository to these names when this repository is registered on a target.
    pub fn aliases(mut self, aliases: impl IntoIterator<Item = String>) -> Self {
        for alias in aliases {
            self = self.alias(alias);
        }
        self
    }

    /// Set the type of delivery blob to generate when copying blobs into the repository.
    pub fn delivery_blob_type(mut self, delivery_blob_type: DeliveryBlobType) -> Self {
        self.delivery_blob_type = delivery_blob_type;
        self
    }

    /// Set the path to the blob repo.
    pub fn blob_repo_path(mut self, blob_repo_path: Utf8PathBuf) -> Self {
        self.blob_repo_path = blob_repo_path;
        self
    }

    /// Build a [FileSystemRepository].
    pub fn build(self) -> FileSystemRepository {
        FileSystemRepository {
            metadata_repo_path: self.metadata_repo_path.clone(),
            blob_repo_path: self.blob_repo_path,
            copy_mode: self.copy_mode,
            aliases: self.aliases,
            delivery_blob_type: self.delivery_blob_type,
            tuf_repo: TufFileSystemRepositoryBuilder::new(self.metadata_repo_path)
                .targets_prefix("targets")
                .build(),
        }
    }
}

/// Serve a repository from the file system.
#[derive(Debug)]
pub struct FileSystemRepository {
    metadata_repo_path: Utf8PathBuf,
    blob_repo_path: Utf8PathBuf,
    copy_mode: CopyMode,
    aliases: BTreeSet<String>,
    delivery_blob_type: DeliveryBlobType,
    tuf_repo: TufFileSystemRepository<Pouf1>,
}

impl FileSystemRepository {
    /// Construct a [FileSystemRepositoryBuilder].
    pub fn builder(
        metadata_repo_path: Utf8PathBuf,
        blob_repo_path: Utf8PathBuf,
    ) -> FileSystemRepositoryBuilder {
        FileSystemRepositoryBuilder::new(metadata_repo_path, blob_repo_path)
    }

    /// Construct a [FileSystemRepository].
    pub fn new(metadata_repo_path: Utf8PathBuf, blob_repo_path: Utf8PathBuf) -> Self {
        Self::builder(metadata_repo_path, blob_repo_path).build()
    }

    pub fn blob_repo_path(&self) -> &Utf8PathBuf {
        &self.blob_repo_path
    }

    fn fetch<'a>(
        &'a self,
        repo_path: &Utf8Path,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        let file_path = sanitize_path(repo_path, resource_path);
        async move {
            let file_path = file_path?;
            let mut file = std::fs::File::open(&file_path).map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    Error::NotFound
                } else {
                    Error::Io(err)
                }
            })?;

            let total_len = file.metadata().map_err(Error::Io)?.len();

            let content_range = match range {
                Range::Full => ContentRange::Full { complete_len: total_len },
                Range::Inclusive { first_byte_pos, last_byte_pos } => {
                    if first_byte_pos > last_byte_pos
                        || first_byte_pos >= total_len
                        || last_byte_pos >= total_len
                    {
                        return Err(Error::RangeNotSatisfiable);
                    }

                    file.seek(SeekFrom::Start(first_byte_pos)).map_err(Error::Io)?;

                    ContentRange::Inclusive {
                        first_byte_pos,
                        last_byte_pos,
                        complete_len: total_len,
                    }
                }
                Range::From { first_byte_pos } => {
                    if first_byte_pos >= total_len {
                        return Err(Error::RangeNotSatisfiable);
                    }

                    file.seek(SeekFrom::Start(first_byte_pos)).map_err(Error::Io)?;

                    ContentRange::Inclusive {
                        first_byte_pos,
                        last_byte_pos: total_len - 1,
                        complete_len: total_len,
                    }
                }
                Range::Suffix { len } => {
                    if len > total_len {
                        return Err(Error::RangeNotSatisfiable);
                    }
                    let start = total_len - len;
                    file.seek(SeekFrom::Start(start)).map_err(Error::Io)?;

                    ContentRange::Inclusive {
                        first_byte_pos: start,
                        last_byte_pos: total_len - 1,
                        complete_len: total_len,
                    }
                }
            };

            let content_len = content_range.content_len();

            Ok(Resource {
                content_range,
                stream: Box::pin(file_stream(content_len, file, Some(file_path))),
            })
        }
        .boxed()
    }
}

impl RepoProvider for FileSystemRepository {
    #[cfg(not(target_os = "fuchsia"))]
    fn spec(&self) -> RepositorySpec {
        RepositorySpec::FileSystem {
            metadata_repo_path: self.metadata_repo_path.clone(),
            blob_repo_path: self.blob_repo_path.clone(),
            aliases: self.aliases.clone(),
        }
    }

    fn aliases(&self) -> &BTreeSet<String> {
        &self.aliases
    }

    fn fetch_metadata_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.fetch(&self.metadata_repo_path, resource_path, range)
    }

    fn fetch_blob_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.fetch(&self.blob_repo_path, resource_path, range)
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn supports_watch(&self) -> bool {
        true
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn watch(&self) -> Result<BoxStream<'static, ()>> {
        // Since all we are doing is signaling that the timestamp file is changed, it's it's fine
        // if the channel is full, since that just means we haven't consumed our notice yet.
        let (mut sender, receiver) = futures::channel::mpsc::channel(1);

        let mut watcher = recommended_watcher(move |event: notify::Result<notify::Event>| {
            let event = match event {
                Ok(event) => event,
                Err(err) => {
                    warn!("error receving notify event: {}", err);
                    return;
                }
            };

            if !matches!(
                event.kind,
                notify::EventKind::Create(_)
                    | notify::EventKind::Modify(_)
                    | notify::EventKind::Remove(_)
            ) {
                return;
            }

            // Send an event if any applied to timestamp.json.
            let timestamp_name = OsStr::new("timestamp.json");
            if event.paths.iter().any(|p| p.file_name() == Some(timestamp_name))
                && let Err(e) = sender.try_send(())
            {
                if e.is_full() {
                    // It's okay to ignore a full channel, since that just means that the other
                    // side of the channel still has an outstanding notice, which should be the
                    // same effect if we re-sent the event.
                } else if !e.is_disconnected() {
                    warn!("Error sending event: {:?}", e);
                }
            }
        })?;

        // Watch the repo path instead of directly watching timestamp.json to avoid
        // https://github.com/notify-rs/notify/issues/165.
        watcher.watch(self.metadata_repo_path.as_std_path(), RecursiveMode::NonRecursive)?;

        Ok(WatchStream { _watcher: watcher, receiver }.boxed())
    }

    fn blob_modification_time<'a>(
        &'a self,
        path: &str,
    ) -> BoxFuture<'a, Result<Option<SystemTime>>> {
        let file_path = sanitize_path(&self.blob_repo_path, path);
        async move {
            let file_path = file_path?;
            Ok(Some(fs::metadata(&file_path)?.modified()?))
        }
        .boxed()
    }

    fn blob_type(&self) -> DeliveryBlobType {
        self.delivery_blob_type
    }
}

impl TufRepositoryProvider<Pouf1> for FileSystemRepository {
    fn fetch_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.tuf_repo.fetch_metadata(meta_path, version)
    }

    fn fetch_target<'a>(
        &'a self,
        target_path: &TargetPath,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.tuf_repo.fetch_target(target_path)
    }
}

impl TufRepositoryStorage<Pouf1> for FileSystemRepository {
    fn store_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
        metadata: &'a mut (dyn AsyncRead + Send + Unpin + 'a),
    ) -> BoxFuture<'a, tuf::Result<()>> {
        self.tuf_repo.store_metadata(meta_path, version, metadata)
    }

    fn store_target<'a>(
        &'a self,
        target_path: &TargetPath,
        target: &'a mut (dyn AsyncRead + Send + Unpin + 'a),
    ) -> BoxFuture<'a, tuf::Result<()>> {
        self.tuf_repo.store_target(target_path, target)
    }
}

impl RepoStorage for FileSystemRepository {
    fn store_blob<'a>(
        &'a self,
        hash: &Hash,
        len: u64,
        src: &Utf8Path,
    ) -> BoxFuture<'a, Result<()>> {
        let src = src.to_path_buf();
        let hash_str = hash.to_string();
        let hash = *hash;

        async move {
            let src_metadata = fs::metadata(&src)?;
            if src_metadata.len() != len {
                return Err(anyhow!(BlobSizeMismatchError {
                    hash,
                    path: src.clone(),
                    manifest_size: len,
                    file_size: src_metadata.len(),
                }));
            }

            let dst = sanitize_path(
                &self.blob_repo_path,
                &format!("{}/{hash_str}", u32::from(self.delivery_blob_type)),
            )?;
            let existing_len = match fs::File::open(&dst) {
                Ok(file) => {
                    if let Ok(len) = delivery_blob::decompressed_size_from_reader(file) {
                        Some(len)
                    } else {
                        // In the event that the delivery blob is corrupt, log a warning and
                        // return None to signify that it needs to be written.
                        warn!("corrupt delivery blob found at {dst}, overwriting");
                        None
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(anyhow!(e)),
            };

            if self.copy_mode == CopyMode::CopyOverwrite || existing_len != Some(len) {
                generate_delivery_blob(&src, &dst, self.delivery_blob_type).await?;
            }

            Ok(())
        }
        .boxed()
    }

    fn store_delivery_blob<'a>(
        &'a self,
        hash: &Hash,
        src: &Utf8Path,
        delivery_blob_type: DeliveryBlobType,
    ) -> BoxFuture<'a, Result<()>> {
        let src = src.to_path_buf();
        let hash = *hash;

        async move {
            if delivery_blob_type != self.delivery_blob_type {
                warn!(
                    "storing delivery blob type {:?} in repository with delivery blob type {:?}",
                    delivery_blob_type, self.delivery_blob_type,
                );
                // TODO: convert the delivery blob to the expected type?
            }
            let dst = sanitize_path(
                &self.blob_repo_path,
                &format!("{}/{hash}", u32::from(delivery_blob_type)),
            )?;

            let src_metadata = fs::metadata(&src)?;
            let dst_metadata = match fs::metadata(&dst) {
                Ok(metadata) => Some(metadata),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(anyhow!(e)),
            };

            let dst_is_hardlink = if let Some(dst_metadata) = &dst_metadata {
                dst_metadata.nlink() > 1
            } else {
                false
            };

            let dst_exists = dst_metadata.is_some();
            let dst_dirty = !dst_exists;

            match self.copy_mode {
                CopyMode::Copy => {
                    if dst_dirty || dst_is_hardlink {
                        copy_blob(&src, &dst).await?
                    }
                }
                CopyMode::CopyOverwrite => copy_blob(&src, &dst).await?,
                CopyMode::HardLink => {
                    let is_hardlink = if let Some(dst_metadata) = &dst_metadata {
                        src_metadata.dev() == dst_metadata.dev()
                            && src_metadata.ino() == dst_metadata.ino()
                    } else {
                        false
                    };

                    if is_hardlink {
                        // No work to do if src and dest are already hardlinks.
                    } else {
                        // Create the parent directory if it doesn't yet exist.
                        if let Some(parent) = dst.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        match fs::hard_link(&src, &dst) {
                            Ok(()) => {
                                // FIXME(b/271694204): Workaround an unknown issue where hardlinks
                                // aren't readable immediately after creation in some environments.
                                if fs::metadata(&dst).is_err() {
                                    fuchsia_async::Timer::new(std::time::Duration::from_secs(1))
                                        .await;
                                    if fs::metadata(&dst).is_err() {
                                        copy_blob(&src, &dst).await?
                                    }
                                }
                            }
                            Err(_) if dst_dirty => copy_blob(&src, &dst).await?,
                            Err(_) => {
                                // The dest file exists and has the right size,
                                // but we failed to make it a hardlink.
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        .boxed()
    }
}

async fn create_temp_file(path: &Utf8Path) -> Result<TempPath> {
    let temp_file = if let Some(parent) = path.parent() {
        DirBuilder::new().recursive(true).create(parent)?;

        NamedTempFile::new_in(parent)?
    } else {
        NamedTempFile::new_in(".")?
    };

    Ok(temp_file.into_temp_path())
}

// Set the blob at `path` to be read-only.
async fn set_blob_read_only(path: &Utf8Path) -> Result<()> {
    let file = fs::File::open(path)?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(true);
    file.set_permissions(permissions)?;

    Ok(())
}

// Performs a Copy-on-Write (reflink) of the file at `src_path` to `dst_path`.
#[cfg(target_os = "linux")]
async fn reflink(src_path: &Utf8Path, dst_path: &Utf8Path) -> Result<(), std::io::Error> {
    use std::os::fd::AsRawFd;

    let src = fs::File::open(src_path)?;
    let dst = fs::File::create(dst_path)?;

    // Safe because this is a synchronous syscall and the raw fds don't outlive the call.
    let res = unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) };

    match res {
        -1 => {
            let err = std::io::Error::last_os_error();

            drop(dst);
            let _ = fs::remove_file(dst_path);

            match err.raw_os_error().unwrap() {
                // The filesystem does not support reflinks.
                libc::EOPNOTSUPP |
                // src_path and dst_path are different filesystems.
                libc::EXDEV |
                // An invalid ioctl number was specified in an ioctl system call.
                libc::ENOTTY => {
                    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, err))
                }
                _ => Err(err),
            }
        }
        _ => Ok(()),
    }
}

#[cfg(not(target_os = "linux"))]
async fn reflink(_src_path: &Utf8Path, _dst_path: &Utf8Path) -> Result<(), std::io::Error> {
    use libc as _;
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

async fn copy_blob(src: &Utf8Path, dst: &Utf8Path) -> Result<()> {
    let temp_path = create_temp_file(dst).await?;
    match reflink(src, (*temp_path).try_into()?).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
            let src = src.to_owned();
            let temp_path = temp_path.to_path_buf();
            fasync::unblock(move || fs::copy(src, &temp_path)).await?;
        }
        Err(e) => return Err(anyhow!(e)),
    }
    temp_path.persist(dst)?;

    set_blob_read_only(dst).await
}

pub(crate) async fn generate_delivery_blob(
    src: &Utf8Path,
    dst: &Utf8Path,
    blob_type: DeliveryBlobType,
) -> Result<()> {
    let src_blob = fs::read(src).with_context(|| format!("reading {src}"))?;

    let temp_path = create_temp_file(dst).await?;
    let file = std::fs::File::create(&temp_path)?;
    fasync::unblock(move || {
        delivery_blob::generate_to(blob_type, &src_blob, std::io::BufWriter::new(file))
    })
    .await
    .context("generate delivery blob")?;

    temp_path.persist(dst)?;

    set_blob_read_only(dst).await
}

#[cfg(not(target_os = "fuchsia"))]
#[pin_project::pin_project]
struct WatchStream {
    _watcher: notify::RecommendedWatcher,
    #[pin]
    receiver: futures::channel::mpsc::Receiver<()>,
}

#[cfg(not(target_os = "fuchsia"))]
impl Stream for WatchStream {
    type Item = ();
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().receiver.poll_next(cx)
    }
}

/// Make sure the resource is inside the repo_path.
fn sanitize_path(repo_path: &Utf8Path, resource_path: &str) -> Result<Utf8PathBuf, Error> {
    let resource_path = Utf8Path::new(resource_path);

    let mut parts = vec![];
    for component in resource_path.components() {
        match component {
            Utf8Component::Normal(part) => {
                parts.push(part);
            }
            _ => {
                warn!("invalid resource_path: {}", resource_path);
                return Err(Error::InvalidPath(resource_path.into()));
            }
        }
    }

    let path = parts.into_iter().collect::<Utf8PathBuf>();
    Ok(repo_path.join(path))
}

#[derive(Debug, thiserror::Error)]
#[error(
    "blob {hash} at {path:?} is {file_size} bytes in size, \
     but the package manifest indicates it should be {manifest_size} bytes in size"
)]
struct BlobSizeMismatchError {
    hash: Hash,
    path: Utf8PathBuf,
    manifest_size: u64,
    file_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::repo_tests::{self, TestEnv as _};
    use crate::util::CHUNK_SIZE;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::{FutureExt, StreamExt};
    use std::fs::File;
    use std::io::Write as _;
    use std::time::Duration;
    struct TestEnv {
        _tmp: tempfile::TempDir,
        metadata_path: Utf8PathBuf,
        blob_path: Utf8PathBuf,
        repo: FileSystemRepository,
    }

    impl TestEnv {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = Utf8Path::from_path(tmp.path()).unwrap();
            let metadata_path = dir.join("metadata");
            let blob_path = dir.join("blobs");
            std::fs::create_dir(&metadata_path).unwrap();
            std::fs::create_dir(&blob_path).unwrap();

            Self {
                _tmp: tmp,
                metadata_path: metadata_path.clone(),
                blob_path: blob_path.clone(),
                repo: FileSystemRepository::new(metadata_path, blob_path),
            }
        }
    }

    #[async_trait::async_trait]
    impl repo_tests::TestEnv for TestEnv {
        fn supports_range(&self) -> bool {
            true
        }

        fn write_metadata(&self, path: &str, bytes: &[u8]) {
            let file_path = self.metadata_path.join(path);
            let mut f = File::create(file_path).unwrap();
            f.write_all(bytes).unwrap();
        }

        fn write_blob(&self, path: &str, bytes: &[u8]) {
            let file_path = self.blob_path.join(path);
            let mut f = File::create(file_path).unwrap();
            f.write_all(bytes).unwrap();
        }

        fn repo(&self) -> &dyn RepoProvider {
            &self.repo
        }
    }

    repo_tests::repo_test_suite! {
        env = TestEnv::new();
        chunk_size = CHUNK_SIZE;
    }

    #[fuchsia::test]
    async fn test_blob_modification_time() {
        let env = TestEnv::new();

        let f = File::create(env.blob_path.join("empty-blob")).unwrap();
        let blob_mtime = f.metadata().unwrap().modified().unwrap();
        drop(f);

        assert_matches!(
            env.repo.blob_modification_time("empty-blob").await,
            Ok(Some(t)) if t == blob_mtime
        );
    }

    #[fuchsia::test]
    async fn test_reject_invalid_paths() {
        let env = TestEnv::new();
        env.write_metadata("empty", b"");

        assert_matches!(repo_tests::read_metadata(&env, "empty", Range::Full).await, Ok(body) if body == b"");
        assert_matches!(repo_tests::read_metadata(&env, "subdir/../empty", Range::Full).await,
            Err(Error::InvalidPath(path)) if path == Utf8Path::new("subdir/../empty")
        );
    }

    #[fuchsia::test]
    async fn test_watch() {
        let env = TestEnv::new();

        // We support watch.
        assert!(env.repo.supports_watch());

        let mut watch_stream = env.repo.watch().unwrap().fuse();

        // Try to read from the stream. This should not return anything since we haven't created a
        // file yet.
        futures::select! {
            _ = watch_stream.next() => panic!("should not have received an event"),
            _ = fasync::Timer::new(Duration::from_millis(10)).fuse() => (),
        };

        // Next, write to the file and make sure we observe an event.
        env.write_metadata("timestamp.json", br#"{"version":1}"#);

        futures::select! {
            result = watch_stream.next() => {
                assert_eq!(result, Some(()));
            },
            _ = fasync::Timer::new(Duration::from_secs(10)).fuse() => {
                panic!("wrote to timestamp.json, but did not get an event");
            },
        };

        // Write to the file again and make sure we receive another event.
        env.write_metadata("timestamp.json", br#"{"version":2}"#);

        futures::select! {
            result = watch_stream.next() => {
                assert_eq!(result, Some(()));
            },
            _ = fasync::Timer::new(Duration::from_secs(10)).fuse() => {
                panic!("wrote to timestamp.json, but did not get an event");
            },
        };

        // FIXME(https://github.com/notify-rs/notify/pull/337): On OSX, notify uses a
        // crossbeam-channel in `Drop` to shut down the interior thread. Unfortunately this can
        // trip over an issue where OSX will tear down the thread local storage before shutting
        // down the thread, which can trigger a panic. To avoid this issue, sleep a little bit
        // after shutting down our stream.
        drop(watch_stream);
        fasync::Timer::new(Duration::from_millis(100)).await;
    }

    #[fuchsia::test]
    async fn test_watch_ignores_access() {
        let env = TestEnv::new();

        // Write an initial timestamp.json file.
        env.write_metadata("timestamp.json", br#"{"version":1}"#);

        let mut watch_stream = env.repo.watch().unwrap().fuse();

        // Opening and reading timestamp.json should not trigger a watch event.
        let _ = std::fs::read(env.metadata_path.join("timestamp.json")).unwrap();

        futures::select! {
            _ = watch_stream.next() => {
                panic!("reading timestamp.json should not trigger a watch event")
            }
            _ = fasync::Timer::new(Duration::from_millis(200)).fuse() => (),
        };

        drop(watch_stream);
        fasync::Timer::new(Duration::from_millis(100)).await;
    }

    #[fuchsia::test]
    async fn test_store_blob_verifies_src_length() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        std::fs::create_dir(&metadata_repo_path).unwrap();
        std::fs::create_dir(&blob_repo_path).unwrap();

        let repo = FileSystemRepository::builder(metadata_repo_path, blob_repo_path.clone())
            .copy_mode(CopyMode::Copy)
            .build();

        // Store the blob.
        let contents = b"hello world";
        let path = dir.join("my-blob");
        std::fs::write(&path, contents).unwrap();

        let hash = fuchsia_merkle::root_from_slice(contents);
        let err = repo.store_blob(&hash, contents.len() as u64 + 1, &path).await.unwrap_err();
        assert_matches!(err.downcast_ref::<BlobSizeMismatchError>(), Some(_));
    }

    #[fuchsia::test]
    async fn test_store_blob_copy_detects_length_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        std::fs::create_dir(&metadata_repo_path).unwrap();
        std::fs::create_dir(&blob_repo_path).unwrap();

        let repo = FileSystemRepository::builder(metadata_repo_path, blob_repo_path.clone())
            .copy_mode(CopyMode::Copy)
            .build();

        // The blob contents and its hash.
        let contents = b"hello world";
        let hash = fuchsia_merkle::root_from_slice(contents);

        let path = dir.join("my-blob");
        std::fs::write(&path, contents).unwrap();

        assert_matches!(repo.store_blob(&hash, contents.len() as u64, &path).await, Ok(()));

        // Make sure we can read it back.
        let blob_path = blob_repo_path.join(format!("1/{hash}"));
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents[..]);

        assert!(std::fs::metadata(&blob_path).unwrap().permissions().readonly());

        // Next, overwrite a blob that already exists.
        let contents2 = b"another hello world";
        let path2 = dir.join("my-blob2");
        std::fs::write(&path2, contents2).unwrap();
        assert_matches!(repo.store_blob(&hash, contents2.len() as u64, &path2).await, Ok(()));

        // Make sure we get the new contents back.
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents2[..]);
    }

    #[fuchsia::test]
    async fn test_store_blob_copy_skips_present_blobs_of_correct_length() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        std::fs::create_dir(&metadata_repo_path).unwrap();
        std::fs::create_dir(&blob_repo_path).unwrap();

        let repo = FileSystemRepository::builder(metadata_repo_path, blob_repo_path.clone())
            .copy_mode(CopyMode::Copy)
            .build();

        // Store the blob.
        let contents = b"hello world.";
        let path = dir.join("my-blob");
        std::fs::write(&path, contents).unwrap();

        let hash = fuchsia_merkle::root_from_slice(contents);
        assert_matches!(repo.store_blob(&hash, contents.len() as u64, &path).await, Ok(()));

        // Make sure we can read it back.
        let blob_path = blob_repo_path.join(format!("1/{hash}"));
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents[..]);

        assert!(std::fs::metadata(&blob_path).unwrap().permissions().readonly());

        // Next, we won't overwrite a blob that already exists.
        let contents2 = b"Hello World!";
        let path2 = dir.join("my-blob2");
        std::fs::write(&path2, contents2).unwrap();
        assert_matches!(repo.store_blob(&hash, contents2.len() as u64, &path2).await, Ok(()));

        // Make sure we get the original contents back.
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents[..]);
    }

    #[fuchsia::test]
    async fn test_store_blob_copy_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        std::fs::create_dir(&metadata_repo_path).unwrap();
        std::fs::create_dir(&blob_repo_path).unwrap();

        let repo = FileSystemRepository::builder(metadata_repo_path, blob_repo_path.clone())
            .copy_mode(CopyMode::CopyOverwrite)
            .build();

        // Store the blob.
        let contents = b"hello world";
        let path = dir.join("my-blob");
        std::fs::write(&path, contents).unwrap();

        let hash = fuchsia_merkle::root_from_slice(contents);
        assert_matches!(repo.store_blob(&hash, contents.len() as u64, &path).await, Ok(()));

        // Make sure we can read it back.
        let blob_path = blob_repo_path.join(format!("1/{hash}"));
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents[..]);

        assert!(std::fs::metadata(&blob_path).unwrap().permissions().readonly());

        // Next, overwrite a blob that already exists.
        let contents2 = b"another blob";
        let path2 = dir.join("my-blob2");
        std::fs::write(&path2, contents2).unwrap();
        assert_matches!(repo.store_blob(&hash, contents2.len() as u64, &path2).await, Ok(()));

        // Make sure we get the new contents back.
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents2[..]);
    }

    #[fuchsia::test]
    async fn test_store_delivery_blob_hard_link() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        std::fs::create_dir(&metadata_repo_path).unwrap();
        std::fs::create_dir(&blob_repo_path).unwrap();

        let repo = FileSystemRepository::builder(metadata_repo_path, blob_repo_path.clone())
            .copy_mode(CopyMode::HardLink)
            .build();

        // Store the blob.
        let contents = b"hello world";
        let hash = fuchsia_merkle::root_from_slice(contents);

        let uncompressed_path = dir.join("my-blob");
        std::fs::write(&uncompressed_path, contents).unwrap();
        let path = dir.join("my-delivery-blob");
        generate_delivery_blob(&uncompressed_path, &path, DeliveryBlobType::Type1).await.unwrap();

        assert_matches!(
            repo.store_delivery_blob(&hash, &path, DeliveryBlobType::Type1).await,
            Ok(())
        );

        // Make sure we can read it back.
        let blob_path = blob_repo_path.join(format!("1/{hash}"));
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual: Vec<u8> = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents[..]);

        #[cfg(target_family = "unix")]
        async fn check_links(blob_path: &Utf8Path) {
            use std::os::unix::fs::MetadataExt as _;

            assert_eq!(std::fs::metadata(blob_path).unwrap().nlink(), 2);
        }

        #[cfg(not(target_family = "unix"))]
        async fn check_links(_blob_path: &Utf8Path) {}

        // Make sure the hard link count was incremented.
        check_links(&blob_path).await;
    }

    #[fuchsia::test]
    async fn test_store_blob_generates_delivery_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        std::fs::create_dir(&metadata_repo_path).unwrap();
        std::fs::create_dir(&blob_repo_path).unwrap();

        let repo = FileSystemRepository::builder(metadata_repo_path, blob_repo_path.clone())
            .delivery_blob_type(DeliveryBlobType::Type1)
            .build();

        // Store the blob.
        let contents = b"hello world";
        let hash = fuchsia_merkle::root_from_slice(contents);

        let path = dir.join("my-blob");
        std::fs::write(&path, contents).unwrap();

        assert_matches!(repo.store_blob(&hash, contents.len() as u64, &path).await, Ok(()));

        // Make sure we can read the delivery blob.
        let blob_path = blob_repo_path.join("1").join(hash.to_string());
        let delivery_blob = std::fs::read(&blob_path).unwrap();
        let actual = delivery_blob::decompress(&delivery_blob).unwrap();
        assert_eq!(&actual, &contents[..]);

        assert!(std::fs::metadata(&blob_path).unwrap().permissions().readonly());
    }

    #[fuchsia::test]
    async fn test_store_delivery_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let metadata_repo_path = dir.join("metadata");
        let blob_repo_path = dir.join("blobs");
        std::fs::create_dir(&metadata_repo_path).unwrap();
        std::fs::create_dir(&blob_repo_path).unwrap();

        let repo = FileSystemRepository::builder(metadata_repo_path, blob_repo_path.clone())
            .delivery_blob_type(DeliveryBlobType::Type1)
            .build();

        // Store the blob.
        let contents = b"hello world";
        let uncompressed_path = dir.join("my-blob");
        std::fs::write(&uncompressed_path, contents).unwrap();
        let path = dir.join("my-delivery-blob");
        generate_delivery_blob(&uncompressed_path, &path, DeliveryBlobType::Type1).await.unwrap();
        let delivery_blob = std::fs::read(&path).unwrap();

        let hash = fuchsia_merkle::root_from_slice(contents);
        assert_matches!(
            repo.store_delivery_blob(&hash, &path, DeliveryBlobType::Type1).await,
            Ok(())
        );

        // Make sure we can read the delivery blob.
        let blob_path = blob_repo_path.join("1").join(hash.to_string());
        let stored_delivery_blob = std::fs::read(&blob_path).unwrap();
        assert_eq!(stored_delivery_blob, delivery_blob);

        assert!(std::fs::metadata(&blob_path).unwrap().permissions().readonly());
    }
}
