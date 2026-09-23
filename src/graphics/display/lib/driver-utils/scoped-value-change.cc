// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-utils/scoped-value-change.h"

#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <mutex>
#include <unordered_set>

namespace display {

namespace {

struct ChangeTracker {
  static ChangeTracker& Get() {
    static ChangeTracker singleton;
    return singleton;
  }
  std::mutex mutex;
  std::unordered_set<void*> variables __TA_GUARDED(mutex);
};

}  // namespace

// static
void ScopedValueChange<void>::AddedChangeTo(void* variable) {
  ChangeTracker& change_tracker = ChangeTracker::Get();
  change_tracker.mutex.lock();
  auto [it, inserted] = change_tracker.variables.emplace(variable);
  change_tracker.mutex.unlock();

  ZX_DEBUG_ASSERT_MSG(inserted,
                      "Multiple ScopedValueChange instances created for the same variable");
}

// static
void ScopedValueChange<void>::RemovedChangeTo(void* variable) {
  ChangeTracker& change_tracker = ChangeTracker::Get();
  change_tracker.mutex.lock();
  const size_t erase_count = change_tracker.variables.erase(variable);
  change_tracker.mutex.unlock();

  ZX_DEBUG_ASSERT_MSG(erase_count == 1, "Bug in ScopedValueChange lifecycle / reference counting");
}

}  // namespace display
