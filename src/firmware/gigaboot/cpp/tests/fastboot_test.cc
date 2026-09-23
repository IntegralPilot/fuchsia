// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fastboot.h"

#include <lib/abr/abr.h>
#include <lib/abr/data.h>
#include <lib/abr/util.h>
#include <lib/fastboot/test/test-transport.h>
#include <lib/zbitl/view.h>
#include <lib/zircon_boot/test/mock_zircon_boot_ops.h>
#include <lib/zircon_boot/zbi_utils.h>
#include <sparse_format.h>

#include "zircon/hw/gpt.h"

#undef error  // Macro from sparse_format.h that interferes with other symbols.

#include <algorithm>
#include <cctype>
#include <numeric>
#include <vector>

#include <fbl/vector.h>
#include <gtest/gtest.h>

#include "backends.h"
#include "boot_zbi_items.h"
#include "gmock/gmock.h"
#include "gpt.h"
#include "lib/fit/internal/result.h"
#include "lib/fit/result.h"
#include "mock_boot_service.h"
#include "mock_efi_variables.h"
#include "partition.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "utils.h"

// For "ABC"sv literal string view operator
using namespace std::literals;

using ::testing::_;
using ::testing::DoAll;
using ::testing::ElementsAreArray;
using ::testing::IsEmpty;
using ::testing::Return;
using ::testing::SetArgReferee;
using ::testing::StrEq;

namespace gigaboot {

class PartitionCustomizer {
 public:
  static std::span<const PartitionMap::PartitionEntry> PARTITION_SPAN;

  explicit PartitionCustomizer(std::span<const PartitionMap::PartitionEntry> span) {
    old_span_ = PARTITION_SPAN;
    PARTITION_SPAN = span;
  }

  ~PartitionCustomizer() { PARTITION_SPAN = old_span_; }

 private:
  std::span<const PartitionMap::PartitionEntry> old_span_;
};

std::span<const PartitionMap::PartitionEntry> PartitionCustomizer::PARTITION_SPAN = {};

const std::span<const PartitionMap::PartitionEntry> GetPartitionCustomizations() {
  return PartitionCustomizer::PARTITION_SPAN;
}

namespace {

class TestTcpTransport : public TcpTransportInterface {
 public:
  // Add data to the input stream.
  void AddInputData(const void* data, size_t size) {
    const uint8_t* start = static_cast<const uint8_t*>(data);
    in_data_.insert(in_data_.end(), start, start + size);
  }

  // Add a fastboot packet (length-prefixed byte sequence) to input stream.
  void AddFastbootPacket(const void* data, uint64_t size) {
    uint64_t be_size = ToBigEndian(size);
    AddInputData(&be_size, sizeof(be_size));
    AddInputData(data, size);
  }

  bool Read(void* out, size_t size) override {
    if (offset_ + size > in_data_.size()) {
      return false;
    }

    memcpy(out, in_data_.data() + offset_, size);
    offset_ += size;
    return true;
  }

  bool Write(const void* data, size_t size) override {
    const uint8_t* start = static_cast<const uint8_t*>(data);
    out_data_.insert(out_data_.end(), start, start + size);
    return true;
  }

  const std::vector<uint8_t>& GetOutData() { return out_data_; }

  void PopOutput(size_t size) {
    ASSERT_GE(out_data_.size(), size);
    out_data_ = std::vector<uint8_t>(out_data_.begin() + size, out_data_.end());
  }

  void PopAndCheckOutput(std::string_view expected) {
    ASSERT_GE(out_data_.size(), expected.size());
    std::string_view actual(reinterpret_cast<char*>(out_data_.data()), expected.size());
    ASSERT_EQ(actual, expected);
    PopOutput(expected.size());
  }

 private:
  size_t offset_ = 0;
  std::vector<uint8_t> in_data_;
  std::vector<uint8_t> out_data_;
};

constexpr size_t kDownloadBufferSize = 8192;
uint8_t download_buffer[kDownloadBufferSize];

void CheckPacketsEqual(const fastboot::Packets& lhs, const fastboot::Packets& rhs) {
  ASSERT_EQ(lhs.size(), rhs.size());
  for (size_t i = 0; i < lhs.size(); i++) {
    ASSERT_EQ(lhs[i], rhs[i]);
  }
}

TEST(FastbootTest, FastbootContinueTest) {
  Fastboot fastboot(download_buffer, ZirconBootOps());
  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("continue"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  ASSERT_EQ(transport.GetOutPackets().back(), "OKAY");
  ASSERT_TRUE(fastboot.IsContinue());
}

struct BasicVarTestCase {
  const std::string_view var;
  const std::string_view expected_val;
};

using BasicVarTest = ::testing::TestWithParam<BasicVarTestCase>;

TEST_P(BasicVarTest, TestBasicVar) {
  BasicVarTestCase const& test_case = GetParam();

  MockZirconBootOps zb_ops;
  Fastboot fastboot(download_buffer, zb_ops.GetZirconBootOps());
  fastboot::TestTransport transport;

  std::string command("getvar:");
  command += test_case.var;
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  ASSERT_NO_FATAL_FAILURE(
      CheckPacketsEqual(transport.GetOutPackets(),
                        fastboot::Packets{std::string("OKAY").append(test_case.expected_val)}));
}

INSTANTIATE_TEST_SUITE_P(FastbootGetVarsTests, BasicVarTest,
                         testing::ValuesIn<BasicVarTest::ParamType>({
                             {"slot-count", "2"},
                             {"slot-suffixes", "a,b"},
                             {"max-download-size", "0x00002000"},
                             {"hw-revision", BOARD_NAME},
                             {"version", "0.4"},
                         }),
                         [](testing::TestParamInfo<BasicVarTest::ParamType> const& info) {
                           const std::string_view& v = info.param.var;
                           std::string name;
                           std::transform(
                               v.cbegin(), v.cend(), std::back_inserter(name),
                               [](unsigned char c) { return std::isalnum(c) ? c : '_'; });
                           return name;
                         });

TEST(FastbootTcpSessionTest, ExitOnFastbootContinue) {
  Fastboot fastboot(download_buffer, ZirconBootOps());
  TestTcpTransport transport;

  // Handshake message
  const char handshake_message[] = "FB01";
  transport.AddInputData(handshake_message, strlen(handshake_message));

  // fastboot continue
  const char fastboot_cmd_continue[] = "continue";
  transport.AddFastbootPacket(fastboot_cmd_continue, strlen(fastboot_cmd_continue));

  // Add another packet, which should not be processed.
  const char fastboot_cmd_not_processed[] = "not-processed";
  transport.AddFastbootPacket(fastboot_cmd_not_processed, strlen(fastboot_cmd_not_processed));

  FastbootTcpSession(transport, fastboot);

  // API should return the same "FB01"
  ASSERT_NO_FATAL_FAILURE(transport.PopAndCheckOutput("FB01"));

  // Continue should return "OKAY"
  ASSERT_NO_FATAL_FAILURE(
      transport.PopAndCheckOutput("\x00\x00\x00\x00\x00\x00\x00\x04"
                                  "OKAY"sv));

  // The 'not-processed' command packet should not be processed. We shouldn't get
  // any new data in the output. (in this case it will be a failure message if processed).
  ASSERT_TRUE(transport.GetOutData().empty());
}

TEST(FastbootTcpSessionTest, HandshakeFailsNotFB) {
  Fastboot fastboot(download_buffer, ZirconBootOps());
  TestTcpTransport transport;

  // Handshake message
  const char handshake_message[] = "AC01";
  transport.AddInputData(handshake_message, strlen(handshake_message));

  FastbootTcpSession(transport, fastboot);

  // API should write the same "FB01" no matter what is received
  ASSERT_NO_FATAL_FAILURE(transport.PopAndCheckOutput("FB01"));

  // Nothing should have been written.
  ASSERT_TRUE(transport.GetOutData().empty());
}

TEST(FastbootTcpSessionTest, HandshakeFailsNotNumericVersion) {
  Fastboot fastboot(download_buffer, ZirconBootOps());
  TestTcpTransport transport;

  // Handshake message
  const char handshake_message[] = "FBxx";
  transport.AddInputData(handshake_message, strlen(handshake_message));

  FastbootTcpSession(transport, fastboot);

  // API should write the same "FB01" no matter what is received
  ASSERT_NO_FATAL_FAILURE(transport.PopAndCheckOutput("FB01"));

  // Nothing should have been written.
  ASSERT_TRUE(transport.GetOutData().empty());
}

TEST(FastbootTcpSessionTest, ExitWhenNoMoreData) {
  Fastboot fastboot(download_buffer, ZirconBootOps());
  TestTcpTransport transport;

  // Handshake message
  const char handshake_message[] = "FB01";
  transport.AddInputData(handshake_message, strlen(handshake_message));

  FastbootTcpSession(transport, fastboot);

  // API should returns the same "FB01"
  ASSERT_NO_FATAL_FAILURE(transport.PopAndCheckOutput("FB01"));

  // No more data should be written.
  ASSERT_TRUE(transport.GetOutData().empty());
}

TEST(FastbootTcpSessionTest, ExitOnCommandFailure) {
  Fastboot fastboot(download_buffer, ZirconBootOps());
  TestTcpTransport transport;

  // Handshake message
  const char handshake_message[] = "FB01";
  transport.AddInputData(handshake_message, strlen(handshake_message));

  // fastboot continue
  const char fastboot_cmd_continue[] = "unknown-cmd";
  transport.AddFastbootPacket(fastboot_cmd_continue, strlen(fastboot_cmd_continue));

  FastbootTcpSession(transport, fastboot);

  // Check and skip handshake message
  ASSERT_NO_FATAL_FAILURE(transport.PopAndCheckOutput("FB01"));
  // Skips 8- bytes length prefix
  ASSERT_NO_FATAL_FAILURE(transport.PopOutput(8));
  ASSERT_NO_FATAL_FAILURE(transport.PopAndCheckOutput("FAIL"));
}

class FastbootFlashTest : public ::testing::Test {
 public:
  FastbootFlashTest()
      : image_device_({"path-A", "path-B", "path-C", "image"}),
        block_device_({"path-A", "path-B", "path-C"}, 1024) {
    stub_service_.AddDevice(&image_device_);

    // Add a block device for fastboot flash test.
    stub_service_.AddDevice(&block_device_);
    block_device_.InitializeGpt();
  }

  void AddPartition(const gpt_entry_t& new_entry) { block_device_.AddGptPartition(new_entry); }

  uint8_t* BlockDeviceStart() { return block_device_.fake_disk_io_protocol().contents(0).data(); }

  // A helper to download data to fastboot
  void DownloadData(Fastboot& fastboot, const std::vector<uint8_t>& download_content) {
    char download_command[fastboot::kMaxCommandPacketSize];
    snprintf(download_command, fastboot::kMaxCommandPacketSize, "download:%08zx",
             download_content.size());
    fastboot::TestTransport transport;
    transport.AddInPacket(std::string(download_command));
    zx::result ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok());
    // Download
    transport.AddInPacket(download_content);
    ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok());

    char expected_data_message[fastboot::kMaxCommandPacketSize];
    snprintf(expected_data_message, fastboot::kMaxCommandPacketSize, "DATA%08zx",
             download_content.size());
    CheckPacketsEqual(transport.GetOutPackets(), {
                                                     expected_data_message,
                                                     "OKAY",
                                                 });
  }

