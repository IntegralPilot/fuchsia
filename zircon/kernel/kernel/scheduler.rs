// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Scheduler interface.

use super::types::cpu_mask_t;

unsafe extern "C" {
    fn cpp_scheduler_peek_active_mask() -> cpu_mask_t;
}

/// Returns the bitmask of currently active CPUs managed by the scheduler.
pub fn peek_active_mask() -> cpu_mask_t {
    // SAFETY: FFI call reads the kernel atomic active scheduler mask.
    unsafe { cpp_scheduler_peek_active_mask() }
}
