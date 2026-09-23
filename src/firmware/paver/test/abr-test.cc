// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/abr/abr.h"

#include <endian.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/cksum.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/directory.h>
#include <zircon/hw/gpt.h>

#include <algorithm>
#include <iostream>

#include <zxtest/zxtest.h>

#include "src/firmware/paver/abr-client.h"
#include "src/firmware/paver/astro.h"
#include "src/firmware/paver/luis.h"
#include "src/firmware/paver/moonflower.h"
#include "src/firmware/paver/sherlock.h"
#include "src/firmware/paver/test/test-utils.h"
#include "src/firmware/paver/uefi.h"
#include "src/lib/uuid/uuid.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
namespace {

using device_watcher::RecursiveWaitForFile;
using driver_integration_test::IsolatedDevmgr;
using paver::MoonflowerGptEntryAttributes;
using uuid::Uuid;

class CurrentSlotUuidTest : public PaverTest {
 protected:
  static constexpr int kBlockSize = 512;
  static constexpr int kDiskBlocks = 1024;
  static constexpr uint8_t kZirconType[GPT_GUID_LEN] = GPT_ZIRCON_ABR_TYPE_GUID;
  static constexpr uint8_t kTestUuid[GPT_GUID_LEN] = {0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                                                      0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb,
                                                      0xcc, 0xdd, 0xee, 0xff};
  void SetUp() override {
    PaverTest::SetUp();

    IsolatedDevmgr::Args args = DevmgrArgs();
    ASSERT_OK(IsolatedDevmgr::Create(&args, &devmgr_));
  }

  virtual IsolatedDevmgr::Args DevmgrArgs() {
    IsolatedDevmgr::Args args;
    args.disable_block_watcher = false;
    return args;
  }

  fbl::unique_fd BlockDirFd() {
    fbl::unique_fd fd;
    auto [block, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    EXPECT_OK(fdio_open3_at(devmgr_.RealmExposedDir().handle()->get(), "block",
                            static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                            server.TakeChannel().release()));
    EXPECT_OK(fdio_fd_create(block.TakeChannel().release(), fd.reset_and_get_address()));
    return fd;
  }

  zx::result<paver::BlockDevices> CreateBlockDevices() {
    return paver::BlockDevices::CreateFromFshostBlockDir(BlockDirFd());
  }

  void CreatePartitioner(std::unique_ptr<paver::GptDevicePartitioner>& out) {
    zx::result devices = CreateBlockDevices();
    ASSERT_OK(devices);
    zx::result gpt = paver::GptDevicePartitioner::InitializeGpt(
        *devices, devmgr_.RealmExposedDir(), paver::PaverConfig{.zvb_current_slot = "_a"});
    ASSERT_OK(gpt);
    out = std::move(gpt->gpt);
  }

  void CreateGptDevice(const std::vector<PartitionDescription>& partitions) {
    fidl::ClientEnd svc_root = devmgr_.RealmExposedDir();
    fbl::unique_fd fd;
    ASSERT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
    ASSERT_NO_FATAL_FAILURE(
        BlockDevice::CreateWithGpt(fd, kDiskBlocks, kBlockSize, partitions, &disk_));
  }

  IsolatedDevmgr devmgr_;
  std::unique_ptr<BlockDevice> disk_;
};

TEST_F(CurrentSlotUuidTest, TestZirconAIsSlotA) {
  ASSERT_NO_FATAL_FAILURE(CreateGptDevice({
      PartitionDescription{"zircon_a", Uuid(kZirconType), 0x22, 0x1, Uuid(kTestUuid)},
  }));

  std::unique_ptr<paver::GptDevicePartitioner> partitioner;
  ASSERT_NO_FATAL_FAILURE(CreatePartitioner(partitioner));
  auto result = abr::PartitionUuidToConfiguration(partitioner->devices(), Uuid(kTestUuid));
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kA);
}

TEST_F(CurrentSlotUuidTest, TestZirconAWithUnderscore) {
  ASSERT_NO_FATAL_FAILURE(CreateGptDevice({
      PartitionDescription{"zircon_a", Uuid(kZirconType), 0x22, 0x1, Uuid(kTestUuid)},
  }));

  std::unique_ptr<paver::GptDevicePartitioner> partitioner;
  ASSERT_NO_FATAL_FAILURE(CreatePartitioner(partitioner));
  auto result = abr::PartitionUuidToConfiguration(partitioner->devices(), Uuid(kTestUuid));
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kA);
}

