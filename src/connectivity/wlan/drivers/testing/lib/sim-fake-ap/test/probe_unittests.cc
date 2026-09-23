// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-frame.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-sta-ifc.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"

using ::wlan::common::MacAddr;

namespace wlan::testing {
namespace {

constexpr zx::duration kSimulatedClockDuration = zx::sec(10);

}  // namespace

using ::testing::NotNull;

constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kAp1Channel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};
constexpr simulation::WlanTxInfo kAp1TxInfo = {
    .primary_channel = kAp1Channel,
    .bandwidth = wlan_ieee80211::ChannelBandwidth::kCbw20,
    .vht_secondary_80_channel = {.band = kAp1Channel.band, .number = 0}};
const fuchsia_wlan_ieee80211::Ssid kAp1Ssid = {'F', 'u', 'c', 'h', 's', 'i', 'a', ' ',
                                               'F', 'a', 'k', 'e', ' ', 'A', 'P', '1'};
const MacAddr kAp1Bssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});

constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kAp2Channel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 10};
constexpr simulation::WlanTxInfo kAp2TxInfo = {
    .primary_channel = kAp2Channel,
    .bandwidth = wlan_ieee80211::ChannelBandwidth::kCbw20,
    .vht_secondary_80_channel = {.band = kAp2Channel.band, .number = 0}};
const fuchsia_wlan_ieee80211::Ssid kAp2Ssid = {'F', 'u', 'c', 'h', 's', 'i', 'a', ' ',
                                               'F', 'a', 'k', 'e', ' ', 'A', 'P', '2'};
const MacAddr kAp2Bssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xcc});

const MacAddr kClientMacAddr({0x11, 0x22, 0x33, 0x44, 0xee, 0xff});

class ProbeTest : public ::testing::Test, public simulation::StationIfc {
 public:
  ProbeTest()
      : ap_1_(&env_, kAp1Bssid, kAp1Ssid, kAp1Channel, wlan_ieee80211::ChannelBandwidth::kCbw20,
              {.band = kAp1Channel.band, .number = 0}),
        ap_2_(&env_, kAp2Bssid, kAp2Ssid, kAp2Channel, wlan_ieee80211::ChannelBandwidth::kCbw20,
              {.band = kAp2Channel.band, .number = 0}) {
    env_.AddStation(this);
  }

  simulation::Environment env_;
  simulation::FakeAp ap_1_;
  simulation::FakeAp ap_2_;

  unsigned probe_resp_count_ = 0;
  std::list<MacAddr> bssid_resp_list_;
  std::list<fuchsia_wlan_ieee80211::Ssid> ssid_resp_list_;
  std::list<fuchsia_wlan_ieee80211::wire::ChannelNumber> channel_resp_list_;
  std::list<double> sig_strength_resp_list;

 private:
  // StationIfc methods
  void Rx(std::shared_ptr<const simulation::SimFrame> frame,
          std::shared_ptr<const simulation::WlanRxInfo> info) override;
};

void ProbeTest::Rx(std::shared_ptr<const simulation::SimFrame> frame,
                   std::shared_ptr<const simulation::WlanRxInfo> info) {
  ASSERT_EQ(frame->FrameType(), simulation::SimFrame::FRAME_TYPE_MGMT);

  auto mgmt_frame = std::static_pointer_cast<const simulation::SimManagementFrame>(frame);

  if (mgmt_frame->MgmtFrameType() != simulation::SimManagementFrame::FRAME_TYPE_PROBE_RESP) {
    GTEST_FAIL();
  }

  probe_resp_count_++;
  channel_resp_list_.push_back(info->primary);
  sig_strength_resp_list.push_back(info->signal_strength);
  auto probe_resp_frame = std::static_pointer_cast<const simulation::SimProbeRespFrame>(mgmt_frame);
  bssid_resp_list_.push_back(probe_resp_frame->src_addr_);
  std::shared_ptr<simulation::InformationElement> ssid_generic_ie =
      probe_resp_frame->FindIe(simulation::InformationElement::IE_TYPE_SSID);
  ASSERT_THAT(ssid_generic_ie, NotNull());
  auto ssid_ie = std::static_pointer_cast<simulation::SsidInformationElement>(ssid_generic_ie);
  ssid_resp_list_.push_back(ssid_ie->ssid_);
}

