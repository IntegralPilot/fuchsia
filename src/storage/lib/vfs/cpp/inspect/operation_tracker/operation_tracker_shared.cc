// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <optional>
#include <utility>

#include "src/storage/lib/vfs/cpp/inspect/operation_tracker/operation_tracker_base.h"

#ifdef __Fuchsia__
#include <lib/zx/clock.h>
#endif

namespace {

// Wrapper for zx_clock_get_monotonic() that acts as a stub for host builds where the syscall is
// not available.
zx::time GetCurrentTime() {
#ifdef __Fuchsia__
  return zx::clock::get_monotonic();
#else
  return zx::time::infinite_past();
#endif
}

}  // namespace

namespace fs_inspect {

OperationTracker::TrackerEvent OperationTracker::NewEvent() {
  return OperationTracker::TrackerEvent(this);
}

OperationTracker::TrackerEvent::TrackerEvent(OperationTracker* tracker)
    : tracker_(tracker), start_(GetCurrentTime()), status_(std::nullopt) {
  ZX_ASSERT(tracker != nullptr);
}

OperationTracker::TrackerEvent::TrackerEvent(TrackerEvent&& other) noexcept
    : tracker_(std::exchange(other.tracker_, nullptr)),
      start_(other.start_),
      status_(other.status_) {}

OperationTracker::TrackerEvent::~TrackerEvent() {
  // Handle case where object was moved from.
  if (!tracker_) {
    return;
  }

  ZX_DEBUG_ASSERT(status_.has_value());
  if (!status_.has_value()) {
    FX_LOGS(ERROR) << "TrackerEvent was destroyed without setting status!";
    // Status was not set, assume operation failed. Increment error/total operation counts.
    tracker_->OnError();
  } else if (status_.value() == ZX_OK) {
    // Operation succeeded, record latency and increment ok/total operation counts.
    tracker_->OnSuccess(GetCurrentTime() - start_);
  } else {
    // Operation failed with a specific error code, record it and increment the error/total counts.
    tracker_->OnError(status_.value());
  }
}

void OperationTracker::TrackerEvent::SetStatus(zx_status_t status) { status_ = status; }

}  // namespace fs_inspect
