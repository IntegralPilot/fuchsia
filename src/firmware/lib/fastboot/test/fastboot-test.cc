// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/lib/fastboot/test/fastboot-test.h"

#include <fcntl.h>
#include <fidl/fuchsia.buildinfo/cpp/wire.h>
#include <fidl/fuchsia.buildinfo/cpp/wire_test_base.h>
#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.fshost/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire_test_base.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <fidl/fuchsia.paver/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async/dispatcher.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fastboot/fastboot.h>
#include <lib/fastboot/test/test-transport.h>

#include <future>
#include <tuple>
#include <unordered_map>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/testing/fake-paver.h"

namespace fastboot {

namespace {

using fuchsia_hardware_power_statecontrol::ShutdownAction;

TEST(FastbootTest, NoPacket) {
  Fastboot fastboot(0x40000);
  TestTransport transport;
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  Packets expected_packets = {};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST(FastbootTest, GetVarMaxDownloadSize) {
  Fastboot fastboot(0x40000);
  const char command[] = "getvar:max-download-size";
  TestTransport transport;
  transport.AddInPacket(command, strlen(command));
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  Packets expected_packets = {"OKAY0x00040000"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST(FastbootTest, GetVarUnknownVariable) {
  Fastboot fastboot(0x40000);
  const char command[] = "getvar:unknown";
  TestTransport transport;
  transport.AddInPacket(command, strlen(command));
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST(FastbootTest, GetVarNotEnoughArgument) {
  Fastboot fastboot(0x40000);
  const char command[] = "getvar";
  TestTransport transport;
  transport.AddInPacket(command, strlen(command));
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST(FastbootTest, UnknownCommand) {
  Fastboot fastboot(0x40000);
  const char command[] = "Unknown";
  TestTransport transport;
  transport.AddInPacket(command, strlen(command));
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

const char kNewLineMessage[] =
    "test line 1\n"
    "test line 2";
const size_t kNewLineMessageSplits = 2ULL;

TEST(FastbootResponseTest, FailNewLineMessageNotSplit) {
  std::string_view message(kNewLineMessage);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kFail, message, &transport, zx::ok());
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0], std::string("FAIL") + kNewLineMessage);
}

TEST(FastbootResponseTest, OkayNewLineMessageNotSplit) {
  std::string_view message(kNewLineMessage);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kOkay, message, &transport, zx::ok());
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0], std::string("OKAY") + kNewLineMessage);
}

TEST(FastbootResponseTest, InfoNewLineMessageSplitAtNewLine) {
  std::string_view message(kNewLineMessage);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kInfo, message, &transport, zx::ok());
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), kNewLineMessageSplits);
  ASSERT_EQ(sent_packets[0], std::string("INFO") + "test line 1");
  ASSERT_EQ(sent_packets[1], std::string("INFO") + "test line 2");
}

const char kLongMessage[] =
    "0123456789abcdef"
    "1123456789abcdef"
    "2123456789abcdef"
    "3123456789abcdef"
    "4123456789abcdef"
    "5123456789abcdef"
    "6123456789abcdef"
    "7123456789abcdef";
const size_t kLongMessageSplits = 3ULL;

TEST(FastbootResponseTest, FailLongMessageNotSplit) {
  std::string_view message(kLongMessage);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kFail, message, &transport, zx::ok());
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0], (std::string("FAIL") + kLongMessage).substr(0, kMaxCommandPacketSize));
}

TEST(FastbootResponseTest, OkayLongMessageNotSplit) {
  std::string_view message(kLongMessage);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kOkay, message, &transport, zx::ok());
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0], (std::string("OKAY") + kLongMessage).substr(0, kMaxCommandPacketSize));
}

TEST(FastbootResponseTest, InfoLongMessageSplitAtNewLine) {
  std::string_view message(kLongMessage);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kInfo, message, &transport, zx::ok());
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), kLongMessageSplits);
  std::string res_message;
  for (auto& pkt : sent_packets) {
    EXPECT_EQ(pkt.compare(0, 4, "INFO"), 0);
    res_message += pkt.substr(4);
  }
  EXPECT_EQ(res_message, kLongMessage);
}

TEST(FastbootResponseTest, InfoLongMessageErrorAdded) {
  const zx_status_t kExpectedError = ZX_ERR_NOT_SUPPORTED;
  const std::string kExpectedErrorString = "(ZX_ERR_NOT_SUPPORTED)";
  std::string_view message(kLongMessage);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kInfo, message, &transport,
                                    zx::error(kExpectedError));
  ASSERT_TRUE(ret.is_error());
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), kLongMessageSplits);
  std::string res_message;
  for (auto& pkt : sent_packets) {
    EXPECT_EQ(pkt.compare(0, 4, "INFO"), 0);
    res_message += pkt.substr(4);
  }
  EXPECT_EQ(res_message, kLongMessage + kExpectedErrorString);
}

// Test to cover error string at the end of the message that can be truncated (it doesn't wrap as
// message)
TEST(FastbootResponseTest, InfoLongMessageErrorAddedTruncated) {
  const zx_status_t kExpectedError = ZX_ERR_NOT_SUPPORTED;
  const std::string kExpectedErrorString = "(ZX_ERR_";  // NOT_SUPPORTED)"; Truncated error message
  std::string message = std::string(kLongMessage) + "padding                                    ";
  TestTransport transport;
  auto ret = Fastboot::SendResponse(Fastboot::ResponseType::kInfo, message, &transport,
                                    zx::error(kExpectedError));
  ASSERT_TRUE(ret.is_error());
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), kLongMessageSplits);
  std::string res_message;
  for (auto& pkt : sent_packets) {
    EXPECT_EQ(pkt.compare(0, 4, "INFO"), 0);
    res_message += pkt.substr(4);
  }
  EXPECT_EQ(res_message, message + kExpectedErrorString);
}

