// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/mock-backlight.h"

#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <mutex>
#include <utility>

#include "src/graphics/display/lib/api-types/cpp/backlight-state.h"

namespace display::testing {

struct MockBacklight::Expectation {
  GetMaxBrightnessNitsChecker get_max_brightness_nits_checker;
  GetBacklightStateChecker get_backlight_state_checker;
  SetBacklightStateChecker set_backlight_state_checker;
};

MockBacklight::MockBacklight() = default;

MockBacklight::~MockBacklight() {
  ZX_ASSERT_MSG(check_all_calls_replayed_called_, "CheckAllCallsReplayed() not called on a mock");
}

void MockBacklight::ExpectGetMaxBrightnessNits(GetMaxBrightnessNitsChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.get_max_brightness_nits_checker = std::move(checker)});
}

void MockBacklight::ExpectGetBacklightState(GetBacklightStateChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.get_backlight_state_checker = std::move(checker)});
}

void MockBacklight::ExpectSetBacklightState(SetBacklightStateChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.set_backlight_state_checker = std::move(checker)});
}

void MockBacklight::CheckAllCallsReplayed() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(expectations_.size() == call_index_, "%zu expected calls were not received",
                expectations_.size() - call_index_);
  check_all_calls_replayed_called_ = true;
}

zx::result<float> MockBacklight::GetMaxBrightnessNits() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.get_max_brightness_nits_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.get_max_brightness_nits_checker();
}

zx::result<BacklightState> MockBacklight::GetBacklightState() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.get_backlight_state_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.get_backlight_state_checker();
}

zx::result<> MockBacklight::SetBacklightState(const BacklightState& state) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.set_backlight_state_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.set_backlight_state_checker(state);
}

}  // namespace display::testing
