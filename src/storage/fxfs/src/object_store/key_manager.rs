// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::log::*;
use crate::object_store::object_record::EncryptionKeys;
use crate::object_store::{FSCRYPT_KEY_ID, VOLUME_DATA_KEY_ID};
use anyhow::Error;
use event_listener::Event;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use fxfs_crypto::{Cipher, CipherHolder, CipherSet, Crypt, FindKeyResult};
use fxfs_trace::{TraceFutureExt, trace_future_args};
use scopeguard::ScopeGuard;
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use zx_status as zx;

/// This timeout controls when entries are moved from `hash` to `pending_purge` and then dumped.
/// Entries will remain in the cache until they remain inactive from between PURGE_TIMEOUT and 2 *
/// PURGE_TIMEOUT.  This is deliberately set to 37 seconds (rather than a round number) to reduce
/// the chance that this timer ends up always firing at the same time as other timers.
const PURGE_TIMEOUT: Duration = Duration::from_secs(37);

/// A simple cache that purges entries periodically when `purge` is called.  The API is similar to
/// HashMap's.
struct Cache<V> {
    hash: BTreeMap<u64, V>,
    pending_purge: BTreeMap<u64, V>,
    permanent: BTreeMap<u64, V>,
}

impl<V> Cache<V> {
    fn new() -> Self {
        Self { hash: BTreeMap::new(), pending_purge: BTreeMap::new(), permanent: BTreeMap::new() }
    }

    fn get(&mut self, key: u64) -> Option<&V> {
        match self.hash.entry(key) {
            Entry::Occupied(o) => Some(o.into_mut()),
            Entry::Vacant(v) => {
                // If we find an entry in `pending_purge`, move it into `hash`.
                if let Some(value) = self.pending_purge.remove(&key) {
                    Some(v.insert(value))
                } else {
                    self.permanent.get(&key)
                }
            }
        }
    }

    fn insert(&mut self, key: u64, value: V, permanent: bool) {
        if permanent {
            self.permanent.insert(key, value);
        } else {
            self.hash.insert(key, value);
        }
    }

    fn merge(&mut self, key: u64, merge: impl FnOnce(Option<&V>) -> V) {
        match self.hash.entry(key) {
            Entry::Occupied(mut o) => {
                let new = merge(Some(o.get()));
                *o.get_mut() = new;
            }
            Entry::Vacant(v) => {
                // If we find an entry in `pending_purge`, move it into `hash`.
                if let Some(value) = self.pending_purge.remove(&key) {
                    v.insert(merge(Some(&value)));
                } else {
                    v.insert(merge(None));
                }
            }
        }
    }

    /// This purges entries that haven't been accessed since the last call to purge.
    ///
    /// Returns true if the cache no longer has any purgeable entries.
    fn purge(&mut self) -> bool {
        self.pending_purge = std::mem::take(&mut self.hash);
        self.pending_purge.is_empty()
    }

    fn clear(&mut self) {
        self.hash.clear();
        self.pending_purge.clear();
        self.permanent.clear();
    }

    fn clear_cached(&mut self) {
        self.hash.clear();
        self.pending_purge.clear();
    }

    fn remove(&mut self, key: u64) {
        self.hash.remove(&key);
        self.pending_purge.remove(&key);
        self.permanent.remove(&key);
    }
}

pub struct KeyManager {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    keys: Cache<Arc<CipherSet>>,
    unwrapping: BTreeMap<u64, Arc<UnwrapResult>>,
    purge_task: Option<fasync::Task<()>>,
}

impl Inner {
    fn start_purge_task(&mut self, inner: &Arc<Mutex<Inner>>) {
        self.purge_task.get_or_insert_with(move || {
            let inner = inner.clone();
            fasync::Task::spawn(
                async move {
                    loop {
                        fasync::Timer::new(PURGE_TIMEOUT).await;
                        let mut inner = inner.lock();
                        if inner.keys.purge() {
                            inner.purge_task = None;
                            break;
                        }
                    }
                }
                .trace(trace_future_args!("KeyManager::purge_task")),
            )
        });
    }
}

