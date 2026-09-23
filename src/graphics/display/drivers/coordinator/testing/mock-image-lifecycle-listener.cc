// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/testing/mock-image-lifecycle-listener.h"

#include <lib/zx/time.h>
#include <zircon/assert.h>

#include <mutex>
#include <utility>

#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"

namespace display_coordinator::testing {

struct MockImageLifecycleListener::Expectation {
  ImageWillBeDestroyedChecker image_will_be_destroyed_checker;
};

MockImageLifecycleListener::MockImageLifecycleListener() = default;

MockImageLifecycleListener::~MockImageLifecycleListener() {
  ZX_ASSERT_MSG(check_all_calls_replayed_called_, "CheckAllCallsReplayed() not called on a mock");
}

void MockImageLifecycleListener::ExpectImageWillBeDestroyed(ImageWillBeDestroyedChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.image_will_be_destroyed_checker = std::move(checker)});
}

void MockImageLifecycleListener::CheckAllCallsReplayed() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(expectations_.size() == call_index_, "%zu expected calls were not received",
                expectations_.size() - call_index_);
  check_all_calls_replayed_called_ = true;
}

void MockImageLifecycleListener::ImageWillBeDestroyed(display::DriverImageId driver_image_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.image_will_be_destroyed_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.image_will_be_destroyed_checker(driver_image_id);
}

}  // namespace display_coordinator::testing
