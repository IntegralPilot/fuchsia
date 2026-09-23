// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {

// Sample commands to use for Get and Set
#define SAMPLE_GET 98   // Equivalent of WLC_GET_REVINFO
#define SAMPLE_SET 263  // Equivalent of WLC_SET_VAR

// Arbitrary large size for command buffer
#define CMD_SIZE 1024

// Interface ID
#define IFACE_VALID 0
#define IFACE_INVALID 10

class FactoryDeviceTest : public SimTest {
 public:
  FactoryDeviceTest() = default;
  void Init();
  void CreateInterface();
  void DeleteInterface();

 protected:
  uint8_t cmd_buf[CMD_SIZE];

 private:
  SimInterface client_ifc_;
};

void FactoryDeviceTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  ASSERT_EQ(SimTest::CreateFactoryClient(), ZX_OK);
}

TEST_F(FactoryDeviceTest, IovarGetSuccess) {
  Init();
  auto cmd_vec = ::fidl::VectorView<uint8_t>::FromExternal(cmd_buf, CMD_SIZE);
  auto status = factory_client_->Get(IFACE_VALID, SAMPLE_GET, cmd_vec);
  ASSERT_FALSE(status->is_error());
}

TEST_F(FactoryDeviceTest, IovarGetFail) {
  Init();
  auto cmd_vec = ::fidl::VectorView<uint8_t>::FromExternal(cmd_buf, CMD_SIZE);
  auto status = factory_client_->Get(IFACE_INVALID, SAMPLE_GET, cmd_vec);
  ASSERT_TRUE(status->is_error());
}

TEST_F(FactoryDeviceTest, IovarSetSuccess) {
  Init();
  auto cmd_vec = ::fidl::VectorView<uint8_t>::FromExternal(cmd_buf, CMD_SIZE);
  auto status = factory_client_->Set(IFACE_VALID, SAMPLE_SET, cmd_vec);
  ASSERT_FALSE(status->is_error());
}

TEST_F(FactoryDeviceTest, IovarSetFail) {
  Init();
  auto cmd_vec = ::fidl::VectorView<uint8_t>::FromExternal(cmd_buf, CMD_SIZE);
  auto status = factory_client_->Set(IFACE_INVALID, SAMPLE_SET, cmd_vec);
  ASSERT_TRUE(status->is_error());
}

}  // namespace wlan::brcmfmac
