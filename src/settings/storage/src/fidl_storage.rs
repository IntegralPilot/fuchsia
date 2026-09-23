// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::UpdateState;
use crate::private::Sealed;
use crate::storage_factory::{DefaultLoader, NoneT};
use anyhow::{Context, Error, bail, format_err};
use fidl::{Persistable, Status, persist, unpersist};
use fidl_fuchsia_io::DirectoryProxy;
use fuchsia_async::{MonotonicInstant, Task, Timer};
use fuchsia_fs::Flags;
use fuchsia_fs::file::ReadError;
use fuchsia_fs::node::OpenError;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::future::OptionFuture;
use futures::lock::{Mutex, MutexGuard};
use futures::{FutureExt, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use std::pin::pin;
use std::rc::Rc;
use zx::MonotonicDuration;

/// Minimum amount of time between flushing to disk, in milliseconds. The flush call triggers
/// file I/O which is slow.
const MIN_FLUSH_INTERVAL_MS: i64 = 500;
const MAX_FLUSH_INTERVAL_MS: i64 = 1_800_000; // 30 minutes
const MIN_FLUSH_DURATION: MonotonicDuration = MonotonicDuration::from_millis(MIN_FLUSH_INTERVAL_MS);

pub trait FidlStorageConvertible {
    type Storable;
    type Loader;

    const KEY: &'static str;

    fn to_storable(self) -> Self::Storable;
    fn from_storable(storable: Self::Storable) -> Self;
}

/// Stores device level settings in persistent storage.
/// User level settings should not use this.
pub struct FidlStorage {
    /// Map of [`FidlStorageConvertible`] keys to their typed storage.
    typed_storage_map: HashMap<&'static str, TypedStorage>,

    typed_loader_map: HashMap<&'static str, Box<dyn Any>>,

    /// If true, reads will be returned from the data in memory rather than reading from storage.
    caching_enabled: bool,

    /// If true, writes to the underlying storage will only occur at most every
    /// [MIN_WRITE_INTERVAL_MS].
    debounce_writes: bool,

    storage_dir: DirectoryProxy,
}

/// A wrapper for managing all communication and caching for one particular type of data being
/// stored. The actual types are erased.
struct TypedStorage {
    /// Sender to communicate with task loop that handles flushes.
    flush_sender: UnboundedSender<()>,

    /// Cached storage managed through interior mutability.
    cached_storage: Rc<Mutex<CachedStorage>>,
}

/// `CachedStorage` abstracts over a cached value that's read from and written
/// to some backing store.
struct CachedStorage {
    /// Cache for the most recently read or written value. The value is stored as the encoded bytes
    /// of the persistent fidl.
    current_data: Option<Vec<u8>>,

    /// File path that will be used to write a temporary file when syncing to disk. After syncing,
    /// this file is deleted.
    ///
    /// The approach used for syncing is:
    /// * Write data to temp file
    /// * Rename temp file to permanent file
    /// * Delete temp file.
    ///
    /// This ensures that even if there's a power cut, the data in the permanent file is never
    /// partially written.
    temp_file_path: String,

    /// File path to used for permanent file storage on disk.
    file_path: String,
}

impl CachedStorage {
    /// Triggers a sync on the file proxy.
    async fn sync(&mut self, storage_dir: &DirectoryProxy) -> Result<(), Error> {
        // Scope is important. file_proxy needs to be out-of-scope when the directory is renamed.
        {
            let file_proxy = fuchsia_fs::directory::open_file(
                storage_dir,
                &self.temp_file_path,
                Flags::FLAG_MUST_CREATE
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
            // This is only returned when the directory is backed by a VFS, so this is fine to
            // ignore.
            .or_else(|e| if let zx::Status::NOT_SUPPORTED = e { Ok(()) } else { Err(e) })
            .context("failed to sync rename to directory")
    }
}

impl FidlStorage {
    /// Construct a fidl storage from:
    /// * The iterable item, which will produce the keys for storage
    /// * A generator function that will produce a file proxy for each key. It will return the temp
    ///   file path and final file path for storing the data for this key.
    ///
    /// On success, returns the FidlStorage as well as the list of background synchronizing tasks.
    /// The background tasks can be awaited or detached.
    pub async fn with_file_proxy<I, G>(
        iter: I,
        storage_dir: DirectoryProxy,
        files_generator: G,
    ) -> Result<(Self, Vec<Task<()>>), Error>
    where
        I: IntoIterator<Item = (&'static str, Option<Box<dyn Any>>)>,
        G: Fn(&'static str) -> Result<(String, String), Error>,
    {
        let mut typed_storage_map = HashMap::new();
        let iter = iter.into_iter();
        typed_storage_map.reserve(iter.size_hint().0);
        let mut typed_loader_map = HashMap::new();
        let mut sync_tasks = Vec::with_capacity(iter.size_hint().0);
        for (key, loader) in iter {
            // Generate a separate file proxy for each key.
            let (flush_sender, flush_receiver) = futures::channel::mpsc::unbounded::<()>();
            let (temp_file_path, file_path) =
                files_generator(key).context("failed to generate file")?;

            let cached_storage = Rc::new(Mutex::new(CachedStorage {
                current_data: None,
                temp_file_path,
                file_path,
            }));
            let storage = TypedStorage { flush_sender, cached_storage: Rc::clone(&cached_storage) };

            // Each key has an independent flush queue.
            let sync_task = Task::local(Self::synchronize_task(
                Clone::clone(&storage_dir),
                cached_storage,
                flush_receiver,
            ));
            sync_tasks.push(sync_task);
            let _ = typed_storage_map.insert(key, storage);
            if let Some(loader) = loader {
                let _ = typed_loader_map.insert(key, loader);
            }
        }
        Ok((
            FidlStorage {
                caching_enabled: true,
                debounce_writes: true,
                typed_storage_map,
                typed_loader_map,
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

        // The time of the last flush. Initialized to MIN_FLUSH_INTERVAL_MS before the
        // current time so that the first flush always goes through, no matter the
        // timing.
        let mut last_flush: MonotonicInstant = MonotonicInstant::now() - MIN_FLUSH_DURATION;

        // Timer for flush cooldown. OptionFuture allows us to wait on the future even
        // if it's None.
        let mut next_flush_timer = pin!(OptionFuture::<Timer>::from(None).fuse());
        let mut retries = 0;
        let mut retrying = false;

        let flush_fuse = flush_receiver.fuse();

        futures::pin_mut!(flush_fuse);
        loop {
            futures::select! {
                _ = flush_fuse.select_next_some() => {
                    // Flush currently unable to complete. Don't prevent exponential
                    // backoff from occurring.
                    if retrying {
                        continue;
                    }

                    // Received a request to do a flush.
                    let now = MonotonicInstant::now();
                    let next_flush_time = if now - last_flush > MIN_FLUSH_DURATION {
                        // Last flush happened more than MIN_FLUSH_INTERVAL_MS ago,
                        // flush immediately in next iteration of loop.
                        now
                    } else {
                        // Last flush was less than MIN_FLUSH_INTERVAL_MS ago, schedule
                        // it accordingly. It's okay if the time is in the past, Timer
                        // will still trigger on the next loop iteration.
                        last_flush + MIN_FLUSH_DURATION
                    };

                    has_pending_flush = true;
                    next_flush_timer.set(OptionFuture::from(Some(Timer::new(next_flush_time))).fuse());
                }

                _ = next_flush_timer => {
                    // Timer triggered, check for pending syncs.
                    if has_pending_flush {
                        let mut cached_storage = cached_storage.lock().await;

                        // If the sync fails, exponentionally backoff the syncs until a
                        // maximum wait time.
                        if let Err(e) = cached_storage.sync(&storage_dir).await {
                            retrying = true;
                            let flush_duration = MonotonicDuration::from_millis(
                                2_i64.saturating_pow(retries)
                                    .saturating_mul(MIN_FLUSH_INTERVAL_MS)
                                    .min(MAX_FLUSH_INTERVAL_MS)
                            );
                            let next_flush_time = MonotonicInstant::now() + flush_duration;
                            log::error!(
                                "Failed to sync write to disk for {:?}, delaying by {:?}, \
                                    caused by: {:?}",
                                cached_storage.file_path,
                                flush_duration,
                                e
                            );

                            // Reset the timer so we can try again in the future
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

    pub fn set_caching_enabled(&mut self, enabled: bool) {
        self.caching_enabled = enabled;
    }

    pub fn set_debounce_writes(&mut self, debounce: bool) {
        self.debounce_writes = debounce;
    }

    async fn inner_write(
        &self,
        key: &'static str,
        new_value: Vec<u8>,
    ) -> Result<UpdateState, Error> {
        let typed_storage = self
            .typed_storage_map
            .get(key)
            .ok_or_else(|| format_err!("Invalid data keyed by {}", key))?;
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
                        Err(e) => {
                            bail!("failed to get value from fidl storage for {:?}: {:?}", key, e)
                        }
                    },
                    Err(OpenError::OpenError(Status::NOT_FOUND)) => None,
                    Err(e) => bail!("unable to read data on disk for {:?}: {:?}", key, e),
                };
                bytes.as_ref()
            }
        };

        Ok(if cached_value.map(|c| *c != new_value).unwrap_or(true) {
            cached_storage.current_data = Some(new_value);
            if !self.debounce_writes {
                // Not debouncing writes for testing, just sync immediately.
                cached_storage
                    .sync(&self.storage_dir)
                    .await
                    .with_context(|| format!("Failed to sync data for key {key:?}"))?;
            } else {
                typed_storage.flush_sender.unbounded_send(()).with_context(|| {
                    format!("flush_sender failed to send flush message, associated key is {key}")
                })?;
            }
            UpdateState::Updated
        } else {
            UpdateState::Unchanged
        })
    }

    /// Write `new_value` to storage. The write will be persisted to disk at a set interval.
    pub async fn write<T>(&self, new_value: T) -> Result<UpdateState, Error>
    where
        T: FidlStorageConvertible,
        <T as FidlStorageConvertible>::Storable: Persistable,
    {
        let new_value = persist(&new_value.to_storable())?;
        self.inner_write(T::KEY, new_value).await
    }

    /// Test-only method to write directly to disk without touching the cache. This is used for
    /// setting up data as if it existed on disk before the storage was constructed.
    pub async fn write_test_bytes(&self, key: &'static str, value: Vec<u8>) -> Result<(), Error> {
        self.inner_write(key, value).await.map(|_| ())
    }

    async fn get_inner(&self, key: &'static str) -> MutexGuard<'_, CachedStorage> {
        let typed_storage = self
            .typed_storage_map
            .get(key)
            // TODO(https://fxbug.dev/42064613) Replace this with an error result.
            .unwrap_or_else(|| panic!("Invalid data keyed by {key}"));
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
                // TODO(https://fxbug.dev/42064613) Replace this with an error result.
                Err(e) => panic!("failed to open file for {key:?}: {e:?}"),
            }
        {
            let data = match fuchsia_fs::file::read(&file_proxy).await {
                Ok(data) => Some(data),
                Err(ReadError::ReadError(Status::NOT_FOUND)) => None,
                // TODO(https://fxbug.dev/42064613) Replace this with an error result.
                Err(e) => panic!("failed to get fidl data from disk for {key:?}: {e:?}"),
            };

            cached_storage.current_data = data;
        }

        cached_storage
    }

    /// Gets the latest value cached locally, or loads the value from storage.
    /// Doesn't support multiple concurrent callers of the same struct.
    pub async fn get<T>(&self) -> T
    where
        T: FidlStorageConvertible,
        T::Storable: Persistable,
        T::Loader: DefaultDispatcher<T>,
    {
        match self.get_inner(T::KEY).await.current_data.as_ref().map(|data| {
            T::from_storable(
                unpersist(data).expect("Should not be able to save mismatching types in file"),
            )
        }) {
            Some(data) => data,
            None => <T::Loader as DefaultDispatcher<T>>::get_default(self),
        }
    }
}

pub trait DefaultDispatcher<T>: Sealed {
    fn get_default(_: &FidlStorage) -> T;
}

impl<T> DefaultDispatcher<T> for NoneT
where
    T: Default,
{
    fn get_default(_: &FidlStorage) -> T {
        T::default()
    }
}

impl<T, L> DefaultDispatcher<T> for L
where
    T: FidlStorageConvertible<Loader = L>,
    L: DefaultLoader<Result = T> + 'static,
{
    fn get_default(storage: &FidlStorage) -> T {
        match storage.typed_loader_map.get(T::KEY) {
            Some(loader) => match loader.downcast_ref::<T::Loader>() {
                Some(loader) => loader.default_value(),
                None => {
                    panic!("Mismatch key and loader for key {}", T::KEY);
                }
            },
            None => panic!("Missing loader for {}", T::KEY),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fasync::TestExecutor;
    use fidl::epitaph::ChannelEpitaphExt;
    use fidl_fuchsia_io as fio;
    use fidl_test_storage::{TestStruct, WrongStruct};
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use std::sync::Arc;
    use std::task::Poll;
    use test_case::test_case;

    const VALUE0: i32 = 3;
    const VALUE1: i32 = 33;
    const VALUE2: i32 = 128;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct LibTestStruct {
        value: i32,
    }

    impl FidlStorageConvertible for LibTestStruct {
        type Storable = TestStruct;
        type Loader = NoneT;
        const KEY: &'static str = "testkey";

        fn to_storable(self) -> Self::Storable {
            TestStruct { value: self.value }
        }

        fn from_storable(storable: Self::Storable) -> Self {
            Self { value: storable.value }
        }
    }

    impl Default for LibTestStruct {
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
        let value_to_get = LibTestStruct { value: VALUE1 };
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let content = persist(&value_to_get.to_storable()).unwrap();
        std::fs::write(tempdir.path().join("xyz.pfidl"), content).expect("failed to write file");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) = FidlStorage::with_file_proxy(
            vec![(LibTestStruct::KEY, None)],
            storage_dir,
            move |_| Ok((String::from("xyz_temp.pfidl"), String::from("xyz.pfidl"))),
        )
        .await
        .expect("should be able to generate file");
        for task in sync_tasks {
            task.detach();
        }
        let result = storage.get::<LibTestStruct>().await;

        assert_eq!(result.value, VALUE1);
    }

    #[fuchsia::test]
    async fn test_get_default() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) = FidlStorage::with_file_proxy(
            vec![(LibTestStruct::KEY, None)],
            storage_dir,
            move |_| Ok((String::from("xyz_temp.pfidl"), String::from("xyz.pfidl"))),
        )
        .await
        .expect("file proxy should be created");
        for task in sync_tasks {
            task.detach();
        }
        let result = storage.get::<LibTestStruct>().await;

        assert_eq!(result.value, VALUE0);
    }

    /// Proxies directory request to a real directory while allowing for some of the requests to be
    /// intercepted.
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
        // TODO(b/356474618): re-enable and fix existing occurance
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

        /// Returns a receiver that will be notified after each Sync request to the real directory
        /// has completed.
        fn install_sync_notifier(&self) -> futures::channel::mpsc::UnboundedReceiver<()> {
            let (sender, receiver) = futures::channel::mpsc::unbounded();
            self.inner.lock().unwrap().sync_notifier = Some(sender);
            receiver
        }

        /// Sets a callback to be called on every Open request. If the callback returns an error,
        /// then the request will be failed with that error instead of being forwarded to the real
        /// directory.
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
                        let create = flags.intersects(fio::Flags::FLAG_MUST_CREATE);
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

    /// Repeatedly polls `fut` until it returns `Poll::Ready`. When using a `TestExecutor` with fake
    /// time, only `run_until_stalled` can be used but `run_until_stalled` is incompatible with
    /// external filesystems. This function bridges the gap by continuously polling the future until
    /// the filesystem responds.
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

    /// Asserts that a file doesn't exist.
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

    /// Verifies the contents of a file.
    fn assert_file_contents(
        executor: &mut TestExecutor,
        directory: &fio::DirectoryProxy,
        file_name: &str,
        expected_contents: TestStruct,
    ) {
        let read_fut = fuchsia_fs::directory::read_file(directory, file_name);
        let data = run_until_ready(executor, read_fut).expect("reading file");
        let data = fidl::unpersist::<TestStruct>(&data).expect("failed to read file as TestStruct");
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

        let storage_fut = FidlStorage::with_file_proxy(
            vec![(LibTestStruct::KEY, None)],
            Clone::clone(&storage_dir),
            move |_| Ok((String::from("xyz_temp.pfidl"), String::from("xyz.pfidl"))),
        );
        futures::pin_mut!(storage_fut);

        let (storage, _sync_tasks) =
            if let Poll::Ready(storage) = executor.run_until_stalled(&mut storage_fut) {
                storage.expect("file proxy should be created")
            } else {
                panic!("storage creation stalled");
            };

        // Write to device storage.
        let value_to_write = LibTestStruct { value: written_value };
        let write_future = storage.write(value_to_write);
        futures::pin_mut!(write_future);

        // Initial cache check is done if no read was ever performed.
        assert_matches!(
            run_until_ready(&mut executor, &mut write_future),
            Result::Ok(UpdateState::Updated)
        );

        // Storage is not yet ready.
        assert_file_not_found(&mut executor, &storage_dir, "xyz.pfidl");

        // Wait for the sync task to complete.
        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");

        // Validate the value matches what was set.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write.to_storable(),
        );
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

        let storage_fut = FidlStorage::with_file_proxy(
            vec![(LibTestStruct::KEY, None)],
            Clone::clone(&storage_dir),
            move |_| Ok((String::from("xyz_temp.pfidl"), String::from("xyz.pfidl"))),
        );
        futures::pin_mut!(storage_fut);

        let (storage, _sync_tasks) =
            if let Poll::Ready(storage) = executor.run_until_stalled(&mut storage_fut) {
                storage.expect("file proxy should be created")
            } else {
                panic!("storage creation stalled");
            };

        // Write to device storage.
        let value_to_write = LibTestStruct { value: written_value };
        let write_future = storage.write(value_to_write);
        futures::pin_mut!(write_future);

        // Initial cache check is done if no read was ever performed.
        assert_matches!(
            run_until_ready(&mut executor, &mut write_future),
            Result::Ok(UpdateState::Updated)
        );

        // Storage is not yet ready.
        assert_file_not_found(&mut executor, &storage_dir, "xyz.pfidl");

        // Wait for the sync task to complete.
        run_until_ready(&mut executor, &mut sync_receiver.next()).expect("directory never synced");

        // Validate that the file has been synced.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write.to_storable(),
        );

        // Write second time to device storage.
        let value_to_write2 = LibTestStruct { value: second_value };
        let write_future = storage.write(value_to_write2);
        futures::pin_mut!(write_future);

        // Initial cache check is done if no read was ever performed.
        assert_matches!(
            run_until_ready(&mut executor, &mut write_future),
            Result::Ok(UpdateState::Updated)
        );

        // Storage is not yet ready, should still equal old value.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write.to_storable(),
        );

        // Move executor to just before sync interval.
        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000 - 1));
        assert!(!executor.wake_expired_timers());

        // Move executor to just after sync interval. It should run now.
        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000));
        run_until_ready(&mut executor, &mut sync_receiver.next()).expect("directory never synced");

        // Validate that the file has been synced.

        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write2.to_storable(),
        );
    }

    #[derive(Copy, Clone, Default, Debug)]
    struct LibWrongStruct;

    impl FidlStorageConvertible for LibWrongStruct {
        type Storable = WrongStruct;
        type Loader = NoneT;
        const KEY: &'static str = "WRONG_STRUCT";

        fn to_storable(self) -> Self::Storable {
            WrongStruct
        }

        fn from_storable(_: Self::Storable) -> Self {
            LibWrongStruct
        }
    }

    // Test that attempting to write two kinds of structs to a storage instance that only supports
    // one results in a failure.
    #[fuchsia::test]
    async fn test_write_with_mismatch_type_returns_error() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);

        let (storage, sync_tasks) = FidlStorage::with_file_proxy(
            vec![(LibTestStruct::KEY, None)],
            storage_dir,
            move |_| Ok((String::from("xyz_temp.pfidl"), String::from("xyz.pfidl"))),
        )
        .await
        .expect("file proxy should be created");
        for task in sync_tasks {
            task.detach();
        }

        // Write successfully to storage once.
        let result = storage.write(LibTestStruct { value: VALUE2 }).await;
        assert!(result.is_ok());

        // Write to device storage again with a different type to validate that the type can't
        // be changed.
        let result = storage.write(LibWrongStruct).await;
        assert_matches!(result, Err(e) if e.to_string() == "Invalid data keyed by WRONG_STRUCT");
    }

    // Test that multiple writes to FidlStorage will cause a write each time, but will only
    // sync to the fs at an interval.
    #[fuchsia::test]
    fn test_multiple_write_debounce() {
        // Custom executor for this test so that we can advance the clock arbitrarily and verify the
        // state of the executor at any given point.
        let mut executor = TestExecutor::new_with_fake_time();
        executor.set_fake_time(MonotonicInstant::from_nanos(0));

        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let storage_dir = open_tempdir(&tempdir);
        let (interceptor, storage_dir) = DirectoryInterceptor::new(storage_dir);
        let mut sync_receiver = interceptor.install_sync_notifier();

        let storage_fut = FidlStorage::with_file_proxy(
            vec![(LibTestStruct::KEY, None)],
            Clone::clone(&storage_dir),
            move |_| Ok((String::from("xyz_temp.pfidl"), String::from("xyz.pfidl"))),
        );
        let (storage, _sync_tasks) =
            run_until_ready(&mut executor, storage_fut).expect("file proxy should be created");

        let first_value = VALUE1;
        let second_value = VALUE2;
        let third_value = VALUE0;

        // First write finishes immediately.
        let value_to_write = LibTestStruct { value: first_value };
        // Initial cache check is done if no read was ever performed.
        let result = run_until_ready(&mut executor, storage.write(value_to_write));
        assert_matches!(result, Result::Ok(UpdateState::Updated));

        // Storage is not yet ready.
        assert_file_not_found(&mut executor, &storage_dir, "xyz.pfidl");

        // Wake the initial time without advancing the clock. Confirms that the first write is
        // "immediate".
        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");

        // Validate that the file has been synced.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write.to_storable(),
        );

        // Write second time to device storage.
        let value_to_write2 = LibTestStruct { value: second_value };
        let result = run_until_ready(&mut executor, storage.write(value_to_write2));
        // Value is marked as updated after the write.
        assert_matches!(result, Result::Ok(UpdateState::Updated));

        // Validate the updated values are still returned from the storage cache.
        let data = run_until_ready(&mut executor, storage.get::<LibTestStruct>());
        assert_eq!(data, value_to_write2);

        // But the data has not been persisted to disk.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write.to_storable(),
        );

        // Now write a third time before advancing the clock.
        let value_to_write3 = LibTestStruct { value: third_value };
        let result = run_until_ready(&mut executor, storage.write(value_to_write3));
        // Value is marked as updated after the write.
        assert_matches!(result, Result::Ok(UpdateState::Updated));

        // Validate the updated values are still returned from the storage cache.

        let data = run_until_ready(&mut executor, storage.get::<LibTestStruct>());
        assert_eq!(data, value_to_write3);

        // But the data has still not been persisted to disk.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write.to_storable(),
        );

        // Move clock to just before sync interval.
        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000 - 1));
        assert!(!executor.wake_expired_timers());

        // And validate that the data has still not been synced to disk.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write.to_storable(),
        );

        // Move executor to just after sync interval.
        executor.set_fake_time(MonotonicInstant::from_nanos(MIN_FLUSH_INTERVAL_MS * 1_000_000));
        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");

        // Validate that the file has finally been synced.
        assert_file_contents(
            &mut executor,
            &storage_dir,
            "xyz.pfidl",
            value_to_write3.to_storable(),
        );
    }

    // Tests that syncing can recover after a failed write. The test cases list the number of failed
    // attempts and the maximum amount of time waited from the previous write.
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
            if path == "abc_tmp.pfidl" && create && *attempts_guard < retry_count {
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
            temp_file_path: "abc_tmp.pfidl".to_owned(),
            file_path: "abc.pfidl".to_owned(),
        }));

        let (sender, receiver) = futures::channel::mpsc::unbounded();

        // Call spawn in a future since we have to be in an executor context to call spawn.
        let task = fasync::Task::local(FidlStorage::synchronize_task(
            Clone::clone(&storage_dir),
            Rc::clone(&cached_storage),
            receiver,
        ));
        futures::pin_mut!(task);

        executor.set_fake_time(MonotonicInstant::from_nanos(0));
        sender.unbounded_send(()).expect("can send flush signal");
        assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);

        let mut clock_nanos = 0;
        // (2^i) * 500 = exponential backoff.
        // 1,000,000 = convert ms to ns.
        for new_duration in (0..retry_count).map(|i| {
            (2_i64.pow(i as u32) * MIN_FLUSH_INTERVAL_MS).min(max_wait_time as i64) * 1_000_000
                - (i == retry_count - 1) as i64
        }) {
            executor.set_fake_time(MonotonicInstant::from_nanos(clock_nanos));
            // Task should not complete while retrying.
            assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);

            // Check that files don't exist.
            assert_file_not_found(&mut executor, &storage_dir, "abc_tmp.pfidl");
            assert_file_not_found(&mut executor, &storage_dir, "abc.pfidl");

            clock_nanos += new_duration;
        }

        executor.set_fake_time(MonotonicInstant::from_nanos(clock_nanos));
        // At this point the clock should be 1ns before the timer, so it shouldn't wake.
        assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);

        // Check that files don't exist.
        assert_file_not_found(&mut executor, &storage_dir, "abc_tmp.pfidl");
        assert_file_not_found(&mut executor, &storage_dir, "abc.pfidl");

        // Now pass the timer where we can read the result.
        clock_nanos += 1;
        executor.set_fake_time(MonotonicInstant::from_nanos(clock_nanos));
        assert_eq!(executor.run_until_stalled(&mut task), Poll::Pending);
        run_until_ready(&mut executor, sync_receiver.next()).expect("directory never synced");

        // Check that the file now matches what was in the cache.
        let read_fut = fuchsia_fs::directory::read_file(&storage_dir, "abc.pfidl");
        let data = run_until_ready(&mut executor, read_fut).expect("reading file");
        assert_eq!(data, expected_data);

        drop(sender);
        // Ensure the task can properly exit.
        run_until_ready(&mut executor, task);
    }
}
