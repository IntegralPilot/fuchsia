// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_bus_mapper.h>
#include <lib/magma_service/test_util/platform_pci_device_helper.h>

#include <vector>

#include <gtest/gtest.h>

#include "test_platform_bus_mapper_cases.h"

TEST(PlatformPciDevice, BusMapperBasic) {
  magma::PlatformPciDevice* platform_device = TestPlatformPciDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  TestPlatformBusMapperCases::Basic(mapper.get(), 1);
  TestPlatformBusMapperCases::Basic(mapper.get(), 2);
  TestPlatformBusMapperCases::Basic(mapper.get(), 10);
}

TEST(PlatformPciDevice, BusMapperOverlapped) {
  magma::PlatformPciDevice* platform_device = TestPlatformPciDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  TestPlatformBusMapperCases::Overlapped(mapper.get(), 12);
}

TEST(PlatformPciDevice, BusMapperContiguous) {
  magma::PlatformPciDevice* platform_device = TestPlatformPciDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  TestPlatformBusMapperCases::Contiguous(mapper.get());
}

// Test that we can map a 512MB buffer which requires multiple pins
TEST(PlatformPciDevice, BusMapperLarge) {
  magma::PlatformPciDevice* platform_device = TestPlatformPciDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  uint64_t constexpr kZirconPinPageLimit = 512 * 1024 * 1024 / 4096;
  TestPlatformBusMapperCases::Basic(mapper.get(), kZirconPinPageLimit);
}
