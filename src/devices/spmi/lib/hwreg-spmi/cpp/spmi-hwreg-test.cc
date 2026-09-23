// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/mock-spmi/mock-spmi.h>

#include <gtest/gtest.h>
#include <hwreg/spmi.h>

namespace {

class DummySpmiRegister : public hwreg::SpmiRegisterBase<DummySpmiRegister, uint8_t> {
 public:
  DEF_BIT(7, test_bit);
  DEF_FIELD(3, 0, test_field);

  static auto Get() { return hwreg::SpmiRegisterAddr<DummySpmiRegister>(0xAB); }
};

class SpmiHwregTest : public testing::Test {
 public:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_spmi::Device>::Create();

    mock_spmi_.SyncCall([&](mock_spmi::MockSpmi* spmi) {
      spmi->bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                 std::move(endpoints.server), spmi, [](fidl::UnbindInfo) {});
    });
    spmi_client_ = std::move(endpoints.client);
  }

 protected:
  async_patterns::TestDispatcherBound<mock_spmi::MockSpmi>& mock_spmi() { return mock_spmi_; }
  fdf_testing::DriverRuntime& driver_runtime() { return driver_runtime_; }
  fidl::ClientEnd<fuchsia_hardware_spmi::Device> TakeSpmiClient() {
    return std::move(spmi_client_);
  }
  fidl::UnownedClientEnd<fuchsia_hardware_spmi::Device> BorrowSpmiClient() {
    return spmi_client_.borrow();
  }

 private:
  fdf_testing::DriverRuntime driver_runtime_;
  fdf::UnownedSynchronizedDispatcher env_dispatcher_ = driver_runtime_.StartBackgroundDispatcher();
  async_patterns::TestDispatcherBound<mock_spmi::MockSpmi> mock_spmi_{
      env_dispatcher_->async_dispatcher(), std::in_place};
  fidl::ClientEnd<fuchsia_hardware_spmi::Device> spmi_client_;
};