  MockStubService& stub_service() { return stub_service_; }
  Device& image_device() { return image_device_; }
  BlockDevice& block_device() { return block_device_; }
  MockZirconBootOps& mock_zb_ops() { return mock_zb_ops_; }

 private:
  MockStubService stub_service_;
  Device image_device_;
  BlockDevice block_device_;
  MockZirconBootOps mock_zb_ops_;
};

class FastbootSlotTest : public ::testing::Test {
 public:
  void InitializeAbr(AbrSlotIndex slot) {
    mock_zb_ops_.AddPartition(GPT_DURABLE_BOOT_NAME, sizeof(AbrData));
    ZirconBootOps zb_ops = mock_zb_ops_.GetZirconBootOps();
    AbrOps abr_ops = GetAbrOpsFromZirconBootOps(&zb_ops);
    AbrResult res;

    if (slot == kAbrSlotIndexR) {
      res = AbrMarkSlotUnbootable(&abr_ops, kAbrSlotIndexA, kAbrUnbootableReasonNone);
      ASSERT_EQ(res, kAbrResultOk);
      res = AbrMarkSlotUnbootable(&abr_ops, kAbrSlotIndexB, kAbrUnbootableReasonNone);
      ASSERT_EQ(res, kAbrResultOk);
    } else {
      res = AbrMarkSlotActive(&abr_ops, slot);
      ASSERT_EQ(res, kAbrResultOk);
    }
  }

  void MarkUnbootable(AbrSlotIndex slot, AbrUnbootableReason reason = kAbrUnbootableReasonNone) {
    mock_zb_ops_.AddPartition(GPT_DURABLE_BOOT_NAME, sizeof(AbrData));
    ZirconBootOps zb_ops = mock_zb_ops_.GetZirconBootOps();
    AbrOps abr_ops = GetAbrOpsFromZirconBootOps(&zb_ops);
    AbrResult res = AbrMarkSlotUnbootable(&abr_ops, slot, reason);
    ASSERT_EQ(res, kAbrResultOk);
  }

  void MarkSuccesful(AbrSlotIndex slot) {
    mock_zb_ops_.AddPartition(GPT_DURABLE_BOOT_NAME, sizeof(AbrData));
    ZirconBootOps zb_ops = mock_zb_ops_.GetZirconBootOps();
    AbrOps abr_ops = GetAbrOpsFromZirconBootOps(&zb_ops);
    AbrResult res = AbrMarkSlotSuccessful(&abr_ops, slot, false);
    ASSERT_EQ(res, kAbrResultOk);
  }

  MockZirconBootOps& mock_zb_ops() { return mock_zb_ops_; }

