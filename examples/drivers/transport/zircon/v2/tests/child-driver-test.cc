// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/transport/zircon/v2/child-driver.h"

#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

namespace testing {

namespace {

const std::string kTestName = "test_i2c";

}  // namespace

class FakeI2cServer : public fidl::WireServer<fuchsia_hardware_i2c::Device> {
 public:
  FakeI2cServer() {
    read_buffer_ = {0xA, 0xB, 0xC};
    read_vectors_.emplace_back(fidl::VectorView<uint8_t>::FromExternal(read_buffer_));
  }

  void Transfer(TransferRequestView request, TransferCompleter::Sync& completer) override {
    completer.ReplySuccess(
        fidl::VectorView<fidl::VectorView<uint8_t>>::FromExternal(read_vectors_));
  }

  void GetName(GetNameCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::StringView::FromExternal(kTestName));
  }

 private:
  std::vector<fidl::VectorView<uint8_t>> read_vectors_;
  std::vector<uint8_t> read_buffer_;
};

class ZirconTransportTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    fuchsia_hardware_i2c::Service::InstanceHandler handler({
        .device =
            bindings_.CreateHandler(&server_, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                    fidl::kIgnoreBindingClosure),
    });
    auto result = to_driver_vfs.AddService<fuchsia_hardware_i2c::Service>(std::move(handler));
    EXPECT_EQ(ZX_OK, result.status_value());
    return zx::ok();
  }

 private:
  FakeI2cServer server_;
  fidl::ServerBindingGroup<fuchsia_hardware_i2c::Device> bindings_;
};

class FixtureConfig final {
 public:
  using DriverType = zircon_transport::ChildZirconTransportDriver;
  using EnvironmentType = ZirconTransportTestEnvironment;
};

class ChildZirconTransportDriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(ChildZirconTransportDriverTest, VerifyQueryValues) {
  // Verify that the queried values match the fake parent driver server.
  EXPECT_EQ(kTestName, driver_test().driver()->name());
}

}  // namespace testing
