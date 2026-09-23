// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-i2c.h"

#include <fidl/fuchsia.hardware.amlogic.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2c.businfo/cpp/wire.h>
#include <fidl/fuchsia.hardware.i2cimpl/cpp/driver/wire.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/zx/clock.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <optional>
#include <span>
#include <vector>

#include <fake-mmio-reg/fake-mmio-reg.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "aml-i2c-regs.h"
#include "src/lib/testing/predicates/status.h"

namespace aml_i2c {

class TestAmlI2c : public AmlI2c {
 public:
  explicit TestAmlI2c() : AmlI2c() {}

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(fdf_internal::DriverServer2<TestAmlI2c>::initialize,
                                          fdf_internal::DriverServer2<TestAmlI2c>::destroy);
  }

  static void set_mmio(fdf::MmioBuffer mmio) { mmio_.emplace(std::move(mmio)); }

 protected:
  zx::result<fdf::MmioBuffer> MapMmio(fdf::PDev& pdev) override {
    if (mmio_) {
      return zx::ok(*std::move(mmio_));
    }
    return zx::error(ZX_ERR_BAD_STATE);
  }

 private:
  static std::optional<fdf::MmioBuffer> mmio_;
};

std::optional<fdf::MmioBuffer> TestAmlI2c::mmio_;

class FakeAmlI2cController {
 public:
  struct Transfer {
    std::vector<uint8_t> write_data;
    std::vector<uint8_t> token_list;
    uint32_t target_addr;
    bool is_read;

    void ExpectTokenListEq(const std::vector<uint8_t>& expected) const {
      ASSERT_EQ(token_list.size(), expected.size());
      EXPECT_EQ(token_list, expected);
    }
  };

  FakeAmlI2cController() : mmio_(sizeof(reg_values_[0]), 8) {
    for (size_t i = 0; i < std::size(reg_values_); i++) {
      mmio_[i * sizeof(reg_values_[0])].SetReadCallback(ReadRegCallback(i));
      mmio_[i * sizeof(reg_values_[0])].SetWriteCallback(WriteRegCallback(i));
    }
    mmio_[kControlReg].SetWriteCallback([&](uint64_t value) { WriteControlReg(value); });
  }

  FakeAmlI2cController(FakeAmlI2cController&& other) = delete;
  FakeAmlI2cController& operator=(FakeAmlI2cController&& other) = delete;

  FakeAmlI2cController(const FakeAmlI2cController& other) = delete;
  FakeAmlI2cController& operator=(const FakeAmlI2cController& other) = delete;

  fdf::MmioBuffer GetMmioBuffer() { return mmio_.GetMmioBuffer(); }

  void SetReadData(cpp20::span<const uint8_t> read_data) { read_data_ = read_data; }

  std::vector<Transfer> GetTransfers() { return std::move(transfers_); }

  cpp20::span<uint32_t> mmio() { return {reg_values_, std::size(reg_values_)}; }

  void set_interrupt(zx::unowned_interrupt interrupt) { irq_ = std::move(interrupt); }

 private:
  std::function<uint64_t(void)> ReadRegCallback(size_t offset) {
    return [this, offset]() { return reg_values_[offset]; };
  }