TEST_F(SpmiHwregTest, Read) {
  mock_spmi().SyncCall(
      [](mock_spmi::MockSpmi* spmi) { spmi->ExpectRegisterRead(0xAB, 1, {0x8A}); });

  auto dut = DummySpmiRegister::Get().FromValue(0).ReadFrom(TakeSpmiClient());
  EXPECT_TRUE(dut.is_ok());
  EXPECT_EQ(dut->test_bit(), 1);
  EXPECT_EQ(dut->test_field(), 0xA);

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, Write) {
  auto dut = DummySpmiRegister::Get().FromValue(0);
  dut.set_test_bit(1);
  dut.set_test_field(0xA);

  mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) { spmi->ExpectRegisterWrite(0xAB, {0x8A}); });
  EXPECT_TRUE(dut.WriteTo(TakeSpmiClient()).is_ok());

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, UnownedRead) {
  mock_spmi().SyncCall(
      [](mock_spmi::MockSpmi* spmi) { spmi->ExpectRegisterRead(0xAB, 1, {0x8A}); });

  auto dut = DummySpmiRegister::Get().FromValue(0).ReadFrom(BorrowSpmiClient());
  EXPECT_TRUE(dut.is_ok());
  EXPECT_EQ(dut->test_bit(), 1);
  EXPECT_EQ(dut->test_field(), 0xA);

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, UnownedWrite) {
  auto dut = DummySpmiRegister::Get().FromValue(0);
  dut.set_test_bit(1);
  dut.set_test_field(0xA);

  mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) { spmi->ExpectRegisterWrite(0xAB, {0x8A}); });
  EXPECT_TRUE(dut.WriteTo(BorrowSpmiClient()).is_ok());

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, ArrayRead) {
  constexpr uint8_t kExpected[10] = {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99};

  {
    mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) {
      spmi->ExpectRegisterRead(0xAB, 7, {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66});
    });
    auto read_result = hwreg::SpmiRegisterArray(0xAB, 7).ReadFrom(BorrowSpmiClient());
    ASSERT_FALSE(read_result.is_error());
    EXPECT_TRUE(memcmp(read_result->regs().data(), kExpected, 7) == 0);
  }

  {
    mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) {
      spmi->ExpectRegisterRead(0xAB, 10,
                               {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99});
    });
    auto read_result = hwreg::SpmiRegisterArray(0xAB, 10).ReadFrom(TakeSpmiClient());
    ASSERT_FALSE(read_result.is_error());
    EXPECT_TRUE(memcmp(read_result->regs().data(), kExpected, 10) == 0);
  }

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, ArrayWrite) {
  {
    auto values = hwreg::SpmiRegisterArray(0xAB, 7);
    values.regs() = {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66};

    mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) {
      spmi->ExpectRegisterWrite(0xAB, {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66});
    });
    ASSERT_FALSE(values.WriteTo(BorrowSpmiClient()).is_error());
  }

  {
    auto values = hwreg::SpmiRegisterArray(0xAB, 10);
    values.regs() = {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99};

    mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) {
      spmi->ExpectRegisterWrite(0xAB, {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99});
    });
    ASSERT_FALSE(values.WriteTo(TakeSpmiClient()).is_error());
  }

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, AsyncRead) {
  mock_spmi().SyncCall(
      [](mock_spmi::MockSpmi* spmi) { spmi->ExpectRegisterRead(0xAB, 1, {0x8A}); });

  fidl::WireClient<fuchsia_hardware_spmi::Device> client(
      TakeSpmiClient(), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  std::optional<zx::result<DummySpmiRegister>> result;
  DummySpmiRegister::Get().ReadFrom(client, [&](zx::result<DummySpmiRegister> r) { result = r; });

  driver_runtime().RunUntil([&]() { return result.has_value(); });

  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->is_ok());
  EXPECT_EQ((*result)->test_bit(), 1);
  EXPECT_EQ((*result)->test_field(), 0xA);

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, AsyncWrite) {
  auto dut = DummySpmiRegister::Get().FromValue(0);
  dut.set_test_bit(1);
  dut.set_test_field(0xA);

  mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) { spmi->ExpectRegisterWrite(0xAB, {0x8A}); });

  fidl::WireClient<fuchsia_hardware_spmi::Device> client(
      TakeSpmiClient(), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  std::optional<zx::result<>> result;
  dut.WriteTo(client, [&](zx::result<> r) { result = r; });

  driver_runtime().RunUntil([&]() { return result.has_value(); });

  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->is_ok());

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, AsyncArrayRead) {
  constexpr uint8_t kExpected[7] = {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66};

  mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) {
    spmi->ExpectRegisterRead(0xAB, 7, {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66});
  });

  fidl::WireClient<fuchsia_hardware_spmi::Device> client(
      TakeSpmiClient(), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  std::optional<zx::result<hwreg::SpmiRegisterArray>> result;
  hwreg::SpmiRegisterArray(0xAB, 7).ReadFrom(
      client, [&](zx::result<hwreg::SpmiRegisterArray> r) { result = r; });

  driver_runtime().RunUntil([&]() { return result.has_value(); });

  ASSERT_TRUE(result.has_value());
  ASSERT_FALSE(result->is_error());
  EXPECT_EQ(result->value().regs(), std::vector<uint8_t>(kExpected, kExpected + 7));

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

TEST_F(SpmiHwregTest, AsyncArrayWrite) {
  auto values = hwreg::SpmiRegisterArray(0xAB, 7);
  values.regs() = {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66};

  mock_spmi().SyncCall([](mock_spmi::MockSpmi* spmi) {
    spmi->ExpectRegisterWrite(0xAB, {0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66});
  });

  fidl::WireClient<fuchsia_hardware_spmi::Device> client(
      TakeSpmiClient(), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  std::optional<zx::result<>> result;
  values.WriteTo(client, [&](zx::result<> r) { result = r; });

  driver_runtime().RunUntil([&]() { return result.has_value(); });

  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->is_ok());

  mock_spmi().SyncCall(&mock_spmi::MockSpmi::VerifyAndClear);
}

}  // namespace