 private:
  MockZirconBootOps mock_zb_ops_;
};

struct FastbootSlotTestCase {
  AbrSlotIndex slot_index;
  char const* slot_str;
};

class FastbootSlotAllSlotsTest : public FastbootSlotTest,
                                 public testing::WithParamInterface<FastbootSlotTestCase> {};

TEST_P(FastbootSlotAllSlotsTest, TestFastbootGetSlot) {
  FastbootSlotTestCase const& test_case = GetParam();

  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOps();
  Fastboot fastboot(download_buffer, zb_ops);
  fastboot::TestTransport transport;

  InitializeAbr(test_case.slot_index);
  transport.AddInPacket(std::string("getvar:current-slot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  fastboot::Packets expected_packets = {std::string{"OKAY"} + test_case.slot_str};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

INSTANTIATE_TEST_SUITE_P(
    FastbootSlotAllSlotsTests, FastbootSlotAllSlotsTest,
    testing::ValuesIn<FastbootSlotAllSlotsTest::ParamType>({
        {kAbrSlotIndexA, "a"},
        {kAbrSlotIndexB, "b"},
        {kAbrSlotIndexR, "r"},
    }),
    [](testing::TestParamInfo<FastbootSlotAllSlotsTest::ParamType> const& info) {
      return info.param.slot_str;
    });

using FastbootSlotABTest = FastbootSlotAllSlotsTest;

TEST_P(FastbootSlotABTest, TestFastbootSetActive) {
  FastbootSlotTestCase const& test_case = GetParam();

  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOps();
  Fastboot fastboot(download_buffer, zb_ops);
  fastboot::TestTransport transport;

  InitializeAbr(kAbrSlotIndexR);
  transport.AddInPacket(std::string{"set_active:"} + test_case.slot_str);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
  transport.ClearOutPackets();

  transport.AddInPacket(std::string("getvar:current-slot"));
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  expected_packets[0] += test_case.slot_str;
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_P(FastbootSlotABTest, GetVarSlotLastSetActive) {
  FastbootSlotTestCase const& test_case = GetParam();

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  InitializeAbr(test_case.slot_index);
  transport.AddInPacket(std::string("getvar:slot-last-set-active"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  fastboot::Packets expected_packets{std::string{"OKAY"} + test_case.slot_str};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_P(FastbootSlotABTest, GetVarSlotUnbootable) {
  FastbootSlotTestCase const& test_case = GetParam();

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  MarkSuccesful(test_case.slot_index);

  std::string command = std::string{"getvar:slot-unbootable:"} + test_case.slot_str;
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAYno"}));

  MarkUnbootable(test_case.slot_index);
  transport.ClearOutPackets();
  transport.AddInPacket(command);
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAYyes"}));
}

TEST_P(FastbootSlotABTest, GetVarSlotUnbootableReason) {
  FastbootSlotTestCase const& test_case = GetParam();

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  MarkSuccesful(test_case.slot_index);

  std::string command = std::string{"getvar:slot-unbootable-reason:"} + test_case.slot_str;

  // Bootable -> "N/A"
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAYN/A"}));

  // No more tries -> "1 (no more attempts)"
  MarkUnbootable(test_case.slot_index, kAbrUnbootableReasonNoMoreTries);
  transport.ClearOutPackets();
  transport.AddInPacket(command);
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(
      CheckPacketsEqual(transport.GetOutPackets(), {"OKAY1 (no more attempts)"}));
}

TEST_P(FastbootSlotABTest, GetVarSlotRetryCount) {
  FastbootSlotTestCase const& test_case = GetParam();

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  InitializeAbr(test_case.slot_index);
  std::string command = std::string("getvar:slot-retry-count:") + test_case.slot_str;
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAY7"}));

  MarkUnbootable(test_case.slot_index);
  transport.ClearOutPackets();

  transport.AddInPacket(command);
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAY0"}));
}

TEST_P(FastbootSlotABTest, GetVarSlotSuccessful) {
  FastbootSlotTestCase const& test_case = GetParam();

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  MarkSuccesful(test_case.slot_index);
  std::string command = std::string{"getvar:slot-successful:"} + test_case.slot_str;
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAYyes"}));

  MarkUnbootable(test_case.slot_index);
  transport.ClearOutPackets();
  transport.AddInPacket(command);
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAYno"}));
}

INSTANTIATE_TEST_SUITE_P(FastbootSlotABTests, FastbootSlotABTest,
                         testing::ValuesIn<FastbootSlotABTest::ParamType>({
                             {kAbrSlotIndexA, "a"},
                             {kAbrSlotIndexB, "b"},
                         }),
                         [](testing::TestParamInfo<FastbootSlotABTest::ParamType> const& info) {
                           return info.param.slot_str;
                         });

TEST_F(FastbootSlotTest, GetVarSlotSuccessfulR) {
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;
  InitializeAbr(kAbrSlotIndexR);

  std::string command = std::string{"getvar:slot-successful:r"};
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAYyes"}));
}

TEST_F(FastbootSlotTest, GetVarSlotBootableR) {
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;
  InitializeAbr(kAbrSlotIndexR);

  std::string command = std::string{"getvar:slot-unbootable:r"};
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAYno"}));
}

TEST_F(FastbootSlotTest, GetVarAll) {
  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOps();
  Fastboot fastboot(download_buffer, zb_ops);
  fastboot::TestTransport transport;

  InitializeAbr(kAbrSlotIndexA);
  transport.AddInPacket(std::string_view("getvar:all"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  // Check for a few variables, making sure that we check
  // * Compile time constant variables
  // * Runtime dynamic variables with no extra parameters
  // * Runtime dynamic variables with multiple possible parameters
  const fastboot::Packets& actual = transport.GetOutPackets();
  EXPECT_NE(std::find(actual.begin(), actual.end(), "INFOversion:0.4"), actual.end());
  EXPECT_NE(std::find(actual.begin(), actual.end(), "INFOcurrent-slot:a"), actual.end());
  EXPECT_NE(std::find(actual.begin(), actual.end(), "INFOslot-successful:a:no"), actual.end());
  EXPECT_NE(std::find(actual.begin(), actual.end(), "INFOslot-successful:b:no"), actual.end());
}

TEST_F(FastbootSlotTest, GetVarAllErrors) {
  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOps();
  Fastboot fastboot(download_buffer, zb_ops);
  fastboot::TestTransport transport;

  mock_zb_ops().RemoveAllPartitions();
  transport.AddInPacket(std::string_view("getvar:all"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  // We should still get some output, but not any variables that read partitions.
  const fastboot::Packets& actual = transport.GetOutPackets();
  EXPECT_EQ(std::find(actual.begin(), actual.end(), "INFOcurrent-slot:a"), actual.end());
  EXPECT_EQ(std::find(actual.begin(), actual.end(), "INFOcurrent-slot:b"), actual.end());

  // Failures should give some message
  EXPECT_NE(std::find(actual.begin(), actual.end(), "FAILFailed to get slot last set active(-1)"),
            actual.end());

  // We should get variables emitted before the error without a problem
  EXPECT_NE(std::find(actual.begin(), actual.end(), "INFOmax-download-size:0x00002000"),
            actual.end());
}

struct FastbootSetActiveErrorTestCase {
  char const* name;
  char const* user_str;
};

class FastbootSetActiveUserErrorTest
    : public FastbootSlotTest,
      public testing::WithParamInterface<FastbootSetActiveErrorTestCase> {};

TEST_P(FastbootSetActiveUserErrorTest, TestFastbootSetActiveUserError) {
  FastbootSetActiveErrorTestCase const& test_case = GetParam();

  InitializeAbr(kAbrSlotIndexR);
  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOps();
  Fastboot fastboot(download_buffer, zb_ops);
  fastboot::TestTransport transport;

  std::string cmd = std::string{"set_active"} + test_case.user_str;
  transport.AddInPacket(cmd);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

INSTANTIATE_TEST_SUITE_P(
    FastbootSetActiveUserErrorTests, FastbootSetActiveUserErrorTest,
    testing::ValuesIn<FastbootSetActiveUserErrorTest::ParamType>({
        {"no_name", ""},
        {"no_such_name", ":squid"},
        {"cant_set_r", ":r"},
    }),
    [](testing::TestParamInfo<FastbootSetActiveUserErrorTest::ParamType> const& info) {
      return info.param.name;
    });

TEST_F(FastbootSlotTest, SetActiveSlotWriteFailure) {
  // Do NOT call InitializeAbr in order to simulate
  // a write error due to a missing partition.

  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOps();

  Fastboot fastboot(download_buffer, zb_ops);
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("set_active:b"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

struct VarErrorTestCase {
  char const* test_name;
  char const* var;
};

using VarErrorTest = ::testing::TestWithParam<VarErrorTestCase>;

TEST_P(VarErrorTest, TestVarError) {
  VarErrorTestCase const& test_case = GetParam();

  MockZirconBootOps mock_zb_ops;
  Fastboot fastboot(download_buffer, mock_zb_ops.GetZirconBootOps());
  fastboot::TestTransport transport;
  std::string command = std::string{"getvar:"} + test_case.var;

  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

INSTANTIATE_TEST_SUITE_P(VarErrorTests, VarErrorTest,
                         testing::ValuesIn<VarErrorTest::ParamType>({
                             // slot-retry-count
                             {"slot_retry_count_too_few_args", "slot-retry-count"},
                             {"slot_retry_count_invalid_name", "slot-retry-count:r"},
                             {"slot_retry_count_read_error", "slot-retry-count:a"},
                             // slot-successful
                             {"slot_successful_too_few_args", "slot-successful"},
                             {"slot_successful_invalid_name", "slot-successful:squid"},
                             {"slot_successful_read_error", "slot-successful:a"},
                             // slot-last-set-actvie
                             {"slot_last_set_active_read_error", "slot-last-set-active"},
                             // slot-unbootable
                             {"slot_unbootable_too_few_args", "slot-unbootable"},
                             {"slot_unbootable_invalid_name", "slot-unbootable:squid"},
                             {"slot_unbootable_read_error", "slot-unbootable:a"},
                             // nonexistent variable
                             {"nonexistent_variable", "non-existing"},
                             // too few args
                             {"no_var", ""},
                         }),
                         [](testing::TestParamInfo<VarErrorTest::ParamType> const& info) {
                           return info.param.test_name;
                         });

constexpr gpt_entry_t GenerateBasicGptEntry(const char* name, size_t partition_size) {
  gpt_entry_t entry = {
      // The GUID and flags aren't important for the lookup.
      .first = kGptFirstUsableBlocks,
      .last = kGptFirstUsableBlocks + DivideRoundUp(partition_size, kBlockSize),
  };
  SetGptEntryName(name, entry);
  return entry;
}

TEST_F(FastbootFlashTest, FlashPartition) {
  constexpr size_t partition_size = 0x100;
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  AddPartition(GenerateBasicGptEntry(GPT_ZIRCON_A_NAME, partition_size));
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());

  // Download some data to flash to the partition.
  std::vector<uint8_t> download_content(partition_size);
  std::iota(download_content.begin(), download_content.end(), 0);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("flash:") + GPT_ZIRCON_A_NAME);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));

  uint8_t read_buf[partition_size] = {0};
  auto gpt_device = FindEfiGptDevice();
  ASSERT_TRUE(gpt_device.is_ok());
  ASSERT_TRUE(gpt_device->ReadPartition(GPT_ZIRCON_A_NAME, 0, sizeof(read_buf), read_buf).is_ok());
  ASSERT_TRUE(memcmp(read_buf, download_content.data(), download_content.size()) == 0);
}

TEST_F(FastbootFlashTest, DownloadTooLarge) {
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());

  char download_command[fastboot::kMaxCommandPacketSize];
  snprintf(download_command, fastboot::kMaxCommandPacketSize, "download:%08zx",
           kDownloadBufferSize + 1);

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string(download_command));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, FlashPartitionFailedToWritePartition) {
  // Do NOT add any partitions. Write should fail.
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());

  // Download some data to flash to the partition.
  std::vector<uint8_t> download_content(128, 0);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("flash:") + GPT_ZIRCON_A_NAME);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  // Should fail while searching for gpt device.
  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

template <typename T>
void AddBytes(std::vector<uint8_t>& vec, const T& data) {
  const uint8_t* data_bytes = reinterpret_cast<const uint8_t*>(&data);
  vec.insert(vec.end(), data_bytes, data_bytes + sizeof(data));
}

std::vector<uint8_t> SetupSparseImage(size_t partition_size, size_t chunk_blocks) {
  std::vector<uint8_t> buffer;
  buffer.reserve((sizeof(sparse_header_t) + sizeof(chunk_header_t) + sizeof(uint32_t)));
  sparse_header_t header = {
      SPARSE_HEADER_MAGIC,
      1,
      0,
      sizeof(sparse_header_t),
      sizeof(chunk_header_t),
      static_cast<uint32_t>(partition_size),
      1,
      1,
      0xDEADBEEF,
  };
  AddBytes(buffer, header);
  chunk_header_t chunk = {
      CHUNK_TYPE_FILL,
      0,
      static_cast<uint16_t>(chunk_blocks),
      sizeof(chunk_header_t) + sizeof(uint32_t),
  };
  AddBytes(buffer, chunk);
  uint32_t payload = 0xFAFAFAFA;
  AddBytes(buffer, payload);

  return buffer;
}

TEST_F(FastbootFlashTest, FlashSparseImage) {
  constexpr size_t partition_size = 0x100;
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  AddPartition(GenerateBasicGptEntry(GPT_ZIRCON_A_NAME, partition_size));
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());

  std::vector<uint8_t> sparse_image = SetupSparseImage(partition_size, 1);

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, sparse_image));

  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("flash:") + GPT_ZIRCON_A_NAME);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAY"}));

  std::vector<uint8_t> actual(partition_size);
  auto gpt_device = FindEfiGptDevice();
  ASSERT_TRUE(gpt_device.is_ok());
  ASSERT_TRUE(
      gpt_device->ReadPartition(GPT_ZIRCON_A_NAME, 0, actual.size(), actual.data()).is_ok());

  std::vector<uint8_t> expected(partition_size, 0xFA);
  ASSERT_EQ(actual, expected);
}

TEST_F(FastbootFlashTest, FlashSparseImageOldName) {
  constexpr size_t partition_size = 0x100;
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  AddPartition(GenerateBasicGptEntry(GUID_FVM_NAME, partition_size));
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());

  std::vector<uint8_t> sparse_image = SetupSparseImage(partition_size, 1);

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, sparse_image));

  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("flash:") + GPT_FVM_NAME);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAY"}));

  std::vector<uint8_t> actual(partition_size);
  auto gpt_device = FindEfiGptDevice();
  ASSERT_TRUE(gpt_device.is_ok());
  ASSERT_TRUE(gpt_device->ReadPartition(GUID_FVM_NAME, 0, actual.size(), actual.data()).is_ok());

  std::vector<uint8_t> expected(partition_size, 0xFA);
  ASSERT_EQ(actual, expected);
}

TEST_F(FastbootFlashTest, FlashSparseImageNoSuchPartition) {
  constexpr size_t partition_size = 0x100;
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  AddPartition(GenerateBasicGptEntry(GPT_ZIRCON_A_NAME, partition_size));
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());

