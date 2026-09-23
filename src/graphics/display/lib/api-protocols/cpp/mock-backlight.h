// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_BACKLIGHT_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_BACKLIGHT_H_

#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <cstddef>
#include <mutex>
#include <vector>

#include "src/graphics/display/lib/api-protocols/cpp/backlight-interface.h"
#include "src/graphics/display/lib/api-types/cpp/backlight-state.h"

namespace display::testing {

// Strict mock for BacklightInterface implementations.
//
// This is a very rare case where strict mocking is warranted. The code under
// test is an adapter that maps FIDL calls 1:1 to C++ calls. So, the API
// contract being tested is expressed in terms of individual function calls.
class MockBacklight : public display::BacklightInterface {
 public:
  // Expectation containers for display::BacklightInterface:
  using GetMaxBrightnessNitsChecker = fit::function<zx::result<float>()>;
  using GetBacklightStateChecker = fit::function<zx::result<BacklightState>()>;
  using SetBacklightStateChecker = fit::function<zx::result<>(const BacklightState& state)>;

  MockBacklight();
  MockBacklight(const MockBacklight&) = delete;
  MockBacklight& operator=(const MockBacklight&) = delete;
  ~MockBacklight();

  // Expectations for display::BacklightInterface:
  void ExpectGetMaxBrightnessNits(GetMaxBrightnessNitsChecker checker);
  void ExpectGetBacklightState(GetBacklightStateChecker checker);
  void ExpectSetBacklightState(SetBacklightStateChecker checker);

  // Must be called at least once during an instance's lifetime.
  //
  // Tests are recommended to call this in a TearDown() method, or at the end of
  // the test case implementation.
  void CheckAllCallsReplayed();

  // display::BacklightInterface:
  zx::result<float> GetMaxBrightnessNits() override;
  zx::result<BacklightState> GetBacklightState() override;
  zx::result<> SetBacklightState(const BacklightState& state) override;

 private:
  struct Expectation;

  std::mutex mutex_;
  std::vector<Expectation> expectations_ __TA_GUARDED(mutex_);
  size_t call_index_ __TA_GUARDED(mutex_) = 0;
  bool check_all_calls_replayed_called_ __TA_GUARDED(mutex_) = false;
};

}  // namespace display::testing

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_BACKLIGHT_H_
