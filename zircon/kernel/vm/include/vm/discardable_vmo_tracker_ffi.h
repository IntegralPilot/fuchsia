// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_DISCARDABLE_VMO_TRACKER_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_DISCARDABLE_VMO_TRACKER_FFI_H_

#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include "vm/discardable_vmo_tracker.h"

__BEGIN_CDECLS

bool cpp_discardable_vmo_tracker_debug_is_reclaimable(const DiscardableVmoTracker* tracker);
bool cpp_discardable_vmo_tracker_debug_is_unreclaimable(const DiscardableVmoTracker* tracker);
bool cpp_discardable_vmo_tracker_debug_is_discarded(const DiscardableVmoTracker* tracker);
uint64_t cpp_discardable_vmo_tracker_debug_get_lock_count(const DiscardableVmoTracker* tracker);
void cpp_discardable_vmo_tracker_debug_discardable_page_counts(
    DiscardableVmoTracker::DiscardablePageCounts* out_counts);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_DISCARDABLE_VMO_TRACKER_FFI_H_