struct UnwrapResult {
    event: Event,
    error: UnsafeCell<Result<(), zx::Status>>,
    // Protected by the mutex on Inner.
    cancelled: UnsafeCell<bool>,
}

impl UnwrapResult {
    fn new() -> Arc<Self> {
        Arc::new(UnwrapResult {
            event: Event::new(),
            error: UnsafeCell::new(Ok(())),
            cancelled: UnsafeCell::new(false),
        })
    }

    /// Returns true if cancelled.
    fn set(
        &self,
        inner: &Arc<Mutex<Inner>>,
        object_id: u64,
        permanent: bool,
        result: Result<Option<Arc<CipherSet>>, zx::Status>,
    ) -> bool {
        let mut guard = inner.lock();
        // SAFETY: Safe because we hold the lock on `inner`.
        let cancelled = unsafe { *self.cancelled.get() };
        let set_error = |error| {
            // SAFETY: This is safe because we have exclusive access until we call notify below.
            unsafe {
                *self.error.get() = error;
            }
        };
        if cancelled {
            set_error(Err(zx::Status::CANCELED));
        } else if let Err(error) = &result {
            error!(error:?, oid = object_id; "Failed to unwrap keys");
            set_error(Err(*error));
        }
        if let Entry::Occupied(o) = guard.unwrapping.entry(object_id) {
            if std::ptr::eq(Arc::as_ptr(o.get()), self) {
                o.remove();
                if !cancelled {
                    if let Ok(Some(keys)) = &result {
                        guard.keys.insert(object_id, keys.clone(), permanent);
                        guard.start_purge_task(inner);
                    }
                }
            }
        }
        self.event.notify(usize::MAX);
        cancelled
    }
}

unsafe impl Send for UnwrapResult {}
unsafe impl Sync for UnwrapResult {}

