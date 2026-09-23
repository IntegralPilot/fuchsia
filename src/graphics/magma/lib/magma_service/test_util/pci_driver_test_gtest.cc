// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define MAGMA_DLOG_ENABLE 1

#include <lib/magma/util/dlog.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>
#include <lib/magma_service/test_util/platform_pci_device_helper.h>
#include <zircon/types.h>

#include <thread>

#include <gtest/gtest.h>

namespace {
magma::PlatformPciDevice* platform_pci_device_s = nullptr;

void* test_device_s = nullptr;
}  // namespace

magma::PlatformPciDevice* TestPlatformPciDevice::GetInstance() { return platform_pci_device_s; }

msd::DeviceHandle* GetTestDeviceHandle() {
  return reinterpret_cast<msd::DeviceHandle*>(test_device_s);
}

zx_status_t magma_indriver_test(magma::PlatformPciDevice* platform_pci_device) {
  MAGMA_DLOG("running magma unit tests");
  platform_pci_device_s = platform_pci_device;
  test_device_s = platform_pci_device->GetDeviceHandle();
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
