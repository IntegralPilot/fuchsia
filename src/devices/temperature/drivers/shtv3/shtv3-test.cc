// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "shtv3.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/runtime/testing/cpp/sync_helpers.h>
#include <lib/fake-i2c/fake-i2c.h>

#include <optional>
#include <string_view>

#include <zxtest/zxtest.h>

namespace {

bool FloatNear(float a, float b) { return std::abs(a - b) < 0.001f; }

}  // namespace

namespace temperature {

class FakeShtv3Device : public fake_i2c::FakeI2c {
 public:
  enum State {
    kNeedReset,
    kIdle,
    kMeasurementStarted,
    kMeasurementDone,
    kError,
  };

  State state() const { return state_; }

 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    if (write_buffer_size == 2) {
      const uint16_t command = (write_buffer[0] << 8) | write_buffer[1];
      if (command == 0x805d) {  // Soft reset
        state_ = kIdle;
        return ZX_OK;
      }
      if (command == 0x7866 && state_ == kIdle) {  // Start measurement
        state_ = kMeasurementStarted;
        return ZX_OK;
      }
    } else if (write_buffer_size == 0) {
      if (state_ == kMeasurementStarted) {
        state_ = kMeasurementDone;
        return ZX_ERR_IO;  // The sensor will NACK reads until the measurement is done.
      }
      if (state_ == kMeasurementDone) {
        state_ = kIdle;
        read_buffer[0] = 0x5f;
        read_buffer[1] = 0xd1;
        *read_buffer_size = 2;
        return ZX_OK;
      }
    }

    state_ = kError;
    return ZX_ERR_IO;
  }

 private:
  State state_ = kNeedReset;
};

class Shtv3Test : public zxtest::Test {
 public:
  void SetUp() override {
    EXPECT_OK(i2c_loop_.StartThread());

    ASSERT_OK(fdf::RunOnDispatcherSync(i2c_loop_.dispatcher(), [this]() {
      fake_i2c_.emplace();
      auto server = fidl::CreateEndpoints<fuchsia_hardware_i2c::Device>(&i2c_client_);
      EXPECT_TRUE(server.is_ok());
      fidl::BindServer(i2c_loop_.dispatcher(), std::move(server.value()), &*fake_i2c_);
    }));
  }

  void TearDown() override {
    ASSERT_OK(fdf::RunOnDispatcherSync(i2c_loop_.dispatcher(), [this]() { fake_i2c_.reset(); }));
  }

 protected:
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> i2c_client_;
  std::optional<FakeShtv3Device> fake_i2c_;
  async::Loop i2c_loop_{&kAsyncLoopConfigNeverAttachToThread};
};

TEST_F(Shtv3Test, ReadTemperature) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  Shtv3Device dut(nullptr, std::move(i2c_client_), {});
  EXPECT_OK(dut.Init());

  ASSERT_OK(fdf::RunOnDispatcherSync(
      i2c_loop_.dispatcher(), [this]() { EXPECT_EQ(fake_i2c_->state(), FakeShtv3Device::kIdle); }));

  auto endpoints = fidl::Endpoints<fuchsia_hardware_temperature::Device>::Create();

  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &dut);

  fidl::WireClient<fuchsia_hardware_temperature::Device> client(std::move(endpoints.client),
                                                                loop.dispatcher());

  client->GetTemperatureCelsius().Then([&loop](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_OK(result->status);
    EXPECT_TRUE(FloatNear(result->temp, 20.5f));
    loop.Quit();
  });

  loop.Run();

  ASSERT_OK(fdf::RunOnDispatcherSync(
      i2c_loop_.dispatcher(), [this]() { EXPECT_EQ(fake_i2c_->state(), FakeShtv3Device::kIdle); }));
}

TEST_F(Shtv3Test, GetSensorName) {
  constexpr char kSensorName[] = "sensor name";

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  Shtv3Device dut(nullptr, std::move(i2c_client_), kSensorName);
  EXPECT_OK(dut.Init());

  auto endpoints = fidl::Endpoints<fuchsia_hardware_temperature::Device>::Create();

  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &dut);

  fidl::WireClient<fuchsia_hardware_temperature::Device> client(std::move(endpoints.client),
                                                                loop.dispatcher());

  client->GetSensorName().Then([&](auto& result) {
    ASSERT_TRUE(result.ok());
    const std::string_view name(result->name.data(), result->name.size());
    EXPECT_STREQ(name, kSensorName);
    loop.Quit();
  });

  loop.Run();
}

}  // namespace temperature
