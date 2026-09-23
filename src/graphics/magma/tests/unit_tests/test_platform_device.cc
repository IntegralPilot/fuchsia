// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_device.h>
#include <lib/magma/platform/platform_thread.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/test_util/platform_device_helper.h>

#include <thread>

#include <gtest/gtest.h>

TEST(PlatformDevice, Basic) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);

  auto platform_mmio = platform_device->CpuMapMmio(0);
  EXPECT_TRUE(platform_mmio.get());
}

TEST(PlatformDevice, MapMmio) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);

  uint32_t index = 0;

  // Map once.
  auto mmio = platform_device->CpuMapMmio(index);
  ASSERT_TRUE(mmio);

  // Map again.
  auto mmio2 = platform_device->CpuMapMmio(index);
  EXPECT_TRUE(mmio2);
}

TEST(PlatformDevice, SetRole) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);

  void* device_handle = platform_device->GetDeviceHandle();
  ASSERT_TRUE(device_handle);

  std::thread test_thread([device_handle]() {
    EXPECT_TRUE(magma::PlatformThreadHelper::SetRole(device_handle, "fuchsia.default"));
  });

  test_thread.join();
}

#ifndef __STDC_NO_THREADS__
static int thread_function(void* input) {
  auto mutex = reinterpret_cast<std::mutex*>(input);
  std::unique_lock<std::mutex> lock(*mutex);
  return 0;
}

TEST(PlatformDevice, SetThreadRole) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);

  void* device_handle = platform_device->GetDeviceHandle();
  ASSERT_TRUE(device_handle);

  // Block the thread to prevent it from exiting before we set the profile
  std::mutex mutex;
  std::unique_lock<std::mutex> lock(mutex);

  thrd_t thread;
  ASSERT_EQ(0, thrd_create(&thread, &thread_function, &mutex));
  EXPECT_TRUE(magma::PlatformThreadHelper::SetThreadRole(device_handle, thread, "fuchsia.default"));

  lock.unlock();
  thrd_join(thread, nullptr);
}
#endif

TEST(PlatformDevice, RegisterInterrupt) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_NE(platform_device, nullptr);

  // Less than 100 interrupts should be assigned to this driver.
  auto interrupt = platform_device->RegisterInterrupt(100);
  EXPECT_EQ(nullptr, interrupt);

  uint32_t index = 0;
  interrupt = platform_device->RegisterInterrupt(index);
  ASSERT_NE(nullptr, interrupt);

  std::thread thread([interrupt_raw = interrupt.get()] {
    DLOG("waiting for interrupt");
    interrupt_raw->Wait();
    DLOG("returned from interrupt");
  });

  interrupt->Signal();

  DLOG("waiting for thread");
  thread.join();
}