impl KeyManager {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            keys: Cache::new(),
            unwrapping: BTreeMap::new(),
            purge_task: None,
        }));

        Self { inner }
    }

    /// Retrieves the key with id VOLUME_DATA_KEY_ID from the cache but won't initiate unwrapping if
    /// no key is present.  If the key is currently in the process of being unwrapped, this will
    /// wait until that has finished.  This should be used with permanent keys.  This will return
    /// None if the key isn't present in the cache, but can also return None if they key isn't
    /// present in the set of keys.
    pub async fn get(&self, object_id: u64) -> Result<Option<Arc<dyn Cipher>>, Error> {
        loop {
            let (unwrap_result, listener) = {
                let mut inner = self.inner.lock();

                if let Some(keys) = inner.keys.get(object_id) {
                    return match keys.find_key(VOLUME_DATA_KEY_ID) {
                        FindKeyResult::NotFound => Ok(None),
                        FindKeyResult::Unavailable => Err(FxfsError::NoKey.into()),
                        FindKeyResult::Key(key) => Ok(Some(key)),
                    };
                }
                let unwrap_result = match inner.unwrapping.entry(object_id) {
                    Entry::Vacant(_) => return Ok(None),
                    Entry::Occupied(o) => o.get().clone(),
                };
                let listener = unwrap_result.event.listen();
                (unwrap_result, listener)
            };
            listener.await;
            // SAFETY: This is safe because there can be no mutations happening at this point.
            let error = unsafe { *unwrap_result.error.get() };
            error?;
        }
    }

    /// This retrieves keys from the cache or initiates unwrapping if they are not in the cache.  If
    /// `force` is true, then this will always attempt to unwrap the keys again, even if the keys
    /// are present in the cache.  `encryption_keys` is a future to be used to retrieve the wrapped
    /// keys.  It is passed in an `Option` so that callers can tell if the keys were freshly
    /// retrieved.
    pub async fn get_keys(
        &self,
        object_id: u64,
        crypt: &dyn Crypt,
        encryption_keys: &mut Option<impl AsyncFnOnce() -> Result<EncryptionKeys, Error>>,
        permanent: bool,
        force: bool,
    ) -> Result<Arc<CipherSet>, Error> {
        let inner = self.inner.clone();
        let mut unwrap_result;

        loop {
            let listener = {
                let mut inner = inner.lock();

                if !force {
                    if let Some(keys) = inner.keys.get(object_id) {
                        return Ok(keys.clone());
                    }
                }

                match inner.unwrapping.entry(object_id) {
                    Entry::Vacant(v) => {
                        unwrap_result = UnwrapResult::new();
                        v.insert(unwrap_result.clone());
                        break;
                    }
                    Entry::Occupied(o) => {
                        unwrap_result = o.get().clone();
                        let listener = unwrap_result.event.listen();
                        listener
                    }
                }
            };

            listener.await;
            // SAFETY: This is safe because there can be no mutations happening at this
            // point.
            let error = unsafe { *unwrap_result.error.get() };
            error?;
        }

        // Use a guard in case we're dropped.
        let mut result = scopeguard::guard(Ok(None), |result| {
            unwrap_result.set(&inner, object_id, permanent, result);
        });

        let encryption_keys = match encryption_keys.take().unwrap()().await {
            Ok(encryption_keys) => encryption_keys,
            Err(error) => {
                error!(error:?; "Failed to get key specs");
                *result = Err(zx::Status::INTERNAL);
                return Err(zx::Status::INTERNAL.into());
            }
        };

        match crypt.unwrap_keys(&encryption_keys, object_id).await {
            Ok(cipher_set) => {
                let keys = Arc::new(cipher_set);
                let _ = ScopeGuard::into_inner(result);
                if unwrap_result.set(&inner, object_id, permanent, Ok(Some(keys.clone()))) {
                    Err(zx::Status::CANCELED.into())
                } else {
                    Ok(keys)
                }
            }
            Err(error) => {
                *result = Err(error);
                Err(error.into())
            }
        }
    }

    /// Returns a cipher for the key specified by `key_id`.
    /// Returns Fxfs::NotFound if the key_id is unknown.
    /// Returns Fxfs::NoKey if the requested key is not available (i.e. is locked).
    pub async fn get_key(
        &self,
        object_id: u64,
        crypt: &dyn Crypt,
        encryption_keys: impl AsyncFnOnce() -> Result<EncryptionKeys, Error>,
        key_id: u64,
    ) -> Result<Arc<dyn Cipher>, Error> {
        let mut encryption_keys = Some(encryption_keys);
        let mut force = false;
        loop {
            let keys = self
                .get_keys(
                    object_id,
                    crypt,
                    &mut encryption_keys,
                    /* permanent= */ false,
                    force,
                )
                .await?;
            return match keys.find_key(key_id) {
                FindKeyResult::NotFound => Err(FxfsError::NotFound.into()),
                FindKeyResult::Unavailable => {
                    if force || encryption_keys.is_none() {
                        Err(FxfsError::NoKey.into())
                    } else {
                        force = true;
                        continue;
                    }
                }
                FindKeyResult::Key(k) => Ok(k),
            };
        }
    }

    /// For files, the only way we can tell whether it has an fscrypt encryption key is if there's a
    /// key with id FSCRYPT_KEY_ID.  This function will return that key if it is present, but will
    /// otherwise fall back to the the key with id VOLUME_DATA_KEY_ID.  If the fscrypt encryption
    /// key cannot be unwrapped, this will return FxfsError::NoKey.
    pub async fn get_fscrypt_key_if_present(
        &self,
        object_id: u64,
        crypt: &dyn Crypt,
        encryption_keys: impl AsyncFnOnce() -> Result<EncryptionKeys, Error>,
    ) -> Result<(u64, Arc<dyn Cipher>), Error> {
        let mut encryption_keys = Some(encryption_keys);
        let mut force = false;
        loop {
            let keys = self
                .get_keys(
                    object_id,
                    crypt,
                    &mut encryption_keys,
                    /* permanent= */ false,
                    force,
                )
                .await?;
            return match keys.find_key(FSCRYPT_KEY_ID) {
                FindKeyResult::NotFound => {
                    Ok((VOLUME_DATA_KEY_ID, to_result(keys.find_key(VOLUME_DATA_KEY_ID))?))
                }
                FindKeyResult::Unavailable => {
                    if force || encryption_keys.is_none() {
                        Err(FxfsError::NoKey.into())
                    } else {
                        force = true;
                        continue;
                    }
                }
                FindKeyResult::Key(k) => Ok((FSCRYPT_KEY_ID, k)),
            };
        }
    }

    /// This function is for directories which know whether they should be using an fscrypt
    /// encryption key, and can tolerate the key being unavailable.  This returns None if
    /// the key is currently unavailable.
    pub async fn get_fscrypt_key(
        &self,
        object_id: u64,
        crypt: &dyn Crypt,
        encryption_keys: impl AsyncFnOnce() -> Result<EncryptionKeys, Error>,
    ) -> Result<CipherHolder, Error> {
        let mut encryption_keys = Some(encryption_keys);
        let mut force = false;
        loop {
            let keys = self
                .get_keys(
                    object_id,
                    crypt,
                    &mut encryption_keys,
                    /* permanent= */ false,
                    force,
                )
                .await?;
            return match keys.find_key(FSCRYPT_KEY_ID) {
                FindKeyResult::NotFound => Err(FxfsError::NotFound.into()),
                FindKeyResult::Unavailable => {
                    if force || encryption_keys.is_none() {
                        Ok(CipherHolder::Unavailable)
                    } else {
                        force = true;
                        continue;
                    }
                }
                FindKeyResult::Key(k) => Ok(CipherHolder::Cipher(k)),
            };
        }
    }

    /// This inserts the keys into the cache.  Any existing keys will be overwritten.  It's
    /// unspecified what happens if keys for the object are currently being unwrapped.
    pub fn insert(&self, object_id: u64, keys: Arc<CipherSet>, permanent: bool) {
        let mut inner = self.inner.lock();
        inner.keys.insert(object_id, keys, permanent);
        inner.start_purge_task(&self.inner);
    }

    /// This merges into the cache.  `merge` is a callback that receives the existing keys, if any,
    /// as an argument.  It's unspecified what happens if keys for the object are currently being
    /// unwrapped.
    pub fn merge(
        &self,
        object_id: u64,
        merge: impl FnOnce(Option<&Arc<CipherSet>>) -> Arc<CipherSet>,
    ) {
        let mut inner = self.inner.lock();
        inner.keys.merge(object_id, merge);
        inner.start_purge_task(&self.inner);
    }

    /// Removes keys.  Returns a future that can (optionally) be awaited after which any
    /// task that might be running to fetch keys will have finished.
    pub fn remove(&self, object_id: u64) -> impl Future<Output = ()> {
        {
            let mut inner = self.inner.lock();
            inner.keys.remove(object_id);
            if let Some(u) = inner.unwrapping.get(&object_id) {
                // SAFETY: Safe because of lock on `inner`.
                unsafe { *u.cancelled.get() = true };
            }
        }
        let inner = self.inner.clone();
        async move {
            let listener = {
                if let Some(u) = inner.lock().unwrapping.get(&object_id) {
                    u.event.listen()
                } else {
                    return;
                }
            };
            listener.await;
        }
    }

    /// This clears the caches of all keys.
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.keys.clear();
        inner.unwrapping.clear();
    }

    /// Clears only non-permanent cached keys, leaving permanent store keys intact.
    pub fn clear_cached_keys(&self) {
        let mut inner = self.inner.lock();
        inner.keys.clear_cached();
        inner.purge_task.take();
    }
}

