// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/wlan_interface.h"

namespace wlan::brcmfmac {

class PhyMacRolesTest : public SimTest {
 public:
  PhyMacRolesTest() = default;
  void Init();
};

void PhyMacRolesTest::Init() { ASSERT_EQ(SimTest::Init(), ZX_OK); }

TEST_F(PhyMacRolesTest, VerifyMacRoles) {
  Init();
  auto result = client_->GetSupportedMacRoles();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(result->supported_mac_roles()->size(), 2u);
  EXPECT_EQ(result->supported_mac_roles().value()[0], fuchsia_wlan_common::WlanMacRole::kClient);
  EXPECT_EQ(result->supported_mac_roles().value()[1], fuchsia_wlan_common::WlanMacRole::kAp);
}

}  // namespace wlan::brcmfmac
