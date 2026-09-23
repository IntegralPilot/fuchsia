// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/errors.h>

#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {
namespace {

constexpr zx::duration kSimulatedClockDuration = zx::sec(10);

}  // namespace

const MacAddr kDefaultMac({0x12, 0x34, 0x56, 0x65, 0x43, 0x21});

// Verify that a query for security features support works on a client interface
TEST_F(SimTest, ClientIfcQuerySecuritySupport) {
  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kDefaultMac), ZX_OK);

  wlan_common::SecuritySupport resp;
  env_->ScheduleNotification(std::bind(&SimInterface::QuerySecuritySupport, &client_ifc, &resp),
                             zx::sec(1));
  env_->Run(kSimulatedClockDuration);

  ASSERT_TRUE(resp.has_sae());
  ASSERT_TRUE(resp.sae().has_driver_handler_supported());
  EXPECT_FALSE(resp.sae().driver_handler_supported());
  ASSERT_TRUE(resp.sae().has_sme_handler_supported());
  EXPECT_TRUE(resp.sae().sme_handler_supported());
  ASSERT_TRUE(resp.has_mfp());
  ASSERT_TRUE(resp.mfp().has_supported());
  EXPECT_TRUE(resp.mfp().supported());
}

// Verify that a query for spectrum management features support works on a client interface
TEST_F(SimTest, ClientIfcQuerySpectrumManagementSupport) {
  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kDefaultMac), ZX_OK);

  wlan_common::SpectrumManagementSupport resp;
  env_->ScheduleNotification(
      std::bind(&SimInterface::QuerySpectrumManagementSupport, &client_ifc, &resp), zx::sec(1));
  env_->Run(kSimulatedClockDuration);

  ASSERT_TRUE(resp.has_dfs());
  ASSERT_TRUE(resp.dfs().has_supported());
  EXPECT_TRUE(resp.dfs().supported());
}

// Verify that there's no duplicate counter/gauge ID or counter/gauge name returned
// in QueryTelemetrySupport
TEST_F(SimTest, ClientIfcQueryTelemetrySupport_NoDuplicate) {
  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kDefaultMac), ZX_OK);

  fuchsia_wlan_stats::wire::TelemetrySupport resp;
  env_->ScheduleNotification(std::bind(&SimInterface::QueryTelemetrySupport, &client_ifc, &resp),
                             zx::sec(1));
  env_->Run(kSimulatedClockDuration);

  auto counter_configs = resp.inspect_counter_configs();
  std::set<uint16_t> counter_ids;
  std::set<std::string> counter_names;
  for (auto config : counter_configs) {
    if (counter_ids.contains(config.counter_id())) {
      ASSERT_TRUE(false, "Duplicate counter id %u", config.counter_id());
    }
    counter_ids.insert(config.counter_id());

    std::string counter_name(config.counter_name().get());
    if (counter_names.contains(counter_name)) {
      ASSERT_TRUE(false, "Duplicate counter name %s", counter_name.c_str());
    }
    counter_names.insert(counter_name);
  }

  auto gauge_configs = resp.inspect_gauge_configs();
  std::set<uint16_t> gauge_ids;
  std::set<std::string> gauge_names;
  for (auto config : gauge_configs) {
    if (gauge_ids.contains(config.gauge_id())) {
      ASSERT_TRUE(false, "Duplicate gauge id %u", config.gauge_id());
    }
    gauge_ids.insert(config.gauge_id());

    std::string gauge_name(config.gauge_name().get());
    if (gauge_names.contains(gauge_name)) {
      ASSERT_TRUE(false, "Duplicate gauge name %s", gauge_name.c_str());
    }
    gauge_names.insert(gauge_name);
  }
}

}  // namespace wlan::brcmfmac