TEST(FastbootResponseTest, BadType) {
  const zx_status_t kExpectedError = ZX_ERR_INVALID_ARGS;
  Fastboot::ResponseType bad_type = static_cast<Fastboot::ResponseType>(123);
  TestTransport transport;
  auto ret = Fastboot::SendResponse(bad_type, {}, &transport, zx::ok());
  ASSERT_TRUE(ret.is_error());
  EXPECT_EQ(ret.error_value(), kExpectedError);
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 0ULL);
}

}  // namespace

namespace {

TEST_F(FastbootDownloadTest, DownloadSucceed) {
  Fastboot fastboot(0x40000);

  std::vector<uint8_t> download_content;
  for (size_t i = 0; i <= 0xff; i++) {
    download_content.push_back(static_cast<uint8_t>(i));
  }

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
}

TEST_F(FastbootDownloadTest, DownloadCompleteResetState) {
  Fastboot fastboot(0x40000);

  std::vector<uint8_t> download_content;
  for (size_t i = 0; i <= std::numeric_limits<uint8_t>::max(); i++) {
    download_content.push_back(static_cast<uint8_t>(i));
  }

  // Test the download command twice. The second time is to test that Fastboot re-enter
  // the command waiting state after a complete download.
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
  // Make sure that all states are reset.
  ASSERT_EQ(fastboot.remaining_download_size(), 0ULL);
  ASSERT_EQ(fastboot.state(), FastbootBase::State::kCommand);

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
}

TEST(FastbootTest, DownloadFailsOnUnexpectedAmountOfData) {
  Fastboot fastboot(0x40000);

  std::vector<uint8_t> download_content;
  for (size_t i = 0; i <= std::numeric_limits<uint8_t>::max(); i++) {
    download_content.push_back(static_cast<uint8_t>(i));
  }

  std::string size_hex_str = fxl::StringPrintf("%08zx", download_content.size());

  std::string command = "download:" + size_hex_str;
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  // Transmit the first half.
  std::vector<uint8_t> first_half(download_content.begin(),
                                  download_content.begin() + download_content.size() / 2);
  transport.AddInPacket(first_half);
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  // The second transmit sends the entire download, which will exceed expected size.
  transport.AddInPacket(download_content);
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  // Check that the last packet is a FAIL response
  ASSERT_EQ(transport.GetOutPackets().size(), 2ULL);
  ASSERT_EQ(transport.GetOutPackets().back().compare(0, 4, "FAIL"), 0);

  ASSERT_EQ(fastboot.total_download_size(), 0ULL);
  ASSERT_EQ(fastboot.remaining_download_size(), 0ULL);
  ASSERT_EQ(fastboot.state(), FastbootBase::State::kCommand);
}

TEST(FastbootTest, DownloadFailsOnZeroSizeDownload) {
  Fastboot fastboot(0x40000);
  std::string command = "download:00000000";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);

  ASSERT_EQ(fastboot.total_download_size(), 0ULL);
  ASSERT_EQ(fastboot.remaining_download_size(), 0ULL);
  ASSERT_EQ(fastboot.state(), FastbootBase::State::kCommand);
}

TEST(FastbootTest, DownloadFailsOnNotEnoughArgument) {
  Fastboot fastboot(0x40000);
  std::string command = "download";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);

  ASSERT_EQ(fastboot.total_download_size(), 0ULL);
  ASSERT_EQ(fastboot.remaining_download_size(), 0ULL);
  ASSERT_EQ(fastboot.state(), FastbootBase::State::kCommand);
}

class FastbootFailGetDownloadBuffer : public Fastboot {
 public:
  using Fastboot::Fastboot;

 private:
  zx::result<void*> GetDownloadBuffer(size_t total_download_size) override {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
};

TEST(FastbootTest, DownloadFailsOnetDownloadBuffer) {
  FastbootFailGetDownloadBuffer fastboot(0x40000);

  std::vector<uint8_t> download_content;
  for (size_t i = 0; i <= std::numeric_limits<uint8_t>::max(); i++) {
    download_content.push_back(static_cast<uint8_t>(i));
  }

  std::string size_hex_str = fxl::StringPrintf("%08zx", download_content.size());

  std::string command = "download:" + size_hex_str;
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  // Check that the last packet is a FAIL response
  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  ASSERT_EQ(transport.GetOutPackets().back().compare(0, 4, "FAIL"), 0);

  ASSERT_EQ(fastboot.total_download_size(), 0ULL);
  ASSERT_EQ(fastboot.remaining_download_size(), 0ULL);
  ASSERT_EQ(fastboot.state(), FastbootBase::State::kCommand);
}

class FastbootFlashTest : public FastbootDownloadTest {
 protected:
  FastbootFlashTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread), vfs_(loop_.dispatcher()) {
    // Set up a svc root directory with a paver service entry.
    auto root_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    root_dir->AddEntry(
        fidl::DiscoverableProtocolName<fuchsia_paver::Paver>,
        fbl::MakeRefCounted<fs::Service>([this](fidl::ServerEnd<fuchsia_paver::Paver> request) {
          fake_paver_.Connect(loop_.dispatcher(), std::move(request));
          return ZX_OK;
        }));
    zx::result server_end = fidl::CreateEndpoints(&svc_local_);
    EXPECT_TRUE(server_end.is_ok()) << server_end.status_string();
    vfs_.ServeDirectory(root_dir, std::move(server_end.value()));
    loop_.StartThread("fastboot-flash-test-loop");
  }

