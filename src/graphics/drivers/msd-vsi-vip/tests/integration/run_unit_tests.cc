// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.development/cpp/wire.h>
#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/fit/defer.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/channel.h>

#include <shared_mutex>
#include <thread>

#include <gtest/gtest.h>

#include "driver_registry.h"
#include "magma_vendor_queries.h"

namespace {
const std::string kProductionDriver =
    "fuchsia-pkg://" VSI_PRODUCTION_DRIVER_PACKAGE "#meta/msd_vsi.cm";
const std::string kTestDriver = "fuchsia-pkg://" VSI_TEST_DRIVER_PACKAGE "#meta/libmsd_vsi_test.cm";
}  // namespace

// The test build of the MSD runs a bunch of unit tests automatically when it loads. We need to
// unload the normal MSD to replace it with the test MSD so we can run those tests and query the
// test results.
TEST(UnitTests, UnitTests) {
  auto manager = component::Connect<fuchsia_driver_development::Manager>();

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

  RestartAndWait(kProductionDriver, "/svc/fuchsia.gpu.magma.TestService");

  auto cleanup = fit::defer([&]() {
    // Ignore errors when trying to get the existing driver back to a working state.
    (void)manager_client->EnableDriver(fidl::StringView::FromExternal(kProductionDriver),
                                       fidl::StringView());
    (void)manager_client->DisableDriver(fidl::StringView::FromExternal(kTestDriver),
                                        fidl::StringView());
    RestartAndWait(kTestDriver, "/svc/fuchsia.gpu.magma.TrustedService");
  });

  {
    zx::result channel = magma::TestDeviceBase::GetTestFromVendorId(MAGMA_VENDOR_ID_VSI);
    ASSERT_TRUE(channel.is_ok()) << channel.status_string();

    const fidl::WireResult result = fidl::WireCall(*channel)->GetUnitTestStatus();
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    const fidl::WireResponse response = result.value();
    ASSERT_EQ(response.status, ZX_OK) << zx_status_get_string(response.status);
  }
}
