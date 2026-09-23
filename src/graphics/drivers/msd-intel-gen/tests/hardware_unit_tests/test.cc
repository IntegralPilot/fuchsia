// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.development/cpp/wire.h>
#include <fidl/fuchsia.driver.registrar/cpp/wire.h>
#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/channel.h>
#include <lib/zx/clock.h>

#include <shared_mutex>
#include <thread>

#include <gtest/gtest.h>

#include "magma_intel_gen_vendor_id.h"

namespace {
const std::string kProductionDriver =
    "fuchsia-pkg://fuchsia.com/msd-intel-gen#meta/libmsd_intel.cm";
const std::string kTestDriver =
    "fuchsia-pkg://fuchsia.com/msd_intel_gen_hardware_unit_tests#meta/libmsd_intel_test.cm";
}  // namespace

inline void RestartAndWait(std::string driver_url) {
  auto manager = component::Connect<fuchsia_driver_development::Manager>();

  fidl::WireSyncClient manager_client(*std::move(manager));
  auto test_device = magma::TestDeviceBase(MAGMA_VENDOR_ID_INTEL);
  ASSERT_NO_FATAL_FAILURE() << "Failed to create test device";

  auto restart_result = manager_client->RestartDriverHosts(
      fidl::StringView::FromExternal(driver_url),
      fuchsia_driver_development::wire::RestartRematchFlags::kRequested);

  ASSERT_TRUE(restart_result.ok()) << restart_result.status_string();
  EXPECT_TRUE(restart_result->is_ok()) << restart_result->error_value();

  {
    auto channel = test_device.magma_channel();
    // Use the existing channel to wait for the device handle to close.
    EXPECT_EQ(ZX_OK,
              channel.handle()->wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr));
  }

  // Loop until a new device with the correct specs is found.
  auto deadline_time = zx::clock::get_monotonic() + zx::sec(40);
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
      if (magma_status == MAGMA_STATUS_OK && vendor_id == MAGMA_VENDOR_ID_INTEL) {
        return;
      }
    }
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  GTEST_FATAL_FAILURE_("We failed to find the GPU before the deadline");
}

TEST(HardwareUnitTests, All) {
  auto manager = component::Connect<fuchsia_driver_development::Manager>();

  // The test build of the MSD runs a bunch of unit tests automatically when it loads. We need to
  // unload the normal MSD to replace it with the test MSD so we can run those tests and query the
  // test results.
  fidl::WireSyncClient manager_client(*std::move(manager));
  // May fail if the production driver hasn't been enabled before, so ignore error.
  (void)manager_client->DisableDriver(fidl::StringView::FromExternal(kProductionDriver),
                                      fidl::StringView());

  auto registrar = component::Connect<fuchsia_driver_registrar::DriverRegistrar>();

  ASSERT_TRUE(registrar.is_ok());

  auto registrar_client = fidl::WireSyncClient(std::move(*registrar));

  {
    auto result = registrar_client->Register(fidl::StringView::FromExternal(kTestDriver));

    ASSERT_TRUE(result.ok()) << result.status_string();
    ASSERT_FALSE(result->is_error()) << result->error_value();
  }

  auto cleanup = fit::defer([&]() {
    // Ignore errors when trying to get the existing driver back to a working state.
    (void)manager_client->EnableDriver(fidl::StringView::FromExternal(kProductionDriver),
                                       fidl::StringView());
    (void)manager_client->DisableDriver(fidl::StringView::FromExternal(kTestDriver),
                                        fidl::StringView());
    RestartAndWait(kTestDriver);
  });

  ASSERT_NO_FATAL_FAILURE(RestartAndWait(kProductionDriver));

  {
    zx::result channel = magma::TestDeviceBase::GetTestFromVendorId(MAGMA_VENDOR_ID_INTEL);
    ASSERT_TRUE(channel.is_ok()) << channel.status_string();

    const fidl::WireResult result = fidl::WireCall(*channel)->GetUnitTestStatus();
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    const fidl::WireResponse response = result.value();
    ASSERT_EQ(response.status, ZX_OK) << zx_status_get_string(response.status);
  }
}
