// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define MAGMA_DLOG_ENABLE 1

#include <lib/magma/platform/platform_logger.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma_service/test_util/platform_device_helper.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>

#include <gtest/gtest.h>

#include "parent_device.h"

namespace {
ParentDevice* test_device_s;
}  // namespace

msd::DeviceHandle* GetTestDeviceHandle() { return test_device_s->ToDeviceHandle(); }

magma::PlatformDevice* TestPlatformDevice::GetInstance() {
  return test_device_s->GetPlatformDevice();
}

zx_status_t magma_indriver_test(ParentDevice* device) {
  DLOG("running magma unit tests");
  test_device_s = device;
  const int kArgc = 1;
  const char* argv[kArgc + 1] = {"magma_indriver_test"};
  testing::InitGoogleTest(const_cast<int*>(&kArgc), const_cast<char**>(argv));

  // Use a thread with a well-known lifetime to run the test to ensure all
  // gtest-internal TLS state is released by the end of testing.
  zx_status_t status;
  std::thread test_thread([&]() {
    printf("[DRV START=]\n");
    status = RUN_ALL_TESTS() == 0 ? ZX_OK : ZX_ERR_INTERNAL;
    printf("[DRV END===]\n[==========]\n");
  });
  test_thread.join();
  return status;
}

// Should never happen.
extern "C" void _Exit(int value) {
  fprintf(stderr, "GTEST called _Exit\n");
  while (true) {
  }
}
