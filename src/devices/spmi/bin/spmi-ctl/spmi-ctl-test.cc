// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <fidl/fuchsia.hardware.spmi/cpp/test_base.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>

#include <zxtest/zxtest.h>

#include "lib/zx/result.h"
#include "spmi-ctl-impl.h"

class FakeSpmi : public fidl::testing::TestBase<fuchsia_hardware_spmi::Device>,
                 public fidl::Server<fuchsia_hardware_spmi::Debug> {
 public:
  explicit FakeSpmi(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  // FIDL natural C++ methods for fuchsia.hardware.spmi.
  void GetProperties(GetPropertiesCompleter::Sync& completer) override {
    fuchsia_hardware_spmi::DeviceGetPropertiesResponse response;
    response.sid(123);
    response.register_width_bytes(register_width_bytes_);
    completer.Reply(std::move(response));
  }
  void RegisterRead(RegisterReadRequest& request, RegisterReadCompleter::Sync& completer) override {
    read_size_ = request.size_bytes();
    read_addresses_.push_back(request.address());
    // Allow multi-register reads when requested addresses fall within the written range.
    const size_t reg_count = (data_.size() + register_width_bytes_ - 1) / register_width_bytes_;
    if (address_ && request.address() >= *address_ && request.address() < *address_ + reg_count) {
      const size_t offset = (request.address() - *address_) * register_width_bytes_;
      std::vector<uint8_t> data;
      for (size_t i = 0; i < request.size_bytes(); ++i) {
        if (offset + i < data_.size()) {
          data.push_back(data_[offset + i]);
        } else {
          data.push_back(0);
        }
      }
      return completer.Reply(zx::ok(data));
    }
    completer.Reply(zx::error(fuchsia_hardware_spmi::DriverError::kBadState));
  }
  void RegisterWrite(RegisterWriteRequest& request,
                     RegisterWriteCompleter::Sync& completer) override {
    address_.emplace(request.address());
    data_ = request.data();
    return completer.Reply(zx::ok());
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_spmi::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}
  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    FAIL();
  }

  void ConnectTarget(ConnectTargetRequest& request,
                     ConnectTargetCompleter::Sync& completer) override {
    if (request.target_id() >= fuchsia_hardware_spmi::kMaxTargets) {
      completer.Reply(fit::error(fuchsia_hardware_spmi::DriverError::kInvalidArgs));
      return;
    }
    target_id_ = request.target_id();
    device_bindings_.AddBinding(dispatcher_, std::move(request.server()), this,
                                fidl::kIgnoreBindingClosure);
    completer.Reply(zx::ok());
  }
  void GetControllerProperties(GetControllerPropertiesCompleter::Sync& completer) override {
    completer.Reply({{"spmi-controller"}});
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_spmi::Debug> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  uint8_t target_id() const { return target_id_; }
  std::vector<uint8_t>& data() { return data_; }
  uint16_t address() { return *address_; }
  size_t read_size() { return read_size_; }
  void set_register_width_bytes(uint32_t width) { register_width_bytes_ = width; }
  const std::vector<uint16_t>& read_addresses() const { return read_addresses_; }
  void clear_read_addresses() { read_addresses_.clear(); }

 private:
  async_dispatcher_t* const dispatcher_;
  uint8_t target_id_{fuchsia_hardware_spmi::kMaxTargets};
  std::optional<uint16_t> address_;
  std::vector<uint8_t> data_;
  size_t read_size_{0};
  // Default register width in bytes.
  uint32_t register_width_bytes_{1};
  std::vector<uint16_t> read_addresses_;
  fidl::ServerBindingGroup<fuchsia_hardware_spmi::Device> device_bindings_;
};

class SpmiCtlTest : public zxtest::Test {
 public:
  void SetUp() override {
    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigAttachToCurrentThread);
    ASSERT_OK(loop_->StartThread("spmi-ctl-test-loop"));
    spmi_ = std::make_unique<FakeSpmi>(loop_->dispatcher());
  }

  void TearDown() override { loop_->Shutdown(); }

  int CallSpmiCtl(std::vector<std::string> args) {
    constexpr size_t kMaxArgs = 64;
    char* argv[kMaxArgs];
    ZX_ASSERT(args.size() <= kMaxArgs);
    for (size_t i = 0; i < args.size(); ++i) {
      argv[i] = const_cast<char*>(args[i].c_str());
    }
    fidl::ClientEnd<fuchsia_hardware_spmi::Debug> client;
    zx::result server = fidl::CreateEndpoints(&client);
    ZX_ASSERT(server.status_value() == ZX_OK);
    fidl::BindServer(loop_->dispatcher(), std::move(server.value()), spmi_.get());
    spmi_ctl_.emplace(SpmiCtl(std::move(client)));
    return spmi_ctl_->Execute(static_cast<int>(args.size()), argv);
  }

 protected:
  std::unique_ptr<async::Loop> loop_;
  std::unique_ptr<FakeSpmi> spmi_;
  std::optional<SpmiCtl> spmi_ctl_;
};

