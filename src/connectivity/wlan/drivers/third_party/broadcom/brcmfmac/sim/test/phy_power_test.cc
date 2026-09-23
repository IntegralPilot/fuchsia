// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/errors.h>

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/common.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/defs.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {

class PhyPowerTest : public SimTest {
 public:
  PhyPowerTest() = default;
  void Init();
  zx_status_t PowerDown();
  zx_status_t PowerUp();
  zx_status_t Reset();
  zx_status_t GetPowerState(bool* power_on);

 private:
};

void PhyPowerTest::Init() { ASSERT_EQ(SimTest::Init(), ZX_OK); }

zx_status_t PhyPowerTest::PowerDown() {
  auto result = client_->PowerDown();
  if (result.is_error()) {
    return result.error_value().is_domain_error() ? result.error_value().domain_error()
                                                  : result.error_value().framework_error().status();
  }
  return ZX_OK;
}

zx_status_t PhyPowerTest::PowerUp() {
  auto result = client_->PowerUp();
  if (result.is_error()) {
    return result.error_value().is_domain_error() ? result.error_value().domain_error()
                                                  : result.error_value().framework_error().status();
  }
  return ZX_OK;
}

zx_status_t PhyPowerTest::Reset() {
  auto result = client_->Reset();
  if (result.is_error()) {
    return result.error_value().is_domain_error() ? result.error_value().domain_error()
                                                  : result.error_value().framework_error().status();
  }
  return ZX_OK;
}

zx_status_t PhyPowerTest::GetPowerState(bool* power_on) {
  auto result = client_->GetPowerState();
  if (result.is_error()) {
    return result.error_value().is_domain_error() ? result.error_value().domain_error()
                                                  : result.error_value().framework_error().status();
  }
  return ZX_OK;
}

// Test PowerDown
TEST_F(PhyPowerTest, PowerDownSuccess) {
  zx_status_t status;

  Init();

  // Set a valid PS mode and verify it succeeds
  status = PowerDown();
  ASSERT_EQ(status, ZX_OK);
}

TEST_F(PhyPowerTest, PowerUpSuccess) {
  zx_status_t status;

  Init();

  // Set a valid PS mode and verify it succeeds
  status = PowerUp();
  ASSERT_EQ(status, ZX_OK);
}

TEST_F(PhyPowerTest, ResetSuccess) {
  zx_status_t status;

  Init();

  // Set a valid PS mode and verify it succeeds
  status = Reset();
  ASSERT_EQ(status, ZX_OK);
}

TEST_F(PhyPowerTest, GetPowerStateSuccess) {
  zx_status_t status;

  Init();

  // Set a valid PS mode and verify it succeeds
  [[maybe_unused]] bool power_state;
  status = GetPowerState(&power_state);
  ASSERT_EQ(status, ZX_OK);
}

}  // namespace wlan::brcmfmac
