// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sys::{
    ZX_HANDLE_INVALID, ZX_KOID_INVALID, ZX_OK, zx_futex_get_owner, zx_futex_requeue,
    zx_futex_requeue_single_owner, zx_futex_t, zx_futex_wait, zx_futex_wake,
    zx_futex_wake_handle_close_thread_exit, zx_futex_wake_single_owner,
};
use crate::{Koid, MonotonicInstant, NullableHandle, Status, Thread};

/// A safe wrapper around zx_futex_t, generally called as part of higher-level synchronization
/// primitives.
#[derive(Debug)]
pub struct Futex {
    inner: zx_futex_t,
}

impl std::ops::Deref for Futex {
    type Target = zx_futex_t;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Futex {
    #[inline]
    pub const fn new(value: i32) -> Self {
        Self { inner: zx_futex_t::new(value) }
    }

    fn value_ptr(&self) -> *const zx_futex_t {
        std::ptr::addr_of!(self.inner)
    }

    /// See https://fuchsia.dev/reference/syscalls/futex_wake.
    #[inline]
    pub fn wake(&self, wake_count: u32) {
        // SAFETY: Arguments for this system call do not have any liveness or validity requirements.
        let status = unsafe { zx_futex_wake(self.value_ptr(), wake_count) };
        assert_eq!(status, ZX_OK, "only fails due to misaligned or unmapped pointers");
    }

    /// Wakes the maximum number of waiters possible.
    ///
    /// See https://fuchsia.dev/reference/syscalls/futex_wake.
    #[inline]
    pub fn wake_all(&self) {
        self.wake(u32::MAX)
    }

    /// See https://fuchsia.dev/reference/syscalls/futex_wake_single_owner.
    #[inline]
    pub fn wake_single_owner(&self) {
        // SAFETY: Arguments for this system call do not have any liveness or validity requirements.
        let status = unsafe { zx_futex_wake_single_owner(self.value_ptr()) };
        assert_eq!(status, ZX_OK, "only fails due to misaligned or unampped pointers")
    }

    /// See https://fuchsia.dev/reference/syscalls/futex_wake_handle_close_thread_exit.
    pub fn wake_handle_close_thread_exit(
        &self,
        wake_count: u32,
        new_value: i32,
        to_close: NullableHandle,
    ) -> ! {
        // SAFETY: Arguments for this system call do not have any liveness or validity requirements.
        unsafe {
            zx_futex_wake_handle_close_thread_exit(
                self.value_ptr(),
                wake_count,
                new_value,
                to_close.raw_handle(),
            );
        }
        unreachable!("zx_futex_wake_handle_close_thread_exit() does not return.");
    }

    /// See https://fuchsia.dev/reference/syscalls/futex_requeue.
    #[inline]
    pub fn requeue(
        &self,
        wake_count: u32,
        current_value: i32,
        requeue_to: &Self,
        requeue_count: u32,
        new_requeue_owner: Option<&Thread>,
    ) -> Result<(), Status> {
        // SAFETY: Arguments for this system call do not have any liveness or validity requirements.
        Status::ok(unsafe {
            zx_futex_requeue(
                self.value_ptr(),
                wake_count,
                current_value,
                requeue_to.value_ptr(),
                requeue_count,
                new_requeue_owner.map(|o| o.raw_handle()).unwrap_or(ZX_HANDLE_INVALID),
            )
        })
    }

    /// See https://fuchsia.dev/reference/syscalls/futex_requeue_single_owner.
    #[inline]
    pub fn requeue_single_owner(
        &self,
        current_value: i32,
        requeue_to: &Self,
        requeue_count: u32,
        new_requeue_owner: Option<&Thread>,
    ) -> Result<(), Status> {
        // SAFETY: Arguments for this system call do not have any liveness or validity requirements.
        Status::ok(unsafe {
            zx_futex_requeue_single_owner(
                self.value_ptr(),
                current_value,
                requeue_to.value_ptr(),
                requeue_count,
                new_requeue_owner.map(|o| o.raw_handle()).unwrap_or(ZX_HANDLE_INVALID),
            )
        })
    }

    /// See https://fuchsia.dev/reference/syscalls/futex_wait.
    #[inline]
    pub fn wait(
        &self,
        current_value: i32,
        new_owner: Option<&Thread>,
        deadline: MonotonicInstant,
    ) -> Result<(), Status> {
        // SAFETY: Arguments for this system call do not have any liveness or validity requirements.
        Status::ok(unsafe {
            zx_futex_wait(
                self.value_ptr(),
                current_value,
                new_owner.map(|o| o.raw_handle()).unwrap_or(ZX_HANDLE_INVALID),
                deadline.into_nanos(),
            )
        })
    }

    /// See https://fuchsia.dev/reference/syscalls/futex_get_owner.
    pub fn get_owner(&self) -> Option<Koid> {
        let mut koid = ZX_KOID_INVALID;

        // SAFETY: Arguments for this system call do not have any liveness or validity requirements.
        // `&mut koid` is valid for the entire duration of the call.
        Status::ok(unsafe { zx_futex_get_owner(self.value_ptr(), &mut koid) })
            .expect("get_owner only fails due to misaligned or unmapped pointers");

        if koid != ZX_KOID_INVALID { Some(Koid::from_raw(koid)) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Duration, Unowned};

    #[test]
    fn basic_wait_wake() {
        // SAFETY: converting between versions of the same types, the handle is valid
        let main_thread = unsafe {
            Unowned::from_raw_handle(fuchsia_runtime::with_thread_self(|thread| {
                // Need the raw_handle method from fuchsia_runtime's dependency.
                thread.as_handle_ref().raw_handle()
            }))
        };
        let futex = Futex::new(0);
        std::thread::scope(|s| {
            s.spawn(|| futex.wait(0, Some(&*main_thread), MonotonicInstant::INFINITE).unwrap());
            while futex.get_owner().is_none() {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            futex.wake_single_owner();
        });
    }

    #[test]
    fn cant_wait_with_wrong_value() {
        let futex = Futex::new(0);
        assert_eq!(futex.wait(5, None, MonotonicInstant::INFINITE).unwrap_err(), Status::BAD_STATE);
    }

    #[test]
    fn wait_timed_out() {
        assert_eq!(
            Futex::new(0)
                .wait(0, None, MonotonicInstant::after(Duration::from_seconds(1)))
                .unwrap_err(),
            Status::TIMED_OUT,
        );
    }
}
