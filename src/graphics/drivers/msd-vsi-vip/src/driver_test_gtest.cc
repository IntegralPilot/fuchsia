// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define MAGMA_DLOG_ENABLE 1

#include <lib/magma/platform/zircon/zircon_platform_device_dfv2.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma_service/test_util/gtest_printer.h>
#include <lib/magma_service/test_util/platform_device_helper.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>

#include <gtest/gtest.h>

#include "parent_device_dfv2.h"

namespace {

std::unique_ptr<magma::PlatformDevice> platform_device_s;
void* test_device_s;

}  // namespace

magma::PlatformDevice* TestPlatformDevice::GetInstance() { return platform_device_s.get(); }

msd::DeviceHandle* GetTestDeviceHandle() {
  return reinterpret_cast<msd::DeviceHandle*>(test_device_s);
}

zx_status_t magma_indriver_test(ParentDeviceDfv2* device) {
  MAGMA_DLOG("running magma unit tests");
  zx::result platform_device_client =
      device->incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (!platform_device_client.is_ok()) {
    return platform_device_client.status_value();
  }
  platform_device_s = std::make_unique<magma::ZirconPlatformDeviceDfv2>(
      fidl::WireSyncClient(std::move(platform_device_client.value())));
  test_device_s = device;
  const int kArgc = 1;
  const char* argv[kArgc + 1] = {"magma_indriver_test"};
  testing::InitGoogleTest(const_cast<int*>(&kArgc), const_cast<char**>(argv));

  testing::TestEventListeners& listeners = testing::UnitTest::GetInstance()->listeners();
  delete listeners.Release(listeners.default_result_printer());
  listeners.Append(new magma::GtestPrinter);

  // Use a thread with a well-known lifetime to run the test to ensure all
  // gtest-internal TLS state is released by the end of testing.
  zx_status_t status;
  std::thread test_thread([&]() {
    MAGMA_LOG(INFO, "[DRV START=]\n");
    status = RUN_ALL_TESTS() == 0 ? ZX_OK : ZX_ERR_INTERNAL;
    MAGMA_LOG(INFO, "[DRV END===]\n[==========]\n");
  });
  test_thread.join();
  platform_device_s.reset();
  return status;
}

// Should never happen.
extern "C" void _Exit(int value) {
  fprintf(stderr, "GTEST called _Exit\n");
  while (true) {
  }
}
