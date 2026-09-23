// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/errors.h>

#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

// Some default AP and association request values
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kSwitchedChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 20};
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kSecondSwitchedChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 30};
const uint16_t kDefaultCSACount = 3;
const MacAddr kDefaultBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});

class ChannelSwitchTest : public SimTest {
 public:
  // How long an individual test will run for. We need an end time because tests run until no more
  // events remain and so we need to stop aps from beaconing to drain the event queue.
  static constexpr zx::duration kTestDuration = zx::sec(100);

  void Init();

  // Schedule a future SetChannel event for the first AP in list
  void ScheduleChannelSwitch(const fuchsia_wlan_ieee80211::wire::ChannelNumber& new_channel,
                             zx::duration when);

  // Send one fake CSA beacon using the identification consistent of default ssid and bssid.
  void SendFakeCSABeacon(const fuchsia_wlan_ieee80211::wire::ChannelNumber& dst_channel);

 protected:
  // Number of received assoc responses.
  size_t assoc_resp_count_ = 0;

  // This is the interface we will use for our single client interface
  SimInterface client_ifc_;
  std::list<simulation::FakeAp*> aps_;
};

void ChannelSwitchTest::SendFakeCSABeacon(
    const fuchsia_wlan_ieee80211::wire::ChannelNumber& dst_channel) {
  const simulation::WlanTxInfo kDefaultTxInfo = {
      .primary_channel = kDefaultChannel,
      .bandwidth = wlan_ieee80211::ChannelBandwidth::kCbw20,
      .vht_secondary_80_channel = {.band = kDefaultChannel.band, .number = 0}};

  simulation::SimBeaconFrame fake_csa_beacon(kDefaultSsid, kDefaultBssid);
  fake_csa_beacon.AddCsaIe(dst_channel, kDefaultCSACount);

  env_->Tx(fake_csa_beacon, kDefaultTxInfo, this);
}

// Create our device instance and hook up the callbacks
void ChannelSwitchTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
}

// This function schedules a Setchannel() event for the first AP in AP list.
void ChannelSwitchTest::ScheduleChannelSwitch(
    const fuchsia_wlan_ieee80211::wire::ChannelNumber& new_channel, zx::duration when) {
  env_->ScheduleNotification(
      [ap = aps_.front(), new_channel] {
        ap->SetChannel(new_channel, fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
      },
      when);
}

TEST_F(ChannelSwitchTest, ChannelSwitch) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));
  ScheduleChannelSwitch(kSwitchedChannel, zx::msec(500));
  env_->Run(kTestDuration);

  // Channel switch will only be triggered when associated.
  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 1U);
  EXPECT_EQ(client_ifc_.stats_.csa_indications.front().new_primary_channel.number,
            kSwitchedChannel.number);
}

// This test case verifies that in a single CSA beacon interval, if AP want to switch back to old
// channel, the client will simply cancel this switch.
TEST_F(ChannelSwitchTest, SwitchBackInSingleInterval) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));
  ScheduleChannelSwitch(kSwitchedChannel, zx::msec(500));
  ScheduleChannelSwitch(kDefaultChannel, zx::msec(550));

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 0U);
}

// This test verifies that in a single CSA beacon interval, if sim-fake-ap change destination
// channel to change to, sim-fw only send up one channel switch event with the newest dst channel.
TEST_F(ChannelSwitchTest, ChangeDstAddressWhenSwitching) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));
  ScheduleChannelSwitch(kSwitchedChannel, zx::msec(500));
  ScheduleChannelSwitch(kSecondSwitchedChannel, zx::msec(550));

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 1U);
  EXPECT_EQ(client_ifc_.stats_.csa_indications.front().new_primary_channel.number,
            kSecondSwitchedChannel.number);
}

// This test case verifies that two continuous channel switches will work as long as they are in two
// separate CSA beacon intervals of AP, which means when AP's channel actually changed.
TEST_F(ChannelSwitchTest, SwitchBackInDiffInterval) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));
  ScheduleChannelSwitch(kSwitchedChannel, zx::msec(500));
  // The default CSA beacon interval is 150 msec, so when it comes to 700 msec, it's the second time
  // CSA is triggered.
  ScheduleChannelSwitch(kDefaultChannel, zx::msec(700));

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 2U);
  EXPECT_EQ(client_ifc_.stats_.csa_indications.front().new_primary_channel.number,
            kSwitchedChannel.number);
  client_ifc_.stats_.csa_indications.pop_front();
  EXPECT_EQ(client_ifc_.stats_.csa_indications.front().new_primary_channel.number,
            kDefaultChannel.number);
}