TEST_F(CurrentSlotUuidTest, TestZirconAMixedCase) {
  ASSERT_NO_FATAL_FAILURE(CreateGptDevice({
      PartitionDescription{"ZiRcOn_A", Uuid(kZirconType), 0x22, 0x1, Uuid(kTestUuid)},
  }));

  std::unique_ptr<paver::GptDevicePartitioner> partitioner;
  ASSERT_NO_FATAL_FAILURE(CreatePartitioner(partitioner));
  auto result = abr::PartitionUuidToConfiguration(partitioner->devices(), Uuid(kTestUuid));
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kA);
}

TEST_F(CurrentSlotUuidTest, TestZirconB) {
  ASSERT_NO_FATAL_FAILURE(CreateGptDevice({
      PartitionDescription{"zircon_b", Uuid(kZirconType), 0x22, 0x1, Uuid(kTestUuid)},
  }));

  std::unique_ptr<paver::GptDevicePartitioner> partitioner;
  ASSERT_NO_FATAL_FAILURE(CreatePartitioner(partitioner));
  auto result = abr::PartitionUuidToConfiguration(partitioner->devices(), Uuid(kTestUuid));
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kB);
}

TEST_F(CurrentSlotUuidTest, TestZirconR) {
  ASSERT_NO_FATAL_FAILURE(CreateGptDevice({
      PartitionDescription{"ZIRCON_R", Uuid(kZirconType), 0x22, 0x1, Uuid(kTestUuid)},
  }));

  std::unique_ptr<paver::GptDevicePartitioner> partitioner;
  ASSERT_NO_FATAL_FAILURE(CreatePartitioner(partitioner));
  auto result = abr::PartitionUuidToConfiguration(partitioner->devices(), Uuid(kTestUuid));
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kRecovery);
}

TEST_F(CurrentSlotUuidTest, TestInvalid) {
  ASSERT_NO_FATAL_FAILURE(CreateGptDevice({
      PartitionDescription{"ZERCON_R", Uuid(kZirconType), 0x22, 0x1, Uuid(kTestUuid)},
  }));

  std::unique_ptr<paver::GptDevicePartitioner> partitioner;
  ASSERT_NO_FATAL_FAILURE(CreatePartitioner(partitioner));
  auto result = abr::PartitionUuidToConfiguration(partitioner->devices(), Uuid(kTestUuid));
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_NOT_SUPPORTED);
}

TEST(CurrentSlotTest, TestA) {
  auto result = abr::CurrentSlotToConfiguration("_a");
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kA);
}

TEST(CurrentSlotTest, TestB) {
  auto result = abr::CurrentSlotToConfiguration("_b");
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kB);
}

TEST(CurrentSlotTest, TestR) {
  auto result = abr::CurrentSlotToConfiguration("_r");
  ASSERT_OK(result);
  ASSERT_EQ(result.value(), fuchsia_paver::wire::Configuration::kRecovery);
}

TEST(CurrentSlotTest, TestInvalid) {
  auto result = abr::CurrentSlotToConfiguration("_x");
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_NOT_SUPPORTED);
}

class MoonflowerAbrClientTest : public CurrentSlotUuidTest {
 protected:
  // These GUIDs indicate the active partition.
  static constexpr Uuid kBootTypeGuid = Uuid(GPT_ZIRCON_ABR_TYPE_GUID);
  static constexpr Uuid kSuperTypeGuid = Uuid(GPT_FVM_TYPE_GUID);
  static constexpr Uuid kVbmetaTypeGuid = Uuid(GPT_VBMETA_ABR_TYPE_GUID);
  static constexpr Uuid kFlippedTypeGuid = Uuid(GPT_FACTORY_TYPE_GUID);
  // The inactive type GUID must be one of the known supported values or else the partitioner
  // will fail to initialize.
  static constexpr const Uuid& kInactiveTypeGuid = paver::MoonflowerPartitioner::kInactiveTypeGuid;

  IsolatedDevmgr::Args DevmgrArgs() override {
    IsolatedDevmgr::Args args;
    args.board_name = "sorrel";
    args.disable_block_watcher = false;
    return args;
  }

