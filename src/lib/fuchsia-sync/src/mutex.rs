// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx::sys;

unsafe extern "C" {
    fn sync_mutex_lock(lock: *const sys::zx_futex_t);
    fn sync_mutex_trylock(lock: *const sys::zx_futex_t) -> sys::zx_status_t;
    fn sync_mutex_unlock(lock: *const sys::zx_futex_t);
}

// See SYNC_MUTEX_INIT in lib/sync/mutex.h
const SYNC_MUTEX_INIT: i32 = 0;

#[repr(transparent)]
pub struct RawSyncMutex(sys::zx_futex_t);

impl RawSyncMutex {
    #[inline]
    fn as_futex_ptr(&self) -> *const sys::zx_futex_t {
        std::ptr::addr_of!(self.0)
    }
}

// SAFETY: This trait requires that "[i]mplementations of this trait must ensure
// that the mutex is actually exclusive: a lock can't be acquired while the mutex
// is already locked." This guarantee is provided by libsync's APIs.
unsafe impl lock_api::RawMutex for RawSyncMutex {
    const INIT: RawSyncMutex = RawSyncMutex(sys::zx_futex_t::new(SYNC_MUTEX_INIT));

    // libsync does not require the lock / unlock operations to happen on the same thread.
    // However, we set this to no send to catch mistakes where folks accidentally hold a lock across
    // an async await, which is often not intentional behavior and can lead to a deadlock. If
    // sufficient need is required, this may be changed back to `lock_api::GuardSend`.
    type GuardMarker = lock_api::GuardNoSend;

    #[inline]
    fn lock(&self) {
        // SAFETY: This call requires we pass a non-null pointer to a valid futex.
        // This is guaranteed by using `self` through a shared reference.
        unsafe {
            sync_mutex_lock(self.as_futex_ptr());
        }
    }

    #[inline]
    fn try_lock(&self) -> bool {
        // SAFETY: This call requires we pass a non-null pointer to a valid futex.
        // This is guaranteed by using `self` through a shared reference.
        unsafe { sync_mutex_trylock(self.as_futex_ptr()) == sys::ZX_OK }
    }

    #[inline]
    unsafe fn unlock(&self) {
        unsafe { sync_mutex_unlock(self.as_futex_ptr()) }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Mutex;

    #[test]
    fn test_lock_and_unlock() {
        let value = Mutex::<u32>::new(5);
        let mut guard = value.lock();
        assert_eq!(*guard, 5);
        *guard = 6;
        assert_eq!(*guard, 6);
        std::mem::drop(guard);
    }

    #[test]
    fn test_try_lock() {
        // Testing that try_lock fails technically creates a "cycle". Bypass the lock cycle wrappers
        // to test this.
        let value = lock_api::Mutex::<RawSyncMutex, u32>::new(5);
        let _guard = value.lock();
        assert!(value.try_lock().is_none());
    }

    #[test]
    #[cfg(detect_lock_cycles)]
    #[should_panic(expected = "Found cycle in mutex dependency graph")]
    fn test_mutex_reentrancy() {
        let value = Mutex::<u32>::new(5);
        let _guard = value.lock();
        let _guard2 = value.lock();
    }
}
