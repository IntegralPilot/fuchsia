// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use heap_bindings as bindings;

/// Tracks an allocation of the given `size` in the heap profiler. This can be used to make visible
/// allocations that, while not from the general kernel heap, are notionally heap-like or from
/// separate slab / other non-general heap allocators.
/// The return value is a cookie that, along with `size` can be passed to `profile_track_free` to
/// indicate deallocation.
pub fn profile_track_alloc(size: usize) -> u32 {
    // SAFETY: There are no obligations.
    unsafe { bindings::cpp_profile_track_alloc(size) }
}

pub fn profile_track_free(cookie: u32, size: usize) {
    // SAFETY: There are no obligations.
    unsafe { bindings::cpp_profile_track_free(cookie, size) }
}
