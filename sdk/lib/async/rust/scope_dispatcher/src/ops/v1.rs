// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use libasync_sys::{async_dispatcher_t, async_guest_bell_trap_t, async_receiver_t};
use zx::sys::{ZX_ERR_NOT_SUPPORTED, zx_status_t};

use crate::ScopeDispatcher;

mod tasks;
mod waits;

pub use tasks::*;
pub use waits::*;

// ops_v1 dispatch functions
pub unsafe extern "C" fn now(dispatcher_ptr: *mut async_dispatcher_t) -> i64 {
    // Safety: The caller is responsible for ensuring this is only ever called on a dispatcher
    // object that was originally obtained from [`ScopeDispatcher::as_ptr`].
    let dispatcher = unsafe { ScopeDispatcher::from_ptr(dispatcher_ptr) };
    dispatcher.global_handle().now().into_nanos()
}
pub unsafe extern "C" fn queue_packet(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _receiver_ptr: *mut async_receiver_t,
    _data: *const [u8; 32],
) -> zx_status_t {
    ZX_ERR_NOT_SUPPORTED
}
pub unsafe extern "C" fn set_guest_bell_trap(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _trap_ptr: *mut async_guest_bell_trap_t,
    _guest: u32,
    _addr: usize,
    _len: usize,
) -> zx_status_t {
    ZX_ERR_NOT_SUPPORTED
}

#[cfg(test)]
mod test {
    use fuchsia_async::{MonotonicInstant, TestExecutor};
    use libasync_dispatcher::AsAsyncDispatcherRef;

    use super::*;

    #[test]
    fn test_now() {
        let mut test_executor = TestExecutor::new_with_fake_time();
        test_executor.set_fake_time(MonotonicInstant::from_nanos(1000));
        let scope_dispatcher =
            ScopeDispatcher::new_on_executor(test_executor.global_handle().clone());
        assert_eq!(scope_dispatcher.as_async_dispatcher_ref().now(), 1000);
        assert_eq!(
            test_executor.run_until_stalled(&mut scope_dispatcher.shutdown()),
            core::task::Poll::Ready(())
        );
    }
}
