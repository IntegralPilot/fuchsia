// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_TESTS_INTEGRATION_DRIVER_REGISTRY_H_
#define SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_TESTS_INTEGRATION_DRIVER_REGISTRY_H_

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.registrar/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/clock.h>

#include <gtest/gtest.h>

#include "magma_vendor_queries.h"

inline void WaitForTestDevice() {
  auto deadline_time = zx::clock::get_monotonic() + zx::sec(10);
  while (zx::clock::get_monotonic() < deadline_time) {
    zx::result result = magma::TestDeviceBase::GetTestFromVendorId(MAGMA_VENDOR_ID_MALI);
    if (result.is_ok()) {
      return;
    }
    if (result.error_value() != ZX_ERR_NOT_FOUND) {
      GTEST_FATAL_FAILURE_("Bad result from GetTestFromVendorId") << result.status_string();
    }
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  GTEST_FATAL_FAILURE_("We failed to find the test GPU before the deadline");
}

inline void WaitForDevice() {
  // Loop until a new device with the correct specs is found.
  auto deadline_time = zx::clock::get_monotonic() + zx::sec(10);
  while (zx::clock::get_monotonic() < deadline_time) {
    for (auto& p : std::filesystem::directory_iterator("/svc/fuchsia.gpu.magma.Service")) {
      auto magma_client = component::Connect<fuchsia_gpu_magma::TestDevice>(
          static_cast<std::string>(p.path()) + "/device");

      magma_device_t device;
      EXPECT_EQ(MAGMA_STATUS_OK,
                magma_device_import(magma_client->TakeChannel().release(), &device));

      uint64_t vendor_id;
      magma_status_t magma_status =
          magma_device_query(device, MAGMA_QUERY_VENDOR_ID, NULL, &vendor_id);

      magma_device_release(device);
      if (magma_status == MAGMA_STATUS_OK && vendor_id == MAGMA_VENDOR_ID_MALI) {
        return;
      }
    }
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  GTEST_FATAL_FAILURE_("We failed to find the GPU before the deadline");
}

inline void RestartAndWait(std::string driver_url) {
  auto manager = component::Connect<fuchsia_driver_development::Manager>();

  fidl::WireSyncClient manager_client(*std::move(manager));
  auto test_device = magma::TestDeviceBase(MAGMA_VENDOR_ID_MALI);
  ASSERT_NO_FATAL_FAILURE() << "Failed to create test device";

  auto restart_result = manager_client->RestartDriverHosts(
      fidl::StringView::FromExternal(driver_url),
      fuchsia_driver_development::wire::RestartRematchFlags::kRequested |
          fuchsia_driver_development::wire::RestartRematchFlags::kCompositeSpec);

  ASSERT_TRUE(restart_result.ok()) << restart_result.status_string();
  EXPECT_TRUE(restart_result->is_ok()) << restart_result->error_value();

  {
    auto channel = test_device.magma_channel();
    // Use the existing channel to wait for the device handle to close.
    EXPECT_EQ(ZX_OK,
              channel.handle()->wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr));
  }

  WaitForDevice();
}

#endif  // SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_TESTS_INTEGRATION_DRIVER_REGISTRY_H_
