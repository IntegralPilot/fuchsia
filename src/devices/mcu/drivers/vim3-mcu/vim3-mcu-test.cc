// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vim3-mcu.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/mock-i2c/mock-i2c.h>

#include <fbl/vector.h>
#include <sdk/lib/inspect/testing/cpp/zxtest/inspect.h>
#include <zxtest/zxtest.h>

namespace stm {
TEST(Vim3McuTest, FanLevel) {
  mock_i2c::MockI2c mock_i2c;
  // Fan status control transactions.
  mock_i2c.ExpectWriteStop({0x88, 1});
  // Enable WOL transactions.
  mock_i2c.ExpectWriteStop({0x21, 0x03});
  // Read PCIe enabled transactions.
  mock_i2c.ExpectWrite({0x33}).ExpectReadStop({0});

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();

  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &mock_i2c);
  EXPECT_OK(loop.StartThread());

  ddk::I2cChannel i2c(std::move(endpoints.client));
  StmMcu device(nullptr, std::move(i2c));
  device.Init();
  mock_i2c.VerifyAndClear();
}

TEST(Vim3McuTest, PCIeEnabled) {
  mock_i2c::MockI2c mock_i2c;
  // Fan status control transactions.
  mock_i2c.ExpectWriteStop({0x88, 1});
  // Enable WOL transactions.
  mock_i2c.ExpectWriteStop({0x21, 0x03});
  // Read PCIe enabled transactions.
  mock_i2c.ExpectWrite({0x33}).ExpectReadStop({1});
  // Disable PCIe transactions.
  mock_i2c.ExpectWriteStop({0x33, 0});

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();

  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &mock_i2c);
  EXPECT_OK(loop.StartThread());

  ddk::I2cChannel i2c(std::move(endpoints.client));
  StmMcu device(nullptr, std::move(i2c));
  device.Init();
  mock_i2c.VerifyAndClear();

  // Check the inspect property.
  inspect::InspectTestHelper inspector_;
  ASSERT_NO_FATAL_FAILURE(inspector_.ReadInspect(device.inspect_vmo()));
  ASSERT_NO_FATAL_FAILURE(inspector_.CheckProperty(inspector_.hierarchy().node(), "PCIe enabled",
                                                   inspect::UintPropertyValue(0)));
}

}  // namespace stm