  fidl::ClientEnd<fuchsia_io::Directory>& svc_chan() { return svc_local_; }

  paver_test::FakePaver& paver() { return fake_paver_; }

  ~FastbootFlashTest() override {
    std::promise<zx_status_t> promise;
    vfs_.Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
    EXPECT_EQ(promise.get_future().get(), ZX_OK);
    loop_.Shutdown();
  }

  void TestFlashBootloader(Fastboot& fastboot, fuchsia_paver::wire::Configuration config,
                           const std::string& type_suffix) {
    std::vector<uint8_t> download_content(256, 1);
    ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

    paver_test::FakePaver& fake_paver = paver();
    fake_paver.set_expected_payload_size(download_content.size());

    std::unordered_map<fuchsia_paver::wire::Configuration, std::string> config_to_partition = {
        {fuchsia_paver::wire::Configuration::kA, "bootloader_a"},
        {fuchsia_paver::wire::Configuration::kB, "bootloader_b"},
        {fuchsia_paver::wire::Configuration::kRecovery, "bootloader_r"},
    };

    TestTransport transport;
    std::string command = "flash:" + config_to_partition[config] + type_suffix;

    transport.AddInPacket(command);
    zx::result<> ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << ret.status_string();

    std::vector<std::string> expected_packets = {"OKAY"};
    ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
    ASSERT_EQ(fake_paver.last_firmware_config(), config);
  }

  void TestFlashBootloaderNoFirmwareType(Fastboot& fastboot,
                                         fuchsia_paver::wire::Configuration config) {
    fake_paver_.set_supported_firmware_type("");
    ASSERT_NO_FATAL_FAILURE(TestFlashBootloader(fastboot, config, ""));
    ASSERT_EQ(fake_paver_.last_firmware_type(), "");
  }

  void TestFlashBootloaderWithFirmwareType(Fastboot& fastboot,
                                           fuchsia_paver::wire::Configuration config,
                                           const std::string& type) {
    fake_paver_.set_supported_firmware_type(type);
    ASSERT_NO_FATAL_FAILURE(TestFlashBootloader(fastboot, config, ":" + type));
    ASSERT_EQ(fake_paver_.last_firmware_type(), type);
  }

  void TestFlashAsset(const std::string& partition, fuchsia_paver::wire::Configuration config,
                      fuchsia_paver::wire::Asset asset) {
    Fastboot fastboot(0x40000, std::move(svc_chan()));
    std::vector<uint8_t> download_content(256, 1);
    ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
    paver_test::FakePaver& fake_paver = paver();
    fake_paver.set_expected_payload_size(download_content.size());

    std::string command = "flash:" + partition;
    TestTransport transport;
    transport.AddInPacket(command);
    zx::result<> ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << ret.status_string();

    std::vector<std::string> expected_packets = {"OKAY"};
    ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
    ASSERT_EQ(fake_paver.last_asset_config(), config);
    ASSERT_EQ(fake_paver.last_asset(), asset);
  }

  void TestSetActive(const std::string& slot) {
    Fastboot fastboot(0x40000, std::move(svc_chan()));
    paver().set_abr_supported(true);

    TestTransport transport;
    const std::string command = "set_active:" + slot;
    transport.AddInPacket(command);
    zx::result<> ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << ret.status_string();

    std::vector<std::string> expected_packets = {"OKAY"};
    ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
  }

  async::Loop loop_;
  fs::ManagedVfs vfs_;
  paver_test::FakePaver fake_paver_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_local_;
};

TEST_F(FastbootFlashTest, FlashFailsOnNotEnoughArguments) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));

  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  std::string command = "flash";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, FlashFailsOnUnsupportedPartition) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));

  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  std::string command = "flash:unknown-partition";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, FlashBootloaderNoAbrNoFirmwareType) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));

  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  paver_test::FakePaver& fake_paver = paver();
  fake_paver.set_expected_payload_size(download_content.size());

  TestTransport transport;
  std::string command = "flash:bootloader";

  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
  ASSERT_EQ(fake_paver.last_firmware_config(), fuchsia_paver::wire::Configuration::kA);
}

TEST_F(FastbootFlashTest, FlashBootloaderNoAbrWithFirmwareType) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  paver_test::FakePaver& fake_paver = paver();
  fake_paver.set_expected_payload_size(download_content.size());

  std::string firmware_type = "firmware_type";
  fake_paver.set_supported_firmware_type(firmware_type);

  TestTransport transport;
  std::string command = "flash:bootloader:" + firmware_type;

  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
  ASSERT_EQ(fake_paver.last_firmware_config(), fuchsia_paver::wire::Configuration::kA);
  ASSERT_EQ(fake_paver_.last_firmware_type(), firmware_type);
}