  std::vector<uint8_t> sparse_image = SetupSparseImage(partition_size, 1);

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, sparse_image));

  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("flash:") + "nonexistant");
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
}

TEST_F(FastbootFlashTest, FlashSparseImageTooBig) {
  constexpr size_t partition_size = 0x100;
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  AddPartition(GenerateBasicGptEntry(GPT_ZIRCON_A_NAME, partition_size));
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());

  std::vector<uint8_t> sparse_image = SetupSparseImage(partition_size, 0x100);

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, sparse_image));

  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("flash:") + GPT_ZIRCON_A_NAME);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
}

// TODO(b/235489025): Extends `StubBootServices` to cover the mock of efi_runtime_services.
EFIAPI efi_status ResetSystemSucceed(efi_reset_type, efi_status, size_t, void*) {
  return EFI_SUCCESS;
}

class EfiBootbyteOwner {
 public:
  EfiBootbyteOwner() {
    EFI_RETVAL = 0;
    DATA = 0;
  }

  EfiBootbyteOwner(efi_status status, RebootMode mode) {
    EFI_RETVAL = status;
    DATA = static_cast<uint8_t>(mode);
  }

  static EFIAPI efi_status GetVariable(char16_t* var_name, efi_guid* vendor_guid,
                                       uint32_t* attributes, size_t* data_size, void* data) {
    *data_size = sizeof(DATA);
    *reinterpret_cast<uint8_t*>(data) = DATA;

    return EFI_RETVAL;
  }

  static EFIAPI efi_status SetVariable(char16_t* var_name, efi_guid* vendor_guid,
                                       uint32_t attributes, size_t data_size, const void* data) {
    DATA = *reinterpret_cast<uint8_t const*>(data);

    return EFI_RETVAL;
  }

  ~EfiBootbyteOwner() {
    DATA = 0;
    EFI_RETVAL = EFI_SUCCESS;
  }

 private:
  static uint8_t DATA;
  static efi_status EFI_RETVAL;
};

uint8_t EfiBootbyteOwner::DATA = 0;
efi_status EfiBootbyteOwner::EFI_RETVAL = 0;

