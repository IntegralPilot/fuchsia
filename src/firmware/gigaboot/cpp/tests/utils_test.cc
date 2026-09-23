// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "utils.h"

#include <algorithm>
#include <memory>
#include <string_view>

#include <efi/global-variable.h>
#include <efi/types.h>
#include <gtest/gtest.h>

#include "efi/boot-services.h"
#include "efi/protocol/loaded-image.h"
#include "gmock/gmock.h"
#include "gpt.h"
#include "mock_boot_service.h"
#include "phys/efi/main.h"

using ::testing::ContainerEq;

namespace gigaboot {
namespace {

TEST(GigabootTest, PrintTpm2Capability) {
  MockStubService stub_service;
  Device image_device({"path", "image"});  // dont care
  Tcg2Device tcg2_device;
  auto cleanup = SetupEfiGlobalState(stub_service, image_device);

  stub_service.AddDevice(&image_device);
  stub_service.AddDevice(&tcg2_device);

  ASSERT_EQ(PrintTpm2Capability(), EFI_SUCCESS);
}

TEST(GigabootTest, PrintTpm2CapabilityTpm2NotSupported) {
  MockStubService stub_service;
  Device image_device({"path", "image"});  // dont care
  auto cleanup = SetupEfiGlobalState(stub_service, image_device);
  stub_service.AddDevice(&image_device);
  ASSERT_NE(PrintTpm2Capability(), EFI_SUCCESS);
}

uint8_t secureboot_val = 0;
EFIAPI efi_status test_get_secureboot_var(char16_t* name, efi_guid* guid, uint32_t* flags,
                                          size_t* length, void* data) {
  const char16_t kVariable[] = u"SecureBoot";
  EXPECT_EQ(0, memcmp(kVariable, name, sizeof(kVariable)));
  EXPECT_EQ(0, memcmp(&GlobalVariableGuid, guid, sizeof(GlobalVariableGuid)));
  EXPECT_EQ(*length, 1ULL);
  memcpy(data, &secureboot_val, 1);
  return EFI_SUCCESS;
}

TEST(GigabootTest, IsSecureBootOn) {
  MockStubService stub_service;
  Device image_device({"path", "image"});  // dont care
  auto cleanup = SetupEfiGlobalState(stub_service, image_device);
  efi_runtime_services services{
      .GetVariable = test_get_secureboot_var,
  };
  gEfiSystemTable->RuntimeServices = &services;

  secureboot_val = 0;
  auto ret = IsSecureBootOn();
  ASSERT_TRUE(ret.is_ok());
  EXPECT_FALSE(*ret);

  secureboot_val = 1;
  ret = IsSecureBootOn();
  ASSERT_TRUE(ret.is_ok());
  EXPECT_TRUE(*ret);
}

EFIAPI efi_status test_get_secureboot_fail(char16_t*, efi_guid*, uint32_t*, size_t*, void*) {
  return EFI_NOT_FOUND;
}

// Handles common boilerplate for commandline reboot mode tests.
class CommandlineRebootModeTest : public testing::Test {
 public:
  // Sets up the EFI test environment.
  //
  // Call this once at the beginning of the test, and retain the result until the end of the test.
  auto SetupEfiState() {
    stub_service_ = std::make_unique<MockStubService>();
    image_device_ = std::make_unique<Device>(std::vector<std::string_view>({"disk0", "image"}));

    auto cleanup = SetupEfiGlobalState(*stub_service_, *image_device_);
    gEfiLoadedImage->LoadOptions = nullptr;
    return cleanup;
  }

  // Sets the EFI Loaded Image command line.
  //
  // `contents` are not copied, and must remain valid until EFI cleanup.
  static void SetImageCommandline(void* contents, size_t size) {
    ASSERT_NE(gEfiLoadedImage, nullptr);

    gEfiLoadedImage->LoadOptions = contents;
    gEfiLoadedImage->LoadOptionsSize = static_cast<uint32_t>(size);
  }