TEST_F(FastbootFlashTest, FlashBooloaderASlot) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  ASSERT_NO_FATAL_FAILURE(
      TestFlashBootloaderNoFirmwareType(fastboot, fuchsia_paver::wire::Configuration::kA));
}

TEST_F(FastbootFlashTest, FlashBooloaderBSlot) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  ASSERT_NO_FATAL_FAILURE(
      TestFlashBootloaderNoFirmwareType(fastboot, fuchsia_paver::wire::Configuration::kB));
}

TEST_F(FastbootFlashTest, FlashBooloaderRSlot) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  ASSERT_NO_FATAL_FAILURE(
      TestFlashBootloaderNoFirmwareType(fastboot, fuchsia_paver::wire::Configuration::kRecovery));
}

TEST_F(FastbootFlashTest, FlashBooloaderASlotWithFirmwareType) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  ASSERT_NO_FATAL_FAILURE(TestFlashBootloaderWithFirmwareType(
      fastboot, fuchsia_paver::wire::Configuration::kA, "firmware_type"));
}

TEST_F(FastbootFlashTest, FlashBooloaderBSlotWithFirmwareType) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  ASSERT_NO_FATAL_FAILURE(TestFlashBootloaderWithFirmwareType(
      fastboot, fuchsia_paver::wire::Configuration::kB, "firmware_type"));
}

TEST_F(FastbootFlashTest, FlashBooloaderRSlotWithFirmwareType) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  ASSERT_NO_FATAL_FAILURE(TestFlashBootloaderWithFirmwareType(
      fastboot, fuchsia_paver::wire::Configuration::kRecovery, "firmware_type"));
}

TEST_F(FastbootFlashTest, FlashBooloaderWriteFail) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));

  // Insert a write firmware error
  paver_test::FakePaver& fake_paver = paver();
  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
  fake_paver.set_expected_payload_size(0);

  std::string command = "flash:bootloader_a";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_FALSE(ret.is_ok());

  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, FlashBooloaderUnsupportedFirmwareType) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));

  // Insert an unsupported firmware failure
  paver().set_supported_firmware_type("unsupported");

  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  std::string command = "flash:bootloader_a";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, FlashFuchsiaEsp) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));

  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  paver_test::FakePaver& fake_paver = paver();
  fake_paver.set_expected_payload_size(download_content.size());

  std::string command = "flash:fuchsia-esp";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));

  ASSERT_EQ(fake_paver.last_firmware_config(), fuchsia_paver::wire::Configuration::kA);
  ASSERT_EQ(fake_paver.last_firmware_type(), "");
}

TEST_F(FastbootFlashTest, FlashAssetZirconA) {
  TestFlashAsset("zircon_a", fuchsia_paver::wire::Configuration::kA,
                 fuchsia_paver::wire::Asset::kKernel);
}

TEST_F(FastbootFlashTest, FlashAssetZirconB) {
  TestFlashAsset("zircon_b", fuchsia_paver::wire::Configuration::kB,
                 fuchsia_paver::wire::Asset::kKernel);
}

TEST_F(FastbootFlashTest, FlashAssetZirconR) {
  TestFlashAsset("zircon_r", fuchsia_paver::wire::Configuration::kRecovery,
                 fuchsia_paver::wire::Asset::kKernel);
}

TEST_F(FastbootFlashTest, FlashAssetLegacyZirconA) {
  TestFlashAsset("zircon_a", fuchsia_paver::wire::Configuration::kA,
                 fuchsia_paver::wire::Asset::kKernel);
}

TEST_F(FastbootFlashTest, FlashAssetLegacyZirconB) {
  TestFlashAsset("zircon_b", fuchsia_paver::wire::Configuration::kB,
                 fuchsia_paver::wire::Asset::kKernel);
}

TEST_F(FastbootFlashTest, FlashAssetLegacyZirconR) {
  TestFlashAsset("zircon_r", fuchsia_paver::wire::Configuration::kRecovery,
                 fuchsia_paver::wire::Asset::kKernel);
}

TEST_F(FastbootFlashTest, FlashAssetVerifiedBootMetadataA) {
  TestFlashAsset("vbmeta_a", fuchsia_paver::wire::Configuration::kA,
                 fuchsia_paver::wire::Asset::kVerifiedBootMetadata);
}

TEST_F(FastbootFlashTest, FlashAssetVerifiedBootMetadataB) {
  TestFlashAsset("vbmeta_b", fuchsia_paver::wire::Configuration::kB,
                 fuchsia_paver::wire::Asset::kVerifiedBootMetadata);
}

TEST_F(FastbootFlashTest, FlashAssetVerifiedBootMetadataR) {
  TestFlashAsset("vbmeta_r", fuchsia_paver::wire::Configuration::kRecovery,
                 fuchsia_paver::wire::Asset::kVerifiedBootMetadata);
}

TEST_F(FastbootFlashTest, FlashAssetFail) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
  // Trigger an internal error by using an incorrect size
  paver().set_expected_payload_size(128);

  std::string command = "flash:zircon_a";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_FALSE(ret.is_ok());
  ASSERT_EQ(transport.GetOutPackets().back().compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, SetActiveSlotA) {
  TestSetActive("a");
  ASSERT_TRUE(paver().abr_data().slot_a.active);
}

TEST_F(FastbootFlashTest, SetActiveSlotB) {
  TestSetActive("b");
  ASSERT_TRUE(paver().abr_data().slot_b.active);
}

