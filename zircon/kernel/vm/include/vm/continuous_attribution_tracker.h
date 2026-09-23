// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_CONTINUOUS_ATTRIBUTION_TRACKER_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_CONTINUOUS_ATTRIBUTION_TRACKER_H_

#include <assert.h>
#include <stdint.h>
#include <zircon/compiler.h>

#include <ktl/type_traits.h>
#include <object/opaque_storage.h>

class ContinuousAttributionTracker;

__BEGIN_CDECLS

void rust_continuous_attribution_tracker_move_init(ContinuousAttributionTracker* tracker,
                                                   ContinuousAttributionTracker* source);
void rust_continuous_attribution_tracker_move_assign(ContinuousAttributionTracker* tracker,
                                                     ContinuousAttributionTracker* source);
uint32_t rust_continuous_attribution_tracker_fetch_current(
    const ContinuousAttributionTracker* tracker);
uint32_t rust_continuous_attribution_tracker_fetch_hwm_and_reset(
    ContinuousAttributionTracker* tracker);
void rust_continuous_attribution_tracker_increment(ContinuousAttributionTracker* tracker,
                                                   uint32_t by);
void rust_continuous_attribution_tracker_decrement(ContinuousAttributionTracker* tracker,
                                                   uint32_t by);

__END_CDECLS

// Tracks the number of populated slots in the VmCowPages' local page list. If the VmCowPages
// changes a slot to being populated, or vice versa, that should be reported to
// ContinuousAttributionTracker.
//
// This class is not thread safe and should only be accessed under the lock of the associated
// VmCowPages.
class ContinuousAttributionTracker final {
 public:
  ContinuousAttributionTracker() = default;
  ~ContinuousAttributionTracker() = default;

  // Move and move assignment zero the |source|.
  ContinuousAttributionTracker(ContinuousAttributionTracker&& source) {
    rust_continuous_attribution_tracker_move_init(this, &source);
  }
  ContinuousAttributionTracker& operator=(ContinuousAttributionTracker&& source) {
    if (this != &source) {
      rust_continuous_attribution_tracker_move_assign(this, &source);
    }
    return *this;
  }
  ContinuousAttributionTracker(ContinuousAttributionTracker&) = delete;
  ContinuousAttributionTracker& operator=(ContinuousAttributionTracker&) = delete;

  // Returns the tracked count of populated slots.
  uint32_t FetchCurrent() const { return rust_continuous_attribution_tracker_fetch_current(this); }

  // Get the greatest number of populated slots since the statistic was last reset.
  //
  // Resets the high-water mark.
  uint32_t FetchHwmAndReset() {
    return rust_continuous_attribution_tracker_fetch_hwm_and_reset(this);
  }

  // Increments the count by |by|. This quantity must be strictly positive.
  void Increment(uint32_t by) { rust_continuous_attribution_tracker_increment(this, by); }

  // Decrements the count by |by|. This quantity must be strictly positive.
  void Decrement(uint32_t by) { rust_continuous_attribution_tracker_decrement(this, by); }

 private:
  [[maybe_unused]] OpaqueStorage<8, 4> storage_{};
};

// ContinuousAttributionTracker uses a 32-bit count to represent the number of populated slots in
// the local page list. Since a ContinuousAttributionTracker is intended to be stored inline in a
// VmCowPages, reducing its size (by not using a 64-bit count) is a substantial memory saving for
// the system.
static_assert(sizeof(ContinuousAttributionTracker) == 8);
static_assert(alignof(ContinuousAttributionTracker) == 4);

// The stub continuous attribution tracker. This object stores no data. Intended to be used in place
// of the regular continuous attribution tracker, unless users opt-in to its existence.
class StubContinuousAttributionTracker final {
 public:
  StubContinuousAttributionTracker() = default;
  ~StubContinuousAttributionTracker() = default;

  StubContinuousAttributionTracker(StubContinuousAttributionTracker&& source) = default;
  StubContinuousAttributionTracker& operator=(StubContinuousAttributionTracker&& source) = default;
  StubContinuousAttributionTracker(StubContinuousAttributionTracker&) = delete;
  StubContinuousAttributionTracker& operator=(StubContinuousAttributionTracker&) = delete;

  // Unconditionally panics.
  uint32_t FetchCurrent() const { PANIC("stub"); }

  // Unconditionally panics.
  uint32_t FetchHwmAndReset() { PANIC("stub"); }

  void Increment(uint32_t by) {}
  void Decrement(uint32_t by) {}
};

// The continuous attribution tracker supports an empty "stubbed out" state.
static_assert(ktl::is_empty_v<StubContinuousAttributionTracker>);
static_assert(alignof(StubContinuousAttributionTracker) == 1);

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_CONTINUOUS_ATTRIBUTION_TRACKER_H_