TEST_F(FastbootFlashTest, RebootNormal) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  EfiBootbyteOwner efi_var;
  efi_runtime_services runtime_services{
      .GetVariable = efi_var.GetVariable,
      .SetVariable = efi_var.SetVariable,
      .ResetSystem = ResetSystemSucceed,
  };
  gEfiSystemTable->RuntimeServices = &runtime_services;

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;
  mock_zb_ops().AddPartition(GPT_DURABLE_BOOT_NAME, sizeof(AbrData));

  // Set to a different initial boot mode.
  ASSERT_TRUE(SetRebootMode(RebootMode::kBootloader));

  transport.AddInPacket(std::string("reboot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));

  AbrOps abr_ops = mock_zb_ops().GetAbrOps();
  AbrDataOneShotFlags one_shot_flags;
  ASSERT_EQ(AbrGetAndClearOneShotFlags(&abr_ops, &one_shot_flags), kAbrResultOk);
  std::optional<RebootMode> mode_option = GetOneShotRebootMode(one_shot_flags);
  ASSERT_TRUE(mode_option);
  ASSERT_EQ(*mode_option, RebootMode::kNormal);
}

TEST_F(FastbootFlashTest, RebootBootloader) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  EfiBootbyteOwner efi_var;
  efi_runtime_services runtime_services{
      .GetVariable = efi_var.GetVariable,
      .SetVariable = efi_var.SetVariable,
      .ResetSystem = ResetSystemSucceed,
  };
  gEfiSystemTable->RuntimeServices = &runtime_services;

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;
  mock_zb_ops().AddPartition(GPT_DURABLE_BOOT_NAME, sizeof(AbrData));

  // Set to a different initial boot mode.
  ASSERT_TRUE(SetRebootMode(RebootMode::kNormal));

  transport.AddInPacket(std::string("reboot-bootloader"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));

  AbrOps abr_ops = mock_zb_ops().GetAbrOps();
  AbrDataOneShotFlags one_shot_flags;
  ASSERT_EQ(AbrGetAndClearOneShotFlags(&abr_ops, &one_shot_flags), kAbrResultOk);
  std::optional<RebootMode> mode_option = GetOneShotRebootMode(one_shot_flags);
  ASSERT_TRUE(mode_option);
  ASSERT_EQ(*mode_option, RebootMode::kBootloader);
}

TEST_F(FastbootFlashTest, RebootRecovery) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  EfiBootbyteOwner efi_var;
  efi_runtime_services runtime_services{
      .GetVariable = efi_var.GetVariable,
      .SetVariable = efi_var.SetVariable,
      .ResetSystem = ResetSystemSucceed,
  };
  gEfiSystemTable->RuntimeServices = &runtime_services;

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;
  mock_zb_ops().AddPartition(GPT_DURABLE_BOOT_NAME, sizeof(AbrData));

  // Set to a different initial boot mode.
  ASSERT_TRUE(SetRebootMode(RebootMode::kNormal));

  transport.AddInPacket(std::string("reboot-recovery"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));

  AbrOps abr_ops = mock_zb_ops().GetAbrOps();
  AbrDataOneShotFlags one_shot_flags;
  ASSERT_EQ(AbrGetAndClearOneShotFlags(&abr_ops, &one_shot_flags), kAbrResultOk);
  std::optional<RebootMode> mode_option = GetOneShotRebootMode(one_shot_flags);
  ASSERT_TRUE(mode_option);
  ASSERT_EQ(*mode_option, RebootMode::kRecovery);
}