  // Initializes the fake disk with a GPT.
  //
  // `block_device_path` defaults to a path matching the loaded image.
  //
  // Can only be called once per test, and only after `SetupEfiState()`.
  void InitDiskGpt(std::string_view block_device_path = "disk0") {
    ASSERT_TRUE(stub_service_);
    ASSERT_TRUE(image_device_);
    ASSERT_FALSE(block_device_);

    block_device_ =
        std::make_unique<BlockDevice>(std::vector<std::string_view>({block_device_path}), 1024);
    stub_service_->AddDevice(image_device_.get());
    stub_service_->AddDevice(block_device_.get());

    block_device_->InitializeGpt();
  }

  // Sets the on-disk command line. This automatically calls `InitDiskGpt()` as well.
  //
  // `contents` are copied internally so do not need to stay valid.
  // `block_device_path` defaults to a path matching the loaded image.
  void SetDiskCommandline(const char* contents, std::string_view block_device_path = "disk0") {
    InitDiskGpt(block_device_path);

    // Add the `kDiskCommandlinePartitionName` partition.
    gpt_entry_t partition{{}, {}, kGptFirstUsableBlocks, kGptFirstUsableBlocks + 5, 0, {}};
    SetGptEntryName(kDiskCommandlinePartitionName, partition);
    block_device_->AddGptPartition(partition);

    // Write the contents to the fake backing storage.
    ASSERT_LT(strlen(contents) + 1, (partition.last - partition.first + 1) * kBlockSize);
    auto part_data =
        &block_device_->fake_disk_io_protocol().contents(0)[partition.first * kBlockSize];
    strcpy(reinterpret_cast<char*>(part_data), contents);
  }

