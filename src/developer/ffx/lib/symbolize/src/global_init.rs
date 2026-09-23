// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Module for managing the global initialization state of zxdb and curl.

use fuchsia_sync::Mutex;

/// A type responsible for managing global initialization state. As long as one of these is live,
/// curl and other globally initialized C++ state will be live. When the last one goes out of scope
/// the global cleanup routine will be called.
#[derive(Debug)]
pub(crate) struct GlobalInitHandle {
    // Include a private field so we don't accidentally construct one of these outside this module.
    _inner: (),
}

static GLOBAL_INIT: Mutex<u64> = Mutex::new(0);

impl GlobalInitHandle {
    pub(crate) fn new() -> Self {
        // Ensure the lock is held for the entire function to avoid racing with any other
        // initialization or cleanup.
        let mut state = GLOBAL_INIT.lock();
        if *state == 0 {
            // SAFETY: Basic FFI call. This won't be called unless either no GlobalInitHandle has
            // been previously constructed or the cleanup routine has already been called.
            unsafe { symbolizer_sys::symbolizer_global_init() };
        }
        *state += 1;
        Self { _inner: () }
    }
}

impl Drop for GlobalInitHandle {
    fn drop(&mut self) {
        // Ensure the lock is held for the entire function to avoid racing with any other
        // initialization or cleanup.
        let mut state = GLOBAL_INIT.lock();
        if *state == 1 {
            // This is the last handle live and the global state needs to be cleaned up.

            // SAFETY: Basic FFI call. This won't be called unless either no GlobalInitHandle has
            // been previously constructed or the cleanup routine has already been called.
            unsafe { symbolizer_sys::symbolizer_global_cleanup() };
        }
        *state -= 1;
    }
}
