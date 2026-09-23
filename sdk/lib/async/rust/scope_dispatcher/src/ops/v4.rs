// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use libasync_sys::async_dispatcher_t;
use std::sync::Arc;
use zx::sys::{ZX_OK, zx_status_t};

use crate::ScopeDispatcher;

// ops_v4 dispatch functions
pub unsafe extern "C" fn acquire_shared_ref(
    dispatcher_ptr: *mut async_dispatcher_t,
) -> zx_status_t {
    // SAFETY: The caller is responsible for ensuring that the pointer to the dispatcher object
    // is valid, and the dispatcher object is the first member of the `repr(C)` ScopeDispatcher
    // struct.
    unsafe { Arc::increment_strong_count(dispatcher_ptr.cast::<ScopeDispatcher>()) };
    ZX_OK
}
pub unsafe extern "C" fn release_shared_ref(
    dispatcher_ptr: *mut async_dispatcher_t,
) -> zx_status_t {
    // SAFETY: The caller is responsible for ensuring that the pointer to the dispatcher object
    // is valid, and the dispatcher object is the first member of the `repr(C)` ScopeDispatcher
    // struct.
    unsafe { Arc::decrement_strong_count(dispatcher_ptr.cast::<ScopeDispatcher>()) };
    ZX_OK
}

#[cfg(test)]
mod test {
    use libasync_dispatcher::GetAsyncDispatcher;

    use super::*;

    #[fuchsia::test]
    async fn test_refcounting() {
        let scope_dispatcher = ScopeDispatcher::new();
        assert_eq!(Arc::strong_count(&scope_dispatcher), 2);

        let async_dispatcher = scope_dispatcher.get_async_dispatcher();
        assert_eq!(Arc::strong_count(&scope_dispatcher), 3);
        drop(async_dispatcher);

        assert_eq!(Arc::strong_count(&scope_dispatcher), 2);
        scope_dispatcher.shutdown().await;
    }
}
