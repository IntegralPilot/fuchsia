// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/wlan/ieee80211/cpp/fidl.h>

#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

constexpr uint16_t kDefaultCh = 149;
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = kDefaultCh};
const MacAddr kDefaultBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});
const MacAddr kFakeMac({0xde, 0xad, 0xbe, 0xef, 0x00, 0x02});
const simulation::WlanTxInfo kDefaultTxInfo = {
    .primary_channel = kDefaultChannel,
    .bandwidth = wlan_ieee80211::ChannelBandwidth::kCbw20,
    .vht_secondary_80_channel = {.band = kDefaultChannel.band, .number = 0}};

class MfgTest : public SimTest {
 public:
  static constexpr zx::duration kTestDuration = zx::sec(100);
  void CreateIF(wlan_common::WlanMacRole role);
  void DelIF(SimInterface* ifc);
  void StartSoftAP();
  void TxAuthAndAssocReq();

 protected:
  SimInterface client_ifc_;
  SimInterface softap_ifc_;
};

void MfgTest::CreateIF(wlan_common::WlanMacRole role) {
  switch (role) {
    case wlan_common::WlanMacRole::kClient:
      ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
      break;
    case wlan_common::WlanMacRole::kAp:
      ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kAp, &softap_ifc_), ZX_OK);
      break;
    default:
      // Other mac roles shouldn't appear in this test.
      ASSERT_TRUE(0);
  }
}
void MfgTest::DelIF(SimInterface* ifc) { EXPECT_EQ(DeleteInterface(ifc), ZX_OK); }

void MfgTest::StartSoftAP() { softap_ifc_.StartSoftAp(kDefaultSoftApSsid, kDefaultChannel); }

void MfgTest::TxAuthAndAssocReq() {
  // Get the mac address of the SoftAP
  MacAddr soft_ap_mac;
  softap_ifc_.GetMacAddr(&soft_ap_mac);
  const fuchsia_wlan_ieee80211::Ssid ssid = {'S', 'i', 'm', '_', 'A', 'P'};
  // Pass the auth stop for softAP iface before assoc.
  simulation::SimAuthFrame auth_req_frame(kFakeMac, soft_ap_mac, 1, simulation::AUTH_TYPE_OPEN,
                                          fuchsia_wlan_ieee80211::StatusCode::kSuccess);
  env_->Tx(auth_req_frame, kDefaultTxInfo, this);
  simulation::SimAssocReqFrame assoc_req_frame(kFakeMac, soft_ap_mac, ssid);
  env_->Tx(assoc_req_frame, kDefaultTxInfo, this);
}

// Check to make sure only one IF can be active at anytime with MFG FW.
TEST_F(MfgTest, BasicTest) {
  Init();
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);

  // SoftAP If creation should fail as Client IF has already been created.
  ASSERT_NE(StartInterface(wlan_common::WlanMacRole::kAp, &softap_ifc_, kDefaultBssid), ZX_OK);

  // Now delete the Client IF and SoftAP creation should pass
  EXPECT_EQ(DeleteInterface(&client_ifc_), ZX_OK);
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kAp, &softap_ifc_, kDefaultBssid), ZX_OK);
  // Now that SoftAP IF is created, Client IF creation should fail
  ASSERT_NE(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
  EXPECT_EQ(DeleteInterface(&softap_ifc_), ZX_OK);
}

// Start client and SoftAP interfaces and check if
// the client can associate to a FakeAP and a fake client can associate to the
// SoftAP.
TEST_F(MfgTest, CheckConnections) {
  Init();
  CreateIF(wlan_common::WlanMacRole::kClient);
  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20,
                        {.band = kDefaultChannel.band, .number = 0});

  // Associate to FakeAp
  client_ifc_.AssociateWith(ap, zx::msec(10));
  env_->ScheduleNotification(std::bind(&MfgTest::DelIF, this, &client_ifc_), zx::msec(100));
  env_->ScheduleNotification(std::bind(&MfgTest::CreateIF, this, wlan_common::WlanMacRole::kAp),
                             zx::msec(200));
  env_->ScheduleNotification(std::bind(&MfgTest::StartSoftAP, this), zx::msec(300));
  // Associate to SoftAP
  env_->ScheduleNotification(std::bind(&MfgTest::TxAuthAndAssocReq, this), zx::msec(400));
  env_->ScheduleNotification(std::bind(&MfgTest::DelIF, this, &softap_ifc_), zx::msec(500));

  env_->Run(kTestDuration);

  // Check if the client's assoc with FakeAP succeeded
  EXPECT_EQ(client_ifc_.stats_.connect_attempts, 1u);
  EXPECT_EQ(client_ifc_.stats_.connect_successes, 1u);
  // Deletion of the client IF should have resulted in disassoc of the
  // client (cleanup during IF delete).
  EXPECT_EQ(client_ifc_.stats_.disassoc_indications.size(), 1u);
  // Verify Assoc with SoftAP succeeded
  ASSERT_EQ(softap_ifc_.stats_.assoc_indications.size(), 1u);
  ASSERT_EQ(softap_ifc_.stats_.auth_indications.size(), 1u);
}
}  // namespace wlan::brcmfmac
