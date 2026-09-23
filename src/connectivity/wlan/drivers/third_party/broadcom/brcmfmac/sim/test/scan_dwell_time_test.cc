// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/clock.h>

#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

// Fake AP configuration
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};
const MacAddr kDefaultBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});
constexpr zx::duration kBeaconInterval = zx::msec(SimInterface::kDefaultPassiveScanDwellTimeMs - 1);

// How many scans we will run. Each time we will expect to see a beacon from the fake AP.
constexpr size_t kTotalScanCount = 10;

class ScanTest : public SimTest {
 public:
  ScanTest() = default;
  void Init();

 protected:
  // This is the interface we will use for our single client interface
  SimInterface client_ifc_;
};

void ScanTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
}

TEST_F(ScanTest, PassiveDwellTime) {
  constexpr zx::duration kScanStartTime = zx::sec(1);

  // A scan should roughly take dwell time * # of channels being scanned. Double that, just to
  // be sure we have enough time to complete.
  const zx::duration kScanMaxTime = zx::msec(SimInterface::kDefaultScanChannels.size() *
                                             SimInterface::kDefaultPassiveScanDwellTimeMs * 2);

  // Create our simulated device
  Init();

  // Start up a single AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(kBeaconInterval);

  for (size_t scan_attempt = 0; scan_attempt < kTotalScanCount; scan_attempt++) {
    zx_time_t start_timestamp = zx::clock::get_monotonic().get();
    env_->ScheduleNotification(
        std::bind(&SimInterface::StartScan, &client_ifc_, scan_attempt, false,
                  std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>>{}),
        kScanStartTime);
    env_->Run(kScanMaxTime);

    // Check scan result code
    auto scan_result_code = client_ifc_.ScanResultCode(scan_attempt);
    EXPECT_TRUE(scan_result_code);

    // Check list of bsses seen
    EXPECT_EQ(*scan_result_code, wlan_fullmac_wire::WlanScanResult::kSuccess);
    auto scan_result_list = client_ifc_.ScanResultList(scan_attempt);
    EXPECT_GT(scan_result_list->size(), 0U);
    for (const fuchsia_wlan_fullmac::WlanFullmacImplIfcOnScanResultRequest& scan_result :
         *scan_result_list) {
      auto& bss = scan_result.bss();
      EXPECT_EQ(kDefaultBssid, MacAddr(bss->bssid().data()));
      auto ssid = brcmf_find_ssid_in_ies(bss->ies().data(), bss->ies().size());
      EXPECT_EQ(kDefaultSsid, ssid);
      EXPECT_EQ(kDefaultChannel.number, bss->primary().number());
      EXPECT_GT(scan_result.timestamp_nanos(), start_timestamp);
    }
  }
}

}  // namespace wlan::brcmfmac
