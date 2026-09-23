// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/errors.h>

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {
namespace {

constexpr zx::duration kSimulatedClockDuration = zx::sec(10);

}  // namespace

constexpr uint64_t kScanTxnId = 0x4a65616e6e65;
const fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultChannelsList[11] = {
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 1},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 2},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 3},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 4},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 5},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 6},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 7},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 8},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 10},
    {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 11},
};

// For this test, we don't want to use the default scan handlers provided by SimInterface
class EscanArgsIfc : public SimInterface {
 public:
  void OnScanEnd(OnScanEndRequestView request, OnScanEndCompleter::Sync& completer) override;
  bool ScanCompleted() { return scan_completed_; }
  wlan_fullmac_wire::WlanScanResult ScanResult() { return scan_result_; }

 private:
  bool scan_completed_ = false;
  wlan_fullmac_wire::WlanScanResult scan_result_;
};

void EscanArgsIfc::OnScanEnd(OnScanEndRequestView request, OnScanEndCompleter::Sync& completer) {
  EXPECT_EQ(request->txn_id(), kScanTxnId);
  scan_completed_ = true;
  scan_result_ = request->code();
  completer.Reply();
}

class EscanArgsTest : public SimTest {
 public:
  void Init();
  void RunScanTest(const wlan_fullmac_wire::WlanFullmacImplStartScanRequest& req);

 protected:
  EscanArgsIfc client_ifc_;
};

void EscanArgsTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
}

void EscanArgsTest::RunScanTest(const wlan_fullmac_wire::WlanFullmacImplStartScanRequest& req) {
  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->StartScan(req);
  ASSERT_TRUE(result.ok());
  env_->Run(kSimulatedClockDuration);
  ASSERT_TRUE(client_ifc_.ScanCompleted());
}

// Verify that invalid scan params result in a failed scan result
TEST_F(EscanArgsTest, BadScanArgs) {
  Init();
  {
    auto builder =
        wlan_fullmac_wire::WlanFullmacImplStartScanRequest::Builder(client_ifc_.test_arena_);

    builder.txn_id(kScanTxnId);
    builder.scan_type(wlan_fullmac_wire::WlanScanType::kActive);
    builder.channels(fidl::VectorView<fuchsia_wlan_ieee80211::wire::ChannelNumber>::FromExternal(
        const_cast<fuchsia_wlan_ieee80211::wire::ChannelNumber*>(kDefaultChannelsList), 11));
    builder.min_channel_time(0);
    builder.max_channel_time(0);

    // Dwell time of zero
    RunScanTest(builder.Build());
  }
  EXPECT_NE(client_ifc_.ScanResult(), wlan_fullmac_wire::WlanScanResult::kSuccess);

  // min dwell time > max dwell time
  {
    auto builder =
        wlan_fullmac_wire::WlanFullmacImplStartScanRequest::Builder(client_ifc_.test_arena_);

    builder.txn_id(kScanTxnId);
    builder.scan_type(wlan_fullmac_wire::WlanScanType::kActive);
    builder.channels(fidl::VectorView<fuchsia_wlan_ieee80211::wire::ChannelNumber>::FromExternal(
        const_cast<fuchsia_wlan_ieee80211::wire::ChannelNumber*>(kDefaultChannelsList), 11));
    builder.min_channel_time(SimInterface::kDefaultActiveScanDwellTimeMs + 1);
    builder.max_channel_time(SimInterface::kDefaultActiveScanDwellTimeMs);

    // Dwell time of zero
    RunScanTest(builder.Build());
  }
  EXPECT_NE(client_ifc_.ScanResult(), wlan_fullmac_wire::WlanScanResult::kSuccess);
}

TEST_F(EscanArgsTest, EmptyChannelList) {
  Init();
  auto builder =
      wlan_fullmac_wire::WlanFullmacImplStartScanRequest::Builder(client_ifc_.test_arena_);

  builder.txn_id(kScanTxnId), builder.scan_type(wlan_fullmac_wire::WlanScanType::kActive),
      builder.min_channel_time(SimInterface::kDefaultActiveScanDwellTimeMs + 1);
  builder.max_channel_time(SimInterface::kDefaultActiveScanDwellTimeMs);

  RunScanTest(builder.Build());
  EXPECT_EQ(client_ifc_.ScanResult(), wlan_fullmac_wire::WlanScanResult::kInvalidArgs);
}

}  // namespace wlan::brcmfmac
