// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/function.h>
#include <lib/sync/completion.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_fw.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {
namespace {

class PhyResetTest : public SimTest {
 protected:
  PhyResetTest() = default;

  void SetUp() override { EXPECT_EQ(SimTest::Init(), ZX_OK); }

  void SetSuspendHook(fit::function<zx_status_t()> hook) {
    WithSimDevice([hook = std::move(hook)](SimDevice* device) mutable {
      device->GetSim()->sim_fw->SetSuspendHook(std::move(hook));
    });
  }

  void SetResumeHook(fit::function<zx_status_t()> hook) {
    WithSimDevice([hook = std::move(hook)](SimDevice* device) mutable {
      device->GetSim()->sim_fw->SetResumeHook(std::move(hook));
    });
  }

  zx_status_t CreateInterface(wlan_common::WlanMacRole role, SimInterface* ifc) {
    return StartInterface(role, ifc);
  }
};

TEST_F(PhyResetTest, SuspendResumeHooksCalled) {
  std::atomic<bool> suspend_called = false;
  std::atomic<bool> resume_called = false;

  SetSuspendHook([&]() {
    suspend_called = true;
    return ZX_OK;
  });

  SetResumeHook([&]() {
    resume_called = true;
    return ZX_OK;
  });

  auto res = client_->Reset();
  EXPECT_TRUE(res.is_ok());

  EXPECT_TRUE(suspend_called);
  EXPECT_TRUE(resume_called);
}

TEST_F(PhyResetTest, ResetDestroysExistingInterface) {
  SimInterface client_ifc_;
  EXPECT_EQ(CreateInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);

  uint32_t count = DeviceCount();
  EXPECT_GT(count, 0);

  auto res = client_->Reset();
  EXPECT_TRUE(res.is_ok());

  WaitForDeviceCount(count - 1);

  // Inform the sim framework that the interface was destroyed.
  SimTest::InterfaceDestroyed(&client_ifc_);

  // Then check that we can recreate the iface.
  EXPECT_EQ(CreateInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
}

TEST_F(PhyResetTest, ConsecutiveResets) {
  auto res = client_->Reset();
  EXPECT_TRUE(res.is_ok());

  res = client_->Reset();
  EXPECT_TRUE(res.is_ok());
}

}  // namespace
}  // namespace wlan::brcmfmac
