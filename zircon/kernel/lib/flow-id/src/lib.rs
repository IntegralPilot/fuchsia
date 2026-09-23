// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

use core::sync::atomic::{AtomicU64, Ordering};

// Decrease the likelihood of collisions with zero-based userspace flow ID
// generators by starting in the second half of the flow ID space.
const FIRST_KERNEL_FLOW_ID: u64 = 1 << 63;

static FLOW_ID_GENERATOR: AtomicU64 = AtomicU64::new(FIRST_KERNEL_FLOW_ID);

/// Generates globally unique 64-bit flow IDs for tracing.
pub fn generate() -> u64 {
    FLOW_ID_GENERATOR.fetch_add(1, Ordering::Relaxed)
}

/// Generates globally unique 64-bit flow IDs for tracing (C-exported).
#[unsafe(no_mangle)]
pub extern "C" fn flow_id_generate() -> u64 {
    generate()
}