  std::function<void(uint64_t)> WriteRegCallback(size_t offset) {
    return [this, offset](uint64_t value) {
      EXPECT_EQ(value & 0xffff'ffff, value);
      reg_values_[offset] = static_cast<uint32_t>(value);
    };
  }

  void WriteControlReg(uint64_t value) {
    EXPECT_EQ(value & 0xffff'ffff, value);
    if (value & 1) {
      // Start flag -- process the token list (saving the target address and/or data if needed),
      // then trigger the interrupt.
      ProcessTokenList();
      irq_->trigger(0, zx::clock::get_boot());
    }
    reg_values_[kControlReg / sizeof(uint32_t)] = static_cast<uint32_t>(value);
  }

  void ProcessTokenList() {
    uint64_t token_list = Get64BitReg(kTokenList0Reg);
    uint64_t write_data = Get64BitReg(kWriteData0Reg);
    uint64_t read_data = 0;
    size_t data_count = 0;

    for (uint64_t token = token_list & 0xf;; token_list >>= 4, token = token_list & 0xf) {
      // Skip most token validation as test cases can check against the expected token sequence.
      switch (static_cast<TokenList::Token>(token)) {
        case TokenList::Token::kEnd:
        case TokenList::Token::kStop:
          break;
        case TokenList::Token::kStart: {
          const uint32_t target_addr = reg_values_[kTargetAddrReg / sizeof(uint32_t)];
          EXPECT_EQ(target_addr & 1, 0);
          transfers_.push_back({.target_addr = (target_addr >> 1) & 0x7f});
          break;
        }
        case TokenList::Token::kTargetAddrWr:
          ASSERT_FALSE(transfers_.empty());
          transfers_.back().is_read = false;
          break;
        case TokenList::Token::kTargetAddrRd:
          ASSERT_FALSE(transfers_.empty());
          transfers_.back().is_read = true;
          break;
        case TokenList::Token::kData:
        case TokenList::Token::kDataLast:
          ASSERT_FALSE(transfers_.empty());
          if (transfers_.back().is_read) {
            ASSERT_FALSE(read_data_.empty());
            read_data |= static_cast<uint64_t>(read_data_[0]) << (8 * data_count++);
            read_data_ = read_data_.subspan(1);
          } else {
            transfers_.back().write_data.push_back(write_data & 0xff);
            write_data >>= 8;
          }
          break;
        default:
          ASSERT_TRUE(false) << std::string("Invalid token", token_list & 0xf);
          break;
      }

      ASSERT_FALSE(transfers_.empty());
      transfers_.back().token_list.push_back(token_list & 0xf);

      if (static_cast<TokenList::Token>(token) == TokenList::Token::kEnd) {
        break;
      }
    }

    EXPECT_EQ(token_list, 0);  // There should be no tokens after the end token.

    reg_values_[kReadData0Reg / sizeof(uint32_t)] = read_data & 0xffff'ffff;
    reg_values_[kReadData1Reg / sizeof(uint32_t)] = read_data >> 32;
  }

  uint64_t Get64BitReg(size_t offset) const {
    return reg_values_[offset / sizeof(uint32_t)] |
           (static_cast<uint64_t>(reg_values_[(offset / sizeof(uint32_t)) + 1]) << 32);
  }

  ddk_fake::FakeMmioRegRegion mmio_;
  uint32_t reg_values_[8]{};
  zx::unowned_interrupt irq_;
  std::vector<Transfer> transfers_;
  cpp20::span<const uint8_t> read_data_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  void Init(zx::interrupt interrupt,
            const std::optional<fuchsia_hardware_amlogic_metadata::AmlI2cDelayValues>& metadata) {
    std::map<uint32_t, zx::interrupt> irqs;
    irqs[0] = std::move(interrupt);
    pdev_.SetConfig({.irqs = std::move(irqs)});
    pdev_.AddFidlMetadata(fuchsia_hardware_i2c_businfo::I2CBusMetadata::kSerializableName,
                          fuchsia_hardware_i2c_businfo::I2CBusMetadata{});
    if (metadata.has_value()) {
      pdev_.AddFidlMetadata(fuchsia_hardware_amlogic_metadata::AmlI2cDelayValues::kSerializableName,
                            metadata.value());
    }
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    EXPECT_OK(to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler(dispatcher), "pdev"));

    return zx::ok();
  }

 private:
  fdf_fake::FakePDev pdev_;
};

class TestConfig final {
 public:
  using DriverType = TestAmlI2c;
  using EnvironmentType = TestEnvironment;
};

class AmlI2cTest : public ::testing::Test {
 public:
  // Convenience definitions that don't require casting.
  enum Token : uint8_t {
    kEnd,
    kStart,
    kTargetAddrWr,
    kTargetAddrRd,
    kData,
    kDataLast,
    kStop,
  };

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  void StartDriver(const std::optional<fuchsia_hardware_amlogic_metadata::AmlI2cDelayValues>&
                       metadata = std::nullopt) {
    zx::interrupt interrupt;
    ASSERT_EQ(ZX_OK, zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
    controller_.set_interrupt(interrupt.borrow());

    driver_test_.RunInEnvironmentTypeContext(
        [&](auto& env) { env.Init(std::move(interrupt), std::move(metadata)); });

    TestAmlI2c::set_mmio(controller_.GetMmioBuffer());

    // Start driver.
    ASSERT_TRUE(driver_test_.StartDriver().is_ok());

    driver_test().RunInDriverContext(
        [](TestAmlI2c& driver) { driver.SetTimeout(zx::duration(ZX_TIME_INFINITE)); });

    zx::result connect_result = driver_test().Connect<fuchsia_hardware_i2cimpl::Service::Device>();
    ASSERT_TRUE(connect_result.is_ok());

    i2c_.Bind(std::move(connect_result.value()));
  }

  FakeAmlI2cController& controller() { return controller_; }

  cpp20::span<uint32_t> mmio() { return controller().mmio(); }

  fdf::Arena arena_{'TEST'};
  fdf::WireSyncClient<fuchsia_hardware_i2cimpl::Device> i2c_;

 private:
  static constexpr size_t kMmioSize = sizeof(uint32_t) * 8;

  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;

  FakeAmlI2cController controller_;
};

TEST_F(AmlI2cTest, SmallWrite) {
  StartDriver();

  const std::vector<uint8_t> kWriteData{0x45, 0xd9, 0x65, 0xbc, 0x31, 0x26, 0xd7, 0xe5};

  fidl::VectorView<uint8_t> write_buffer{arena_, kWriteData};
  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x13,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
           fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&write_buffer)),
       .stop = true}};

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});
  ;

  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  EXPECT_EQ(transact_result->value()->read.size(), 0);

  const std::vector transfers = controller().GetTransfers();
  ASSERT_EQ(transfers.size(), 1);

  EXPECT_EQ(transfers[0].target_addr, 0x13);
  ASSERT_EQ(transfers[0].write_data.size(), std::size(kWriteData));
  EXPECT_EQ(transfers[0].write_data, kWriteData);
  transfers[0].ExpectTokenListEq({
      kStart,
      kTargetAddrWr,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kStop,
      kEnd,
  });
}