TEST_F(FastbootFlashTest, SetActiveInvalidSlot) {
  paver().set_abr_supported(true);
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "set_active:r";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, GetVarVersion) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  const char command[] = "getvar:version";
  TestTransport transport;
  transport.AddInPacket(command, strlen(command));
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  std::vector<std::string> expected_packets = {"OKAY0.4"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST_F(FastbootFlashTest, GetVarSlotCount) {
  paver().set_abr_supported(true);
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "getvar:slot-count";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  std::vector<std::string> expected_packets = {"OKAY2"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST_F(FastbootFlashTest, GetVarSlotCountAbrNotSupported) {
  paver().set_abr_supported(false);
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "getvar:slot-count";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  std::vector<std::string> expected_packets = {"OKAY1"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST_F(FastbootFlashTest, GetVarIsUserspace) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "getvar:is-userspace";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  std::vector<std::string> expected_packets = {"OKAYyes"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST_F(FastbootFlashTest, GetVarAll) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "getvar:all";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  std::vector<std::string> expected_packets = {
      "INFOversion: 0.4",
      "INFOslot-count: 1",
      "INFOproduct: [error: ZX_ERR_PEER_CLOSED]",
      "INFOis-userspace: yes",
      "INFOhw-revision: [error: ZX_ERR_PEER_CLOSED]",
      "INFOmax-download-size: 0x00040000",
      "OKAYnot all variables were retrieved successfully",
  };
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST_F(FastbootFlashTest, OemInitPartitionTables) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "oem init-partition-tables";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));

  ASSERT_EQ(paver().GetCommandTrace(),
            std::vector<paver_test::Command>{paver_test::Command::kInitPartitionTables});
}

TEST_F(FastbootFlashTest, OemWipePartitionTables) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "oem wipe-partition-tables";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));

  ASSERT_EQ(paver().GetCommandTrace(),
            std::vector<paver_test::Command>{paver_test::Command::kWipePartitionTables});
}

// Sends |command| to |fastboot| over |transport| and asserts that the operation is successful.
void CommandSuccess(Fastboot& fastboot, TestTransport& transport, std::string_view command) {
  transport.AddInPacket(command);
  ASSERT_TRUE(fastboot.ProcessPacket(&transport).is_ok());
  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  ASSERT_EQ(transport.GetOutPackets()[0], "OKAY");
}

// Sends |command| to |fastboot| over |transport| and asserts that the operation fails.
void CommandFailure(Fastboot& fastboot, TestTransport& transport, std::string_view command) {
  transport.AddInPacket(command);
  ASSERT_TRUE(fastboot.ProcessPacket(&transport).is_error());
  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  ASSERT_EQ(transport.GetOutPackets()[0].substr(0, 4), "FAIL");
}

TEST_F(FastbootFlashTest, FlashGptMeta) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  TestTransport transport;

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, kGptMetaDefault));
  ASSERT_NO_FATAL_FAILURE(CommandSuccess(fastboot, transport, "flash:gpt-meta"));
  ASSERT_EQ(paver().GetCommandTrace(),
            std::vector<paver_test::Command>{paver_test::Command::kInitPartitionTables});
}

TEST_F(FastbootFlashTest, FlashGptMetaNoDownloadFailure) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  TestTransport transport;

  // No data downloaded should cause a failure.
  ASSERT_NO_FATAL_FAILURE(CommandFailure(fastboot, transport, "flash:gpt-meta"));
  ASSERT_TRUE(paver().GetCommandTrace().empty());
}

TEST_F(FastbootFlashTest, FlashGptMetaUnknownContentsFailure) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  TestTransport transport;

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, "default_plus_invalid_extra_contents"));
  ASSERT_NO_FATAL_FAILURE(CommandFailure(fastboot, transport, "flash:gpt-meta"));
  ASSERT_TRUE(paver().GetCommandTrace().empty());
}

TEST_F(FastbootFlashTest, FlashGptMetaInvalidSlotFailure) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  TestTransport transport;

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, kGptMetaDefault));
  // The gpt-meta pseudo-partition doesn't support slot suffixes.
  ASSERT_NO_FATAL_FAILURE(CommandFailure(fastboot, transport, "flash:gpt-meta_a"));
  ASSERT_TRUE(paver().GetCommandTrace().empty());
}

class FastbootRebootTest : public testing::Test {
 public:
  // |TestState| is shared between |Background| and the main test object.
  class TestState {
   public:
    void set_shutdown_action(ShutdownAction action) {
      fbl::AutoLock al(&lock_);
      shutdown_action_ = action;
    }

    std::optional<ShutdownAction> shutdown_action() const {
      fbl::AutoLock al(&lock_);
      return shutdown_action_;
    }

   private:
    std::optional<ShutdownAction> shutdown_action_ __TA_GUARDED(lock_) = std::nullopt;
    mutable fbl::Mutex lock_;
  };

  class MockFidlServer
      : public fidl::testing::WireTestBase<fuchsia_paver::Paver>,
        public fidl::testing::WireTestBase<fuchsia_paver::BootManager>,
        public fidl::testing::WireTestBase<fuchsia_hardware_power_statecontrol::Admin> {
   public:
    MockFidlServer(async_dispatcher_t* dispatcher, std::shared_ptr<TestState> state)
        : dispatcher_(dispatcher), state_(std::move(state)) {}

    fidl::ProtocolHandler<fuchsia_hardware_power_statecontrol::Admin> PublishAdmin() {
      return admin_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure);
    }

