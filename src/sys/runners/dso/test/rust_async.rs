// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_dso::DsoAsyncArgs;
use std::sync::atomic::{AtomicU32, Ordering};

// This is global storage, not thread local so all instances of this component in the DSO
// runner share it.
static RUN_COUNTER: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn rust_async_read_run_counter() -> u32 {
    RUN_COUNTER.load(Ordering::Relaxed)
}

#[fuchsia_dso::main(async, logging = false)]
pub async fn main(args: DsoAsyncArgs) {
    RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Close lifecycle channel immediately to exit the component.
    drop(args.lifecycle);
}