void compareChannel(const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel1,
                    const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel2) {
  EXPECT_EQ(channel1.number, channel2.number);
  EXPECT_EQ(channel1.band, channel2.band);
}

void compareSsid(const fuchsia_wlan_ieee80211::Ssid& ssid1,
                 const fuchsia_wlan_ieee80211::Ssid& ssid2) {
  EXPECT_EQ(ssid1, ssid2);
}

/* Verify that probe request which is sent to a channel with no ap active on, will not get
   any response.
 */
TEST_F(ProbeTest, DifferentChannel) {
  constexpr simulation::WlanTxInfo kWrongChannelTxInfo = {
      .primary_channel = {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 11},
      .bandwidth = wlan_ieee80211::ChannelBandwidth::kCbw20,
      .vht_secondary_80_channel = {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz,
                                   .number = 0}};

  simulation::SimProbeReqFrame probe_req_frame(kClientMacAddr);
  env_.ScheduleNotification(
      std::bind(&simulation::Environment::Tx, &env_, probe_req_frame, kWrongChannelTxInfo, this),
      zx::sec(1));

  env_.Run(kSimulatedClockDuration);
  EXPECT_EQ(probe_resp_count_, 0U);
  EXPECT_EQ(channel_resp_list_.empty(), true);
  EXPECT_EQ(bssid_resp_list_.empty(), true);
  EXPECT_EQ(ssid_resp_list_.empty(), true);
  EXPECT_EQ(sig_strength_resp_list.empty(), true);
}

/* Verify that probe requests sent to different APs on different channels will get appropriate
   responses and in right sequence. Signal strengths are reasonable and weaker for further AP.

   Timeline for this test:
   100 usec: send probe request to First channel.
   200 usec: send probe request to second channel.
   */
TEST_F(ProbeTest, TwoApsBasicUse) {
  env_.MoveStation(this, 0, 0);
  env_.MoveStation(&ap_1_, 0, 0);
  env_.MoveStation(&ap_2_, 10, 0);

  simulation::SimProbeReqFrame chan1_frame(kClientMacAddr);
  env_.ScheduleNotification(
      std::bind(&simulation::Environment::Tx, &env_, chan1_frame, kAp1TxInfo, this), zx::usec(100));

  env_.Run(kSimulatedClockDuration);
  EXPECT_EQ(probe_resp_count_, 1U);

  simulation::SimProbeReqFrame chan2_frame(kClientMacAddr);
  env_.ScheduleNotification(
      std::bind(&simulation::Environment::Tx, &env_, chan2_frame, kAp2TxInfo, this), zx::usec(200));

  env_.Run(kSimulatedClockDuration);
  EXPECT_EQ(probe_resp_count_, 2U);

  ASSERT_EQ(bssid_resp_list_.size(), (size_t)2);
  ASSERT_EQ(ssid_resp_list_.size(), (size_t)2);
  ASSERT_EQ(channel_resp_list_.size(), (size_t)2);
  ASSERT_EQ(sig_strength_resp_list.size(), (size_t)2);

  EXPECT_EQ(bssid_resp_list_.front(), kAp1Bssid);
  bssid_resp_list_.pop_front();
  EXPECT_EQ(bssid_resp_list_.front(), kAp2Bssid);

  compareSsid(ssid_resp_list_.front(), kAp1Ssid);
  ssid_resp_list_.pop_front();
  compareSsid(ssid_resp_list_.front(), kAp2Ssid);

  compareChannel(channel_resp_list_.front(), kAp1TxInfo.primary_channel);
  channel_resp_list_.pop_front();
  compareChannel(channel_resp_list_.front(), kAp2TxInfo.primary_channel);

  double close_signal_strength = sig_strength_resp_list.front();
  sig_strength_resp_list.pop_front();
  ASSERT_LT(close_signal_strength, 0);
  ASSERT_GE(close_signal_strength, sig_strength_resp_list.front());
}

}  // namespace wlan::testing