// Tests that invoking spmi-ctl with unknown command line flags returns an error.
TEST_F(SpmiCtlTest, UnknownCommands) {
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-b"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "--bad"}), -1);
}

// Tests that invalid or out-of-range target identifiers return an error.
TEST_F(SpmiCtlTest, InvalidTarget) {
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-a", "0x1234", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "-a", "0x1234", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "16", "-a", "0x1234", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0x10", "-a", "0x1234", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "100", "-a", "0x1234", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "abc", "-a", "0x1234", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "-1", "-a", "0x1234", "-r", "4"}), -1);
}

// Tests successful register write and read operations using both hex and decimal inputs.
TEST_F(SpmiCtlTest, ReadWriteSuccess) {
  std::vector<uint8_t> canned_data;

  // Write then read 4 bytes using hex inputs with 0x prefix.
  ASSERT_EQ(
      CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1122", "-w", "0x11", "0x22", "0x33", "0x44"}),
      0);
  EXPECT_EQ(spmi_->target_id(), 0);
  canned_data = {0x11, 0x22, 0x33, 0x44};
  EXPECT_TRUE(spmi_->data() == canned_data);
  EXPECT_EQ(spmi_->address(), 0x1122);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1122", "-r", "0x4"}), 0);
  EXPECT_EQ(spmi_->target_id(), 0);
  EXPECT_EQ(spmi_->read_size(), 4);
  EXPECT_EQ(spmi_->address(), 0x1122);

  // Write then read 4 bytes using decimal inputs without 0x prefix.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "4386", "-w", "11", "22", "33", "44"}), 0);
  EXPECT_EQ(spmi_->target_id(), 0);
  canned_data = {11, 22, 33, 44};
  EXPECT_TRUE(spmi_->data() == canned_data);
  EXPECT_EQ(spmi_->address(), 4386);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "4386", "-r", "4"}), 0);
  EXPECT_EQ(spmi_->target_id(), 0);
  EXPECT_EQ(spmi_->read_size(), 4);
  EXPECT_EQ(spmi_->address(), 4386);

  // Write then read 9 bytes.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1111", "-w", "1", "2", "3", "4", "5", "6",
                         "7", "8", "9"}),
            0);
  EXPECT_EQ(spmi_->target_id(), 0);
  canned_data = {1, 2, 3, 4, 5, 6, 7, 8, 9};
  EXPECT_TRUE(spmi_->data() == canned_data);
  EXPECT_EQ(spmi_->address(), 0x1111);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1111", "-r", "9"}), 0);
  EXPECT_EQ(spmi_->target_id(), 0);
  EXPECT_EQ(spmi_->read_size(), 9);
  EXPECT_EQ(spmi_->address(), 0x1111);
}

