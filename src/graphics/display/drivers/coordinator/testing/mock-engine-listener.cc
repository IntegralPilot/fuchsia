// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/testing/mock-engine-listener.h"

#include <lib/zx/time.h>
#include <zircon/assert.h>

#include <memory>
#include <mutex>
#include <utility>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator::testing {

struct MockEngineListener::Expectation {
  OnDisplayAddedChecker on_display_added_checker;
  OnDisplayRemovedChecker on_display_removed_checker;
  OnDisplayVsyncChecker on_display_vsync_checker;
  OnCaptureCompleteChecker on_capture_complete_checker;
};

MockEngineListener::MockEngineListener() = default;

MockEngineListener::~MockEngineListener() {
  ZX_ASSERT_MSG(check_all_calls_replayed_called_, "CheckAllCallsReplayed() not called on a mock");
}

void MockEngineListener::ExpectOnDisplayAdded(OnDisplayAddedChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.on_display_added_checker = std::move(checker)});
}

void MockEngineListener::ExpectOnDisplayRemoved(OnDisplayRemovedChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.on_display_removed_checker = std::move(checker)});
}

void MockEngineListener::ExpectOnDisplayVsync(OnDisplayVsyncChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.on_display_vsync_checker = std::move(checker)});
}

void MockEngineListener::ExpectOnCaptureComplete(OnCaptureCompleteChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.on_capture_complete_checker = std::move(checker)});
}

void MockEngineListener::CheckAllCallsReplayed() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(expectations_.size() == call_index_, "%zu expected calls were not received",
                expectations_.size() - call_index_);
  check_all_calls_replayed_called_ = true;
}

void MockEngineListener::OnDisplayAdded(std::unique_ptr<AddedDisplayInfo> added_display_info) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_display_added_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_display_added_checker(std::move(added_display_info));
}

void MockEngineListener::OnDisplayRemoved(display::DisplayId display_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_display_removed_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_display_removed_checker(display_id);
}

void MockEngineListener::OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                                        display::DriverConfigStamp driver_config_stamp) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_display_vsync_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_display_vsync_checker(display_id, timestamp, driver_config_stamp);
}

void MockEngineListener::OnCaptureComplete() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_capture_complete_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_capture_complete_checker();
}

}  // namespace display_coordinator::testing