  // Disk starts with A slot active and successful.
  void SetUp() override {
    CurrentSlotUuidTest::SetUp();

    ASSERT_NO_FATAL_FAILURE(CreateGptDevice({
        // Type GUIDs start with A partition active.
        PartitionDescription{"boot_a", kBootTypeGuid, 0x22, 0x1},
        PartitionDescription{"boot_b", kInactiveTypeGuid, 0x23, 0x1},
        PartitionDescription{"super", kSuperTypeGuid, 0x24, 0x1},
        PartitionDescription{"vbmeta_a", kVbmetaTypeGuid, 0x25, 0x1},
        PartitionDescription{"vbmeta_b", kInactiveTypeGuid, 0x26, 0x1},
        // "Flipped" partitions start with the type GUID incorrectly swapped. We should correct this
        // state when we modify the type GUIDs.
        PartitionDescription{"flipped_guid_a", kInactiveTypeGuid, 0x27, 0x1},
        PartitionDescription{"flipped_guid_b", kFlippedTypeGuid, 0x28, 0x1},
    }));

    zx::result devices = CreateBlockDevices();
    ASSERT_OK(devices);
    std::shared_ptr<paver::Context> context;
    zx::result partitioner = paver::MoonflowerPartitionerFactory().New(
        *devices, devmgr_.RealmExposedDir(),
        paver::PaverConfig{.arch = paver::Arch::kArm64, .zvb_current_slot = "_a"}, context);
    ASSERT_OK(partitioner);
    zx::result abr_client = partitioner->CreateAbrClient();
    ASSERT_OK(abr_client);
    partitioner_ = std::move(*partitioner);
    abr_client_ = std::move(*abr_client);

    // Set A active + successful.
    ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexA));
    ASSERT_OK(abr_client_->MarkSlotSuccessful(kAbrSlotIndexA));
    ASSERT_OK(abr_client_->Flush());
  }

  zx::result<std::unique_ptr<gpt::GptDevice>> OpenGptDevice() {
    fidl::ClientEnd<fuchsia_storage_block::Block> volume(disk_->Connect().TakeChannel());
    zx::result remote_device = block_client::RemoteBlockDevice::Create(std::move(volume));
    if (remote_device.is_error()) {
      return remote_device.take_error();
    }
    return gpt::GptDevice::Create(std::move(remote_device.value()),
                                  /*blocksize=*/disk_->block_size(),
                                  /*blocks=*/disk_->block_count());
  }

  // Returns an individual partition's type GUID and `MoonflowerGptEntryAttributes`.
  //
  // `index` must match the partition `name`. Any unexpected errors are marked via `EXPECT`
  // macros as test failures.
  std::pair<Uuid, MoonflowerGptEntryAttributes> GetPartitionTypeGuidAndAttributes(
      uint32_t index, std::string_view name) {
    zx::result gpt_result = OpenGptDevice();
    EXPECT_OK(gpt_result);
    std::unique_ptr<gpt::GptDevice> gpt = std::move(gpt_result.value());
    zx::result gpt_entry = gpt->GetPartition(index);
    EXPECT_OK(gpt_entry);

    char cstring_name[GPT_NAME_LEN / 2 + 1] = {0};
    ::utf16_to_cstring(cstring_name, reinterpret_cast<const uint16_t*>(gpt_entry->name),
                       sizeof(cstring_name));
    const std::string_view partition_name = cstring_name;
    EXPECT_EQ(partition_name, name);

    return {Uuid(gpt_entry->type), MoonflowerGptEntryAttributes(gpt_entry->flags)};
  }

  // The possible type GUID states expected by our tests.
  enum class TypeGuidState {
    // A slots have the specific active GUIDs, B slots have inactive shared GUID.
    kActiveA,
    // B slots have the specific active GUIDs, A slots have inactive shared GUID.
    kActiveB,
    // Same as kActiveA, but the "flipped" partition type GUIDs are swapped.
    kActiveAWithFlip,
    // Type GUIDs don't match any expected state.
    kUnknown
  };

  // Scans the GPT and returns the current state of the type GUIDs.
  TypeGuidState GetTypeGuidState() {
    using GuidMap = std::map<std::string_view, Uuid>;

    GuidMap guids_by_name{
        {"boot_a", GetPartitionTypeGuidAndAttributes(0, "boot_a").first},
        {"boot_b", GetPartitionTypeGuidAndAttributes(1, "boot_b").first},
        {"super", GetPartitionTypeGuidAndAttributes(2, "super").first},
        {"vbmeta_a", GetPartitionTypeGuidAndAttributes(3, "vbmeta_a").first},
        {"vbmeta_b", GetPartitionTypeGuidAndAttributes(4, "vbmeta_b").first},
        {"flipped_guid_a", GetPartitionTypeGuidAndAttributes(5, "flipped_guid_a").first},
        {"flipped_guid_b", GetPartitionTypeGuidAndAttributes(6, "flipped_guid_b").first},
    };

    // Type GUIDs if we're in the active A state.
    if (guids_by_name == GuidMap{
                             {"boot_a", kBootTypeGuid},
                             {"boot_b", kInactiveTypeGuid},
                             {"super", kSuperTypeGuid},
                             {"vbmeta_a", kVbmetaTypeGuid},
                             {"vbmeta_b", kInactiveTypeGuid},
                             {"flipped_guid_a", kFlippedTypeGuid},
                             {"flipped_guid_b", kInactiveTypeGuid},
                         }) {
      return TypeGuidState::kActiveA;
    }

    // Type GUIDs if we're in the active B state.
    if (guids_by_name == GuidMap{
                             {"boot_a", kInactiveTypeGuid},
                             {"boot_b", kBootTypeGuid},
                             {"super", kSuperTypeGuid},
                             {"vbmeta_a", kInactiveTypeGuid},
                             {"vbmeta_b", kVbmetaTypeGuid},
                             {"flipped_guid_a", kInactiveTypeGuid},
                             {"flipped_guid_b", kFlippedTypeGuid},
                         }) {
      return TypeGuidState::kActiveB;
    }

    // Type GUIDs if we're in the active A state with "flipped" GUIDs swapped.
    if (guids_by_name == GuidMap{
                             {"boot_a", kBootTypeGuid},
                             {"boot_b", kInactiveTypeGuid},
                             {"super", kSuperTypeGuid},
                             {"vbmeta_a", kVbmetaTypeGuid},
                             {"vbmeta_b", kInactiveTypeGuid},
                             // Flipped - A is inactive, B is active.
                             {"flipped_guid_a", kInactiveTypeGuid},
                             {"flipped_guid_b", kFlippedTypeGuid},
                         }) {
      return TypeGuidState::kActiveAWithFlip;
    }

    // If we got here, we don't match any expected type GUID layout.
    return TypeGuidState::kUnknown;
  }

  std::unique_ptr<paver::DevicePartitioner> partitioner_;
  std::unique_ptr<abr::Client> abr_client_;
};

