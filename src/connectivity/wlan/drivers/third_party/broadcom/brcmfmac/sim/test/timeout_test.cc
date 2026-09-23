// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/errors.h>

#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "src/devices/lib/broadcom/commands.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

// Some default AP and association request values
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};

const MacAddr kDefaultBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});

constexpr uint64_t kDefaultScanTxnId = 0;
constexpr zx::duration kBeaconInterval = zx::msec(SimInterface::kDefaultPassiveScanDwellTimeMs / 2);

class TimeoutTest : public SimTest {
 public:
  // How long an individual test will run for. We need an end time because tests run until no more
  // events remain and so we need to stop aps from beaconing to drain the event queue.
  static constexpr zx::duration kTestDuration = zx::sec(100);

  void Init();

 protected:
  // This is the interface we will use for our single client interface
  SimInterface client_ifc_;
};

// Create our device instance and hook up the callbacks
void TimeoutTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
}

// Verify scan timeout is triggered.
TEST_F(TimeoutTest, ScanTimeout) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(kBeaconInterval);

  // Ignore scan request in sim-fw.
  WithSimDevice([this](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("escan", ZX_OK, BCME_OK, client_ifc_.iface_id_);
  });

  // Start a passive scan
  env_->ScheduleNotification(
      std::bind(&SimInterface::StartScan, &client_ifc_, kDefaultScanTxnId, false,
                std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>>{}),
      zx::msec(10));

  env_->Run(kTestDuration);

  // Verify scan completed
  auto result = client_ifc_.ScanResultCode(kDefaultScanTxnId);
  EXPECT_TRUE(result);

  // Verify result was an error code
  EXPECT_EQ(*result, wlan_fullmac_wire::WlanScanResult::kCanceledByDriverOrFirmware);

  // No results should have been seen
  auto scan_result_list = client_ifc_.ScanResultList(kDefaultScanTxnId);
  EXPECT_EQ(scan_result_list->size(), 0U);
}

// Verify association timeout is triggered.
TEST_F(TimeoutTest, AssocTimeout) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  // Ignore association req in sim-fw.
  WithSimDevice([this](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjCmd(BRCMF_C_SET_SSID, ZX_OK, BCME_OK, client_ifc_.iface_id_);
  });

  client_ifc_.AssociateWith(ap, zx::msec(10));

  // Check 500 ms before connection timer is expected to fire
  static constexpr zx::duration kTempDuration =
      zx::duration(BRCMF_CONNECT_TIMER_DUR_MS - ZX_MSEC(500));
  env_->Run(kTempDuration);
  // Assoc attempts should be 1 but assoc results should be 0.
  EXPECT_EQ(client_ifc_.stats_.connect_attempts, 1U);
  const auto connect_results = &client_ifc_.stats_.connect_results;
  EXPECT_EQ(connect_results->size(), 0U);
  // run for the reminder of the test duration
  env_->Run(kTestDuration);

  // Receiving assoc_resp in SME with error status.
  EXPECT_EQ(client_ifc_.stats_.connect_attempts, 1U);
  EXPECT_EQ(connect_results->size(), 1U);
  EXPECT_EQ(connect_results->front().result_code(),
            wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);
}

// Verify the deauthentication timeout is triggered.
TEST_F(TimeoutTest, DeauthTimeout) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(kBeaconInterval);
  client_ifc_.AssociateWith(ap, zx::msec(10));

  // Ignore deauthentication req in sim-fw.
  WithSimDevice([this](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjCmd(BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON, ZX_OK, BCME_OK,
                                       client_ifc_.iface_id_);
  });
  env_->ScheduleNotification(
      std::bind(&SimInterface::DeauthenticateFrom, &client_ifc_, kDefaultBssid,
                wlan_ieee80211::ReasonCode::kUnspecifiedReason),
      zx::msec(10));

  env_->Run(kTestDuration);

  // deauth_conf have no return status, just verify it's received.
  EXPECT_EQ(client_ifc_.stats_.deauth_results.size(), 1U);
}

// This test case will verify the following scenario:
// - the driver issues a connect command to firmware,
// - before the firmware responds to the connect command, SME sends a deauth_req to driver
// - SME issues a scan after that
// - the scan is successfully executed.
TEST_F(TimeoutTest, ScanAfterAssocTimeout) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(kBeaconInterval);

  // Ignore association req in sim-fw.
  WithSimDevice([this](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjCmd(BRCMF_C_SET_SSID, ZX_OK, BCME_OK, client_ifc_.iface_id_);
  });
  // There are three timers for them, and all have been cancelled.
  client_ifc_.AssociateWith(ap, zx::msec(10));
  env_->ScheduleNotification(
      std::bind(&SimInterface::DeauthenticateFrom, &client_ifc_, kDefaultBssid,
                wlan_ieee80211::ReasonCode::kUnspecifiedReason),
      zx::sec(1));
  env_->ScheduleNotification(
      std::bind(&SimInterface::StartScan, &client_ifc_, kDefaultScanTxnId, false,
                std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>>{}),
      zx::sec(3));

  env_->Run(kTestDuration);

  // This when we issue a deauth request right after and assoc_req, the successful deauth_req will
  // stop the connect timer for assoc_req, thus no assoc_conf event will be received.
  EXPECT_EQ(client_ifc_.stats_.connect_results.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.deauth_results.size(), 1U);

  // Verify that the scan completed successfully
  auto result = client_ifc_.ScanResultCode(kDefaultScanTxnId);
  EXPECT_TRUE(result);
  EXPECT_EQ(*result, wlan_fullmac_wire::WlanScanResult::kSuccess);

  // There is only one AP in the environment, but two scan results will be heard from SME since the
  // scan dwell time is twice the beacon interval.
  auto scan_result_list = client_ifc_.ScanResultList(kDefaultScanTxnId);
  EXPECT_EQ(scan_result_list->size(), 2U);
}

}  // namespace wlan::brcmfmac