 private:
  // unique_ptr<> to allow deferred initialization so test can supply custom parameters if needed.
  std::unique_ptr<MockStubService> stub_service_;
  std::unique_ptr<Device> image_device_;
  std::unique_ptr<BlockDevice> block_device_;
};

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeDefault) {
  auto cleanup = SetupEfiState();

  char16_t args[] = u"foo bar=baz";
  SetImageCommandline(args, sizeof(args));

  ASSERT_EQ(GetCommandlineRebootMode(false), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeFastboot) {
  auto cleanup = SetupEfiState();

  char16_t args[] = u"foo bar=baz boot_mode=fastboot 123abc";
  SetImageCommandline(args, sizeof(args));

  ASSERT_EQ(GetCommandlineRebootMode(false), std::optional(RebootMode::kBootloader));
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeNoLoadedImage) {
  gEfiLoadedImage = nullptr;

  ASSERT_EQ(GetCommandlineRebootMode(false), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeNoLoadOptions) {
  auto cleanup = SetupEfiState();

  ASSERT_EQ(GetCommandlineRebootMode(false), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeNotUcs2Alignment) {
  auto cleanup = SetupEfiState();

  char16_t args[] = u"abc";
  SetImageCommandline(reinterpret_cast<uint8_t*>(args) + 1, sizeof(args) - 1);

  ASSERT_EQ(GetCommandlineRebootMode(false), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeNotUcs2Contents) {
  auto cleanup = SetupEfiState();

  char16_t args[] = u"abc";
  SetImageCommandline(args, 3);  // 3 bytes is never a valid UCS-2 length.

  ASSERT_EQ(GetCommandlineRebootMode(false), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeFromDiskDefault) {
  auto cleanup = SetupEfiState();

  SetDiskCommandline("foo bar=baz");

  ASSERT_EQ(GetCommandlineRebootMode(true), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeFromDiskFastboot) {
  auto cleanup = SetupEfiState();

  SetDiskCommandline("boot_mode=fastboot");

  ASSERT_EQ(GetCommandlineRebootMode(true), RebootMode::kBootloader);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeFromDiskFastbootNoTermination) {
  auto cleanup = SetupEfiState();

  // We should apply termination internally to avoid buffer overflow even if the partition fails
  // to include the terminator within the first `kDiskCommandlineMaxSize` bytes.
  std::string contents("boot_mode=fastboot");
  contents.append(kDiskCommandlineMaxSize, 'a');
  SetDiskCommandline(contents.c_str());

  // In the current simple string-matching implementation, we should be able to detect the fastboot
  // parameter even without input termination.
  ASSERT_EQ(GetCommandlineRebootMode(true), RebootMode::kBootloader);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeFromDiskNoMatchingDisk) {
  auto cleanup = SetupEfiState();

  // We should only look on the block device we loaded from, so this block device should be ignored.
  SetDiskCommandline("boot_mode=fastboot", "non-matching-disk");

  ASSERT_EQ(GetCommandlineRebootMode(true), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeFromDiskNoGpt) {
  auto cleanup = SetupEfiState();

  // Default empty disk has no GPT.
  ASSERT_EQ(GetCommandlineRebootMode(true), std::nullopt);
}

TEST_F(CommandlineRebootModeTest, GetCommandlineRebootModeFromDiskNoCommandlinePartition) {
  auto cleanup = SetupEfiState();

  // Setup the fake disk GPT, but don't add the commandline partition.
  InitDiskGpt();

  ASSERT_EQ(GetCommandlineRebootMode(true), std::nullopt);
}

TEST(GigabootTest, IsSecureBootOnReturnsFalseOnError) {
  MockStubService stub_service;
  Device image_device({"path", "image"});  // dont care
  auto cleanup = SetupEfiGlobalState(stub_service, image_device);
  efi_runtime_services services{
      .GetVariable = test_get_secureboot_fail,
  };
  gEfiSystemTable->RuntimeServices = &services;
  auto ret = IsSecureBootOn();
  ASSERT_TRUE(ret.is_error());
}

struct DynamicPartitionNameTestCase {
  const char* partition_name;
  const char* lookup_name;
};

using DynamicPartitionNameTest = ::testing::TestWithParam<DynamicPartitionNameTestCase>;

TEST_P(DynamicPartitionNameTest, TestDynamicPartitionMapping) {
  MockStubService stub_service;
  Device image_device({"path-A", "path-B", "path-C", "image"});
  BlockDevice block_device({"path-A", "path-B", "path-C"}, 1024);
  auto cleanup = SetupEfiGlobalState(stub_service, image_device);

  stub_service.AddDevice(&image_device);
  stub_service.AddDevice(&block_device);

  block_device.InitializeGpt();

  const DynamicPartitionNameTestCase& test_case = GetParam();
  gpt_entry_t entry{{}, {}, kGptFirstUsableBlocks, kGptFirstUsableBlocks + 5, 0, {}};
  SetGptEntryName(test_case.partition_name, entry);
  block_device.AddGptPartition(entry);

  auto res = FindEfiGptDevice();
  ASSERT_TRUE(res.is_ok());

  EfiGptBlockDevice gpt_device = std::move(res.value());
  ASSERT_TRUE(gpt_device.Load().is_ok());

  std::string_view mapped_name = MaybeMapPartitionName(gpt_device, test_case.lookup_name);
  ASSERT_EQ(mapped_name, test_case.partition_name);
}

INSTANTIATE_TEST_SUITE_P(
    DynamicPartitionNamesTests, DynamicPartitionNameTest,
    testing::ValuesIn<DynamicPartitionNameTest::ParamType>({
        {GPT_ZIRCON_A_NAME, GPT_ZIRCON_A_NAME},
        {GPT_ZIRCON_B_NAME, GPT_ZIRCON_B_NAME},
        {GPT_ZIRCON_R_NAME, GPT_ZIRCON_R_NAME},
        {GPT_DURABLE_BOOT_NAME, GPT_DURABLE_BOOT_NAME},
        {GPT_FVM_NAME, GPT_FVM_NAME},
        {GUID_ZIRCON_A_NAME, GPT_ZIRCON_A_NAME},
        {GUID_ZIRCON_B_NAME, GPT_ZIRCON_B_NAME},
        {GUID_ZIRCON_R_NAME, GPT_ZIRCON_R_NAME},
        {GUID_ABR_META_NAME, GPT_DURABLE_BOOT_NAME},
        {GUID_FVM_NAME, GPT_FVM_NAME},
    }),
    [](testing::TestParamInfo<DynamicPartitionNameTest::ParamType> const& info) {
      // Only alphanumeric chars and _ are allowed in gtest names.
      std::string name(info.param.lookup_name);
      std::replace(name.begin(), name.end(), '-', '_');
      return name + "_" + std::string(name == info.param.partition_name ? "modern" : "legacy");
    });

struct EfiGuidStr {
  efi_guid guid;
  std::string_view str;
};

class EfiGuidTest : public testing::TestWithParam<EfiGuidStr> {};

TEST_P(EfiGuidTest, ToString) {
  const EfiGuidStr& p = GetParam();
  EXPECT_EQ(std::string(ToStr(p.guid).data()), p.str);
  // EXPECT_THAT(p.str, ContainerEq(ToStr(p.guid)));
}

TEST_P(EfiGuidTest, ToGuid) {
  const EfiGuidStr& p = GetParam();
  auto res = ToGuid(p.str);
  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res.value(), p.guid);
}

// Some EFI_GUID fields are in little endian according to spec:
// https://uefi.org/specs/UEFI/2.10/Apx_A_GUID_and_Time_Formats.html
// Assume it doesn't change on different architectures.
INSTANTIATE_TEST_SUITE_P(
    SuccessConvert, EfiGuidTest,
    testing::Values(
        EfiGuidStr{
            .guid{0x00000000, 0x0000, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
            .str{"00000000-0000-0000-0000-000000000000"}},
        EfiGuidStr{
            .guid{0x00000000, 0x0000, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01}},
            .str{"00000000-0000-0000-0000-000000000001"}},
        EfiGuidStr{
            .guid{0x00000000, 0x0000, 0x0000, {0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
            .str{"00000000-0000-0000-0001-000000000000"}},
        EfiGuidStr{
            .guid{0x00000000, 0x0000, 0x0001, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
            .str{"00000000-0000-0001-0000-000000000000"}},
        EfiGuidStr{
            .guid{0x00000000, 0x0001, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
            .str{"00000000-0001-0000-0000-000000000000"}},
        EfiGuidStr{
            .guid{0x00000001, 0x0000, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
            .str{"00000001-0000-0000-0000-000000000000"}},
        EfiGuidStr{
            .guid{0x01020304, 0x0506, 0x0708, {0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10}},
            .str{"01020304-0506-0708-090a-0b0c0d0e0f10"}},
        EfiGuidStr{
            .guid{0xffffffff, 0xffff, 0xffff, {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff}},
            .str{"ffffffff-ffff-ffff-ffff-ffffffffffff"}}));

class EfiGuidStrTest : public testing::TestWithParam<std::string_view> {};

TEST_P(EfiGuidStrTest, ParseFail) {
  auto& str = GetParam();
  auto res = ToGuid(str);
  ASSERT_TRUE(res.is_error());
  EXPECT_EQ(res.error_value(), EFI_INVALID_PARAMETER);
}

INSTANTIATE_TEST_SUITE_P(BadLength, EfiGuidStrTest,
                         testing::Values("",                                      // empty
                                         "00000000-0000-0000-0000-00000000000",   // too short
                                         "00000000-0000-0000-0000-0000000000000"  // too long
                                         ));

INSTANTIATE_TEST_SUITE_P(BadSymbol, EfiGuidStrTest,
                         testing::Values("g0000000-0000-0000-0000-000000000000",
                                         "00000000-.000-0000-0000-000000000000",
                                         "00000000-0000-*000-0000-000000000000",
                                         "00000000-0000-0000-(000-000000000000"));

INSTANTIATE_TEST_SUITE_P(BadDashLocation, EfiGuidStrTest,
                         testing::Values("-000000000000-0000-0000-000000000000",
                                         "00000000-000000-00-0000-000000000000",
                                         "000000000000000000000000000000000000",
                                         "------------------------------------"));

TEST(EfiGuidTest, Endianness) {
  const efi_guid guid{0x03020100, 0x0504, 0x0706, {0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f}};
  std::span<const uint8_t> buf(reinterpret_cast<const uint8_t*>(&guid), sizeof(guid));

  for (size_t i = 0; i < sizeof(guid); i++) {
    EXPECT_EQ(buf[i], i);
  }
}

}  // namespace
}  // namespace gigaboot