// Define == so we can ASSERT_EQ() on `MoonflowerGptEntryAttributes`.
bool operator==(const MoonflowerGptEntryAttributes& a, const MoonflowerGptEntryAttributes& b) {
  return a.flags == b.flags;
}

// Define some common GPT flag states to make tests a bit more readable.
constexpr uint64_t kActivePriority = MoonflowerGptEntryAttributes::kMoonflowerMaxPriority;
constexpr uint64_t kInactivePriority = kActivePriority - 1;
// Active, max attempts remaining.
const MoonflowerGptEntryAttributes kFlagsActive = MoonflowerGptEntryAttributes(0)
                                                      .set_priority(kActivePriority)
                                                      .set_active(true)
                                                      .set_retry_count(kAbrMaxTriesRemaining)
                                                      .set_boot_success(false)
                                                      .set_unbootable(false);
// Inactive, max attempts remaining.
const MoonflowerGptEntryAttributes kFlagsInactive = MoonflowerGptEntryAttributes(0)
                                                        .set_priority(kInactivePriority)
                                                        .set_active(false)
                                                        .set_retry_count(kAbrMaxTriesRemaining)
                                                        .set_boot_success(false)
                                                        .set_unbootable(false);
// Active and successful.
const MoonflowerGptEntryAttributes kFlagsActiveAndSuccessful = MoonflowerGptEntryAttributes(0)
                                                                   .set_priority(kActivePriority)
                                                                   .set_active(true)
                                                                   .set_retry_count(0)
                                                                   .set_boot_success(true)
                                                                   .set_unbootable(false);
