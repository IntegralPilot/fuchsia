// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_OPERATION_TRACKER_BASE_H_
#define SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_OPERATION_TRACKER_BASE_H_

#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include <optional>
#include <type_traits>

//
// Provides tracking of various filesystem operations, including stubs for host builds.
//

namespace fs_inspect {

class OperationTracker {
 public:
  class TrackerEvent;

  /// Record latency/error of the given operation. `op` can be any callable that returns either a
  /// `zx_status_t` or `zx::result`.
  template <typename Operation>
  std::invoke_result_t<Operation> Track(Operation&& op);

  /// Create a `TrackerEvent` used to record a latency or error value. Can be moved between threads.
  /// The returned `TrackerEvent` must not outlive the associated `OperationTracker`.
  /// Time measurement starts when this object is created and ends when it goes out of scope.
  /// Use `TrackerEvent::SetStatus` to record the result of the operation.
  TrackerEvent NewEvent();

 protected:
  virtual void OnSuccess(zx::duration latency_ns) = 0;
  virtual void OnError(zx_status_t error) = 0;
  virtual void OnError() = 0;
};

/// RAII Helper to allow automatic recording of event data when it goes out of scope.
/// __Must not__ outlive the `OperationTracker` it was created from.
class OperationTracker::TrackerEvent final {
 public:
  explicit TrackerEvent(OperationTracker* tracker);
  ~TrackerEvent();

  // Only allow move construction.
  TrackerEvent(TrackerEvent&&) noexcept;
  TrackerEvent(const TrackerEvent&) = delete;
  TrackerEvent& operator=(const TrackerEvent&) = delete;
  TrackerEvent& operator=(TrackerEvent&&) = delete;

  /// Set status of operation. __Must__ be called at least once before this object is destroyed.
  /// __Must__ be called from same thread that destroys this object.
  void SetStatus(zx_status_t status);

 private:
  OperationTracker* tracker_;
  const zx::time start_;
  std::optional<zx_status_t> status_;
};

template <typename Operation>
std::invoke_result_t<Operation> OperationTracker::Track(Operation&& op) {
  auto tracker = NewEvent();
  if constexpr (std::is_same_v<zx_status_t, std::invoke_result_t<Operation>>) {
    zx_status_t status = op();
    tracker.SetStatus(status);
    return status;
  } else {
    zx::result result = op();
    tracker.SetStatus(result.status_value());
    return result;
  }
}

}  // namespace fs_inspect

#endif  // SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_OPERATION_TRACKER_BASE_H_
