// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/discardable_vmo_tracker_ffi.h"

#include <kernel/ffi.h>
#include <ktl/memory.h>

#include "vm/discardable_vmo_tracker.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE bool cpp_discardable_vmo_tracker_debug_is_reclaimable(
    const DiscardableVmoTracker* tracker) {
  return tracker->DebugIsReclaimable();
}

FFI_ALWAYS_INLINE bool cpp_discardable_vmo_tracker_debug_is_unreclaimable(
    const DiscardableVmoTracker* tracker) {
  return tracker->DebugIsUnreclaimable();
}

FFI_ALWAYS_INLINE bool cpp_discardable_vmo_tracker_debug_is_discarded(
    const DiscardableVmoTracker* tracker) {
  return tracker->DebugIsDiscarded();
}

FFI_ALWAYS_INLINE uint64_t
cpp_discardable_vmo_tracker_debug_get_lock_count(const DiscardableVmoTracker* tracker) {
  return tracker->DebugGetLockCount();
}

FFI_ALWAYS_INLINE void cpp_discardable_vmo_tracker_debug_discardable_page_counts(
    DiscardableVmoTracker::DiscardablePageCounts* out_counts) {
  DiscardableVmoTracker::DiscardablePageCounts counts =
      DiscardableVmoTracker::DebugDiscardablePageCounts();
  ktl::construct_at(out_counts, counts);
}

}  // extern "C"