// Inactive and successful.
const MoonflowerGptEntryAttributes kFlagsInactiveAndSuccessful =
    MoonflowerGptEntryAttributes(0)
        .set_priority(kInactivePriority)
        .set_active(false)
        .set_retry_count(0)
        .set_boot_success(true)
        .set_unbootable(false);
// Inactive and unbootable.
const MoonflowerGptEntryAttributes kFlagsUnbootable = MoonflowerGptEntryAttributes(0)
                                                          .set_priority(kInactivePriority)
                                                          .set_active(false)
                                                          .set_retry_count(0)
                                                          .set_boot_success(false)
                                                          .set_unbootable(true);

// Make sure our test setup logic puts us in the expected A state with flipped GUIDs.
TEST_F(MoonflowerAbrClientTest, StartingState) {
  ASSERT_EQ(kFlagsActiveAndSuccessful, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsUnbootable, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  // We haven't touched GUIDs yet - "flipped" type GUIDs should still be in their original state.
  ASSERT_EQ(TypeGuidState::kActiveAWithFlip, GetTypeGuidState());
}

TEST_F(MoonflowerAbrClientTest, SwitchActiveToB) {
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  ASSERT_EQ(kFlagsInactiveAndSuccessful, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsActive, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveB, GetTypeGuidState());
}

TEST_F(MoonflowerAbrClientTest, MarkSuccessfulB) {
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->MarkSlotSuccessful(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  // In libabr when a slot is marked successful, the other slot is reset to default state, so
  // A no longer has the successful bit here.
  ASSERT_EQ(kFlagsInactive, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsActiveAndSuccessful, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveB, GetTypeGuidState());
}

TEST_F(MoonflowerAbrClientTest, SwitchActiveToA) {
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->MarkSlotSuccessful(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexA));
  ASSERT_OK(abr_client_->Flush());

  ASSERT_EQ(kFlagsActive, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsInactiveAndSuccessful, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveA, GetTypeGuidState());
}

TEST_F(MoonflowerAbrClientTest, MarkSuccessfulA) {
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->MarkSlotSuccessful(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexA));
  ASSERT_OK(abr_client_->MarkSlotSuccessful(kAbrSlotIndexA));
  ASSERT_OK(abr_client_->Flush());

  ASSERT_EQ(kFlagsActiveAndSuccessful, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsInactive, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveA, GetTypeGuidState());
}

// Setting a slot active should be idempotent.
TEST_F(MoonflowerAbrClientTest, SwitchActiveToBTwice) {
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  ASSERT_EQ(kFlagsInactiveAndSuccessful, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsActive, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveB, GetTypeGuidState());
}

