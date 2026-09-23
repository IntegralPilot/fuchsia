// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_ENGINE_LISTENER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_ENGINE_LISTENER_H_

#include <lib/fit/function.h>
#include <lib/zx/time.h>
#include <zircon/compiler.h>

#include <memory>
#include <mutex>
#include <vector>

#include "src/graphics/display/drivers/coordinator/engine-listener.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator::testing {

class MockEngineListener : public EngineListener {
 public:
  using OnDisplayAddedChecker = fit::function<void(std::unique_ptr<AddedDisplayInfo>)>;
  using OnDisplayRemovedChecker = fit::function<void(display::DisplayId)>;
  using OnDisplayVsyncChecker =
      fit::function<void(display::DisplayId, zx::time_monotonic, display::DriverConfigStamp)>;
  using OnCaptureCompleteChecker = fit::function<void()>;

  MockEngineListener();
  ~MockEngineListener();

  MockEngineListener(const MockEngineListener&) = delete;
  MockEngineListener& operator=(const MockEngineListener&) = delete;

  void ExpectOnDisplayAdded(OnDisplayAddedChecker checker);
  void ExpectOnDisplayRemoved(OnDisplayRemovedChecker checker);
  void ExpectOnDisplayVsync(OnDisplayVsyncChecker checker);
  void ExpectOnCaptureComplete(OnCaptureCompleteChecker checker);

  void CheckAllCallsReplayed();

  // EngineListener implementation
  void OnDisplayAdded(std::unique_ptr<AddedDisplayInfo> added_display_info) override;
  void OnDisplayRemoved(display::DisplayId display_id) override;
  void OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                      display::DriverConfigStamp driver_config_stamp) override;
  void OnCaptureComplete() override;

 private:
  struct Expectation;

  std::mutex mutex_;
  std::vector<Expectation> expectations_ __TA_GUARDED(mutex_);
  size_t call_index_ __TA_GUARDED(mutex_) = 0;
  bool check_all_calls_replayed_called_ __TA_GUARDED(mutex_) = false;
};

}  // namespace display_coordinator::testing

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_ENGINE_LISTENER_H_