TEST_F(AmlI2cTest, BigWrite) {
  StartDriver();

  const std::vector<uint8_t> kWriteData{0xb9, 0x17, 0x32, 0xba, 0x8e, 0xf7, 0x19, 0xf2, 0x78, 0xbf,
                                        0xcb, 0xd3, 0xdc, 0xad, 0xbd, 0x78, 0x1b, 0xa8, 0xef, 0x1a};

  fidl::VectorView<uint8_t> write_buffer{arena_, kWriteData};
  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x5f,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
           fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&write_buffer)),
       .stop = true}};

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});

  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  EXPECT_EQ(transact_result->value()->read.size(), 0);

  const std::vector transfers = controller().GetTransfers();
  ASSERT_EQ(transfers.size(), 1);

  EXPECT_EQ(transfers[0].target_addr, 0x5f);
  ASSERT_EQ(transfers[0].write_data.size(), kWriteData.size());
  EXPECT_EQ(transfers[0].write_data, kWriteData);
  transfers[0].ExpectTokenListEq({
      // First transfer
      kStart,
      kTargetAddrWr,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kEnd,

      // Second transfer
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kEnd,

      // Third transfer
      kData,
      kData,
      kData,
      kData,
      kStop,
      kEnd,
  });
}

TEST_F(AmlI2cTest, SmallRead) {
  StartDriver();

  const std::vector<uint8_t> kExpectedReadData{0xf0, 0xdb, 0xdf, 0x6b, 0xb9, 0x3e, 0xa6, 0xfa};
  controller().SetReadData({kExpectedReadData.data(), kExpectedReadData.size()});

  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x41,
       .type =
           fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(kExpectedReadData.size()),
       .stop = true}};

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});
  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  const auto& read = transact_result->value()->read;
  ASSERT_EQ(read.size(), 1);
  EXPECT_EQ(read[0].data.size(), kExpectedReadData.size());
  EXPECT_THAT(kExpectedReadData, ::testing::ElementsAreArray(read[0].data));

  const std::vector transfers = controller().GetTransfers();
  ASSERT_EQ(transfers.size(), 1);

  EXPECT_EQ(transfers[0].target_addr, 0x41);
  transfers[0].ExpectTokenListEq({
      kStart,
      kTargetAddrRd,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kDataLast,
      kStop,
      kEnd,
  });
}