// This is the common OTA flow:
// * boot into a successful slot, with other slot unbootable
// * apply the OTA to the other slot
// * mark the other slot active
TEST_F(MoonflowerAbrClientTest, ApplyOtaB) {
  // Initial state: B is unbootable.
  ASSERT_OK(abr_client_->MarkSlotUnbootable(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  // Apply an OTA by marking B active.
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  ASSERT_EQ(kFlagsInactiveAndSuccessful, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsActive, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveB, GetTypeGuidState());
}

// This is the common OTA commit flow:
// * boot into an active-but-not-successful slot
// * the slot gets marked successful
// * the inactive slot gets marked unbootable (to prevent rollbacks)
TEST_F(MoonflowerAbrClientTest, CommitOtaB) {
  // Initial state: B is active but not successful.
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  // OTA is committed by marking B successful and A unbootable in one transaction.
  ASSERT_OK(abr_client_->MarkSlotSuccessful(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->MarkSlotUnbootable(kAbrSlotIndexA));
  ASSERT_OK(abr_client_->Flush());

  ASSERT_EQ(kFlagsUnbootable, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsActiveAndSuccessful, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveB, GetTypeGuidState());
}

TEST_F(MoonflowerAbrClientTest, MarkActiveSlotUnbootable) {
  // Initial state: A is successful, B is active.
  ASSERT_OK(abr_client_->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  // Mark the B slot unbootable.
  ASSERT_OK(abr_client_->MarkSlotUnbootable(kAbrSlotIndexB));
  ASSERT_OK(abr_client_->Flush());

  // The active slot should revert to A now since B is unbootable.
  ASSERT_EQ(kFlagsActiveAndSuccessful, GetPartitionTypeGuidAndAttributes(0, "boot_a").second);
  ASSERT_EQ(kFlagsUnbootable, GetPartitionTypeGuidAndAttributes(1, "boot_b").second);
  ASSERT_EQ(TypeGuidState::kActiveA, GetTypeGuidState());
}

class FakePartitionClient final : public paver::PartitionClient {
 public:
  FakePartitionClient(size_t block_size, size_t partition_size)
      : block_size_(block_size), partition_size_(partition_size) {}

  zx::result<size_t> GetBlockSize() final {
    if (result_ == ZX_OK) {
      return zx::ok(block_size_);
    }
    return zx::error(result_);
  }
  zx::result<size_t> GetPartitionSize() final {
    if (result_ == ZX_OK) {
      return zx::ok(partition_size_);
    }
    return zx::error(result_);
  }

  zx::result<> Read(const zx::vmo& vmo, size_t size) final {
    if (size > partition_size_) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    return zx::make_result(result_);
  }
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) final {
    if (vmo_size > partition_size_) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    return zx::make_result(result_);
  }
  zx::result<> Trim() final { return zx::make_result(result_); }
  zx::result<> Flush() final { return zx::make_result(result_); }

  void set_result(zx_status_t result) { result_ = result; }

 private:
  size_t block_size_;
  size_t partition_size_;

  zx_status_t result_ = ZX_OK;
};

class OneShotFlagsTest : public PaverTest {
 public:
  void SetUp() override {
    PaverTest::SetUp();
    auto partition_client = std::make_unique<FakePartitionClient>(10, 100);
    auto abr_partition_client = abr::AbrPartitionClient::Create(std::move(partition_client));
    ASSERT_OK(abr_partition_client);
    abr_client_ = std::move(abr_partition_client.value());

    // Clear flags
    ASSERT_OK(abr_client_->GetAndClearOneShotFlags());
  }

  std::unique_ptr<abr::Client> abr_client_;
};

TEST_F(OneShotFlagsTest, ClearFlags) {
  // Set some flags to see that they are cleared
  ASSERT_OK(abr_client_->SetOneShotRecovery());
  ASSERT_OK(abr_client_->SetOneShotBootloader());

  // First get flags would return flags
  auto abr_flags_res = abr_client_->GetAndClearOneShotFlags();
  ASSERT_OK(abr_flags_res);
  EXPECT_NE(abr_flags_res.value(), kAbrDataOneShotFlagNone);

  // Second get flags should be cleared
  abr_flags_res = abr_client_->GetAndClearOneShotFlags();
  ASSERT_OK(abr_flags_res);
  EXPECT_EQ(abr_flags_res.value(), kAbrDataOneShotFlagNone);
}

TEST_F(OneShotFlagsTest, SetOneShotRecovery) {
  ASSERT_OK(abr_client_->SetOneShotRecovery());

  // Check if flag is set
  auto abr_flags_res = abr_client_->GetAndClearOneShotFlags();
  ASSERT_OK(abr_flags_res);
  EXPECT_TRUE(AbrIsOneShotRecoveryBootSet(abr_flags_res.value()));
  EXPECT_FALSE(AbrIsOneShotBootloaderBootSet(abr_flags_res.value()));
}

TEST_F(OneShotFlagsTest, SetOneShotBootloader) {
  ASSERT_OK(abr_client_->SetOneShotBootloader());

  // Check if flag is set
  auto abr_flags_res = abr_client_->GetAndClearOneShotFlags();
  ASSERT_OK(abr_flags_res);
  EXPECT_TRUE(AbrIsOneShotBootloaderBootSet(abr_flags_res.value()));
  EXPECT_FALSE(AbrIsOneShotRecoveryBootSet(abr_flags_res.value()));
}

TEST_F(OneShotFlagsTest, Set2Flags) {
  ASSERT_OK(abr_client_->SetOneShotBootloader());
  ASSERT_OK(abr_client_->SetOneShotRecovery());

  // Check if flag is set
  auto abr_flags_res = abr_client_->GetAndClearOneShotFlags();
  ASSERT_OK(abr_flags_res);
  EXPECT_TRUE(AbrIsOneShotBootloaderBootSet(abr_flags_res.value()));
  EXPECT_TRUE(AbrIsOneShotRecoveryBootSet(abr_flags_res.value()));
}

}  // namespace
