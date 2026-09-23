// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <fuchsia/wlan/ieee80211/cpp/fidl.h>
#include <zircon/errors.h>

#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "fidl/fuchsia.wlan.fullmac/cpp/wire_types.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

static constexpr zx::duration kTestDuration = zx::sec(100);
static constexpr auto kDisassocReason = wlan_ieee80211::ReasonCode::kNotAuthenticated;
static constexpr auto kDeauthReason = wlan_ieee80211::ReasonCode::kNotAuthenticated;
static const MacAddr kApBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});
static constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kApChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};
static const MacAddr kStaMacAddr({0x11, 0x22, 0x33, 0x44, 0x55, 0x66});

TEST_F(SimTest, DisassocFromApResultsInDisassocInd) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  client_ifc.AssociateWith(ap, zx::sec(1));
  env_->ScheduleNotification(
      std::bind(&simulation::FakeAp::DisassocSta, &ap, kStaMacAddr, kDisassocReason), zx::sec(2));

  env_->Run(kTestDuration);

  // Make sure association was successful
  ASSERT_EQ(client_ifc.stats_.connect_attempts, 1U);
  ASSERT_EQ(client_ifc.stats_.connect_results.size(), 1U);
  ASSERT_EQ(client_ifc.stats_.connect_results.front().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);

  // Make sure disassociation was successful
  EXPECT_EQ(ap.GetNumAssociatedClient(), 0U);

  // Verify that we get appropriate notification
  ASSERT_EQ(client_ifc.stats_.disassoc_indications.size(), 1U);
  const auto& disassoc_ind = client_ifc.stats_.disassoc_indications.front();
  // Verify reason code is propagated
  EXPECT_EQ(disassoc_ind.reason_code(), static_cast<wlan_ieee80211::ReasonCode>(kDisassocReason));
  // Disassociated by AP so not locally initiated
  ASSERT_TRUE(disassoc_ind.locally_initiated().has_value());
  EXPECT_FALSE(disassoc_ind.locally_initiated().value());
  // And we should see no other disconnects.
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, DeauthFromApResultsInDeauthInd) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  client_ifc.AssociateWith(ap, zx::sec(1));
  env_->ScheduleNotification(
      std::bind(&simulation::FakeAp::DeauthSta, &ap, kStaMacAddr, kDisassocReason), zx::sec(2));

  env_->Run(kTestDuration);

  // Make sure association was successful
  ASSERT_EQ(client_ifc.stats_.connect_attempts, 1U);
  ASSERT_EQ(client_ifc.stats_.connect_results.size(), 1U);
  ASSERT_EQ(client_ifc.stats_.connect_results.front().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);

  // Make sure deauth was successful
  EXPECT_EQ(ap.GetNumAssociatedClient(), 0U);

  // Verify that we get appropriate notification
  ASSERT_EQ(client_ifc.stats_.deauth_indications.size(), 1U);
  const auto& deauth_ind = client_ifc.stats_.deauth_indications.front();
  // Verify reason code is propagated
  EXPECT_EQ(deauth_ind.reason_code(), static_cast<wlan_ieee80211::ReasonCode>(kDeauthReason));
  // Deauthenticated by AP so not locally initiated
  EXPECT_EQ(deauth_ind.locally_initiated(), false);
  // And we should see no other disconnects.
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
}

