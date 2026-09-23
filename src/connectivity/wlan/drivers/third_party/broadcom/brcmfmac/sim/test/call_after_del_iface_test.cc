// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/errors.h>

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {

// Verify that a firmware scan result indication after the interface is stopped does
// not cause a failure.
TEST_F(SimTest, ScanResultAfterIfaceStop) {
  ASSERT_EQ(Init(), ZX_OK);

  SimInterface client_ifc;

  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc), ZX_OK);

  client_ifc.StartScan(0, true);
  // The scan result will arrive after the iface is torn down.
  env_->Run(zx::sec(1));  // This should be a no-op, not a crash.

  EXPECT_EQ(DeleteInterface(&client_ifc), ZX_OK);
}

// Verify that calling WlanPhyDestroyIface() will return a failure if the firmware returns an
// error on iface removal.
TEST_F(SimTest, DeleteIfaceOnFirmwareFailure) {
  ASSERT_EQ(Init(), ZX_OK);

  SimInterface softap_ifc;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kAp, &softap_ifc), ZX_OK);

  // Inject firmware error to "interface_remove" iovar.
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("interface_remove", ZX_OK, BCME_ERROR,
                                         softap_ifc.iface_id_);
  });

  EXPECT_EQ(DeleteInterface(&softap_ifc), ZX_ERR_IO_REFUSED);
}

}  // namespace wlan::brcmfmac
