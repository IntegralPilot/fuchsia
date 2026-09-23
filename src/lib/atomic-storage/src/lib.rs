// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, bail, format_err};
use fidl_fuchsia_io::DirectoryProxy;
use fuchsia_async::{MonotonicInstant, Task, Timer};
use fuchsia_fs::Flags;
use fuchsia_fs::file::ReadError;
use fuchsia_fs::node::OpenError;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::future::OptionFuture;
use futures::lock::{Mutex, MutexGuard};
use futures::{FutureExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::pin::pin;
use std::rc::Rc;
use zx::{MonotonicDuration, Status};

const MIN_FLUSH_INTERVAL_MS: i64 = 500;
const MAX_FLUSH_INTERVAL_MS: i64 = 1_800_000; // 30 minutes
const MIN_FLUSH_DURATION: MonotonicDuration = MonotonicDuration::from_millis(MIN_FLUSH_INTERVAL_MS);

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateState {
    Updated,
    Unchanged,
}

/// A trait for types that have an associated constant key for storage.
pub trait StorageKey: Serialize + DeserializeOwned {
    const KEY: &'static str;
}

/// A highly resilient, atomic JSON file store.
/// Designed as a lightweight replacement for platform-wide persistence.
pub struct AtomicJsonStorage {
    typed_storage_map: HashMap<String, TypedStorage>,
    caching_enabled: bool,
    debounce_writes: bool,
    storage_dir: DirectoryProxy,
}

struct TypedStorage {
    flush_sender: UnboundedSender<()>,
    cached_storage: Rc<Mutex<CachedStorage>>,
}

struct CachedStorage {
    current_data: Option<Vec<u8>>,
    temp_file_path: String,
    file_path: String,
}

impl CachedStorage {
    async fn sync(&mut self, storage_dir: &DirectoryProxy) -> Result<(), Error> {
        {
            let file_proxy = fuchsia_fs::directory::open_file(
                storage_dir,
                &self.temp_file_path,
                Flags::FLAG_MAYBE_CREATE
                    | Flags::FILE_TRUNCATE
                    | fuchsia_fs::PERM_READABLE
                    | fuchsia_fs::PERM_WRITABLE,
            )
            .await
            .with_context(|| format!("unable to open {:?} for writing", self.temp_file_path))?;

            fuchsia_fs::file::write(&file_proxy, self.current_data.as_ref().unwrap())
                .await
                .context("failed to write data to file")?;
            file_proxy
                .close()
                .await
                .context("failed to call close on temp file")?
                .map_err(zx::Status::err_from_raw)?;
        }

        fuchsia_fs::directory::rename(storage_dir, &self.temp_file_path, &self.file_path)
            .await
            .context("failed to rename temp file to permanent file")?;

        storage_dir
            .sync()
            .await
            .context("failed to call sync on directory after rename")?
            .map_err(zx::Status::err_from_raw)
            .or_else(|e| if let zx::Status::NOT_SUPPORTED = e { Ok(()) } else { Err(e) })
            .context("failed to sync rename to directory")
    }
}

impl AtomicJsonStorage {
    /// Initializes the storage for a given set of keys.
    /// Automatically generates safe temp and permanent file paths.
    pub async fn new<I, S>(
        keys: I,
        storage_dir: DirectoryProxy,
    ) -> Result<(Self, Vec<Task<()>>), Error>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut typed_storage_map = HashMap::new();
        let iter = keys.into_iter();
        let mut sync_tasks = Vec::with_capacity(iter.size_hint().0);

        for key in iter {
            let key_str = key.into();

            let temp_file_path = format!("{}_tmp.json", key_str);
            let file_path = format!("{}.json", key_str);

            let (flush_sender, flush_receiver) = futures::channel::mpsc::unbounded::<()>();

            let cached_storage = Rc::new(Mutex::new(CachedStorage {
                current_data: None,
                temp_file_path,
                file_path,
            }));
            let storage = TypedStorage { flush_sender, cached_storage: Rc::clone(&cached_storage) };

            let sync_task = Task::local(Self::synchronize_task(
                Clone::clone(&storage_dir),
                Rc::clone(&cached_storage),
                flush_receiver,
            ));
            sync_tasks.push(sync_task);
            typed_storage_map.insert(key_str, storage);
        }

        Ok((
            AtomicJsonStorage {
                caching_enabled: true,
                debounce_writes: true,
                typed_storage_map,
                storage_dir,
            },
            sync_tasks,
        ))
    }

    async fn synchronize_task(
        storage_dir: DirectoryProxy,
        cached_storage: Rc<Mutex<CachedStorage>>,
        flush_receiver: UnboundedReceiver<()>,
    ) {
        let mut has_pending_flush = false;
        let mut last_flush: MonotonicInstant = MonotonicInstant::now() - MIN_FLUSH_DURATION;
        let mut next_flush_timer = pin!(OptionFuture::<Timer>::from(None).fuse());
        let mut retries = 0;
        let mut retrying = false;

        let flush_fuse = flush_receiver.fuse();
        futures::pin_mut!(flush_fuse);

        loop {
            futures::select! {
                _ = flush_fuse.select_next_some() => {
                    if retrying { continue; }

                    let now = MonotonicInstant::now();
                    let next_flush_time = if now - last_flush > MIN_FLUSH_DURATION {
                        now
                    } else {
                        last_flush + MIN_FLUSH_DURATION
                    };

                    has_pending_flush = true;
                    next_flush_timer.set(OptionFuture::from(Some(Timer::new(next_flush_time))).fuse());
                }

                _ = next_flush_timer => {
                    if has_pending_flush {
                        let mut cached_storage = cached_storage.lock().await;

                        if let Err(e) = cached_storage.sync(&storage_dir).await {
                            retrying = true;
                            let flush_duration = MonotonicDuration::from_millis(
                                2_i64.saturating_pow(retries)
                                    .saturating_mul(MIN_FLUSH_INTERVAL_MS)
                                    .min(MAX_FLUSH_INTERVAL_MS)
                            );
                            let next_flush_time = MonotonicInstant::now() + flush_duration;
                            log::error!(
                                "Failed to sync write to disk for {:?}, delaying by {:?}, caused by: {:?}",
                                cached_storage.file_path, flush_duration, e
                            );

                            next_flush_timer.set(OptionFuture::from(Some(Timer::new(next_flush_time))).fuse());
                            retries += 1;
                            continue;
                        }
                        last_flush = MonotonicInstant::now();
                        has_pending_flush = false;
                        retrying = false;
                        retries = 0;
                    }
                }
                complete => break,
            }
        }
    }

    /// If true, reads will be returned from the data in memory rather than reading from storage.
    pub fn set_caching_enabled(&mut self, enabled: bool) {
        self.caching_enabled = enabled;
    }

    /// If true, writes to the underlying storage will only occur at most every
    /// [MIN_WRITE_INTERVAL_MS].
    pub fn set_debounce_writes(&mut self, debounce: bool) {
        self.debounce_writes = debounce;
    }

    async fn inner_write(
        &self,
        key: &str,
        new_value: Vec<u8>,
        immediate: bool,
    ) -> Result<UpdateState, Error> {
        let typed_storage = self
            .typed_storage_map
            .get(key)
            .ok_or_else(|| format_err!("Invalid storage key: {}", key))?;

        let mut cached_storage = typed_storage.cached_storage.lock().await;

        let bytes;
        let cached_value = match cached_storage.current_data.as_ref() {
            Some(cached_value) => Some(cached_value),
            None => {
                let file_proxy = fuchsia_fs::directory::open_file(
                    &self.storage_dir,
                    &cached_storage.file_path,
                    fuchsia_fs::PERM_READABLE,
                )
                .await;

                bytes = match file_proxy {
                    Ok(file_proxy) => match fuchsia_fs::file::read(&file_proxy).await {
                        Ok(bytes) => Some(bytes),
                        Err(ReadError::Open(OpenError::OpenError(e))) if e == Status::NOT_FOUND => {
                            None
                        }
                        Err(e) => bail!("failed to read json storage for {:?}: {:?}", key, e),
                    },
                    Err(OpenError::OpenError(Status::NOT_FOUND)) => None,
                    Err(e) => bail!("unable to read data on disk for {:?}: {:?}", key, e),
                };
                bytes.as_ref()
            }
        };

        Ok(if cached_value.map(|c| *c != new_value).unwrap_or(true) {
            cached_storage.current_data = Some(new_value);
            if immediate || !self.debounce_writes {
                cached_storage
                    .sync(&self.storage_dir)
                    .await
                    .with_context(|| format!("Failed to sync data for key {key:?}"))?;
            } else {
                typed_storage.flush_sender.unbounded_send(()).with_context(|| {
                    format!("flush_sender failed to send flush message for key {key}")
                })?;
            }
            UpdateState::Updated
        } else {
            UpdateState::Unchanged
        })
    }

    /// Serializes and writes a value to storage using standard Serde boundaries.
    pub async fn write<T: Serialize>(
        &self,
        key: &str,
        new_value: &T,
    ) -> Result<UpdateState, Error> {
        let new_value_bytes =
            serde_json::to_vec(new_value).context("Failed to serialize to JSON")?;
        self.inner_write(key, new_value_bytes, false).await
    }

    /// Serializes and immediately writes a value to storage, bypassing any debounce interval.
    pub async fn write_immediate<T: Serialize>(
        &self,
        key: &str,
        new_value: &T,
    ) -> Result<UpdateState, Error> {
        let new_value_bytes =
            serde_json::to_vec(new_value).context("Failed to serialize to JSON")?;
        self.inner_write(key, new_value_bytes, true).await
    }

    /// Serializes and writes a value whose type implements [`StorageKey`].
    pub async fn write_storable<T: StorageKey>(&self, new_value: &T) -> Result<UpdateState, Error> {
        self.write(T::KEY, new_value).await
    }

    /// Serializes and immediately writes a value whose type implements [`StorageKey`], bypassing
    /// any debounce interval.
    pub async fn write_storable_immediate<T: StorageKey>(
        &self,
        new_value: &T,
    ) -> Result<UpdateState, Error> {
        self.write_immediate(T::KEY, new_value).await
    }

    /// Test-only method to write directly to disk without touching the cache. This is used for
    /// setting up data as if it existed on disk before the storage was constructed.
    pub async fn write_test_bytes(&self, key: &str, value: Vec<u8>) -> Result<(), Error> {
        self.inner_write(key, value, false).await.map(|_| ())
    }

    async fn get_inner(&self, key: &str) -> Result<MutexGuard<'_, CachedStorage>, Error> {
        let typed_storage = self
            .typed_storage_map
            .get(key)
            .ok_or_else(|| format_err!("Invalid storage key: {key}"))?;

        let mut cached_storage = typed_storage.cached_storage.lock().await;

        if (cached_storage.current_data.is_none() || !self.caching_enabled)
            && let Some(file_proxy) = match fuchsia_fs::directory::open_file(
                &self.storage_dir,
                &cached_storage.file_path,
                fuchsia_fs::PERM_READABLE,
            )
            .await
            {
                Ok(file_proxy) => Some(file_proxy),
                Err(OpenError::OpenError(Status::NOT_FOUND)) => None,
                Err(e) => bail!("failed to open file for {key:?}: {e:?}"),
            }
        {
            let data = match fuchsia_fs::file::read(&file_proxy).await {
                Ok(data) => Some(data),
                Err(ReadError::ReadError(Status::NOT_FOUND)) => None,
                Err(e) => bail!("failed to get json data from disk for {key:?}: {e:?}"),
            };
            cached_storage.current_data = data;
        }

        Ok(cached_storage)
    }

    /// Gets the latest value cached locally, or loads the value from storage.
    /// Doesn't support multiple concurrent callers of the same struct.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, Error> {
        let cached_storage = self.get_inner(key).await?;

        match cached_storage.current_data.as_ref() {
            Some(data) => {
                let parsed = serde_json::from_slice::<T>(data)
                    .with_context(|| format!("Failed to parse JSON for key: {}", key))?;
                Ok(Some(parsed))
            }
            None => Ok(None),
        }
    }

    /// Reads a value whose type implements [`StorageKey`].
    pub async fn get_storable<T: StorageKey>(&self) -> Result<Option<T>, Error> {
        self.get::<T>(T::KEY).await
    }

    /// Convenience wrapper that falls back to `T::default()` if the data is missing or corrupt.
    pub async fn get_or_default<T: DeserializeOwned + Default>(&self, key: &str) -> T {
        match self.get::<T>(key).await {
            Ok(Some(value)) => value,
            Ok(None) => T::default(),
            Err(e) => {
                log::error!("Error reading {}: {:?}. Falling back to default.", key, e);
                T::default()
            }
        }
    }

    /// Reads a value whose type implements [`StorageKey`], falling back to `T::default()` if
    /// the data is missing or corrupt.
    pub async fn get_storable_or_default<T: StorageKey + Default>(&self) -> T {
        self.get_or_default::<T>(T::KEY).await
    }

    /// Convenience wrapper that falls back to evaluating `default_fn()` if the data is missing
    /// or corrupt.
    pub async fn get_or_else<T: DeserializeOwned, F: FnOnce() -> T>(
        &self,
        key: &str,
        default_fn: F,
    ) -> T {
        match self.get::<T>(key).await {
            Ok(Some(value)) => value,
            Ok(None) => default_fn(),
            Err(e) => {
                log::error!("Error reading {}: {:?}. Falling back to default.", key, e);
                default_fn()
            }
        }
    }

    /// Reads a value whose type implements [`StorageKey`], falling back to evaluating `default_fn()`
    /// if the data is missing or corrupt.
    pub async fn get_storable_or_else<T: StorageKey, F: FnOnce() -> T>(&self, default_fn: F) -> T {
        self.get_or_else::<T, F>(T::KEY, default_fn).await
    }

    /// Deletes the stored value for a given key, removing the file from storage and clearing the
    /// in-memory cache.
    pub async fn delete(&self, key: &str) -> Result<UpdateState, Error> {
        let typed_storage = self
            .typed_storage_map
            .get(key)
            .ok_or_else(|| format_err!("Invalid storage key: {}", key))?;

        let mut cached_storage = typed_storage.cached_storage.lock().await;
        let had_data = cached_storage.current_data.is_some();
        cached_storage.current_data = None;

        let unlink_res = self
            .storage_dir
            .unlink(&cached_storage.file_path, &fidl_fuchsia_io::UnlinkOptions::default())
            .await
            .context("failed to send unlink request")?;

        match unlink_res {
            Ok(()) => {
                self.storage_dir
                    .sync()
                    .await
                    .context("failed to call sync on directory after unlink")?
                    .map_err(zx::Status::err_from_raw)
                    .or_else(|e| if let zx::Status::NOT_SUPPORTED = e { Ok(()) } else { Err(e) })
                    .context("failed to sync unlink to directory")?;
                Ok(UpdateState::Updated)
            }
            Err(s) if zx::Status::err_from_raw(s) == zx::Status::NOT_FOUND => {
                if had_data {
                    Ok(UpdateState::Updated)
                } else {
                    Ok(UpdateState::Unchanged)
                }
            }
            Err(s) => bail!(
                "failed to unlink file {:?}: {:?}",
                cached_storage.file_path,
                zx::Status::err_from_raw(s)
            ),
        }
    }

    /// Deletes the stored value for a type implementing [`StorageKey`].
    pub async fn delete_storable<T: StorageKey>(&self) -> Result<UpdateState, Error> {
        self.delete(T::KEY).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fasync::TestExecutor;
    use fidl::epitaph::ChannelEpitaphExt;
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use std::task::Poll;
    use test_case::test_case;
    use zx::Status;

    const VALUE0: i32 = 3;
    const VALUE1: i32 = 33;
    const VALUE2: i32 = 128;
    const TEST_KEY: &str = "testkey";

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct TestStruct {
        value: i32,
    }

    impl Default for TestStruct {
        fn default() -> Self {
            Self { value: VALUE0 }
        }
    }

    fn open_tempdir(tempdir: &tempfile::TempDir) -> fio::DirectoryProxy {
        fuchsia_fs::directory::open_in_namespace(
            tempdir.path().to_str().expect("tempdir path is not valid UTF-8"),
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
        )
        .expect("failed to open connection to tempdir")
    }

    #[fuchsia::test]
    async fn test_get() {
        let value_to_get = TestStruct { value: VALUE1 };
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let content = serde_json::to_vec(&value_to_get).unwrap();

        // Write the permanent file to disk directly to simulate pre-existing data
        std::fs::write(tempdir.path().join(format!("{}.json", TEST_KEY)), content)
            .expect("failed to write file");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) = AtomicJsonStorage::new(vec![TEST_KEY], storage_dir)
            .await
            .expect("should be able to initialize storage");

        for task in sync_tasks {
            task.detach();
        }

        let result = storage.get::<TestStruct>(TEST_KEY).await.unwrap().unwrap();
        assert_eq!(result.value, VALUE1);
    }

    #[fuchsia::test]
    async fn test_get_default() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) = AtomicJsonStorage::new(vec![TEST_KEY], storage_dir)
            .await
            .expect("file proxy should be created");

        for task in sync_tasks {
            task.detach();
        }

        // Since no file exists, this should gracefully fall back to the default
        let result = storage.get_or_default::<TestStruct>(TEST_KEY).await;
        assert_eq!(result.value, VALUE0);
    }

    struct DirectoryInterceptor {
        real_dir: fio::DirectoryProxy,
        inner: std::sync::Mutex<DirectoryInterceptorInner>,
    }

    struct DirectoryInterceptorInner {
        sync_notifier: Option<futures::channel::mpsc::UnboundedSender<()>>,
        #[allow(clippy::type_complexity)]
        open_interceptor: Box<dyn Fn(&str, bool) -> Option<Status>>,
    }

    impl DirectoryInterceptor {
        #[allow(clippy::arc_with_non_send_sync)]
        fn new(real_dir: fio::DirectoryProxy) -> (Arc<Self>, fio::DirectoryProxy) {
            let (proxy, requests) =
                fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
            let this = Arc::new(Self {
                real_dir,
                inner: std::sync::Mutex::new(DirectoryInterceptorInner {
                    sync_notifier: None,
                    open_interceptor: Box::new(|_, _| None),
                }),
            });
            fasync::Task::local(this.clone().run(requests)).detach();
            (this.clone(), proxy)
        }

        fn install_sync_notifier(&self) -> futures::channel::mpsc::UnboundedReceiver<()> {
            let (sender, receiver) = futures::channel::mpsc::unbounded();
            self.inner.lock().unwrap().sync_notifier = Some(sender);
            receiver
        }

        #[allow(clippy::type_complexity)]
        fn set_open_interceptor(&self, interceptor: Box<dyn Fn(&str, bool) -> Option<Status>>) {
            self.inner.lock().unwrap().open_interceptor = interceptor;
        }

        async fn run(self: Arc<Self>, mut requests: fio::DirectoryRequestStream) {
            while let Ok(Some(request)) = requests.try_next().await {
                match request {
                    fio::DirectoryRequest::Open {
                        path,
                        flags,
                        options,
                        object,
                        control_handle: _,
                    } => {
                        let create = flags.intersects(
                            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::FLAG_MUST_CREATE,
                        );
                        match (self.inner.lock().unwrap().open_interceptor)(&path, create) {
                            Some(status) => {
                                object.close_with_epitaph(status).expect("failed to send epitaph");
                            }
                            None => {
                                self.real_dir
                                    .open(&path, flags, &options, object)
                                    .expect("failed to forward Open3 request");
                            }
                        }
                    }
                    fio::DirectoryRequest::Sync { responder } => {
                        let response =
                            self.real_dir.sync().await.expect("failed to forward Sync request");
                        responder.send(response).expect("failed to respond to Sync request");
                        if let Some(sender) = &self.inner.lock().unwrap().sync_notifier {
                            sender.unbounded_send(()).unwrap();
                        }
                    }
                    fio::DirectoryRequest::Rename { src, dst_parent_token, dst, responder } => {
                        let response = self
                            .real_dir
                            .rename(&src, dst_parent_token, &dst)
                            .await
                            .expect("failed to forward Rename request");
                        responder.send(response).expect("failed to respond to Rename request");
                    }
                    fio::DirectoryRequest::Unlink { name, options, responder } => {
                        let response = self
                            .real_dir
                            .unlink(&name, &options)
                            .await
                            .expect("failed to forward Unlink request");
                        responder.send(response).expect("failed to respond to Unlink request");
                    }
                    fio::DirectoryRequest::GetToken { responder } => {
                        let response = self
                            .real_dir
                            .get_token()
                            .await
                            .expect("failed to forward GetToken request");
                        responder
                            .send(response.0, response.1)
                            .expect("failed to respond to GetToken request");
                    }
                    request => unimplemented!("request: {:?}", request),
                }
            }
        }
    }

    fn run_until_ready<F>(executor: &mut TestExecutor, fut: F) -> F::Output
    where
        F: std::future::Future,
    {
        let mut fut = std::pin::pin!(fut);
        loop {
            match executor.run_until_stalled(&mut fut) {
                Poll::Ready(result) => return result,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn assert_file_not_found(
        executor: &mut TestExecutor,
        directory: &fio::DirectoryProxy,
        file_name: &str,
    ) {
        let open_fut =
            fuchsia_fs::directory::open_file(directory, file_name, fuchsia_fs::PERM_READABLE);
        let result = run_until_ready(executor, open_fut);
        assert_matches!(result, Result::Err(e) if e.is_not_found_error());
    }

    fn assert_file_contents(
        executor: &mut TestExecutor,
        directory: &fio::DirectoryProxy,
        file_name: &str,
        expected_contents: TestStruct,
    ) {
        let read_fut = fuchsia_fs::directory::read_file(directory, file_name);
        let data = run_until_ready(executor, read_fut).expect("reading file");
        let data =
            serde_json::from_slice::<TestStruct>(&data).expect("failed to read file as TestStruct");
        assert_eq!(data, expected_contents);
    }

    #[fuchsia::test]
    fn test_first_write_syncs_immediately() {
        let written_value = VALUE1;
        let mut executor = TestExecutor::new_with_fake_time();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));

        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);
        let (interceptor, storage_dir) = DirectoryInterceptor::new(storage_dir);
        let mut sync_receiver = interceptor.install_sync_notifier();

        let storage_fut = AtomicJsonStorage::new(vec![TEST_KEY], Clone::clone(&storage_dir));
        futures::pin_mut!(storage_fut);

        let (storage, _sync_tasks) =
            if let Poll::Ready(storage) = executor.run_until_stalled(&mut storage_fut) {
                storage.expect("file proxy should be created")
            } else {
                panic!("storage creation stalled");
            };

        let value_to_write = TestStruct { value: written_value };
        let write_future = storage.write(TEST_KEY, &value_to_write);
        futures::pin_mut!(write_future);

        assert_matches!(
            run_until_ready(&mut executor, &mut write_future),
            Result::Ok(UpdateState::Updated)
        );

        let filename = format!("{}.json", TEST_KEY);
        assert_file_not_found(&mut executor, &storage_dir, &filename);

        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");

        assert_file_contents(&mut executor, &storage_dir, &filename, value_to_write.clone());
    }

    #[fuchsia::test]
    fn test_second_write_syncs_after_interval() {
        let written_value = VALUE1;
        let second_value = VALUE2;
        let mut executor = TestExecutor::new_with_fake_time();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));

        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);
        let (interceptor, storage_dir) = DirectoryInterceptor::new(storage_dir);
        let mut sync_receiver = interceptor.install_sync_notifier();

        let storage_fut = AtomicJsonStorage::new(vec![TEST_KEY], Clone::clone(&storage_dir));
        futures::pin_mut!(storage_fut);

        let (storage, _sync_tasks) =
            if let Poll::Ready(storage) = executor.run_until_stalled(&mut storage_fut) {
                storage.expect("file proxy should be created")
            } else {
                panic!("storage creation stalled");
            };

        let value_to_write = TestStruct { value: written_value };
        let write_future = storage.write(TEST_KEY, &value_to_write);
        futures::pin_mut!(write_future);

        assert_matches!(
            run_until_ready(&mut executor, &mut write_future),
            Result::Ok(UpdateState::Updated)
        );

        let filename = format!("{}.json", TEST_KEY);
        assert_file_not_found(&mut executor, &storage_dir, &filename);

        run_until_ready(&mut executor, &mut sync_receiver.next()).expect("directory never synced");
        assert_file_contents(&mut executor, &storage_dir, &filename, value_to_write.clone());

        // Write second time
        let value_to_write2 = TestStruct { value: second_value };
        let binding = value_to_write2.clone();
        let write_future = storage.write(TEST_KEY, &binding);
        futures::pin_mut!(write_future);

        assert_matches!(
            run_until_ready(&mut executor, &mut write_future),
            Result::Ok(UpdateState::Updated)
        );

        // File on disk should still equal old value
        assert_file_contents(&mut executor, &storage_dir, &filename, value_to_write.clone());

        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000 - 1));
        assert!(!executor.wake_expired_timers());

        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000));
        run_until_ready(&mut executor, &mut sync_receiver.next()).expect("directory never synced");

        assert_file_contents(&mut executor, &storage_dir, &filename, value_to_write2.clone());
    }

    #[fuchsia::test]
    async fn test_unregistered_key_returns_error() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        // Initialize storage strictly with TEST_KEY
        let (storage, sync_tasks) = AtomicJsonStorage::new(vec![TEST_KEY], storage_dir)
            .await
            .expect("file proxy should be created");

        for task in sync_tasks {
            task.detach();
        }

        let result = storage.write(TEST_KEY, &TestStruct { value: VALUE2 }).await;
        assert!(result.is_ok());

        // Attempt to write to a key that wasn't declared in `new()`
        let result = storage.write("unregistered_key", &TestStruct { value: VALUE2 }).await;
        assert_matches!(result, Err(e) if e.to_string().contains("Invalid storage key: unregistered_key"));
    }

    #[fuchsia::test]
    fn test_multiple_write_debounce() {
        let mut executor = TestExecutor::new_with_fake_time();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));

        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);
        let (interceptor, storage_dir) = DirectoryInterceptor::new(storage_dir);
        let mut sync_receiver = interceptor.install_sync_notifier();

        let storage_fut = AtomicJsonStorage::new(vec![TEST_KEY], Clone::clone(&storage_dir));
        let (storage, _sync_tasks) =
            run_until_ready(&mut executor, storage_fut).expect("file proxy should be created");

        let value1 = TestStruct { value: VALUE1 };
        let value2 = TestStruct { value: VALUE2 };
        let value3 = TestStruct { value: VALUE0 };
        let filename = format!("{}.json", TEST_KEY);

        let result = run_until_ready(&mut executor, storage.write(TEST_KEY, &value1));
        assert_matches!(result, Result::Ok(UpdateState::Updated));

        assert_file_not_found(&mut executor, &storage_dir, &filename);

        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");
        assert_file_contents(&mut executor, &storage_dir, &filename, value1.clone());

        let result = run_until_ready(&mut executor, storage.write(TEST_KEY, &value2));
        assert_matches!(result, Result::Ok(UpdateState::Updated));

        let data = run_until_ready(&mut executor, storage.get_or_default::<TestStruct>(TEST_KEY));
        assert_eq!(data, value2);

        // Data has not been persisted to disk yet.
        assert_file_contents(&mut executor, &storage_dir, &filename, value1.clone());

        // Write a third time before advancing clock
        let result = run_until_ready(&mut executor, storage.write(TEST_KEY, &value3));
        assert_matches!(result, Result::Ok(UpdateState::Updated));

        let data = run_until_ready(&mut executor, storage.get_or_default::<TestStruct>(TEST_KEY));
        assert_eq!(data, value3);

        assert_file_contents(&mut executor, &storage_dir, &filename, value1);

        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000 - 1));
        assert!(!executor.wake_expired_timers());

        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000));
        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");

        // Validate final persistence
        assert_file_contents(&mut executor, &storage_dir, &filename, value3);
    }

    #[allow(clippy::unused_unit)]
    #[test_case(1, 500)]
    #[test_case(2, 1_000)]
    #[test_case(3, 2_000)]
    #[test_case(4, 4_000)]
    #[test_case(5, 8_000)]
    #[test_case(6, 16_000)]
    #[test_case(7, 32_000)]
    #[test_case(8, 64_000)]
    #[test_case(9, 128_000)]
    #[test_case(10, 256_000)]
    #[test_case(11, 512_000)]
    #[test_case(12, 1_024_000)]
    #[test_case(13, 1_800_000)]
    #[test_case(14, 1_800_000)]
    fn test_exponential_backoff(retry_count: usize, max_wait_time: usize) {
        let mut executor = TestExecutor::new_with_fake_time();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));

        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);
        let (interceptor, storage_dir) = DirectoryInterceptor::new(storage_dir);
        let attempts = std::sync::Mutex::new(0);

        interceptor.set_open_interceptor(Box::new(move |path, create| {
            let mut attempts_guard = attempts.lock().unwrap();
            if path == "abc_tmp.json" && create && *attempts_guard < retry_count {
                *attempts_guard += 1;
                Some(Status::NO_SPACE)
            } else {
                None
            }
        }));

        let mut sync_receiver = interceptor.install_sync_notifier();
        let expected_data = vec![1];

        let cached_storage = Rc::new(Mutex::new(CachedStorage {
            current_data: Some(expected_data.clone()),
            temp_file_path: "abc_tmp.json".to_owned(),
            file_path: "abc.json".to_owned(),
        }));

        let (sender, receiver) = futures::channel::mpsc::unbounded();

        let task = fasync::Task::local(AtomicJsonStorage::synchronize_task(
            Clone::clone(&storage_dir),
            Rc::clone(&cached_storage),
            receiver,
        ));
        futures::pin_mut!(task);

        executor.set_fake_time(MonotonicInstant::from_nanos(0));
        sender.unbounded_send(()).expect("can send flush signal");
        assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);

        let mut clock_nanos = 0;

        for new_duration in (0..retry_count).map(|i| {
            (2_i64.pow(i as u32) * MIN_FLUSH_INTERVAL_MS).min(max_wait_time as i64) * 1_000_000
                - (i == retry_count - 1) as i64
        }) {
            executor.set_fake_time(MonotonicInstant::from_nanos(clock_nanos));
            assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);

            assert_file_not_found(&mut executor, &storage_dir, "abc_tmp.json");
            assert_file_not_found(&mut executor, &storage_dir, "abc.json");

            clock_nanos += new_duration;
        }

        executor.set_fake_time(MonotonicInstant::from_nanos(clock_nanos));
        assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);

        assert_file_not_found(&mut executor, &storage_dir, "abc_tmp.json");
        assert_file_not_found(&mut executor, &storage_dir, "abc.json");

        clock_nanos += 1;
        executor.set_fake_time(MonotonicInstant::from_nanos(clock_nanos));
        assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);
        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");

        let read_fut = fuchsia_fs::directory::read_file(&storage_dir, "abc.json");
        let data = run_until_ready(&mut executor, read_fut).expect("reading file");
        assert_eq!(data, expected_data);

        drop(sender);
        run_until_ready(&mut executor, task);
    }

    #[fuchsia::test]
    async fn test_write_immediate() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) =
            AtomicJsonStorage::new(vec![TEST_KEY], Clone::clone(&storage_dir))
                .await
                .expect("should be able to initialize storage");

        for task in sync_tasks {
            task.detach();
        }

        let value = TestStruct { value: VALUE2 };
        let res = storage.write_immediate(TEST_KEY, &value).await;
        assert_matches!(res, Ok(UpdateState::Updated));

        // Directly verify file on disk is written immediately
        let data = fuchsia_fs::directory::read_file(&storage_dir, &format!("{}.json", TEST_KEY))
            .await
            .expect("reading file");
        let parsed = serde_json::from_slice::<TestStruct>(&data).expect("deserializing");
        assert_eq!(parsed, value);
    }

    #[fuchsia::test]
    async fn test_get_or_else() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) = AtomicJsonStorage::new(vec![TEST_KEY], storage_dir)
            .await
            .expect("should be able to initialize storage");

        for task in sync_tasks {
            task.detach();
        }

        // Key doesn't exist yet -> evaluates closure
        let fallback_value = TestStruct { value: 999 };
        let result = storage.get_or_else(TEST_KEY, || fallback_value.clone()).await;
        assert_eq!(result, fallback_value);

        // Now write a value
        let stored_value = TestStruct { value: VALUE1 };
        storage.write(TEST_KEY, &stored_value).await.unwrap();

        // Key exists -> returns stored value, closure not used
        let result = storage.get_or_else(TEST_KEY, || fallback_value).await;
        assert_eq!(result, stored_value);
    }

    #[fuchsia::test]
    async fn test_delete() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) =
            AtomicJsonStorage::new(vec![TEST_KEY], Clone::clone(&storage_dir))
                .await
                .expect("should be able to initialize storage");

        for task in sync_tasks {
            task.detach();
        }

        let stored_value = TestStruct { value: VALUE1 };
        storage.write_immediate(TEST_KEY, &stored_value).await.unwrap();
        assert_eq!(storage.get::<TestStruct>(TEST_KEY).await.unwrap(), Some(stored_value));

        // Delete the key
        let del_res = storage.delete(TEST_KEY).await;
        assert_matches!(del_res, Ok(UpdateState::Updated));

        // In-memory cache and get() returns None
        assert_eq!(storage.get::<TestStruct>(TEST_KEY).await.unwrap(), None);

        // Disk file should not exist
        let open_res = fuchsia_fs::directory::open_file(
            &storage_dir,
            &format!("{}.json", TEST_KEY),
            fuchsia_fs::PERM_READABLE,
        )
        .await;
        assert_matches!(open_res, Err(OpenError::OpenError(Status::NOT_FOUND)));

        // Deleting already deleted key returns Unchanged
        let del_res2 = storage.delete(TEST_KEY).await;
        assert_matches!(del_res2, Ok(UpdateState::Unchanged));
    }

    impl StorageKey for TestStruct {
        const KEY: &'static str = TEST_KEY;
    }

    #[fuchsia::test]
    async fn test_storage_key_trait() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) =
            AtomicJsonStorage::new(vec![TestStruct::KEY], Clone::clone(&storage_dir))
                .await
                .expect("should be able to initialize storage");

        for task in sync_tasks {
            task.detach();
        }

        // get_storable on empty store
        assert_eq!(storage.get_storable::<TestStruct>().await.unwrap(), None);
        assert_eq!(storage.get_storable_or_default::<TestStruct>().await, TestStruct::default());
        assert_eq!(
            storage.get_storable_or_else::<TestStruct, _>(|| TestStruct { value: 777 }).await,
            TestStruct { value: 777 }
        );

        // write_storable_immediate
        let val = TestStruct { value: VALUE2 };
        let res = storage.write_storable_immediate(&val).await;
        assert_matches!(res, Ok(UpdateState::Updated));

        assert_eq!(storage.get_storable::<TestStruct>().await.unwrap(), Some(val.clone()));
        assert_eq!(storage.get_storable_or_default::<TestStruct>().await, val.clone());

        // delete_storable
        let del_res = storage.delete_storable::<TestStruct>().await;
        assert_matches!(del_res, Ok(UpdateState::Updated));
        assert_eq!(storage.get_storable::<TestStruct>().await.unwrap(), None);
    }

    #[fuchsia::test]
    async fn test_crash_recovery_temp_file_overwrite() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");

        // Pre-create the _tmp.json file as if a crash happened before rename
        let tmp_path = tempdir.path().join(format!("{}_tmp.json", TEST_KEY));
        std::fs::write(tmp_path, b"corrupt partial data").expect("writing stale tmp file");

        let storage_dir = open_tempdir(&tempdir);
        let (storage, sync_tasks) =
            AtomicJsonStorage::new(vec![TEST_KEY], Clone::clone(&storage_dir))
                .await
                .expect("should be able to initialize storage");

        for task in sync_tasks {
            task.detach();
        }

        // write_immediate should successfully overwrite the stale tmp file and finish rename
        let value = TestStruct { value: VALUE1 };
        let res = storage.write_immediate(TEST_KEY, &value).await;
        assert_matches!(res, Ok(UpdateState::Updated));

        let loaded = storage.get::<TestStruct>(TEST_KEY).await.unwrap();
        assert_eq!(loaded, Some(value));
    }
}