TEST_F(SimTest, DisassocFromApWhileNotConnectedIsIgnored) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  // Simulate receipt of a disassoc ind from the AP by generating the appropriate event in
  // firmware, rather than using the fake AP. Here we test that the driver is resilient to an
  // unexpected disassoc ind; the fake AP will only send a disassoc ind if the client is
  // associated, which would result in this test passing despite no disassoc ind frame being
  // received by the client.
  const auto disassoc_reason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    env_->ScheduleNotification([&] { fw.TriggerFirmwareDisassoc(disassoc_reason); }, zx::sec(1));
  });

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, DeauthFromApWhileNotConnectedIsIgnored) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  // Simulate receipt of a deauth ind from the AP by generating the appropriate event in
  // firmware, rather than using the fake AP. As described above in
  // DisassocFromApWhileNotConnectedIsIgnored.
  const auto deauth_reason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    env_->ScheduleNotification([&] { fw.TriggerFirmwareDeauth(deauth_reason); }, zx::sec(1));
  });

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, DisassocFromUnexpectedBssWhileConnectedIsIgnored) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);
  client_ifc.AssociateWith(ap, zx::sec(1));

  // Simulate receipt of a disassoc ind from the AP by generating the appropriate event in
  // firmware, rather than using the fake AP.
  const auto disassoc_reason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
  const uint8_t unexpected_bssid[ETH_ALEN]{0x34, 0x28, 0x47, 0xaa, 0xab, 0xc3};
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    env_->ScheduleNotification(
        [&] { fw.TriggerFirmwareDisassocIndFromBssid(disassoc_reason, unexpected_bssid); },
        zx::sec(2));
  });

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, DeauthFromUnexpectedBssWhileConnectedIsIgnored) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);
  client_ifc.AssociateWith(ap, zx::sec(1));

  // Simulate receipt of a deauth ind from the AP by generating the appropriate event in
  // firmware, rather than using the fake AP.
  const auto deauth_reason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
  const uint8_t unexpected_bssid[ETH_ALEN]{0x34, 0x28, 0x47, 0xaa, 0xab, 0xc3};
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    env_->ScheduleNotification(
        [&] { fw.TriggerFirmwareDeauthIndFromBssid(deauth_reason, unexpected_bssid); }, zx::sec(2));
  });

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, DisassocFromSmeWhileNotConnectedResultsInDisassocConf) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  // Schedule a disassoc from SME
  env_->ScheduleNotification([&] { client_ifc.DisassociateFrom(kApBssid, kDisassocReason); },
                             zx::sec(0));

  env_->Run(kTestDuration);

  // Verify that we got the disassoc confirmation
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 1U);

  // And we should see no other disconnects.
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, DeauthFromSmeWhileNotConnectedResultsInDeauthConf) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  const auto deauth_reason = wlan_ieee80211::ReasonCode::kLeavingNetworkDisassoc;
  // Schedule a deauth from SME
  env_->ScheduleNotification([&] { client_ifc.DeauthenticateFrom(kApBssid, deauth_reason); },
                             zx::sec(1));

  env_->Run(kTestDuration);

  // Verify that we got the deauth confirmation
  ASSERT_EQ(client_ifc.stats_.deauth_results.size(), 1U);
  const auto& deauth_conf = client_ifc.stats_.deauth_results.front();
  ASSERT_TRUE(deauth_conf.peer_sta_address().has_value());
  ASSERT_EQ(ETH_ALEN, deauth_conf.peer_sta_address()->size());
  EXPECT_BYTES_EQ(deauth_conf.peer_sta_address()->data(), kApBssid.byte, ETH_ALEN);

  // And we should see no other disconnects.
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

// Verify that we properly track the disconnect mode which indicates if a disconnect was initiated
// by SME or not and what kind of disconnect it is. If this is not properly handled we could end up
// in a state where we are disconnected but SME doesn't know about it.
TEST_F(SimTest, SmeDeauthThenConnectThenFwDisassoc) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  client_ifc.AssociateWith(ap, zx::sec(1));
  const auto deauth_reason = wlan_ieee80211::ReasonCode::kLeavingNetworkDisassoc;
  // Schedule a deauth from SME
  env_->ScheduleNotification([&] { client_ifc.DeauthenticateFrom(kApBssid, deauth_reason); },
                             zx::sec(2));
  // Associate again
  client_ifc.AssociateWith(ap, zx::sec(3));
  // Schedule a disassociation from firmware
  const auto disassoc_reason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    // Note that this disassociation cannot go through SME, it has to be initiated by firmware so
    // that the disconnect mode tracking is not modified.
    env_->ScheduleNotification([&] { fw.TriggerFirmwareDisassoc(disassoc_reason); }, zx::sec(4));
  });

  env_->Run(kTestDuration);

  // Make sure associations were successful
  ASSERT_EQ(client_ifc.stats_.connect_attempts, 2U);
  ASSERT_EQ(client_ifc.stats_.connect_results.size(), 2U);
  ASSERT_EQ(client_ifc.stats_.connect_results.front().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);
  ASSERT_EQ(client_ifc.stats_.connect_results.back().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);

  // Make sure disassociation was successful
  EXPECT_EQ(ap.GetNumAssociatedClient(), 0U);

  // Verify that we got the deauth confirmation
  ASSERT_EQ(client_ifc.stats_.deauth_results.size(), 1U);
  const auto& deauth_conf = client_ifc.stats_.deauth_results.front();
  ASSERT_TRUE(deauth_conf.peer_sta_address().has_value());
  ASSERT_EQ(ETH_ALEN, deauth_conf.peer_sta_address()->size());
  EXPECT_BYTES_EQ(deauth_conf.peer_sta_address()->data(), kApBssid.byte, ETH_ALEN);

  // Verify that we got the disassociation indication, not a confirmation or anything else
  ASSERT_EQ(client_ifc.stats_.disassoc_indications.size(), 1U);
  const auto& disassoc_ind = client_ifc.stats_.disassoc_indications.front();
  ASSERT_TRUE(disassoc_ind.reason_code().has_value());
  EXPECT_EQ(disassoc_ind.reason_code(), static_cast<wlan_ieee80211::ReasonCode>(disassoc_reason));
  // Disassoc came from firmware, not AP/SME, so it is locally initiated.
  ASSERT_TRUE(disassoc_ind.locally_initiated().has_value());
  EXPECT_TRUE(disassoc_ind.locally_initiated().value());

  // And we should see no other disconnects.
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, SmeDisassocThenConnectThenFwDisassoc) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  client_ifc.AssociateWith(ap, zx::sec(1));
  // Schedule a disassoc from SME
  env_->ScheduleNotification([&] { client_ifc.DisassociateFrom(kApBssid, kDisassocReason); },
                             zx::sec(2));
  // Associate again
  client_ifc.AssociateWith(ap, zx::sec(3));
  // Schedule a disassociation from firmware
  const auto disassoc_reason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    // Note that this disassociation cannot go through SME, it has to be initiated by firmware so
    // that the disconnect mode tracking is not modified.
    env_->ScheduleNotification([&] { fw.TriggerFirmwareDisassoc(disassoc_reason); }, zx::sec(4));
  });

  env_->Run(kTestDuration);

  // Make sure associations were successful.
  ASSERT_EQ(client_ifc.stats_.connect_attempts, 2U);
  ASSERT_EQ(client_ifc.stats_.connect_results.size(), 2U);
  ASSERT_EQ(client_ifc.stats_.connect_results.front().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);
  ASSERT_EQ(client_ifc.stats_.connect_results.back().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);

  // Make sure final disconnect was successful.
  EXPECT_EQ(ap.GetNumAssociatedClient(), 0U);

  // Disassociated by SME during first connection. Make SME was notified.
  EXPECT_EQ(client_ifc.stats_.disassoc_results.size(), 1U);

  // Disassoc event during second connection. We can tell it was from the
  // second disconnect because of the reason code.
  ASSERT_EQ(client_ifc.stats_.disassoc_indications.size(), 1U);
  const auto& disassoc_ind = client_ifc.stats_.disassoc_indications.front();
  ASSERT_TRUE(disassoc_ind.reason_code().has_value());
  EXPECT_EQ(disassoc_ind.reason_code().value(), disassoc_reason);
  // Firmware-initiated disconnect with no SME-requested disconnect means
  // locally initiated.
  ASSERT_TRUE(disassoc_ind.locally_initiated().has_value());
  EXPECT_TRUE(disassoc_ind.locally_initiated().value());

  // And we should see no other disconnects.
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_indications.size(), 0U);
}