fn to_result(find_key_result: FindKeyResult) -> Result<Arc<dyn Cipher>, FxfsError> {
    match find_key_result {
        FindKeyResult::NotFound => Err(FxfsError::NotFound),
        FindKeyResult::Unavailable => Err(FxfsError::NoKey),
        FindKeyResult::Key(k) => Ok(k),
    }
}

#[cfg(target_os = "fuchsia")]
#[cfg(test)]
mod tests {
    use super::{EncryptionKeys, KeyManager, PURGE_TIMEOUT, to_result};
    use crate::log::*;
    use async_trait::async_trait;
    use fuchsia_async::{self as fasync, MonotonicInstant, TestExecutor};

    use crate::object_store::object_record::EncryptionKey;
    use futures::channel::oneshot;
    use futures::join;
    use fxfs_crypto::{
        Cipher, CipherHolder, Crypt, FXFS_KEY_SIZE, FXFS_WRAPPED_KEY_SIZE, FxfsCipher, FxfsKey,
        KeyPurpose, MutPtrByteSlice, ObjectType, UnwrappedKey, WrappedKey, WrappedKeyBytes,
        WrappingKeyId,
    };
    use std::future::pending;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

    const PLAIN_TEXT: &[u8] = b"The quick brown fox jumps over the lazy dog";
    const ERROR_COUNTER: u8 = 0xff;

