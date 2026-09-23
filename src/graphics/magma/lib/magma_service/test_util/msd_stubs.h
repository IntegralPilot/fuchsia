// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_MSD_STUBS_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_MSD_STUBS_H_

#include <lib/magma_service/msd.h>

#include <cassert>

namespace msd::testing {

// MSD-specific tests may inherit from this class if they only need to implement
// a subset of the functions for a specific test. This code should not be used
// in production, because in production code should implement all cases.
class StubNotificationHandler : public NotificationHandler {
  void NotificationChannelSend(cpp20::span<uint8_t> data) override { assert(false); }
  void ContextKilled() override { assert(false); }
  void PerformanceCounterReadCompleted(const PerfCounterResult& result) override { assert(false); }
  async_dispatcher_t* GetAsyncDispatcher() override {
    assert(false);
    return nullptr;
  }
};

}  // namespace msd::testing

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_MSD_STUBS_H_