TEST_F(SimTest, SmeDisassocThenConnectThenFwDeauth) {
  simulation::FakeAp ap(env_.get(), kApBssid, kDefaultSsid, kApChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc, kStaMacAddr), ZX_OK);

  client_ifc.AssociateWith(ap, zx::sec(1));
  // Schedule a disassoc from SME
  env_->ScheduleNotification([&] { client_ifc.DisassociateFrom(kApBssid, kDisassocReason); },
                             zx::sec(2));
  // Associate again
  client_ifc.AssociateWith(ap, zx::sec(3));
  // Schedule a deauth from firmware
  const auto deauth_reason = wlan_ieee80211::ReasonCode::kInvalidClass2Frame;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    // Note that this deauth cannot go through SME, it has to be initiated by firmware so
    // that the disconnect mode tracking is not modified.
    env_->ScheduleNotification([&] { fw.TriggerFirmwareDeauth(deauth_reason); }, zx::sec(4));
  });

  env_->Run(kTestDuration);

  // Make sure associations were successful.
  ASSERT_EQ(client_ifc.stats_.connect_attempts, 2U);
  ASSERT_EQ(client_ifc.stats_.connect_results.size(), 2U);
  ASSERT_EQ(client_ifc.stats_.connect_results.front().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);
  ASSERT_EQ(client_ifc.stats_.connect_results.back().result_code(),
            wlan_ieee80211::StatusCode::kSuccess);

  // Make sure final disconnect was successful.
  EXPECT_EQ(ap.GetNumAssociatedClient(), 0U);

  // Disassociated by SME during first connection. Make sure SME was notified.
  ASSERT_EQ(client_ifc.stats_.disassoc_results.size(), 1U);

  // Deauth event during second connection manifests as a DeauthInd sent to SME.
  ASSERT_EQ(client_ifc.stats_.deauth_indications.size(), 1U);
  const auto& deauth_ind = client_ifc.stats_.deauth_indications.front();
  EXPECT_TRUE(deauth_ind.locally_initiated());
  ASSERT_EQ(deauth_ind.peer_sta_address()->size(), ETH_ALEN);
  ASSERT_BYTES_EQ(deauth_ind.peer_sta_address()->data(), kApBssid.byte, ETH_ALEN);

  // And we should see no other disconnects.
  EXPECT_EQ(client_ifc.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc.stats_.deauth_results.size(), 0U);
}

}  // namespace wlan::brcmfmac
