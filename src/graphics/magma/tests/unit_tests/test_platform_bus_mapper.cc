// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_bus_mapper.h>
#include <lib/magma_service/test_util/platform_device_helper.h>

#include <vector>

#include <gtest/gtest.h>

#include "test_platform_bus_mapper_cases.h"

TEST(PlatformDevice, BusMapperBasic) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  TestPlatformBusMapperCases::Basic(mapper.get(), 1);
  TestPlatformBusMapperCases::Basic(mapper.get(), 2);
  TestPlatformBusMapperCases::Basic(mapper.get(), 10);
}

TEST(PlatformDevice, BusMapperOverlapped) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  TestPlatformBusMapperCases::Overlapped(mapper.get(), 12);
}

TEST(PlatformDevice, BusMapperContiguous) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  TestPlatformBusMapperCases::Contiguous(mapper.get());
}

TEST(PlatformDevice, BusMapperPhysical) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);

  uint32_t mmio_count = platform_device->GetMmioCount();
  EXPECT_GT(mmio_count, 0u);

  for (uint32_t i = 0; i < mmio_count; i++) {
    auto buffer = platform_device->GetMmioBuffer(i);
    ASSERT_TRUE(buffer);

    auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
    ASSERT_TRUE(mapper);

    uint64_t page_count = buffer->size() / magma::page_size();

    auto bus_mapping = mapper->MapPageRangeBus(buffer.get(), 0, page_count);
    ASSERT_TRUE(bus_mapping);

    for (uint32_t i = 1; i < page_count; ++i) {
      EXPECT_EQ(bus_mapping->Get()[i - 1] + magma::page_size(), bus_mapping->Get()[i]);
    }
  }
}

// Test that we can map a 512MB buffer which requires multiple pins
TEST(PlatformDevice, BusMapperLarge) {
  magma::PlatformDevice* platform_device = TestPlatformDevice::GetInstance();
  ASSERT_TRUE(platform_device);
  auto mapper = magma::PlatformBusMapper::Create(platform_device->GetBusTransactionInitiator());
  ASSERT_TRUE(mapper);

  uint64_t constexpr kZirconPinPageLimit = 512 * 1024 * 1024 / 4096;
  TestPlatformBusMapperCases::Basic(mapper.get(), kZirconPinPageLimit);
}