    fn unwrapped_key(counter: u8) -> UnwrappedKey {
        UnwrappedKey::new([counter; FXFS_KEY_SIZE].to_vec())
    }
    fn cipher(counter: u8) -> Arc<dyn Cipher> {
        Arc::new(FxfsCipher::new(&unwrapped_key(counter)))
    }

    fn padded_plain_text() -> Vec<u8> {
        let mut text = PLAIN_TEXT.to_vec();
        text.resize(4096, 0); // Encryption requires block aligned inputs
        text
    }

    fn cipher_text(counter: u8) -> Vec<u8> {
        let mut text = padded_plain_text();
        cipher(counter)
            .encrypt(0, 0, 0, 0, MutPtrByteSlice::from(&mut text[..]))
            .expect("encrypt failed");
        // Ensure that encrypt changed the text.
        assert_ne!(&text, &padded_plain_text());
        text
    }

    fn encryption_keys() -> EncryptionKeys {
        vec![(
            0,
            EncryptionKey::Fxfs(FxfsKey {
                wrapping_key_id: 0x1234567812345678u128.to_le_bytes(),
                key: WrappedKeyBytes::from([0xff; FXFS_WRAPPED_KEY_SIZE]),
            }),
        )]
        .into()
    }

    struct TestCrypt {
        counter: AtomicU8,
        unwrap_delay: std::time::Duration,
    }

    impl TestCrypt {
        fn new(counter: u8) -> Arc<Self> {
            Arc::new(Self {
                counter: AtomicU8::new(counter),
                unwrap_delay: std::time::Duration::from_secs(1),
            })
        }

        fn with_unwrap_delay(counter: u8, unwrap_delay: std::time::Duration) -> Arc<Self> {
            Arc::new(Self { counter: AtomicU8::new(counter), unwrap_delay })
        }
    }

    #[async_trait]
    impl Crypt for TestCrypt {
        async fn create_key(
            &self,
            _owner: u64,
            _purpose: KeyPurpose,
        ) -> Result<(FxfsKey, UnwrappedKey), zx::Status> {
            unimplemented!("Not used in tests");
        }

        async fn create_key_with_id(
            &self,
            _owner: u64,
            _wrapping_key_id: WrappingKeyId,
            _object_type: ObjectType,
        ) -> Result<(fxfs_crypto::EncryptionKey, UnwrappedKey), zx::Status> {
            unimplemented!("Not used in tests");
        }