    fidl::ProtocolHandler<fuchsia_paver::Paver> PublishPaver() {
      return paver_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure);
    }

   private:
    void Shutdown(ShutdownRequestView request, ShutdownCompleter::Sync& completer) override {
      state_->set_shutdown_action(request->options.action());
      completer.ReplySuccess();
    }

    void FindBootManager(FindBootManagerRequestView request,
                         FindBootManagerCompleter::Sync& completer) override {
      boot_manager_bindings_.AddBinding(dispatcher_, std::move(request->boot_manager), this,
                                        fidl::kIgnoreBindingClosure);
    }

    void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
      FAIL() << "Unexpected call to BuildInfo: " << name;
    }

    async_dispatcher_t* dispatcher_;
    std::shared_ptr<TestState> state_;
    fidl::ServerBindingGroup<fuchsia_paver::Paver> paver_bindings_;
    fidl::ServerBindingGroup<fuchsia_hardware_power_statecontrol::Admin> admin_bindings_;
    fidl::ServerBindingGroup<fuchsia_paver::BootManager> boot_manager_bindings_;
  };

  // This class is managed and used from a parallel background |async::Loop|
  // while the main thread runs blocking operations.
  class MockComponent {
   public:
    MockComponent(fidl::ServerEnd<fuchsia_io::Directory> svc,
                  const std::shared_ptr<TestState>& state)
        : dispatcher_(async_get_default_dispatcher()),
          outgoing_(dispatcher_),
          server_(dispatcher_, state) {
      EXPECT_EQ(outgoing_
                    .AddUnmanagedProtocol<fuchsia_hardware_power_statecontrol::Admin>(
                        server_.PublishAdmin())
                    .status_value(),
                ZX_OK);
      EXPECT_EQ(outgoing_.AddUnmanagedProtocol<fuchsia_paver::Paver>(server_.PublishPaver())
                    .status_value(),
                ZX_OK);
      EXPECT_EQ(outgoing_.Serve(std::move(svc)).status_value(), ZX_OK);
    }

   private:
    async_dispatcher_t* dispatcher_;
    component::OutgoingDirectory outgoing_;
    MockFidlServer server_;
  };

  FastbootRebootTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    loop_.StartThread("fastboot-reboot-test-loop");

    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();

    zx::result svc_local = component::OpenDirectoryAt(endpoints.client, "svc");
    EXPECT_TRUE(svc_local.is_ok()) << svc_local.status_string();

    svc_local_ = std::move(svc_local.value());
    mock_.emplace(std::move(endpoints.server), state_);
  }

  fidl::ClientEnd<fuchsia_io::Directory>& svc_chan() { return svc_local_; }
  const TestState& state() const { return *state_; }

  async::Loop loop_;
  std::shared_ptr<TestState> state_ = std::make_shared<TestState>();
  fidl::ClientEnd<fuchsia_io::Directory> svc_local_;
  async_patterns::DispatcherBound<MockComponent> mock_{loop_.dispatcher()};
};

TEST_F(FastbootRebootTest, RebootShutdownAction) {
  constexpr std::tuple<std::string_view, ShutdownAction> kCommandActionMap[] = {
      {"continue", ShutdownAction::kReboot},
      {"reboot", ShutdownAction::kReboot},
      {"reboot-bootloader", ShutdownAction::kRebootToBootloader},
      {"reboot-fastboot", ShutdownAction::kRebootToRecovery},
      {"reboot-recovery", ShutdownAction::kRebootToRecovery},
  };
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  for (auto [command, expected_action] : kCommandActionMap) {
    TestTransport transport;
    transport.AddInPacket(command);
    zx::result<> ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << "Failed to process packet for command " << command << ": "
                             << ret.status_string();
    std::vector<std::string> expected_packets = {"OKAY"};
    ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets))
        << "wrong response for command " << command;
    ASSERT_EQ(state().shutdown_action(), expected_action)
        << "wrong shutdown action for command " << command;
  }
}

TEST_F(FastbootFlashTest, UnknownOemCommand) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "oem unknown";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

class FastbootFshostTest : public FastbootDownloadTest {
 public:
  class TestState {
   public:
    const std::string& data_file_name() const {
      fbl::AutoLock al(&lock_);
      return data_file_name_;
    }

    void set_data_file_name(std::string v) {
      fbl::AutoLock al(&lock_);
      data_file_name_ = std::move(v);
    }

    const std::string& data_file_content() const {
      fbl::AutoLock al(&lock_);
      return data_file_content_;
    }

    void set_data_file_content(std::string v) {
      fbl::AutoLock al(&lock_);
      data_file_content_ = std::move(v);
    }

    uint64_t data_file_vmo_content_size() const {
      fbl::AutoLock al(&lock_);
      return data_file_vmo_content_size_;
    }

    void set_data_file_vmo_content_size(uint64_t v) {
      fbl::AutoLock al(&lock_);
      data_file_vmo_content_size_ = v;
    }

   private:
    std::string data_file_name_ TA_GUARDED(lock_);
    std::string data_file_content_ TA_GUARDED(lock_);
    uint64_t data_file_vmo_content_size_ TA_GUARDED(lock_);

    mutable fbl::Mutex lock_;
  };