TEST_F(FastbootFlashTest, RebootSetRebootModeFail) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());

  EfiBootbyteOwner efi_var(EFI_DEVICE_ERROR, RebootMode::kNormal);
  efi_runtime_services runtime_services{
      .GetVariable = efi_var.GetVariable,
      .SetVariable = efi_var.SetVariable,
      .ResetSystem = ResetSystemSucceed,
  };
  gEfiSystemTable->RuntimeServices = &runtime_services;

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("reboot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

EFIAPI efi_status ResetSystemFailed(efi_reset_type, efi_status, size_t, void*) {
  return EFI_ABORTED;
}

TEST_F(FastbootFlashTest, RebootResetSystemFail) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  EfiBootbyteOwner efi_var;
  efi_runtime_services runtime_services{
      .GetVariable = efi_var.GetVariable,
      .SetVariable = efi_var.SetVariable,
      .ResetSystem = ResetSystemFailed,
  };
  gEfiSystemTable->RuntimeServices = &runtime_services;

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("reboot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  // We should still receive an OKAY packet.
  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootDeviceLocked) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kLocked);
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOpsWithAvb());
  std::vector<uint8_t> download_content(sizeof(zbi_header_t));
  ASSERT_EQ(zbi_init(download_content.data(), download_content.size()), ZBI_RESULT_OK);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  std::vector<std::string> expected_packets = {"FAILDevice is locked - fastboot boot not allowed"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootDeviceLockCheckFailed) {
  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOps();
  zb_ops.verified_boot_read_is_device_locked = nullptr;
  Fastboot fastboot(download_buffer, zb_ops);
  std::vector<uint8_t> download_content(sizeof(zbi_header_t));
  ASSERT_EQ(zbi_init(download_content.data(), download_content.size()), ZBI_RESULT_OK);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  std::vector<std::string> expected_packets = {"FAILDevice is locked - fastboot boot not allowed"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootNoImageDownloaded) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOpsWithAvb());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  std::vector<std::string> expected_packets = {"FAILNo image downloaded"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootImageTooSmall) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOpsWithAvb());
  std::vector<uint8_t> download_content = {0x01, 0x02, 0x03, 0x04};
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  std::vector<std::string> expected_packets = {"FAILDownload too small to be a ZBI"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootNotZbiContainer) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOpsWithAvb());
  std::vector<uint8_t> download_content(sizeof(zbi_header_t), 0xaa);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  std::vector<std::string> expected_packets = {"FAILDownloaded image is not a ZBI container"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootLengthExceedsDownloadSize) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOpsWithAvb());
  std::vector<uint8_t> download_content(sizeof(zbi_header_t));
  ASSERT_EQ(zbi_init(download_content.data(), download_content.size()), ZBI_RESULT_OK);
  zbi_header_t* header = reinterpret_cast<zbi_header_t*>(download_content.data());
  header->length = 1024;  // Claims more payload than was downloaded.
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  std::vector<std::string> expected_packets = {
      "FAILDownloaded ZBI container length exceeds download size"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootMalformedZbiEntries) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOpsWithAvb());
  std::vector<uint8_t> download_content(sizeof(zbi_header_t) * 2, 0);
  ASSERT_EQ(zbi_init(download_content.data(), download_content.size()), ZBI_RESULT_OK);
  zbi_header_t* container = reinterpret_cast<zbi_header_t*>(download_content.data());
  container->length = sizeof(zbi_header_t);
  zbi_header_t* item =
      reinterpret_cast<zbi_header_t*>(download_content.data() + sizeof(zbi_header_t));
  item->type = ZBI_TYPE_CMDLINE;
  item->length = 0;
  item->magic = 0xdeadbeef;  // Invalid item magic.
  item->flags = ZBI_FLAGS_VERSION;
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  std::vector<std::string> expected_packets = {"FAILMalformed ZBI image"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootAddZbiItemsNullptr) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOpsWithAvb();
  zb_ops.add_zbi_items = nullptr;
  Fastboot fastboot(download_buffer, zb_ops);

  std::vector<uint8_t> download_content(sizeof(zbi_header_t));
  ASSERT_EQ(zbi_init(download_content.data(), download_content.size()), ZBI_RESULT_OK);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  std::vector<std::string> expected_packets = {"FAILFailed to add ZBI items(-1)"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootAddZbiItemsFailure) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  mock_zb_ops().SetAddDeviceZbiItemsMethod(
      [](zbi_header_t*, size_t, const AbrSlotIndex*) { return false; });
  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOpsWithAvb());

  std::vector<uint8_t> download_content(sizeof(zbi_header_t));
  ASSERT_EQ(zbi_init(download_content.data(), download_content.size()), ZBI_RESULT_OK);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  std::vector<std::string> expected_packets = {"FAILFailed to add ZBI items(-1)"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootFlashTest, BootSuccess) {
  mock_zb_ops().SetDeviceLockStatus(MockZirconBootOps::LockStatus::kUnlocked);
  bool add_zbi_items_called = false;
  bool slot_is_null = false;
  mock_zb_ops().SetAddDeviceZbiItemsMethod(
      [&](zbi_header_t* image, size_t capacity, const AbrSlotIndex* slot) {
        add_zbi_items_called = true;
        slot_is_null = (slot == nullptr);
        return true;
      });

  static bool boot_called = false;
  static const zbi_header_t* booted_image = nullptr;
  static size_t booted_capacity = 0;
  boot_called = false;
  booted_image = nullptr;
  booted_capacity = 0;

  ZirconBootOps zb_ops = mock_zb_ops().GetZirconBootOpsWithAvb();
  zb_ops.boot = [](ZirconBootOps* ops, zbi_header_t* image, size_t capacity) {
    boot_called = true;
    booted_image = image;
    booted_capacity = capacity;
  };

  Fastboot fastboot(download_buffer, zb_ops);

  std::vector<uint8_t> download_content(sizeof(zbi_header_t));
  ASSERT_EQ(zbi_init(download_content.data(), download_content.size()), ZBI_RESULT_OK);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  fastboot::TestTransport transport;
  transport.AddInPacket(std::string("boot"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  // Boot() never returns on success on a real device. In this test environment,
  // the mock boot callback returns, so ProcessPacket finishes with an error response.
  ASSERT_TRUE(ret.is_error());

  ASSERT_TRUE(add_zbi_items_called);
  ASSERT_TRUE(slot_is_null);
  ASSERT_TRUE(boot_called);
  ASSERT_EQ(reinterpret_cast<const uint8_t*>(booted_image), download_buffer);
  ASSERT_EQ(booted_capacity, sizeof(download_buffer));

  const auto& out_packets = transport.GetOutPackets();
  ASSERT_GE(out_packets.size(), 1ULL);
  EXPECT_EQ(out_packets[0], "OKAY");
}

TEST_F(FastbootFlashTest, GptReinitialize) {
  PartitionMap::PartitionEntry custom_partitions[] = {
      {GPT_DURABLE_BOOT_NAME, 0x1000, GPT_DURABLE_BOOT_TYPE_GUID},
      {GPT_FVM_NAME, SIZE_MAX, GPT_FVM_TYPE_GUID},
  };
  PartitionCustomizer customizer(custom_partitions);

  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());

  auto res = FindEfiGptDevice();
  ASSERT_TRUE(res.is_ok());
  EfiGptBlockDevice gpt_device = std::move(res.value());

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("oem gpt-init"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAY"}));

  // Check the durable_boot partition
  gpt_entry_t const* durable_boot_entry = gpt_device.FindPartition("durable_boot");
  ASSERT_NE(durable_boot_entry, nullptr);

  PartitionMap::PartitionEntry const& durable_boot_partition = custom_partitions[0];
  size_t durable_boot_size_bytes =
      (durable_boot_entry->last - durable_boot_entry->first + 1) * gpt_device.BlockSize();

  ASSERT_EQ(durable_boot_size_bytes, durable_boot_partition.min_size_bytes);
  ASSERT_TRUE(memcmp(durable_boot_entry->type, durable_boot_partition.type_guid,
                     sizeof(durable_boot_partition.type_guid)) == 0);

  // Check the fvm partition
  gpt_entry_t const* fvm_entry = gpt_device.FindPartition("fvm");
  ASSERT_NE(fvm_entry, nullptr);

  PartitionMap::PartitionEntry const& fvm_partition = *(std::end(custom_partitions) - 1);

  // The fvm partition takes all remaining space on disk,
  // so its last block is the block right before the backup GPT.
  ASSERT_EQ(fvm_entry->last, gpt_device.GptHeader().last);
  ASSERT_TRUE(memcmp(fvm_entry->type, fvm_partition.type_guid, sizeof(fvm_partition.type_guid)) ==
              0);

  auto names = gpt_device.ListPartitionNames();
  ASSERT_EQ(names.size(), 2UL);
  ASSERT_EQ(names[0].data(), custom_partitions[0].name);
  ASSERT_EQ(names[1].data(), custom_partitions[1].name);
}

TEST_F(FastbootFlashTest, GptReinitializeNoMaxSize) {
  PartitionMap::PartitionEntry custom_partitions[] = {
      {GPT_DURABLE_BOOT_NAME, 0x1000, GPT_DURABLE_BOOT_TYPE_GUID},
      {GPT_FACTORY_NAME, 0x1000, GPT_FACTORY_TYPE_GUID},
  };
  PartitionCustomizer customizer(custom_partitions);

  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());

  auto res = FindEfiGptDevice();
  ASSERT_TRUE(res.is_ok());
  EfiGptBlockDevice gpt_device = std::move(res.value());

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("oem gpt-init"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), {"OKAY"}));

  // Check the durable_boot partition
  gpt_entry_t const* durable_boot_entry = gpt_device.FindPartition("durable_boot");
  ASSERT_NE(durable_boot_entry, nullptr);

  PartitionMap::PartitionEntry const& durable_boot_partition = custom_partitions[0];
  size_t durable_boot_size_bytes =
      (durable_boot_entry->last - durable_boot_entry->first + 1) * gpt_device.BlockSize();

  ASSERT_EQ(durable_boot_size_bytes, durable_boot_partition.min_size_bytes);
  ASSERT_TRUE(memcmp(durable_boot_entry->type, durable_boot_partition.type_guid,
                     sizeof(durable_boot_partition.type_guid)) == 0);

  // Check the durable partition
  gpt_entry_t const* durable_entry = gpt_device.FindPartition("factory");
  ASSERT_NE(durable_entry, nullptr);

  PartitionMap::PartitionEntry const& durable_partition = custom_partitions[1];
  size_t durable_size_bytes =
      (durable_entry->last - durable_entry->first + 1) * gpt_device.BlockSize();

  ASSERT_EQ(durable_size_bytes, durable_partition.min_size_bytes);
  ASSERT_TRUE(memcmp(durable_entry->type, durable_partition.type_guid,
                     sizeof(durable_partition.type_guid)) == 0);
}