        async fn unwrap_key(
            &self,
            _wrapped_key: &WrappedKey,
            _owner: u64,
        ) -> Result<UnwrappedKey, zx::Status> {
            if !self.unwrap_delay.is_zero() {
                fasync::Timer::new(self.unwrap_delay).await;
            }
            let counter = self.counter.fetch_add(1, Ordering::Relaxed);
            if counter == ERROR_COUNTER {
                error!("Unwrap failed!");
                Err(zx::Status::INTERNAL)
            } else {
                Ok(unwrapped_key(counter))
            }
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_get_keys() {
        TestExecutor::advance_to(MonotonicInstant::from_nanos(0)).await;

        let crypt = TestCrypt::new(0);
        let manager1 = Arc::new(KeyManager::new());
        let manager2 = manager1.clone();
        let manager3 = manager1.clone();
        let crypt1 = crypt.clone();
        let crypt2 = crypt.clone();

        let task1 = fasync::Task::spawn(async move {
            let mut plaintext = cipher_text(0);
            to_result(
                manager1
                    .get_keys(
                        1,
                        crypt1.as_ref(),
                        &mut Some(async || Ok(encryption_keys())),
                        false,
                        false,
                    )
                    .await
                    .expect("get_keys failed")
                    .find_key(0),
            )
            .unwrap()
            .decrypt(0, 0, 0, 0, MutPtrByteSlice::from(&mut plaintext[..]))
            .expect("decrypt failed");
            assert_eq!(&plaintext, &padded_plain_text());
        });
        let task2 = fasync::Task::spawn(async move {
            let mut plaintext = cipher_text(0);
            to_result(
                manager2
                    .get_keys(
                        1,
                        crypt2.as_ref(),
                        &mut Some(async || Ok(encryption_keys())),
                        false,
                        false,
                    )
                    .await
                    .expect("get_keys failed")
                    .find_key(0),
            )
            .unwrap()
            .decrypt(0, 0, 0, 0, MutPtrByteSlice::from(&mut plaintext[..]))
            .expect("decrypt failed");
            assert_eq!(&plaintext, &padded_plain_text());
        });
        let task3 = fasync::Task::spawn(async move {
            // Make sure this starts after the get_keys.
            fasync::Timer::new(zx::MonotonicDuration::from_millis(500)).await;
            let mut plaintext = cipher_text(0);
            manager3
                .get(1)
                .await
                .expect("get failed")
                .expect("missing key")
                .decrypt(0, 0, 0, 0, MutPtrByteSlice::from(&mut plaintext[..]))
                .expect("decrypt failed");
            assert_eq!(&plaintext, &padded_plain_text());
        });

        TestExecutor::advance_to(MonotonicInstant::after(zx::MonotonicDuration::from_millis(1500)))
            .await;

        task1.await;
        task2.await;
        task3.await;
    }

    #[fuchsia::test]
    async fn test_insert_and_remove() {
        let manager = Arc::new(KeyManager::new());

        manager.insert(1, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), false);
        let mut plaintext = cipher_text(0);
        manager
            .get(1)
            .await
            .expect("get failed")
            .expect("missing key")
            .decrypt(0, 0, 0, 0, MutPtrByteSlice::from(&mut plaintext[..]))
            .expect("decrypt failed");
        assert_eq!(&plaintext, &padded_plain_text());
        let _ = manager.remove(1);
        assert!(manager.get(1).await.expect("get failed").is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_purge() {
        TestExecutor::advance_to(MonotonicInstant::from_nanos(0)).await;

        let manager = Arc::new(KeyManager::new());
        manager.insert(1, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), false);

        TestExecutor::advance_to(MonotonicInstant::after(PURGE_TIMEOUT.into())).await;

        // After 1 period, the key should still be present.
        manager.get(1).await.expect("get failed").expect("missing key");

        TestExecutor::advance_to(MonotonicInstant::after(PURGE_TIMEOUT.into())).await;

        // The last access should have reset the timer and it should still be present.
        manager.get(1).await.expect("get failed").expect("missing key");

        TestExecutor::advance_to(MonotonicInstant::after((2 * PURGE_TIMEOUT).into())).await;

        // The key should have been evicted since two periods passed.
        assert!(manager.get(1).await.expect("get failed").is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_permanent() {
        TestExecutor::advance_to(MonotonicInstant::from_nanos(0)).await;

        let manager = Arc::new(KeyManager::new());
        manager.insert(1, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), true);
        manager.insert(2, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), false);

        // Skip forward two periods which should cause 2 to be purged but not 1.
        TestExecutor::advance_to(MonotonicInstant::after((2 * PURGE_TIMEOUT).into())).await;

        assert!(manager.get(1).await.expect("get failed").is_some());
        assert!(manager.get(2).await.expect("get failed").is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_clear() {
        TestExecutor::advance_to(MonotonicInstant::from_nanos(0)).await;

        let manager = Arc::new(KeyManager::new());
        manager.insert(1, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), true);
        manager.insert(2, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), false);
        manager.insert(3, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), false);

        // Skip forward 1 period which should make keys 2 and 3 pending deletion.
        TestExecutor::advance_to(MonotonicInstant::after(PURGE_TIMEOUT.into())).await;

        // Touch the the second key which should promote it to the active list.
        assert!(manager.get(2).await.expect("get failed").is_some());

        manager.clear();

        // Clearing should have removed all three keys.
        assert!(manager.get(1).await.expect("get failed").is_none());
        assert!(manager.get(2).await.expect("get failed").is_none());
        assert!(manager.get(3).await.expect("get failed").is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_error() {
        TestExecutor::advance_to(MonotonicInstant::from_nanos(0)).await;

        let crypt = TestCrypt::new(ERROR_COUNTER);
        let manager1 = Arc::new(KeyManager::new());
        let manager2 = manager1.clone();
        let crypt1 = crypt.clone();
        let crypt2 = crypt.clone();

        let task1 = fasync::Task::spawn(async move {
            assert!(
                manager1
                    .get_keys(
                        1,
                        crypt1.as_ref(),
                        &mut Some(async || Ok(encryption_keys())),
                        false,
                        false,
                    )
                    .await
                    .is_err()
            );
        });
        let task2 = fasync::Task::spawn(async move {
            assert!(
                manager2
                    .get_keys(
                        1,
                        crypt2.as_ref(),
                        &mut Some(async || Ok(encryption_keys())),
                        false,
                        false,
                    )
                    .await
                    .is_err()
            );
        });

        TestExecutor::advance_to(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)))
            .await;

        task1.await;
        task2.await;
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_wait_after_remove() {
        let manager = Arc::new(KeyManager::new());
        let crypt = TestCrypt::with_unwrap_delay(0, std::time::Duration::ZERO);
        let (sender, receiver) = oneshot::channel();
        let dropped = AtomicBool::new(false);

        assert!(
            join!(
                async {
                    let mut unwrap_keys = Some(async || {
                        struct OnDrop<'a>(&'a AtomicBool);
                        impl Drop for OnDrop<'_> {
                            fn drop(&mut self) {
                                self.0.store(true, Ordering::Relaxed);
                            }
                        }
                        let _on_drop = OnDrop(&dropped);
                        sender.send(()).unwrap();
                        // This should wait until both the remove calls below are waiting.
                        let _ = TestExecutor::poll_until_stalled(pending::<()>()).await;
                        Ok(encryption_keys())
                    });
                    manager.get_keys(1, crypt.as_ref(), &mut unwrap_keys, false, false).await
                },
                async {
                    let _ = receiver.await;
                    join!(
                        async {
                            manager.remove(1).await;
                            assert!(dropped.load(Ordering::Relaxed));
                        },
                        async {
                            manager.remove(1).await;
                            assert!(dropped.load(Ordering::Relaxed));
                        }
                    );
                },
            )
            .0
            .is_err()
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_clear_cached_keys() {
        TestExecutor::advance_to(MonotonicInstant::from_nanos(0)).await;

        let manager = Arc::new(KeyManager::new());
        // Key 1: permanent key (stored in `permanent`)
        manager.insert(1, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), true);
        // Keys 2 & 3: non-permanent keys (initially stored in `hash`)
        manager.insert(2, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), false);
        manager.insert(3, Arc::new(vec![(0, CipherHolder::Cipher(cipher(0)))].into()), false);

        // Advance 1 purge period so keys 2 and 3 move from `hash` into `pending_purge`.
        TestExecutor::advance_to(MonotonicInstant::after(PURGE_TIMEOUT.into())).await;

        // Touch key 2 to promote it back to active `hash`, leaving key 3 in `pending_purge`.
        assert!(manager.get(2).await.expect("get failed").is_some());

        manager.clear_cached_keys();

        // Permanent key 1 must be preserved; non-permanent keys 2 (`hash`) and 3 (`pending_purge`)
        // must be evicted.
        assert!(manager.get(1).await.expect("get failed").is_some());
        assert!(manager.get(2).await.expect("get failed").is_none());
        assert!(manager.get(3).await.expect("get failed").is_none());
    }
}