  class MockFshostRecovery : public fidl::testing::WireTestBase<fuchsia_fshost::Recovery> {
   public:
    explicit MockFshostRecovery(async_dispatcher_t* dispatcher,
                                const std::shared_ptr<TestState>& state)
        : dispatcher_(dispatcher), state_(state) {}

    fidl::ProtocolHandler<fuchsia_fshost::Recovery> Publish() {
      return admin_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure);
    }

   private:
    void WriteDataFile(WriteDataFileRequestView request,
                       WriteDataFileCompleter::Sync& completer) override {
      state_->set_data_file_name(std::string(request->filename.data(), request->filename.size()));
      uint64_t size;
      ASSERT_EQ(request->payload.get_size(&size), ZX_OK);
      std::string data_file_content;
      data_file_content.resize(size);
      ASSERT_EQ(request->payload.read(data_file_content.data(), 0, size), ZX_OK);
      state_->set_data_file_content(std::move(data_file_content));
      uint64_t data_file_vmo_content_size;
      ASSERT_EQ(request->payload.get_prop_content_size(&data_file_vmo_content_size), ZX_OK);
      state_->set_data_file_vmo_content_size(data_file_vmo_content_size);
      completer.ReplySuccess();
    }

    void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
      FAIL() << "Unexpected call to ControllerImpl: " << name;
    }

    async_dispatcher_t* dispatcher_;
    std::shared_ptr<TestState> state_;
    fidl::ServerBindingGroup<fuchsia_fshost::Recovery> admin_bindings_;
  };

  class MockComponent {
   public:
    explicit MockComponent(fidl::ServerEnd<fuchsia_io::Directory> directory,
                           const std::shared_ptr<TestState>& state)
        : dispatcher_(async_get_default_dispatcher()),
          outgoing_(dispatcher_),
          server_(dispatcher_, state) {
      EXPECT_EQ(outgoing_.AddUnmanagedProtocol<fuchsia_fshost::Recovery>(server_.Publish())
                    .status_value(),
                ZX_OK);
      EXPECT_EQ(outgoing_.Serve(std::move(directory)).status_value(), ZX_OK);
    }

   private:
    async_dispatcher_t* dispatcher_;
    component::OutgoingDirectory outgoing_;
    MockFshostRecovery server_;
  };

  FastbootFshostTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    loop_.StartThread("fastboot-fshost-test-loop");
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();

    zx::result svc_local = component::OpenDirectoryAt(endpoints.client, "svc");
    EXPECT_TRUE(svc_local.is_ok()) << svc_local.status_string();

    svc_local_ = std::move(svc_local.value());
    mock_.emplace(std::move(endpoints.server), state_);
  }

  fidl::ClientEnd<fuchsia_io::Directory>& svc_chan() { return svc_local_; }

  const TestState& state() const { return *state_; }

 private:
  async::Loop loop_;
  std::shared_ptr<TestState> state_ = std::make_shared<TestState>();
  fidl::ClientEnd<fuchsia_io::Directory> svc_local_;
  async_patterns::DispatcherBound<MockComponent> mock_{loop_.dispatcher()};
};

TEST_F(FastbootFshostTest, OemAddStagedBootloaderFile) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));

  std::string command =
      "oem add-staged-bootloader-file " + std::string(sshd_host::kAuthorizedKeysBootloaderFileName);
  TestTransport transport;

  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));

  ASSERT_EQ(state().data_file_name(), sshd_host::kAuthorizedKeyPathInData);
  ASSERT_EQ(state().data_file_vmo_content_size(), download_content.size());
  ASSERT_EQ(std::memcmp(state().data_file_content().data(), download_content.data(),
                        download_content.size()),
            0);
}

TEST_F(FastbootFlashTest, OemAddStagedBootloaderFileInvalidNumberOfArguments) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "oem add-staged-bootloader-file";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, OemAddStagedBootloaderFileUnsupportedFile) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::string command = "oem add-staged-bootloader-file unknown";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, FlashRawFVM) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
  paver_test::FakePaver& fake_paver = paver();
  fake_paver.set_expected_payload_size(download_content.size());

  std::string command = "flash:fvm";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST_F(FastbootFlashTest, FlashRawFVMFail) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  std::vector<uint8_t> download_content(256, 1);
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
  paver_test::FakePaver& fake_paver = paver();

  // Use an incorrect size to trigger an error
  fake_paver.set_expected_payload_size(download_content.size() + 1);

  std::string command = "flash:fvm";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());

  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashTest, AndroidSparseImageNotSupported) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  sparse_header_t header{
      .magic = SPARSE_HEADER_MAGIC,
  };
  const uint8_t* header_ptr = reinterpret_cast<const uint8_t*>(&header);
  std::vector<uint8_t> download_content(header_ptr, header_ptr + sizeof(header));
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, download_content));
  paver_test::FakePaver& fake_paver = paver();
  fake_paver.set_expected_payload_size(download_content.size());

  std::string command = "flash:fvm";
  TestTransport transport;
  transport.AddInPacket(command);
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  const std::vector<std::string>& sent_packets = transport.GetOutPackets();
  ASSERT_EQ(sent_packets.size(), 1ULL);
  ASSERT_EQ(sent_packets[0].compare(0, 4, "FAIL"), 0);
}