TEST_F(FastbootFlashTest, GptReinitializeTooBigPartitionsFailure) {
  // There are only 1024 blocks in the mock disk device,
  // which translates to 0x80000 bytes assuming 512 byte blocks.
  PartitionMap::PartitionEntry custom_partitions[] = {
      {GPT_DURABLE_BOOT_NAME, 0xFFFFFF, GPT_DURABLE_BOOT_TYPE_GUID},
  };
  PartitionCustomizer customizer(custom_partitions);

  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());

  auto res = FindEfiGptDevice();
  ASSERT_TRUE(res.is_ok());
  EfiGptBlockDevice gpt_device = std::move(res.value());

  // Quick check to make sure the partition will in fact be too large.
  ASSERT_EQ(gpt_device.BlockSize(), 512UL);

  Fastboot fastboot(download_buffer, mock_zb_ops().GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("oem gpt-init"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST(FastbootTest, GptReinitializeDiskFailure) {
  MockStubService stub_service;
  Device image_device({"path-A", "path-B", "path-C", "image"});
  BlockDevice block_device({"path-A", "path-B", "path-D"}, 1024);
  auto cleanup = SetupEfiGlobalState(stub_service, image_device);

  stub_service.AddDevice(&image_device);
  stub_service.AddDevice(&block_device);

  auto res = FindEfiGptDevice();
  ASSERT_TRUE(res.is_error());

  MockZirconBootOps mock_zb_ops;
  Fastboot fastboot(download_buffer, mock_zb_ops.GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("oem gpt-init"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, GptReinitializeTwoMaxPartFailure) {
  PartitionMap::PartitionEntry custom_partitions[] = {
      {GPT_DURABLE_BOOT_NAME, SIZE_MAX},
      {GPT_FACTORY_NAME, SIZE_MAX},
  };
  PartitionCustomizer customizer(custom_partitions);

  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());

  auto res = FindEfiGptDevice();
  ASSERT_TRUE(res.is_ok());

  MockZirconBootOps mock_zb_ops;
  Fastboot fastboot(download_buffer, mock_zb_ops.GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("oem gpt-init"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
}

TEST_F(FastbootFlashTest, GptReinitializeMaxNotLastFailure) {
  PartitionMap::PartitionEntry custom_partitions[] = {
      {GPT_DURABLE_BOOT_NAME, SIZE_MAX},
      {GPT_FACTORY_NAME, 0x1000},
  };
  PartitionCustomizer customizer(custom_partitions);

  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());

  auto res = FindEfiGptDevice();
  ASSERT_TRUE(res.is_ok());

  MockZirconBootOps mock_zb_ops;
  Fastboot fastboot(download_buffer, mock_zb_ops.GetZirconBootOps());
  fastboot::TestTransport transport;

  transport.AddInPacket(std::string("oem gpt-init"));
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
}

TEST_F(FastbootFlashTest, AddBootloaderFile) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  ClearBootloaderFiles();

  MockZirconBootOps mock_zb_ops;
  Fastboot fastboot(download_buffer, mock_zb_ops.GetZirconBootOps());

  // Download some data to use as file content.
  constexpr char kFileContent[] = "file content";
  std::vector<uint8_t> download_content(kFileContent, kFileContent + sizeof(kFileContent));
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  std::string file_name = "ssh.authorized_keys";
  std::string command = "oem add-staged-bootloader-file " + file_name;
  fastboot::TestTransport transport;
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));

  // Extract the bootloader file content.
  std::vector<zbitl::ByteView> items = FindItems(GetZbiFiles().data(), ZBI_TYPE_BOOTLOADER_FILE);
  ASSERT_EQ(items.size(), 1ULL);

  // Validate file content.
  std::string_view expected("\x13ssh.authorized_keysfile content");
  ASSERT_EQ(expected.size() + 1, items[0].size());
  ASSERT_EQ(memcmp(expected.data(), items[0].data(), expected.size()), 0);
}

TEST_F(FastbootFlashTest, AddBootloaderFileNotEnoughtArguments) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  ClearBootloaderFiles();

  MockZirconBootOps mock_zb_ops;
  Fastboot fastboot(download_buffer, mock_zb_ops.GetZirconBootOps());

  // Download some data to use as file content.
  constexpr char kFileContent[] = "file content";
  std::vector<uint8_t> download_content(kFileContent, kFileContent + sizeof(kFileContent));
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  std::string command = "oem add-staged-bootloader-file";
  fastboot::TestTransport transport;
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, AddBootloaderFileTooLarge) {
  auto cleanup = SetupEfiGlobalState(stub_service(), image_device());
  ClearBootloaderFiles();

  MockZirconBootOps mock_zb_ops;
  Fastboot fastboot(download_buffer, mock_zb_ops.GetZirconBootOps());

  // Download some data to use as file content.
  const std::string kFileContent(kDownloadBufferSize, 'a');
  std::vector<uint8_t> download_content(kFileContent.begin(), kFileContent.end());
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  std::string command = "oem add-staged-bootloader-file ssh.authorized_keys";
  fastboot::TestTransport transport;
  transport.AddInPacket(command);
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
  auto sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

// Fake printer for test to capture dump output. Class wrapper is
// to ensure that the capture doesn't leak between tests since we
// need to use a global singleton to hook into the raw C function.
class PrintCapture {
 public:
  static std::unique_ptr<PrintCapture> Create() {
    if (PrintCapture::instance_) {
      ADD_FAILURE() << "Can't create 2 PrintCapture objects at a time";
      return nullptr;
    }

    PrintCapture::instance_ = new PrintCapture();
    return std::unique_ptr<PrintCapture>(PrintCapture::instance_);
  }

  ~PrintCapture() { instance_ = nullptr; }

  static int PrintFunction(const char* fmt, ...) {
    if (!instance_) {
      ADD_FAILURE() << "No PrintCapture exists";
      return -1;
    }

    va_list ap;
    va_start(ap, fmt);
    VPrintFunction(fmt, ap);
    va_end(ap);

    return 0;
  }

  static int VPrintFunction(const char* fmt, va_list ap) {
    if (!instance_) {
      ADD_FAILURE() << "No PrintCapture exists";
      return -1;
    }

    fxl::StringVAppendf(&instance_->contents(), fmt, ap);
    return 0;
  }

  std::string& contents() { return contents_; }

  std::string dump_contents() {
    std::string ret(std::move(contents_));
    contents_.clear();
    return ret;
  }

 private:
  // Private constructor to prevent instantiating 2 at once.
  PrintCapture() = default;

  // Singleton to dispatch calls to.
  static PrintCapture* instance_;

  std::string contents_;
};
PrintCapture* PrintCapture::instance_;

class FastbootEfiVarTest : public ::testing::Test {
 public:
  void SetUp() override {
    capture = PrintCapture::Create();
    fastboot.SetVPrintFunction(PrintCapture::VPrintFunction);
  }

  void TearDown() override { capture.reset(); }

  std::unique_ptr<PrintCapture> capture;
  fastboot::TestTransport transport;
  MockZirconBootOps mock_zb_ops;
  MockEfiVariables mock_efi_variables;
  Fastboot fastboot =
      Fastboot(download_buffer, mock_zb_ops.GetZirconBootOps(), &mock_efi_variables);
};

TEST_F(FastbootEfiVarTest, EfiGetVarInfo_Error) {
  EXPECT_CALL(mock_efi_variables, EfiQueryVariableInfo())
      .WillOnce(Return(fit::error(EFI_NOT_FOUND)));

  transport.AddInPacket(std::string("oem efi-getvarinfo"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_THAT(capture->dump_contents(), IsEmpty());

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"FAILQueryVariableInfo() failed"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiGetVarInfo_Ok) {
  EXPECT_CALL(mock_efi_variables, EfiQueryVariableInfo())
      .WillOnce(Return(fit::success(EfiVariables::EfiVariableInfo{0, 1, 2})));
  constexpr std::string_view expected_output =
      "\n  Max Storage Size: 0\n  Remaining Variable Storage Size: 1\n  Max Variable Size: 2\n";

  transport.AddInPacket(std::string("oem efi-getvarinfo"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiGetVarNames_Error) {
  EXPECT_CALL(mock_efi_variables, EfiGetNextVariableName(_))
      .WillOnce(Return(fit::error(EFI_INVALID_PARAMETER)));

  transport.AddInPacket(std::string("oem efi-getvarnames"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_THAT(capture->dump_contents(), IsEmpty());

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"FAILEfiVariableName iteration failed"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiGetVarNames_Empty) {
  EXPECT_CALL(mock_efi_variables, EfiGetNextVariableName(_))
      .WillOnce(Return(fit::error(EFI_NOT_FOUND)));
  const auto expected_output = "\n";

  transport.AddInPacket(std::string("oem efi-getvarnames"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

constexpr efi_guid kGuid[] = {
    {0x0, 0x0, 0x0, {0x0}},
    {0x1, 0x1, 0x1, {0x1}},
    {0x00010203, 0x0405, 0x0607, {0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f}},
};
constexpr size_t kVariableIdSize = std::size(kGuid);

const std::array<efi::VariableId, kVariableIdSize>& VariableIds() {
  static std::array<efi::VariableId, kVariableIdSize>* variable_id =
      new std::array<efi::VariableId, kVariableIdSize>{
          efi::VariableId{efi::String("var_0"), kGuid[0]},
          efi::VariableId{efi::String("var_1"), kGuid[1]},
          efi::VariableId{efi::String("var_2"), kGuid[2]},
  };
  return *variable_id;
}

const std::array<std::vector<uint8_t>, kVariableIdSize>& VariableValues() {
  static std::array<std::vector<uint8_t>, kVariableIdSize>* values =
      new std::array<std::vector<uint8_t>, kVariableIdSize>{
          std::vector<uint8_t>{0x00},
          std::vector<uint8_t>{0x01, 0x02},
          std::vector<uint8_t>{0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a,
                               0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15,
                               0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f},
  };
  return *values;
}

TEST_F(FastbootEfiVarTest, EfiGetVarNames_Values) {
  EXPECT_CALL(mock_efi_variables, EfiGetNextVariableName(_))
      .WillOnce(DoAll(SetArgReferee<0>(VariableIds()[0]), Return(fit::success())))
      .WillOnce(DoAll(SetArgReferee<0>(VariableIds()[1]), Return(fit::success())))
      .WillOnce(DoAll(SetArgReferee<0>(VariableIds()[2]), Return(fit::success())))
      .WillOnce(Return(fit::error(EFI_NOT_FOUND)));
  const auto expected_output =
      "00000000-0000-0000-0000-000000000000 var_0\n"
      "00000001-0001-0001-0100-000000000000 var_1\n"
      "00010203-0405-0607-0809-0a0b0c0d0e0f var_2\n"
      "\n";

  transport.AddInPacket(std::string("oem efi-getvarnames"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiGetVarValue_NameWithGuid) {
  fbl::Vector<uint8_t> kVal;
  kVal.resize(VariableValues()[2].size());
  std::copy(VariableValues()[2].begin(), VariableValues()[2].end(), kVal.begin());
  const auto expected_output =
      "00010203-0405-0607-0809-0a0b0c0d0e0f var_2:\n"
      "00010203 04050607 08090a0b 0c0d0e0f |................\n"
      "10111213 14151617 18191a1b 1c1d1e1f |................\n";

  EXPECT_CALL(mock_efi_variables, EfiGetVariable(VariableIds()[2]))
      .WillOnce(Return(fit::success(std::move(kVal))));

  transport.AddInPacket(std::string("oem efi-getvar var_2 00010203-0405-0607-0809-0a0b0c0d0e0f"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiGetVarValue_NameWithoutGuid) {
  fbl::Vector<uint8_t> kVal;
  kVal.resize(VariableValues()[2].size());
  std::copy(VariableValues()[2].begin(), VariableValues()[2].end(), kVal.begin());
  const auto expected_output =
      "00010203-0405-0607-0809-0a0b0c0d0e0f var_2:\n"
      "00010203 04050607 08090a0b 0c0d0e0f |................\n"
      "10111213 14151617 18191a1b 1c1d1e1f |................\n";

  EXPECT_CALL(mock_efi_variables, EfiGetVariable(VariableIds()[2]))
      .WillOnce(Return(fit::success(std::move(kVal))));
  EXPECT_CALL(mock_efi_variables, GetGuid(_)).WillOnce(Return(fit::success(kGuid[2])));

  transport.AddInPacket(std::string("oem efi-getvar var_2"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiGetVarValue_NameWithGuidError) {
  fbl::Vector<uint8_t> kVal;
  kVal.resize(VariableValues()[2].size());
  std::copy(VariableValues()[2].begin(), VariableValues()[2].end(), kVal.begin());
  const auto expected_output = "00010203-0405-0607-0809-0a0b0c0d0e0f var_2:\n";

  EXPECT_CALL(mock_efi_variables, EfiGetVariable(VariableIds()[2]))
      .WillOnce(Return(fit::error(EFI_INVALID_PARAMETER)));

  transport.AddInPacket(std::string("oem efi-getvar var_2 00010203-0405-0607-0809-0a0b0c0d0e0f"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"FAILGetVariable() failed"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiGetVarValue_NameWithoutGuidError) {
  fbl::Vector<uint8_t> kVal;
  kVal.resize(VariableValues()[2].size());
  std::copy(VariableValues()[2].begin(), VariableValues()[2].end(), kVal.begin());
  const auto expected_output = "Vendor GUID search failed (EFI_INVALID_PARAMETER) for: 'var_2'\n";

  EXPECT_CALL(mock_efi_variables, GetGuid(_)).WillOnce(Return(fit::error(EFI_INVALID_PARAMETER)));

  transport.AddInPacket(std::string("oem efi-getvar var_2"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {
      "FAILMultiple entries found with specified name. Please provide G"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiDumpVars_Multiple) {
  fbl::Vector<uint8_t> kVal[kVariableIdSize];
  for (size_t i = 0; i < kVariableIdSize; i++) {
    kVal[i].resize(VariableValues()[i].size());
    std::copy(VariableValues()[i].begin(), VariableValues()[i].end(), kVal[i].begin());
  }
  const auto expected_output =
      "00000000-0000-0000-0000-000000000000 var_0:\n"
      "00                                  |.\n"
      "00000001-0001-0001-0100-000000000000 var_1:\n"
      "0102                                |..\n"
      "00010203-0405-0607-0809-0a0b0c0d0e0f var_2:\n"
      "00010203 04050607 08090a0b 0c0d0e0f |................\n"
      "10111213 14151617 18191a1b 1c1d1e1f |................\n"
      "\n";

  EXPECT_CALL(mock_efi_variables, EfiGetNextVariableName(_))
      .WillOnce(DoAll(SetArgReferee<0>(VariableIds()[0]), Return(fit::success())))
      .WillOnce(DoAll(SetArgReferee<0>(VariableIds()[1]), Return(fit::success())))
      .WillOnce(DoAll(SetArgReferee<0>(VariableIds()[2]), Return(fit::success())))
      .WillOnce(Return(fit::error(EFI_NOT_FOUND)));

  for (size_t i = 0; i < kVariableIdSize; i++) {
    EXPECT_CALL(mock_efi_variables, EfiGetVariable(VariableIds()[i]))
        .WillOnce(Return(fit::success(std::move(kVal[i]))));
  }

  transport.AddInPacket(std::string("oem efi-dumpvars"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiDumpVars_Empty) {
  const auto expected_output = "\n";

  EXPECT_CALL(mock_efi_variables, EfiGetNextVariableName(_))
      .WillOnce(Return(fit::error(EFI_NOT_FOUND)));

  transport.AddInPacket(std::string("oem efi-dumpvars"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_EQ(capture->dump_contents(), expected_output);

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"OKAY"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

TEST_F(FastbootEfiVarTest, EfiDumpVars_Error) {
  EXPECT_CALL(mock_efi_variables, EfiGetNextVariableName(_))
      .WillOnce(Return(fit::error(EFI_INVALID_PARAMETER)));

  transport.AddInPacket(std::string("oem efi-dumpvars"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());
  EXPECT_THAT(capture->dump_contents(), IsEmpty());

  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  fastboot::Packets expected_packets = {"FAILEfiVariableName iteration failed"};
  ASSERT_NO_FATAL_FAILURE(CheckPacketsEqual(transport.GetOutPackets(), expected_packets));
}

class FastbootPrint : public ::testing::Test {
 public:
  fastboot::TestTransport transport;
  MockZirconBootOps mock_zb_ops;
  MockEfiVariables mock_efi_variables;
  Fastboot fastboot =
      Fastboot(download_buffer, mock_zb_ops.GetZirconBootOps(), &mock_efi_variables);
};

TEST_F(FastbootPrint, EfiGetInfoPrint2kInfo) {
  EXPECT_CALL(mock_efi_variables, EfiQueryVariableInfo())
      .WillOnce(Return(fit::success(EfiVariables::EfiVariableInfo{0, 1, 2})));
  constexpr std::string_view expected_output[] = {
      "INFO",
      "INFO  Max Storage Size: 0",
      "INFO  Remaining Variable Storage Size: 1",
      "INFO  Max Variable Size: 2",
      "OKAY",
  };

  transport.AddInPacket(std::string("oem efi-getvarinfo"));

  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok());

  ASSERT_EQ(transport.GetOutPackets().size(), 5ULL);
  EXPECT_THAT(transport.GetOutPackets(), ElementsAreArray(expected_output));
}

}  // namespace

}  // namespace gigaboot
