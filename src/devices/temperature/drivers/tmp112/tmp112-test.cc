// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tmp112.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/mock-i2c/mock-i2c.h>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace {

bool FloatNear(float a, float b) { return std::abs(a - b) < 0.001f; }

}  // namespace

namespace temperature {

using TemperatureClient = fidl::WireSyncClient<fuchsia_hardware_temperature::Device>;

class Tmp112DeviceTest : public zxtest::Test {
 public:
  Tmp112DeviceTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();

    fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), &mock_i2c_);

    ASSERT_OK(loop_.StartThread());
    root_ = MockDevice::FakeRootParent();
    dev_ =
        std::make_unique<Tmp112Device>(root_.get(), ddk::I2cChannel(std::move(endpoints.client)));
  }

 protected:
  async::Loop loop_;
  mock_i2c::MockI2c mock_i2c_;
  std::unique_ptr<Tmp112Device> dev_;
  std::shared_ptr<MockDevice> root_;
};

TEST_F(Tmp112DeviceTest, Init) {
  uint8_t initial_config_val[2] = {kConfigConvertResolutionSet12Bit, 0};
  mock_i2c_.ExpectWrite({kConfigReg}).ExpectReadStop({0x0, 0x0});
  mock_i2c_.ExpectWriteStop({kConfigReg, initial_config_val[0], initial_config_val[1]});
  dev_->Init();

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tmp112DeviceTest, GetTemperatureCelsius) {
  mock_i2c_.ExpectWrite({kTemperatureReg}).ExpectReadStop({0x34, 0x12});
  auto endpoints = fidl::Endpoints<fuchsia_hardware_temperature::Device>::Create();

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), dev_.get());
  ASSERT_OK(loop.StartThread());

  TemperatureClient client;
  client.Bind(std::move(endpoints.client));
  auto result = client->GetTemperatureCelsius();
  EXPECT_OK(result->status);
  EXPECT_TRUE(FloatNear(result->temp, dev_->RegToTemperatureCelsius(0x1234)));

  mock_i2c_.VerifyAndClear();
}

TEST_F(Tmp112DeviceTest, RegToTemperature) {
  EXPECT_TRUE(FloatNear(dev_->RegToTemperatureCelsius(0x1234), 52.0625));
}

}  // namespace temperature
