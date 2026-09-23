// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {
namespace {

constexpr zx::duration kSimulatedClockDuration = zx::sec(10);

}  // namespace

// Verify that we can do an active scan even if the firmware doesn't support randomized mac
// addresses.
TEST_F(SimTest, RandomMacNotSupported) {
  constexpr uint64_t kScanTxnId = 42;

  ASSERT_EQ(PreInit(), ZX_OK);

  // Force failure in the iovar used to set a random mac address
  WithSimDevice([](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("pfn_macaddr", ZX_ERR_IO, BCME_OK);
  });

  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc), ZX_OK);

  env_->ScheduleNotification(
      std::bind(&SimInterface::StartScan, &client_ifc, kScanTxnId, true,
                std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>>{}),
      zx::sec(1));
  env_->Run(kSimulatedClockDuration);

  auto scan_result = client_ifc.ScanResultCode(kScanTxnId);

  // Verify scan completed
  EXPECT_TRUE(scan_result);

  // Verify that scan was successful
  EXPECT_EQ(*scan_result, wlan_fullmac_wire::WlanScanResult::kSuccess);
}

}  // namespace wlan::brcmfmac