TEST_F(ChannelSwitchTest, ChannelSwitchWithSpuriousReassocAndRoamEvents) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));
  ScheduleChannelSwitch(kSwitchedChannel, zx::msec(500));
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    SimFirmware& fw = *device->GetSim()->sim_fw;
    env_->ScheduleNotification([&] { fw.TriggerFirmwareReassocEvent(kDefaultBssid); },
                               zx::msec(501));
    env_->ScheduleNotification([&] { fw.TriggerFirmwareRoamEvent(kDefaultBssid); }, zx::msec(502));
  });

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 1U);
  EXPECT_EQ(client_ifc_.stats_.csa_indications.front().new_primary_channel.number,
            kSwitchedChannel.number);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  EXPECT_EQ(0U, client_ifc_.stats_.roam_attempts);
  EXPECT_EQ(0U, client_ifc_.stats_.roam_result_indications.size());
  EXPECT_EQ(0U, client_ifc_.stats_.roam_confirmations.size());
  EXPECT_EQ(SimInterface::AssocContext::kAssociated, client_ifc_.assoc_ctx_.state);
}

// This test verifies CSA beacons from APs which are not associated with client will not trigger
// channel switch event in driver.
TEST_F(ChannelSwitchTest, NotSwitchForDifferentAP) {
  const fuchsia_wlan_ieee80211::Ssid kWrongSsid = {'F', 'u', 'c', 'h', 's', 'i', 'a',
                                                   ' ', 'F', 'a', 'k', 'e', ' ', 'A'};
  ASSERT_NE(kDefaultSsid.size(), kWrongSsid.size());
  const MacAddr kWrongBssid({0x12, 0x34, 0x56, 0x78, 0x9b, 0xbc});
  ASSERT_NE(kDefaultBssid, kWrongBssid);

  Init();

  simulation::FakeAp wrong_ap(env_.get(), kWrongBssid, kWrongSsid, kDefaultChannel,
                              fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  wrong_ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&wrong_ap);

  simulation::FakeAp right_ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                              fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  right_ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&right_ap);

  client_ifc_.AssociateWith(right_ap, zx::msec(10));

  // This will trigger SetChannel() for first AP in AP list, which is wrong_ap.
  ScheduleChannelSwitch(kSwitchedChannel, zx::msec(500));

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 0U);
}

// This test case verifies that when AP stop beaconing during CSA beacon interval, sim-fw will still
// change its channel. AP will change the change immediately when beacon is stopped.
TEST_F(ChannelSwitchTest, StopStillSwitch) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));
  ScheduleChannelSwitch(kSwitchedChannel, zx::msec(500));

  // Schedule DisableBeacon for sim-fake-ap
  env_->ScheduleNotification(std::bind(&simulation::FakeAp::DisableBeacon, aps_.front()),
                             zx::msec(600));

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 1U);
  EXPECT_EQ(client_ifc_.stats_.csa_indications.front().new_primary_channel.number,
            kSwitchedChannel.number);
}

// This test verifies that the CSA beacon will be ignored when its new channel is the same as
// sim-fw's current channel.
TEST_F(ChannelSwitchTest, ChannelSwitchToSameChannel) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));

  // SendFakeCSABeacon() is using the ssid and bssid of the AP which client is associated to.
  env_->ScheduleNotification(
      std::bind(&ChannelSwitchTest::SendFakeCSABeacon, this, kDefaultChannel), zx::msec(540));

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 0U);
}

// This test verifies that the CSA beacon will be ignored when client is doing a passive scan while
// associated with an AP.
TEST_F(ChannelSwitchTest, ChannelSwitchWhileScanning) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap);

  client_ifc_.AssociateWith(ap, zx::msec(10));

  constexpr uint32_t kScanStartTimeMs = 20;
  env_->ScheduleNotification(
      std::bind(&SimInterface::StartScan, &client_ifc_, 0, false,
                std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>>{}),
      zx::msec(kScanStartTimeMs));

  constexpr uint32_t kCsaBeaconDelayMs =
      kScanStartTimeMs + (SimInterface::kDefaultPassiveScanDwellTimeMs / 2);
  env_->ScheduleNotification(
      std::bind(&ChannelSwitchTest::SendFakeCSABeacon, this, kSwitchedChannel),
      zx::msec(kCsaBeaconDelayMs));

  env_->Run(kTestDuration);

  EXPECT_EQ(client_ifc_.stats_.csa_indications.size(), 0U);
}

}  // namespace wlan::brcmfmac