TEST(FastbootBase, ExtractCommandArgsMultipleArgs) {
  FastbootBase::CommandArgs args;
  FastbootBase::ExtractCommandArgs("cmd:arg1:arg2:arg3:a", ":", args);

  EXPECT_EQ(args.num_args, 5ul);
  EXPECT_EQ(args.args[0], "cmd");
  EXPECT_EQ(args.args[1], "arg1");
  EXPECT_EQ(args.args[2], "arg2");
  EXPECT_EQ(args.args[3], "arg3");
  EXPECT_EQ(args.args[4], "a");
  EXPECT_EQ(args.args[5], "");
}

TEST(FastbootBase, ExtractCommandArgsNoArgs) {
  FastbootBase::CommandArgs args;
  FastbootBase::ExtractCommandArgs("cmd", ":", args);

  EXPECT_EQ(args.num_args, 1ul);
  EXPECT_EQ(args.args[0], "cmd");
  EXPECT_EQ(args.args[1], "");
}

TEST(FastbootBase, ExtractCommandArgsMiddleEmptyArgs) {
  FastbootBase::CommandArgs args;
  FastbootBase::ExtractCommandArgs("cmd::arg2", ":", args);

  EXPECT_EQ(args.num_args, 2ul);
  EXPECT_EQ(args.args[0], "cmd");
  EXPECT_EQ(args.args[1], "arg2");
}

TEST(FastbootBase, ExtractCommandArgsEndEmptyArgs) {
  FastbootBase::CommandArgs args;
  FastbootBase::ExtractCommandArgs("cmd:arg1:", ":", args);

  EXPECT_EQ(args.num_args, 2ul);
  EXPECT_EQ(args.args[0], "cmd");
  EXPECT_EQ(args.args[1], "arg1");
}

TEST(FastbootBase, ExtractCommandArgsMultipleBySpace) {
  FastbootBase::CommandArgs args;
  FastbootBase::ExtractCommandArgs("cmd arg1 arg2 arg3", " ", args);

  EXPECT_EQ(args.num_args, 4ul);
  EXPECT_EQ(args.args[0], "cmd");
  EXPECT_EQ(args.args[1], "arg1");
  EXPECT_EQ(args.args[2], "arg2");
  EXPECT_EQ(args.args[3], "arg3");
  EXPECT_EQ(args.args[4], "");
}

constexpr char kTestBoardConfig[] = "test-board-config";
constexpr char kTestProductConfig[] = "test-product-config";

class FastbootBuildInfoTest : public FastbootDownloadTest {
 public:
  class MockBuildInfoProvider : public fidl::testing::WireTestBase<fuchsia_buildinfo::Provider> {
   public:
    explicit MockBuildInfoProvider(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

    fidl::ProtocolHandler<fuchsia_buildinfo::Provider> Publish() {
      return provider_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure);
    }

   private:
    void GetBuildInfo(GetBuildInfoCompleter::Sync& completer) override {
      fidl::Arena arena;
      auto ret = fuchsia_buildinfo::wire::BuildInfo::Builder(arena)
                     .board_config(fidl::StringView(kTestBoardConfig))
                     .product_config(fidl::StringView(kTestProductConfig))
                     .Build();
      completer.Reply(ret);
    }

    void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
      FAIL() << "Unexpected call to BuildInfo: " << name;
    }

    async_dispatcher_t* dispatcher_;
    fidl::ServerBindingGroup<fuchsia_buildinfo::Provider> provider_bindings_;
  };

  class MockComponent {
   public:
    explicit MockComponent(fidl::ServerEnd<fuchsia_io::Directory> server_end)
        : dispatcher_(async_get_default_dispatcher()),
          outgoing_(dispatcher_),
          provider_server_(dispatcher_) {
      EXPECT_EQ(
          outgoing_.AddUnmanagedProtocol<fuchsia_buildinfo::Provider>(provider_server_.Publish())
              .status_value(),
          ZX_OK);
      EXPECT_EQ(outgoing_.Serve(std::move(server_end)).status_value(), ZX_OK);
    }

   private:
    async_dispatcher_t* dispatcher_;
    component::OutgoingDirectory outgoing_;
    MockBuildInfoProvider provider_server_;
  };

  FastbootBuildInfoTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    loop_.StartThread("fastboot-build-info-test-loop");
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();

    zx::result svc_local = component::OpenDirectoryAt(endpoints.client, "svc");
    EXPECT_TRUE(svc_local.is_ok()) << svc_local.status_string();

    svc_local_ = std::move(svc_local.value());
    mock_.emplace(std::move(endpoints.server));
  }

  fidl::ClientEnd<fuchsia_io::Directory>& svc_chan() { return svc_local_; }

 private:
  async::Loop loop_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_local_;
  async_patterns::DispatcherBound<MockComponent> mock_{loop_.dispatcher()};
};

TEST_F(FastbootBuildInfoTest, GetVarHwRevision) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  const char command[] = "getvar:hw-revision";
  TestTransport transport;
  transport.AddInPacket(command, strlen(command));
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  Packets expected_packets = {"OKAY" + std::string(kTestBoardConfig)};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

TEST_F(FastbootBuildInfoTest, GetVarProduct) {
  Fastboot fastboot(0x40000, std::move(svc_chan()));
  const char command[] = "getvar:product";
  TestTransport transport;
  transport.AddInPacket(command, strlen(command));
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  Packets expected_packets = {"OKAY" + std::string(kTestProductConfig)};
  ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
}

}  // namespace

}  // namespace fastboot