// Tests error conditions during register write and read operations.
TEST_F(SpmiCtlTest, ReadWriteErrors) {
  // No address.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-w", "0x12", "0x34", "0x56"}), -1);
  // Unknown address.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x5678", "-r", "4"}), -1);
  // Address too big (hex).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x10000", "-r", "4"}), -1);
  // Address too big (decimal).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "65536", "-r", "4"}), -1);
  // Write no data.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w"}), -1);
  // Read size 0 (decimal).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "0"}), -1);
  // Read size 0 (hex).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "0x0"}), -1);
  // Read size too big (hex).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "0x100000000"}), -1);
  // Read size too big (decimal).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "4294967296"}), -1);
  // Write byte > 255 (decimal).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "256"}), -1);
  // Write byte > 0xff (hex).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x100"}), -1);
  // Second write byte > 255 (decimal).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10", "256"}), -1);
  // Second write byte > 0xff (hex).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10", "0x100"}), -1);
  // Second write byte < 0.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10", "-1"}), -1);
  // Third write byte > 255 (decimal).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10", "0x20", "256"}), -1);
  // Third write byte > 0xff (hex).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10", "0x20", "0x100"}),
            -1);
  // Third write byte < 0.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10", "0x20", "-1"}), -1);
  // Target with invalid trailing characters.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0abc", "-a", "0x1234", "-w", "0x10"}), -1);
  // Address with invalid trailing characters.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234xyz", "-w", "0x10"}), -1);
  // Read size with invalid trailing characters.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "4xyz"}), -1);
  // Write byte with invalid trailing characters.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "10xyz"}), -1);
}

// Tests base-aware parsing where 0x or 0X prefix indicates hex and no prefix indicates decimal.
TEST_F(SpmiCtlTest, BaseAwareParsing) {
  // Test target parsing in hex (0xa == 10) and decimal (10).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0xa", "-a", "0x1234", "-w", "0x1"}), 0);
  EXPECT_EQ(spmi_->target_id(), 10);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "10", "-a", "0x1234", "-w", "0x1"}), 0);
  EXPECT_EQ(spmi_->target_id(), 10);

  // Test target parsing with uppercase 0X (0XF == 15).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0XF", "-a", "0x1234", "-w", "0x1"}), 0);
  EXPECT_EQ(spmi_->target_id(), 15);

  // Test address parsing: 0x1234 in hex and 4660 in decimal.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10"}), 0);
  EXPECT_EQ(spmi_->address(), 0x1234);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "4660", "-w", "16"}), 0);
  EXPECT_EQ(spmi_->address(), 4660);

  // Test write bytes: 0x10 is 16 in hex, 10 is 10 in decimal.
  std::vector<uint8_t> hex_data = {16};
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x10"}), 0);
  EXPECT_TRUE(spmi_->data() == hex_data);
  std::vector<uint8_t> dec_data = {10};
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "10"}), 0);
  EXPECT_TRUE(spmi_->data() == dec_data);

  // Test read size: 0x2 is 2 bytes, 2 is 2 bytes (with 2 bytes written in advance).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x1", "0x2"}), 0);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "0x2"}), 0);
  EXPECT_EQ(spmi_->read_size(), 2);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "2"}), 0);
  EXPECT_EQ(spmi_->read_size(), 2);
}

// Tests that read requests with sizes greater than 255 bytes (e.g. 256 bytes) do not
// wrap or truncate to 0.
TEST_F(SpmiCtlTest, ReadLargeSize) {
  constexpr uint32_t kLargeReadSize = 256;
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x1"}), 0);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "256"}), 0);
  EXPECT_EQ(spmi_->read_size(), kLargeReadSize);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "0x100"}), 0);
  EXPECT_EQ(spmi_->read_size(), kLargeReadSize);
}

// Verifies that spmi-ctl retrieves and displays device properties including register width.
TEST_F(SpmiCtlTest, GetProperties) {
  // Test with default 1-byte register width.
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-p"}), 0);
  EXPECT_EQ(spmi_->target_id(), 0);

  // Test with 2-byte register width.
  constexpr uint32_t kWidth2Bytes = 2;
  spmi_->set_register_width_bytes(kWidth2Bytes);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-p"}), 0);
  EXPECT_EQ(spmi_->target_id(), 0);
}

// Verifies reading registers with different register widths (1-byte and 2-byte).
TEST_F(SpmiCtlTest, ReadWithRegisterWidth) {
  constexpr uint32_t kRead4Bytes = 4;
  constexpr uint32_t kWidth2Bytes = 2;

  // Read 4 registers with default 1-byte register width (4 bytes total).
  ASSERT_EQ(
      CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x11", "0x22", "0x33", "0x44"}),
      0);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "4"}), 0);
  EXPECT_EQ(spmi_->read_size(), kRead4Bytes);

  // Read 2 registers with 2-byte register width (4 bytes total).
  spmi_->set_register_width_bytes(kWidth2Bytes);
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-r", "2"}), 0);
  EXPECT_EQ(spmi_->read_size(), kRead4Bytes);
}

