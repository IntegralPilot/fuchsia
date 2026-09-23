// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_IMAGE_LIFECYCLE_LISTENER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_IMAGE_LIFECYCLE_LISTENER_H_

#include <lib/fit/function.h>
#include <lib/zx/time.h>
#include <zircon/compiler.h>

#include <mutex>
#include <vector>

#include "src/graphics/display/drivers/coordinator/image-lifecycle-listener.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"

namespace display_coordinator::testing {

// Strict mock for ImageLifecycleListener.
//
// This is a very rare case where strict mocking is warranted. Image destruction
// triggers FIDL calls to engine drivers.
class MockImageLifecycleListener : public ImageLifecycleListener {
 public:
  using ImageWillBeDestroyedChecker = fit::function<void(display::DriverImageId)>;

  MockImageLifecycleListener();
  ~MockImageLifecycleListener();

  MockImageLifecycleListener(const MockImageLifecycleListener&) = delete;
  MockImageLifecycleListener& operator=(const MockImageLifecycleListener&) = delete;

  void ExpectImageWillBeDestroyed(ImageWillBeDestroyedChecker checker);

  void CheckAllCallsReplayed();

  // ImageLifecycleListener implementation
  void ImageWillBeDestroyed(display::DriverImageId driver_image_id) override;

 private:
  struct Expectation;

  std::mutex mutex_;
  std::vector<Expectation> expectations_ __TA_GUARDED(mutex_);
  size_t call_index_ __TA_GUARDED(mutex_) = 0;
  bool check_all_calls_replayed_called_ __TA_GUARDED(mutex_) = false;
};

}  // namespace display_coordinator::testing

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_IMAGE_LIFECYCLE_LISTENER_H_
