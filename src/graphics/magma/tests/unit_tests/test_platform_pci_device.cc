// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_pci_device.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/test_util/platform_pci_device_helper.h>

#include <thread>

#include <gtest/gtest.h>

TEST(PlatformPciDevice, Basic) {
  magma::PlatformPciDevice* platform_device = TestPlatformPciDevice::GetInstance();
  ASSERT_TRUE(platform_device);

  uint16_t vendor_id = 0;
  bool ret = platform_device->ReadPciConfig16(0, &vendor_id);
  EXPECT_TRUE(ret);
  EXPECT_NE(vendor_id, 0);
}

TEST(PlatformPciDevice, MapMmio) {
  magma::PlatformPciDevice* platform_device = TestPlatformPciDevice::GetInstance();
  ASSERT_TRUE(platform_device);

  uint32_t pci_bar = 0;

  // Map once
  auto mmio = platform_device->CpuMapPciMmio(pci_bar);
  EXPECT_TRUE(mmio);

  // Map again
  auto mmio2 = platform_device->CpuMapPciMmio(pci_bar);
  EXPECT_TRUE(mmio2);
}

TEST(PlatformPciDevice, RegisterInterrupt) {
  magma::PlatformPciDevice* platform_device = TestPlatformPciDevice::GetInstance();
  ASSERT_NE(platform_device, nullptr);

  auto interrupt = platform_device->RegisterInterrupt();
  // Interrupt may be null if no core device support.
  if (interrupt) {
    std::thread thread([interrupt_raw = interrupt.get()] {
      DLOG("waiting for interrupt");
      interrupt_raw->Wait();
      DLOG("returned from interrupt");
    });

    interrupt->Signal();

    DLOG("waiting for thread");
    thread.join();
  }
}