TEST_F(AmlI2cTest, BigRead) {
  StartDriver();

  const std::vector<uint8_t> kExpectedReadData{0xb9, 0x17, 0x32, 0xba, 0x8e, 0xf7, 0x19,
                                               0xf2, 0x78, 0xbf, 0xcb, 0xd3, 0xdc, 0xad,
                                               0xbd, 0x78, 0x1b, 0xa8, 0xef, 0x1a};
  controller().SetReadData({kExpectedReadData.data(), kExpectedReadData.size()});

  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x29,
       .type =
           fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(kExpectedReadData.size()),
       .stop = true}};

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});
  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  const auto& read = transact_result->value()->read;
  ASSERT_EQ(read.size(), 1);
  EXPECT_EQ(read[0].data.size(), kExpectedReadData.size());
  EXPECT_THAT(kExpectedReadData, ::testing::ElementsAreArray(read[0].data));

  const std::vector transfers = controller().GetTransfers();
  ASSERT_EQ(transfers.size(), 1);

  EXPECT_EQ(transfers[0].target_addr, 0x29);
  transfers[0].ExpectTokenListEq({
      // First transfer
      kStart,
      kTargetAddrRd,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kEnd,

      // Second transfer
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kEnd,

      // Third transfer
      kData,
      kData,
      kData,
      kDataLast,
      kStop,
      kEnd,
  });
}

TEST_F(AmlI2cTest, EmptyRead) {
  StartDriver();

  controller().SetReadData({});

  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x41,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(0),
       .stop = true}};

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});

  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  const auto& read = transact_result->value()->read;
  ASSERT_EQ(read.size(), 1);
  EXPECT_TRUE(read[0].data.empty());

  const std::vector transfers = controller().GetTransfers();
  ASSERT_TRUE(transfers.empty());
}

TEST_F(AmlI2cTest, NoStopFlag) {
  StartDriver();

  fidl::VectorView<uint8_t> buffer{arena_, 4};
  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x00,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
           fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&buffer)),
       .stop = false}};

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});

  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());

  const std::vector transfers = controller().GetTransfers();
  ASSERT_EQ(transfers.size(), 1);

  transfers[0].ExpectTokenListEq({kStart, kTargetAddrWr, kData, kData, kData, kData, kEnd});
}

TEST_F(AmlI2cTest, TransferError) {
  StartDriver();

  uint8_t buffer[4];
  controller().SetReadData({buffer, std::size(buffer)});
  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {0x00, fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(4), false}};

  mmio()[kControlReg / sizeof(uint32_t)] = 1 << 3;

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});

  ASSERT_OK(transact_result.status());
  EXPECT_TRUE(transact_result->is_error());
}

TEST_F(AmlI2cTest, ManyTransactions) {
  StartDriver();

  const uint32_t kReadCount1 = 20;
  constexpr uint32_t kReadCount2 = 4;
  const std::vector<uint8_t> kExpectedReadData{0x85, 0xb0, 0xd0, 0x1c, 0xc6, 0x8a, 0x35, 0xfc,
                                               0xcf, 0xca, 0x95, 0x01, 0x61, 0x42, 0x60, 0x8c,
                                               0xa6, 0x01, 0xd6, 0x2e, 0x38, 0x20, 0x09, 0xfa};
  controller().SetReadData({kExpectedReadData.data(), kExpectedReadData.size()});

  const std::vector<uint8_t> kExpectedWriteData{0x39, 0xf0, 0xf9, 0x17, 0xad,
                                                0x51, 0xdc, 0x30, 0xe5};

  fidl::VectorView<uint8_t> write_buffer_1{arena_, cpp20::span(kExpectedWriteData.data(), 1)};
  fidl::VectorView<uint8_t> write_buffer_2{
      arena_, cpp20::span(kExpectedWriteData.data() + 1, kExpectedWriteData.size() - 1)};

  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> ops = {
      {.address = 0x1c,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
           fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&write_buffer_1)),
       .stop = false},
      {.address = 0x2d,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(kReadCount1),
       .stop = true},
      {.address = 0x3e,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
           fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&write_buffer_2)),
       .stop = true},
      {.address = 0x4f,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(kReadCount2),
       .stop = false},
  };

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, ops});
  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  const auto& read = transact_result->value()->read;
  ASSERT_EQ(read.size(), 2);

  EXPECT_EQ(read[0].data.size(), kReadCount1);
  std::vector<uint8_t> expected1 = {kExpectedReadData.begin(),
                                    kExpectedReadData.begin() + kReadCount1};
  EXPECT_THAT(expected1, ::testing::ElementsAreArray(read[0].data));

  EXPECT_EQ(read[1].data.size(), kReadCount2);
  std::vector<uint8_t> expected2 = {kExpectedReadData.begin() + kReadCount1,
                                    kExpectedReadData.end()};
  EXPECT_THAT(expected2, ::testing::ElementsAreArray(read[1].data));

  const std::vector transfers = controller().GetTransfers();
  ASSERT_EQ(transfers.size(), 4);

  EXPECT_EQ(transfers[0].target_addr, 0x1c);
  ASSERT_EQ(transfers[0].write_data.size(), 1);
  EXPECT_EQ(transfers[0].write_data[0], kExpectedWriteData[0]);
  transfers[0].ExpectTokenListEq({kStart, kTargetAddrWr, kData, kEnd});

  EXPECT_EQ(transfers[1].target_addr, 0x2d);
  transfers[1].ExpectTokenListEq({
      // First transfer
      kStart,
      kTargetAddrRd,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kEnd,

      // Second transfer
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kEnd,

      // Third transfer
      kData,
      kData,
      kData,
      kDataLast,
      kStop,
      kEnd,
  });

  EXPECT_EQ(transfers[2].target_addr, 0x3e);
  ASSERT_EQ(transfers[2].write_data.size(), write_buffer_2.size());
  std::vector<uint8_t> expected = {kExpectedWriteData.begin() + 1, kExpectedWriteData.end()};
  EXPECT_THAT(expected, ::testing::ElementsAreArray(transfers[2].write_data));
  transfers[2].ExpectTokenListEq({
      kStart,
      kTargetAddrWr,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kData,
      kStop,
      kEnd,
  });

  EXPECT_EQ(transfers[3].target_addr, 0x4f);
  transfers[3].ExpectTokenListEq({
      Token::kStart,
      Token::kTargetAddrRd,
      Token::kData,
      Token::kData,
      Token::kData,
      Token::kDataLast,
      Token::kEnd,
  });
}

