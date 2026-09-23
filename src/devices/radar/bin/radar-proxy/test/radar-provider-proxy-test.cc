// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../radar-provider-proxy.h"

#include <fidl/fuchsia.hardware.radar/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <zxtest/zxtest.h>

#include "radar-proxy-test.h"

namespace radar {
namespace {

using RadarProviderProxyTest = RadarProxyTest<RadarProviderProxy>;

TEST_F(RadarProviderProxyTest, Connect) {
  AddRadarDevice();

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  fidl::Client radar_client(std::move(endpoints->client), loop().dispatcher());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());

    radar_client->GetBurstProperties().Then([&](auto& result) {
      ASSERT_TRUE(result.is_ok());
      EXPECT_EQ(result->size(), 12345);
      loop().Quit();
    });
  });
  loop().Run();
}

TEST_F(RadarProviderProxyTest, Reconnect) {
  AddRadarDevice();

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());
  fidl::Client radar_client(std::move(endpoints->client), loop().dispatcher());

  zx::result new_endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(new_endpoints.is_ok());
  fidl::Client new_radar_client(std::move(new_endpoints->client), loop().dispatcher());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());

    radar_client->GetBurstProperties().Then([&](auto& result) {
      ASSERT_TRUE(result.is_ok());
      EXPECT_EQ(result->size(), 12345);

      // Close the connection between the proxy and the radar driver. Calls to Connect() should
      // then be put in the request queue instead of immediately completed.
      UnbindProviderAndWaitForConnectionAttempt();

      // The connection with the radar driver itself should remain open.
      radar_client->GetBurstProperties().Then([&](auto& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->size(), 12345);

        // Issue a connect request to be completed when a new device shows up.
        dut_client_->Connect(std::move(new_endpoints->server)).Then([&](auto& result) {
          EXPECT_TRUE(result.is_ok());

          // The proxy connected to the new device and completed our request. We should now
          // have a new connection to the underlying driver.
          new_radar_client->GetBurstProperties().Then([&](auto& result) {
            ASSERT_TRUE(result.is_ok());
            EXPECT_EQ(result->size(), 12345);
            loop().Quit();
          });
        });

        // Signal that a new device was added to /dev/class/radar. The proxy should connect
        // to the new device and complete our previous request.
        AddRadarDevice();
      });
    });
  });

  loop().Run();
}

TEST_F(RadarProviderProxyTest, DelayedConnect) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());
  fidl::Client radar_client(std::move(endpoints->client), loop().dispatcher());

  uint32_t burst_size = 0;

  // Attempt to connect before a radar device is available. Our request should be put in a queue and
  // completed when a device is added.
  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());

    radar_client->GetBurstProperties().Then([&](auto& result) {
      burst_size = result.is_ok() ? result->size() : 0;
      loop().Quit();
    });
  });
  loop().RunUntilIdle();

  // Signal to the proxy that a device was added. This should cause it to process any connect
  // requests that came in before this point.
  AddRadarDevice();

  // The loop will be stopped after the burst size call completes.
  loop().Run();

  EXPECT_EQ(burst_size, 12345);
}

}  // namespace
}  // namespace radar