// Verifies dumping registers increments addresses and reads in chunks matching register width.
TEST_F(SpmiCtlTest, DumpWithRegisterWidth) {
  constexpr uint32_t kWidth1Byte = 1;
  constexpr uint32_t kWidth2Bytes = 2;

  // Dump 4 bytes with 1-byte register width reads 1 byte at addresses 0x1000, 0x1001, 0x1002,
  // 0x1003.
  spmi_->set_register_width_bytes(kWidth1Byte);
  ASSERT_EQ(
      CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1000", "-w", "0x11", "0x22", "0x33", "0x44"}),
      0);
  spmi_->clear_read_addresses();
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1000", "-d", "4"}), 0);
  const std::vector<uint16_t> expected_1byte_addrs = {0x1000, 0x1001, 0x1002, 0x1003};
  EXPECT_EQ(spmi_->read_addresses(), expected_1byte_addrs);

  // Dump 4 bytes with 2-byte register width reads 2 bytes at addresses 0x1000 and 0x1001.
  spmi_->set_register_width_bytes(kWidth2Bytes);
  spmi_->clear_read_addresses();
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1000", "-d", "4"}), 0);
  const std::vector<uint16_t> expected_2byte_addrs = {0x1000, 0x1001};
  EXPECT_EQ(spmi_->read_addresses(), expected_2byte_addrs);
}

// Verifies reading registers with explicit register width argument (-i and --width).
TEST_F(SpmiCtlTest, ReadWithRegisterWidthArg) {
  constexpr uint32_t kRead4Bytes = 4;

  ASSERT_EQ(
      CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-w", "0x11", "0x22", "0x33", "0x44"}),
      0);

  // Read 2 registers specifying -i 2 (4 bytes total).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-i", "2", "-r", "2"}), 0);
  EXPECT_EQ(spmi_->read_size(), kRead4Bytes);

  // Read 1 register specifying --width 4 (4 bytes total).
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "--width", "4", "-r", "1"}), 0);
  EXPECT_EQ(spmi_->read_size(), kRead4Bytes);
}

// Verifies dumping registers with explicit register width argument (-i and --width).
TEST_F(SpmiCtlTest, DumpWithRegisterWidthArg) {
  ASSERT_EQ(
      CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1000", "-w", "0x11", "0x22", "0x33", "0x44"}),
      0);

  // Dump 4 bytes with -i 2 reads 2 bytes at addresses 0x1000 and 0x1001.
  spmi_->clear_read_addresses();
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1000", "-i", "2", "-d", "4"}), 0);
  const std::vector<uint16_t> expected_2byte_addrs = {0x1000, 0x1001};
  EXPECT_EQ(spmi_->read_addresses(), expected_2byte_addrs);

  // Dump 4 bytes with --width 4 reads 4 bytes at address 0x1000.
  spmi_->clear_read_addresses();
  ASSERT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1000", "--width", "4", "-d", "4"}), 0);
  const std::vector<uint16_t> expected_4byte_addrs = {0x1000};
  EXPECT_EQ(spmi_->read_addresses(), expected_4byte_addrs);
}

// Verifies errors when dump size is not a multiple of the register width,
// total read bytes overflow, or register width argument is invalid.
TEST_F(SpmiCtlTest, RegisterWidthValidationErrors) {
  constexpr uint32_t kWidth2Bytes = 2;
  spmi_->set_register_width_bytes(kWidth2Bytes);

  // Dump 3 bytes on 2-byte width fails.
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-d", "3"}), -1);

  // Dump 5 bytes with -i 4 fails.
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-i", "4", "-d", "5"}), -1);

  // Read with total bytes overflow fails.
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-i", "2", "-r", "0x80000000"}),
            -1);

  // Invalid width arguments.
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-i", "0", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-i", "-1", "-r", "4"}), -1);
  EXPECT_EQ(CallSpmiCtl({"spmi-ctl", "-t", "0", "-a", "0x1234", "-i", "abc", "-r", "4"}), -1);
}