TEST_F(AmlI2cTest, WriteTransactionTooBig) {
  StartDriver();

  fidl::VectorView<uint8_t> buffer{arena_, 512};
  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x00,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
           fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&buffer)),
       .stop = true}};

  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});

  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  EXPECT_EQ(transact_result->value()->read.size(), 0);

  fidl::VectorView<uint8_t> buffer2{arena_, 513};
  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op2 = {
      {.address = 0x00,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
           fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&buffer2)),
       .stop = true}};

  auto transact_result2 = i2c_.buffer(arena_)->Transact({arena_, op2});
  ASSERT_EQ(ZX_OK, transact_result2.status());
  EXPECT_TRUE(transact_result2->is_error());
}

TEST_F(AmlI2cTest, ReadTransactionTooBig) {
  StartDriver();

  constexpr uint8_t kReadData[512] = {0};
  controller().SetReadData({kReadData, std::size(kReadData)});

  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op = {
      {.address = 0x00,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(512),
       .stop = true}};
  auto transact_result = i2c_.buffer(arena_)->Transact({arena_, op});

  ASSERT_OK(transact_result.status());
  ASSERT_FALSE(transact_result->is_error());
  EXPECT_EQ(transact_result->value()->read.size(), 1);

  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> op2 = {
      {.address = 0x00,
       .type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(513),
       .stop = true}};
  auto transact_result2 = i2c_.buffer(arena_)->Transact({arena_, op2});
  ASSERT_EQ(ZX_OK, transact_result2.status());
  EXPECT_TRUE(transact_result2->is_error());
}

TEST_F(AmlI2cTest, Metadata) {
  static const fuchsia_hardware_amlogic_metadata::AmlI2cDelayValues kMetadata(
      {.quarter_clock_delay = 0x3cd, .clock_low_delay = 0xf12});

  StartDriver(kMetadata);

  EXPECT_EQ(mmio()[kControlReg / sizeof(uint32_t)], 0x3cd << 12);
  EXPECT_EQ(mmio()[kTargetAddrReg / sizeof(uint32_t)], (0xf12 << 16) | (1 << 28));
}

TEST_F(AmlI2cTest, NoMetadata) {
  StartDriver(std::nullopt);

  EXPECT_EQ(mmio()[kControlReg / sizeof(uint32_t)], 0);
  EXPECT_EQ(mmio()[kTargetAddrReg / sizeof(uint32_t)], 0);
}

}  // namespace aml_i2c
