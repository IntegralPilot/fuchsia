// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-sta-ifc.h"

// Verify that signal strengths are being properly calculated and delivered by the environment

using ::wlan::common::MacAddr;

namespace wlan::testing {
namespace {

constexpr zx::duration kSimulatedClockDuration = zx::sec(10);

}  // namespace

using ::testing::NotNull;

constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};

constexpr simulation::WlanTxInfo kDefaultTxInfo = {
    .primary_channel = kDefaultChannel,
    .bandwidth = fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20,
    .vht_secondary_80_channel = {.band = kDefaultChannel.band, .number = 0}};
const fuchsia_wlan_ieee80211::Ssid kDefaultSsid = {'F', 'u', 'c', 'h', 's', 'i', 'a', ' ',
                                                   'F', 'a', 'k', 'e', ' ', 'A', 'P'};
const MacAddr kDefaultBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});

void checkChannel(const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel,
                  fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw,
                  const fuchsia_wlan_ieee80211::wire::ChannelNumber& secondary80) {
  EXPECT_EQ(channel.number, kDefaultTxInfo.primary_channel.number);
  EXPECT_EQ(channel.band, kDefaultTxInfo.primary_channel.band);
  EXPECT_EQ(cbw, kDefaultTxInfo.bandwidth);
  EXPECT_EQ(secondary80.number, kDefaultTxInfo.vht_secondary_80_channel.number);
  EXPECT_EQ(secondary80.band, kDefaultTxInfo.vht_secondary_80_channel.band);
}

class SimStation : public wlan::simulation::StationIfc {
 public:
  SimStation() {
    std::memset(mac_addr_.byte, 0, common::kMacAddrLen - 1);
    mac_addr_.byte[common::kMacAddrLen - 1] = instance_count++;
  }

  // StationIfc methods
  void Rx(std::shared_ptr<const simulation::SimFrame> frame,
          std::shared_ptr<const simulation::WlanRxInfo> info) override {
    checkChannel(info->primary, info->bandwidth, info->vht_secondary_80_channel);
    switch (frame->FrameType()) {
      case simulation::SimFrame::FRAME_TYPE_MGMT: {
        auto mgmt_frame = std::static_pointer_cast<const simulation::SimManagementFrame>(frame);
        RxMgmtFrame(mgmt_frame, info);
        break;
      }

      default:
        break;
    }
  }

  void RxMgmtFrame(std::shared_ptr<const simulation::SimManagementFrame> mgmt_frame,
                   std::shared_ptr<const simulation::WlanRxInfo> info) {
    switch (mgmt_frame->MgmtFrameType()) {
      case simulation::SimManagementFrame::FRAME_TYPE_BEACON: {
        auto beacon_frame = std::static_pointer_cast<const simulation::SimBeaconFrame>(mgmt_frame);
        std::shared_ptr<simulation::InformationElement> ssid_generic_ie =
            beacon_frame->FindIe(simulation::InformationElement::IE_TYPE_SSID);
        ASSERT_THAT(ssid_generic_ie, NotNull());
        auto ssid_ie =
            std::static_pointer_cast<simulation::SsidInformationElement>(ssid_generic_ie);
        EXPECT_EQ(ssid_ie->ssid_, kDefaultSsid);
        EXPECT_EQ(beacon_frame->bssid_, kDefaultBssid);
        signal_received = true;
        signal_strength = info->signal_strength;
        break;
      }
      default:
        break;
    }
  }

  static uint8_t instance_count;
  MacAddr mac_addr_;
  bool signal_received = false;
  double signal_strength = 0;
};

uint8_t SimStation::instance_count = 0;

class LocationTest : public ::testing::Test {
 public:
  LocationTest();
  ~LocationTest();

  simulation::Environment env_;
  std::array<SimStation, 3> stations_{};
};

LocationTest::LocationTest() {
  for (auto sta = stations_.begin(); sta != stations_.end(); sta++) {
    env_.AddStation(&*sta);
  }
}

LocationTest::~LocationTest() {
  for (auto sta = stations_.begin(); sta != stations_.end(); sta++) {
    env_.RemoveStation(&*sta);
  }
}

// Initially, station 0 is closest to station 1.
// After the second move, station 0 is closer to station 2 than station 1.
TEST_F(LocationTest, BasicSignalStrengthTest) {
  env_.MoveStation(&stations_[0], 0, 0);
  env_.MoveStation(&stations_[1], 0, 10);
  env_.MoveStation(&stations_[2], 20, 0);

  simulation::SimBeaconFrame beacon_frame(kDefaultSsid, kDefaultBssid);
  env_.Tx(beacon_frame, kDefaultTxInfo, &stations_[0]);
  env_.Run(kSimulatedClockDuration);
  EXPECT_LT(stations_[1].signal_strength, 0);
  EXPECT_LT(stations_[2].signal_strength, 0);
  EXPECT_LT(stations_[2].signal_strength, stations_[1].signal_strength);

  env_.MoveStation(&stations_[0], 15, 0);

  env_.Tx(beacon_frame, kDefaultTxInfo, &stations_[0]);
  env_.Run(kSimulatedClockDuration);
  EXPECT_LT(stations_[1].signal_strength, 0);
  EXPECT_LT(stations_[2].signal_strength, 0);
  EXPECT_LT(stations_[1].signal_strength, stations_[2].signal_strength);
}

}  // namespace wlan::testing
