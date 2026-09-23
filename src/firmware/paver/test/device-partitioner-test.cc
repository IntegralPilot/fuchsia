// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/device-partitioner.h"

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.fshost/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire.h>
#include <fidl/fuchsia.hardware.ufs/cpp/wire.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <fidl/fuchsia.scheduler/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fidl/fuchsia.storage.partitions/cpp/wire.h>
#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/common_types.h>
#include <fidl/fuchsia.system.state/cpp/fidl.h>
#include <fidl/fuchsia.system.state/cpp/markers.h>
#include <fidl/fuchsia.system.state/cpp/wire.h>
#include <fidl/fuchsia.tracing.provider/cpp/wire.h>
#include <lib/abr/util.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/sync/cpp/completion.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/sys/cpp/testing/service_directory_provider.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/hw/gpt.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <array>
#include <format>
#include <memory>
#include <span>
#include <string_view>
#include <utility>

#include <fbl/unique_fd.h>
#include <gpt/gpt.h>
#include <soc/aml-common/aml-guid.h>
#include <zxtest/zxtest.h>

#include "src/firmware/paver/android.h"
#include "src/firmware/paver/iris.h"
#include "src/firmware/paver/luis.h"
#include "src/firmware/paver/moonflower.h"
#include "src/firmware/paver/nelson.h"
#include "src/firmware/paver/sherlock.h"
#include "src/firmware/paver/system_shutdown_state.h"
#include "src/firmware/paver/test/test-utils.h"
#include "src/firmware/paver/uefi.h"
#include "src/firmware/paver/utils.h"
#include "src/lib/files/directory.h"
#include "src/lib/uuid/uuid.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace paver {
extern zx_duration_t g_wipe_timeout;
}

namespace {

constexpr fidl::UnownedClientEnd<fuchsia_io::Directory> kInvalidSvcRoot =
    fidl::UnownedClientEnd<fuchsia_io::Directory>(ZX_HANDLE_INVALID);

constexpr uint64_t kMebibyte{UINT64_C(1024) * 1024};
constexpr uint64_t kGibibyte{kMebibyte * 1024};

using device_watcher::RecursiveWaitForFile;
using driver_integration_test::IsolatedDevmgr;
using fuchsia_system_state::SystemPowerState;
using fuchsia_system_state::SystemStateTransition;
using paver::PartitionSpec;
using uuid::Uuid;

namespace fio = fuchsia_io;

// New Type GUID's
constexpr uint8_t kDurableBootType[GPT_GUID_LEN] = GPT_DURABLE_BOOT_TYPE_GUID;
constexpr uint8_t kVbMetaType[GPT_GUID_LEN] = GPT_VBMETA_ABR_TYPE_GUID;
constexpr uint8_t kZirconType[GPT_GUID_LEN] = GPT_ZIRCON_ABR_TYPE_GUID;
constexpr uint8_t kNewFvmType[GPT_GUID_LEN] = GPT_FVM_TYPE_GUID;

// Legacy Type GUID's
constexpr uint8_t kBootloaderType[GPT_GUID_LEN] = GUID_BOOTLOADER_VALUE;
constexpr uint8_t kEfiType[GPT_GUID_LEN] = GUID_EFI_VALUE;
constexpr uint8_t kZirconAType[GPT_GUID_LEN] = GUID_ZIRCON_A_VALUE;
constexpr uint8_t kZirconBType[GPT_GUID_LEN] = GUID_ZIRCON_B_VALUE;
constexpr uint8_t kZirconRType[GPT_GUID_LEN] = GUID_ZIRCON_R_VALUE;
constexpr uint8_t kVbMetaAType[GPT_GUID_LEN] = GUID_VBMETA_A_VALUE;
constexpr uint8_t kVbMetaBType[GPT_GUID_LEN] = GUID_VBMETA_B_VALUE;
constexpr uint8_t kVbMetaRType[GPT_GUID_LEN] = GUID_VBMETA_R_VALUE;
constexpr uint8_t kFvmType[GPT_GUID_LEN] = GUID_FVM_VALUE;
constexpr uint8_t kEmptyType[GPT_GUID_LEN] = GUID_EMPTY_VALUE;
constexpr uint8_t kAbrMetaType[GPT_GUID_LEN] = GUID_ABR_META_VALUE;
constexpr uint8_t kStateLinuxGuid[GPT_GUID_LEN] = GUID_LINUX_FILESYSTEM_DATA_VALUE;

constexpr uint8_t kBoot0Type[GPT_GUID_LEN] = GUID_EMMC_BOOT1_VALUE;
constexpr uint8_t kBoot1Type[GPT_GUID_LEN] = GUID_EMMC_BOOT2_VALUE;

constexpr uint8_t kDummyType[GPT_GUID_LEN] = {0xaf, 0x3d, 0xc6, 0x0f, 0x83, 0x84, 0x72, 0x47,
                                              0x8e, 0x79, 0x3d, 0x69, 0xd8, 0x47, 0x7d, 0xe4};

struct PartitionDescription {
  std::string name;
  uuid::Uuid type;
  uint64_t start;
  uint64_t length;
};

TEST(PartitionName, Bootloader) {
  EXPECT_STREQ(PartitionName(paver::Partition::kBootloaderA, paver::PartitionScheme::kNew),
               GPT_BOOTLOADER_A_NAME);
  EXPECT_STREQ(PartitionName(paver::Partition::kBootloaderB, paver::PartitionScheme::kNew),
               GPT_BOOTLOADER_B_NAME);
  EXPECT_STREQ(PartitionName(paver::Partition::kBootloaderR, paver::PartitionScheme::kNew),
               GPT_BOOTLOADER_R_NAME);
  EXPECT_STREQ(PartitionName(paver::Partition::kBootloaderA, paver::PartitionScheme::kLegacy),
               GUID_EFI_NAME);
  EXPECT_STREQ(PartitionName(paver::Partition::kBootloaderB, paver::PartitionScheme::kLegacy),
               GUID_EFI_NAME);
  EXPECT_STREQ(PartitionName(paver::Partition::kBootloaderR, paver::PartitionScheme::kLegacy),
               GUID_EFI_NAME);
}

TEST(PartitionName, AbrMetadata) {
  EXPECT_STREQ(PartitionName(paver::Partition::kAbrMeta, paver::PartitionScheme::kNew),
               GPT_DURABLE_BOOT_NAME);
  EXPECT_STREQ(PartitionName(paver::Partition::kAbrMeta, paver::PartitionScheme::kLegacy),
               GUID_ABR_META_NAME);
}

TEST(PartitionSpec, ToStringDefaultContentType) {
  // This is a bit of a change-detector test since we don't actually care about
  // the string value, but it's the cleanest way to check that the string is
  // 1) non-empty and 2) doesn't contain a type suffix.
  EXPECT_EQ(PartitionSpec(paver::Partition::kZirconA).ToString(), "Zircon A");
  EXPECT_EQ(PartitionSpec(paver::Partition::kVbMetaB).ToString(), "VBMeta B");
}

TEST(PartitionSpec, ToStringWithContentType) {
  EXPECT_EQ(PartitionSpec(paver::Partition::kZirconA, "foo").ToString(), "Zircon A (foo)");
  EXPECT_EQ(PartitionSpec(paver::Partition::kVbMetaB, "a b c").ToString(), "VBMeta B (a b c)");
}

class GptDevicePartitionerTests : public PaverTest {
 protected:
  explicit GptDevicePartitionerTests(fbl::String board_name = fbl::String(),
                                     uint32_t block_size = 512, std::string slot_suffix = "")
      : board_name_(std::move(board_name)),
        slot_suffix_(std::move(slot_suffix)),
        block_size_(block_size) {}

  void SetUp() override {
    PaverTest::SetUp();
    num_devices_ = 0;
    paver::g_wipe_timeout = 0;
    IsolatedDevmgr::Args args = BaseDevmgrArgs();
    args.board_name = board_name_;
    ASSERT_OK(IsolatedDevmgr::Create(&args, &devmgr_));

    ASSERT_OK(RecursiveWaitForFile(devmgr_.devfs_root().get(), "sys/platform").status_value());
  }

  virtual IsolatedDevmgr::Args BaseDevmgrArgs() {
    IsolatedDevmgr::Args args;
    args.disable_block_watcher = false;
    return args;
  }

  zx::result<paver::BlockDevices> CreateBlockDevices() {
    if (board_name_.empty()) {
      return paver::BlockDevices::CreateFromPartitionService(devmgr_.RealmExposedDir());
    }
    return paver::BlockDevices::CreateFromFshostBlockDir(BlockDirFd());
  }

  fidl::ClientEnd<fuchsia_io::Directory> RealmExposedDir() { return devmgr_.RealmExposedDir(); }

  fbl::unique_fd BlockDirFd() {
    fbl::unique_fd fd;
    auto [block, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    EXPECT_OK(fdio_open3_at(devmgr_.RealmExposedDir().handle()->get(), "block",
                            static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                            server.TakeChannel().release()));
    EXPECT_OK(fdio_fd_create(block.TakeChannel().release(), fd.reset_and_get_address()));
    return fd;
  }

  // Create a disk with the default size for a BlockDevice.
  void CreateDisk(std::unique_ptr<BlockDevice>* disk) {
    ASSERT_NO_FATAL_FAILURE(CreateDisk(disk, kBlockCount * block_size_));
  }

  // Create a disk with the given size in bytes and the given type.
  void CreateDisk(std::unique_ptr<BlockDevice>* disk, uint64_t bytes,
                  const uint8_t* type = kEmptyType) {
    ASSERT_TRUE(bytes % block_size_ == 0);
    uint64_t num_blocks = bytes / block_size_;
    fidl::ClientEnd svc_root = RealmExposedDir();
    fbl::unique_fd fd;
    ASSERT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
    ASSERT_NO_FATAL_FAILURE(BlockDevice::Create(disk, fd, type, num_blocks, block_size_));
  }

  // Create a disk with some initial contents.
  void CreateDiskWithContents(std::unique_ptr<BlockDevice>* disk, zx::vmo contents,
                              const uint8_t* type_guid = kEmptyType) {
    fidl::ClientEnd svc_root = RealmExposedDir();
    fbl::unique_fd fd;
    ASSERT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
    ASSERT_NO_FATAL_FAILURE(
        BlockDevice::CreateFromVmo(disk, fd, type_guid, std::move(contents), block_size_));
  }

  // Creates a GPT-formatted device with `init_partitions`.
  void CreateDiskWithGpt(std::unique_ptr<BlockDevice>* disk, size_t size = 0,
                         const std::vector<PartitionDescription>& init_partitions = {}) {
    uint64_t num_blocks = std::max(size / block_size_, kGptBlockCount);
    auto dev = std::make_unique<block_client::FakeBlockDevice>(num_blocks, block_size_);
    zx::result result = dev->VmoChildReference();
    ASSERT_OK(result);
    zx::vmo contents = std::move(result).value();
    zx::result gpt_result = gpt::GptDevice::Create(std::move(dev), block_size_, num_blocks);
    ASSERT_OK(gpt_result);
    std::unique_ptr<gpt::GptDevice> gpt = std::move(*gpt_result);
    ASSERT_OK(gpt->Sync());

    for (auto& part : init_partitions) {
      ASSERT_OK(gpt->AddPartition(part.name.c_str(), part.type.bytes(), Uuid::Generate().bytes(),
                                  part.start, part.length, 0),
                "%s", part.name.c_str());
    }
    ASSERT_OK(gpt->Sync());
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithContents(disk, std::move(contents)));
  }

  // Creates a GPT-formatted device with EFI partition
  void CreateDiskWithUefiGpt(std::unique_ptr<BlockDevice>* disk, size_t size = 0) {
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(disk, size,
                          {
                              PartitionDescription{GUID_EFI_NAME, Uuid(kEfiType), 0x8023, 0x8000},
                          }));
  }

  void ReadBlocks(const BlockDevice* blk_dev, size_t offset_in_blocks, size_t size_in_blocks,
                  uint8_t* out) const {
    zx::vmo vmo;
    const size_t vmo_size = size_in_blocks * block_size_;
    ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo));
    ASSERT_NO_FATAL_FAILURE(blk_dev->Read(vmo, vmo_size, offset_in_blocks, 0));
    ASSERT_OK(vmo.read(out, 0, vmo_size));
  }

  void WriteBlocks(const BlockDevice* blk_dev, size_t offset_in_blocks, size_t size_in_blocks,
                   uint8_t* buffer) const {
    zx::vmo vmo;
    const size_t vmo_size = size_in_blocks * block_size_;
    ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo));
    ASSERT_OK(vmo.write(buffer, 0, vmo_size));
    ASSERT_NO_FATAL_FAILURE(blk_dev->Write(vmo, vmo_size, offset_in_blocks, 0));
  }

  void ValidateBlockContent(const BlockDevice* blk_dev, size_t offset_in_blocks,
                            size_t size_in_blocks, uint8_t value) {
    std::vector<uint8_t> buffer(size_in_blocks * block_size_);
    ASSERT_NO_FATAL_FAILURE(ReadBlocks(blk_dev, offset_in_blocks, size_in_blocks, buffer.data()));
    for (size_t i = 0; i < buffer.size(); i++) {
      ASSERT_EQ(value, buffer[i], "at index: %zu", i);
    }
  }

  // Ensure that the partitions published to fshost match the expected list.
  void EnsurePartitionsMatch(std::span<const PartitionDescription> expected) {
    std::vector<fidl::ClientEnd<fuchsia_storage_block::Block>> devices;
    ASSERT_NO_FATAL_FAILURE(FindAllBlockDevices(&devices));
    std::vector<PartitionDescription> actual;
    for (auto& device : devices) {
      if (std::optional<PartitionDescription> desc = GetPartitionDescription(device); desc) {
        actual.push_back(*desc);
      }
    }

    for (const auto& part : expected) {
      auto match = std::find_if(actual.cbegin(), actual.cend(),
                                [&part](const PartitionDescription& actual_part) {
                                  return actual_part.name == part.name;
                                });
      ASSERT_TRUE(match != actual.end(), "Partition %s not found", part.name.c_str());
      EXPECT_EQ(part.type, match->type, "Partition %s wrong guid", part.name.c_str());
      EXPECT_EQ(part.start, match->start, "Partition %s wrong start", part.name.c_str());
      EXPECT_EQ(part.length, match->length, "Partition %s wrong length", part.name.c_str());
    }
  }

  static std::optional<PartitionDescription> GetPartitionDescription(
      fidl::UnownedClientEnd<fuchsia_storage_block::Block> client) {
    fidl::WireResult metadata = fidl::WireCall(client)->GetMetadata();
    if (!metadata.ok() || !metadata->value()->has_name() || !metadata->value()->has_type_guid()) {
      // Ignore non-Partition devices.
      return std::nullopt;
    }

    const auto& value = metadata->value();
    return PartitionDescription{
        .name = std::string(value->name().cbegin(), value->name().cend()),
        .type = uuid::Uuid(value->type_guid().value.data()),
        .start = value->start_block_offset(),
        .length = value->num_blocks(),
    };
  }

  void FindAllBlockDevices(std::vector<fidl::ClientEnd<fuchsia_storage_block::Block>>* out) {
    fidl::ClientEnd svc_root = RealmExposedDir();
    fbl::unique_fd fd;
    ASSERT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
    std::vector<std::string> entries;
    ASSERT_TRUE(files::ReadDirContentsAt(fd.get(), "fuchsia.storage.partitions.PartitionService",
                                         &entries));
    for (const auto& entry : entries) {
      std::string path =
          std::format("fuchsia.storage.partitions.PartitionService/{}/volume", entry);
      fdio_cpp::UnownedFdioCaller caller(fd.get());
      zx::result partition =
          component::ConnectAt<fuchsia_storage_block::Block>(caller.directory(), path);
      ASSERT_OK(partition);
      out->push_back(std::move(*partition));
    }
  }

  IsolatedDevmgr devmgr_;
  fbl::String board_name_;
  std::string slot_suffix_;
  size_t num_devices_ = 0;
  const uint32_t block_size_;
};

class FakeSystemStateTransition final : public fidl::WireServer<SystemStateTransition> {
 public:
  void GetTerminationSystemState(GetTerminationSystemStateCompleter::Sync& completer) override {
    completer.Reply(state_);
  }
  void GetMexecZbis(GetMexecZbisCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void SetTerminationSystemState(SystemPowerState state) { state_ = state; }

 private:
  fidl::ServerBindingGroup<SystemStateTransition> bindings_;
  SystemPowerState state_ = SystemPowerState::kFullyOn;
};

class FakeSvc {
 public:
  explicit FakeSvc(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
    zx::result server_end = fidl::CreateEndpoints(&root_);
    ASSERT_OK(server_end);
    libsync::Completion completion;
    async::PostTask(
        dispatcher_, [this, server_end = std::move(server_end.value()), &completion]() mutable {
          outgoing_ = std::make_unique<component::OutgoingDirectory>(dispatcher_);
          ASSERT_OK(outgoing_->AddUnmanagedProtocol<SystemStateTransition>(
              [this](fidl::ServerEnd<SystemStateTransition> server) {
                fidl::BindServer(dispatcher_, std::move(server), &fake_system_shutdown_state_);
              }));

          ASSERT_OK(outgoing_->Serve(std::move(server_end)));
          completion.Signal();
        });
    completion.Wait();
  }

  ~FakeSvc() {
    if (outgoing_) {
      libsync::Completion completion;
      async::PostTask(dispatcher_, [this, &completion]() {
        outgoing_.reset();
        completion.Signal();
      });
      completion.Wait();
    }
  }

  FakeSystemStateTransition& fake_system_shutdown_state() { return fake_system_shutdown_state_; }

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> svc() {
    return component::OpenDirectoryAt(root_, component::OutgoingDirectory::kServiceDirectory);
  }

 private:
  async_dispatcher_t* dispatcher_;
  FakeSystemStateTransition fake_system_shutdown_state_;
  fidl::ClientEnd<fuchsia_io::Directory> root_;
  std::unique_ptr<component::OutgoingDirectory> outgoing_;
};

class FakeUfs : public fidl::WireServer<fuchsia_hardware_ufs::Ufs> {
 public:
  void ReadAttribute(ReadAttributeRequestView request,
                     ReadAttributeCompleter::Sync& completer) override {
    if (request->attr.type() == fuchsia_hardware_ufs::wire::AttributeType::kBootLunEn) {
      completer.ReplySuccess(boot_lun_en_);
    } else {
      completer.ReplyError(fuchsia_hardware_ufs::wire::QueryErrorCode::kGeneralFailure);
    }
  }

  void WriteAttribute(WriteAttributeRequestView request,
                      WriteAttributeCompleter::Sync& completer) override {
    if (request->attr.type() == fuchsia_hardware_ufs::wire::AttributeType::kBootLunEn) {
      boot_lun_en_ = request->value;
      completer.ReplySuccess();
    } else {
      completer.ReplyError(fuchsia_hardware_ufs::wire::QueryErrorCode::kGeneralFailure);
    }
  }

  void ReadDescriptor(ReadDescriptorRequestView request,
                      ReadDescriptorCompleter::Sync& completer) override {}
  void WriteDescriptor(WriteDescriptorRequestView request,
                       WriteDescriptorCompleter::Sync& completer) override {}
  void ReadFlag(ReadFlagRequestView request, ReadFlagCompleter::Sync& completer) override {}
  void SetFlag(SetFlagRequestView request, SetFlagCompleter::Sync& completer) override {}
  void ClearFlag(ClearFlagRequestView request, ClearFlagCompleter::Sync& completer) override {}
  void ToggleFlag(ToggleFlagRequestView request, ToggleFlagCompleter::Sync& completer) override {}
  void SendUicCommand(SendUicCommandRequestView request,
                      SendUicCommandCompleter::Sync& completer) override {}
  void ReadBuffer(ReadBufferRequestView request, ReadBufferCompleter::Sync& completer) override {}
  void WriteBuffer(WriteBufferRequestView request, WriteBufferCompleter::Sync& completer) override {
  }

  uint32_t boot_lun_en_ = 1;
};

class FakeUfsSvc {
 public:
  explicit FakeUfsSvc(async_dispatcher_t* dispatcher,
                      fidl::ClientEnd<fuchsia_io::Directory> realm_root)
      : dispatcher_(dispatcher), realm_root_(std::move(realm_root)) {
    zx::result server_end = fidl::CreateEndpoints(&root_);
    ASSERT_OK(server_end);
    libsync::Completion completion;
    async::PostTask(dispatcher_, [this, raw_realm_root = realm_root_.handle()->get(),
                                  server_end = std::move(server_end.value()),
                                  &completion]() mutable {
      outgoing_ = std::make_unique<component::OutgoingDirectory>(dispatcher_);
      ASSERT_OK(outgoing_->AddUnmanagedProtocol<fuchsia_sysinfo::SysInfo>(
          [raw_realm_root](fidl::ServerEnd<fuchsia_sysinfo::SysInfo> server) {
            EXPECT_OK(fdio_service_connect_at(raw_realm_root, "fuchsia.sysinfo.SysInfo",
                                              server.TakeChannel().release()));
          }));

      ASSERT_OK(outgoing_->AddUnmanagedProtocol<fuchsia_storage_partitions::PartitionsManager>(
          [raw_realm_root](fidl::ServerEnd<fuchsia_storage_partitions::PartitionsManager> server) {
            EXPECT_OK(fdio_service_connect_at(raw_realm_root,
                                              "fuchsia.storage.partitions.PartitionsManager",
                                              server.TakeChannel().release()));
          }));

      zx::result partitions_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
      ASSERT_OK(partitions_endpoints);
      ASSERT_EQ(fdio_open3_at(raw_realm_root, "fuchsia.storage.partitions.PartitionService",
                              static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                              partitions_endpoints->server.TakeChannel().release()),
                ZX_OK);
      ASSERT_OK(outgoing_->AddDirectoryAt(std::move(partitions_endpoints->client), "svc",
                                          "fuchsia.storage.partitions.PartitionService"));

      fuchsia_hardware_ufs::Service::InstanceHandler handler({
          .device =
              [this](fidl::ServerEnd<fuchsia_hardware_ufs::Ufs> server) {
                fidl::BindServer(dispatcher_, std::move(server), &fake_ufs_);
              },
      });
      ASSERT_OK(outgoing_->AddService<fuchsia_hardware_ufs::Service>(std::move(handler)));

      ASSERT_OK(outgoing_->Serve(std::move(server_end)));
      completion.Signal();
    });
    completion.Wait();
  }

  ~FakeUfsSvc() {
    if (outgoing_) {
      libsync::Completion completion;
      async::PostTask(dispatcher_, [this, &completion]() {
        outgoing_.reset();
        completion.Signal();
      });
      completion.Wait();
    }
  }

  FakeUfs& fake_ufs() { return fake_ufs_; }

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> svc() {
    return component::OpenDirectoryAt(root_, component::OutgoingDirectory::kServiceDirectory);
  }

 private:
  async_dispatcher_t* dispatcher_;
  FakeUfs fake_ufs_;
  fidl::ClientEnd<fuchsia_io::Directory> root_;
  fidl::ClientEnd<fuchsia_io::Directory> realm_root_;
  std::unique_ptr<component::OutgoingDirectory> outgoing_;
};

class EfiDevicePartitionerTests : public GptDevicePartitionerTests {
 protected:
  EfiDevicePartitionerTests() : GptDevicePartitionerTests(fbl::String()) {
    EXPECT_OK(loop_.StartThread("efi-devicepartitioner-tests-loop"));
  }

  ~EfiDevicePartitionerTests() { loop_.Shutdown(); }

  IsolatedDevmgr::Args BaseDevmgrArgs() override {
    IsolatedDevmgr::Args args = GptDevicePartitionerTests::BaseDevmgrArgs();
    args.service_routes.emplace_back(IsolatedDevmgr::Args::ServiceRoute{
        .name = "fuchsia.system.state.SystemStateTransition",
        .connector =
            [this](zx::channel request, async_dispatcher_t* dispatcher) {
              fidl::BindServer(dispatcher,
                               fidl::ServerEnd<SystemStateTransition>(std::move(request)),
                               &fake_system_state_transition_);
            },
    });
    args.fshost_config.emplace_back(
        component_testing::ConfigCapability{.name = "fuchsia.fshost.RamdiskImage",
                                            .value = component_testing::ConfigValue::Bool(true)});
    return args;
  }

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    zx::result devices = CreateBlockDevices();
    if (devices.is_error()) {
      return devices.take_error();
    }

    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kX64,
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };

    return paver::EfiDevicePartitioner::Initialize(*devices, RealmExposedDir(), paver_config, {});
  }

  void ResetPartitionTablesTest();

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  FakeSystemStateTransition fake_system_state_transition_;
};

TEST_F(EfiDevicePartitionerTests, InitializeWithoutGptFails) {
  std::unique_ptr<BlockDevice> dev;
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&dev));

  ASSERT_NOT_OK(CreatePartitioner({}));
}

TEST_F(EfiDevicePartitionerTests, InitializeTwoCandidatesWithoutFvmFails) {
  std::unique_ptr<BlockDevice> gpt, gpt2;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt));
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt2));

  ASSERT_NOT_OK(CreatePartitioner({}));
}

TEST_F(EfiDevicePartitionerTests, FindOldBootloaderPartitionName) {
  std::unique_ptr<BlockDevice> gpt;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt, 64 * kGibibyte,
                        {
                            PartitionDescription{"efi-system", Uuid(kEfiType), 0x22, 0x8000},
                        }));

  auto partitioner = CreatePartitioner(gpt.get());
  ASSERT_OK(partitioner);
  ASSERT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA)));
}

TEST_F(EfiDevicePartitionerTests, SupportsPartition) {
  std::unique_ptr<BlockDevice> gpt;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithUefiGpt(&gpt, 64 * kGibibyte));

  zx::result status = CreatePartitioner(gpt.get());
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconR)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaR)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta)));
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  EXPECT_TRUE(partitioner->SupportsPartition(
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager, paver::kOpaqueVolumeContentType)));

  // Unsupported partition type.
  EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kUnknown)));

  // Unsupported content type.
  EXPECT_FALSE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA, "foo_type")));
}

TEST_F(EfiDevicePartitionerTests, ValidatePayload) {
  std::unique_ptr<BlockDevice> gpt;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithUefiGpt(&gpt, 64 * kGibibyte));

  zx::result status = CreatePartitioner(gpt.get());
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  // Test invalid partitions.
  ASSERT_NOT_OK(partitioner->ValidatePayload(PartitionSpec(paver::Partition::kZirconA),
                                             std::span<uint8_t>()));
  ASSERT_NOT_OK(partitioner->ValidatePayload(PartitionSpec(paver::Partition::kZirconB),
                                             std::span<uint8_t>()));
  ASSERT_NOT_OK(partitioner->ValidatePayload(PartitionSpec(paver::Partition::kZirconR),
                                             std::span<uint8_t>()));

  // Non-kernel partitions are not validated.
  ASSERT_OK(partitioner->ValidatePayload(PartitionSpec(paver::Partition::kAbrMeta),
                                         std::span<uint8_t>()));
}

TEST_F(EfiDevicePartitionerTests, OnStopRebootBootloader) {
  std::unique_ptr<BlockDevice> gpt;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(
      &gpt, 64 * kGibibyte,
      {
          PartitionDescription{GUID_EFI_NAME, Uuid(kEfiType), 0x8023, 0x8000},
          PartitionDescription{GUID_ABR_META_NAME, Uuid(kAbrMetaType), 0x10023, 0x8},
      }));

  zx::result partitioner_status = CreatePartitioner(gpt.get());
  ASSERT_OK(partitioner_status);
  std::unique_ptr<paver::DevicePartitioner> partitioner = std::move(partitioner_status.value());

  // Set Termination system state to "reboot to bootloader"
  fake_system_state_transition_.SetTerminationSystemState(SystemPowerState::kRebootBootloader);

  // Trigger OnStop event that should set one shot flag
  ASSERT_OK(partitioner->OnStop());

  // Verify ABR flags
  auto partition = partitioner->FindPartition(paver::PartitionSpec(paver::Partition::kAbrMeta));
  ASSERT_OK(partition);
  auto abr_partition_client = abr::AbrPartitionClient::Create(std::move(partition.value()));
  ASSERT_OK(abr_partition_client);
  auto abr_flags_res = abr_partition_client.value()->GetAndClearOneShotFlags();
  ASSERT_OK(abr_flags_res);
  EXPECT_TRUE(AbrIsOneShotBootloaderBootSet(abr_flags_res.value()));
  EXPECT_FALSE(AbrIsOneShotRecoveryBootSet(abr_flags_res.value()));
}

TEST_F(EfiDevicePartitionerTests, OnStopRebootRecovery) {
  std::unique_ptr<BlockDevice> gpt;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(
      &gpt, 64 * kGibibyte,
      {
          PartitionDescription{GUID_EFI_NAME, Uuid(kEfiType), 0x8023, 0x8000},
          PartitionDescription{GUID_ABR_META_NAME, Uuid(kAbrMetaType), 0x10023, 0x8},
      }));

  zx::result partitioner_status = CreatePartitioner(gpt.get());
  ASSERT_OK(partitioner_status);
  std::unique_ptr<paver::DevicePartitioner> partitioner = std::move(partitioner_status.value());

  // Set Termination system state to "reboot to bootloader"
  fake_system_state_transition_.SetTerminationSystemState(SystemPowerState::kRebootRecovery);

  // Trigger OnStop event that should set one shot flag
  ASSERT_OK(partitioner->OnStop());

  // Verify ABR flags
  auto partition = partitioner->FindPartition(paver::PartitionSpec(paver::Partition::kAbrMeta));
  ASSERT_OK(partition);
  auto abr_partition_client = abr::AbrPartitionClient::Create(std::move(partition.value()));
  ASSERT_OK(abr_partition_client);
  auto abr_flags_res = abr_partition_client.value()->GetAndClearOneShotFlags();
  ASSERT_OK(abr_flags_res);
  EXPECT_FALSE(AbrIsOneShotBootloaderBootSet(abr_flags_res.value()));
  EXPECT_TRUE(AbrIsOneShotRecoveryBootSet(abr_flags_res.value()));
}

void EfiDevicePartitionerTests::ResetPartitionTablesTest() {
  const Uuid etc_guid = Uuid::Generate();
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt_dev, 64 * kGibibyte,
                        {
                            PartitionDescription{"efi", Uuid(kEfiType), 0x22, 0x1},
                            PartitionDescription{"efi-system", Uuid(kEfiType), 0x23, 0x8000},
                            PartitionDescription{GUID_EFI_NAME, Uuid(kEfiType), 0x8023, 0x8000},
                            PartitionDescription{"ZIRCON_A", Uuid(kZirconAType), 0x10023, 0x1},
                            PartitionDescription{"zircon_b", Uuid(kZirconBType), 0x10024, 0x1},
                            PartitionDescription{"zircon r", Uuid(kZirconRType), 0x10025, 0x1},
                            PartitionDescription{"vbmeta-a", Uuid(kVbMetaAType), 0x10026, 0x1},
                            PartitionDescription{"VBMETA_B", Uuid(kVbMetaBType), 0x10027, 0x1},
                            PartitionDescription{"VBMETA R", Uuid(kVbMetaRType), 0x10028, 0x1},
                            PartitionDescription{"abrmeta", Uuid(kAbrMetaType), 0x10029, 0x1},
                            PartitionDescription{"FVM", Uuid(kFvmType), 0x10030, 0x1},
                            PartitionDescription{"etc", etc_guid, 0x10031, 0x400},
                        }));

  // Create EFI device partitioner and initialise partition tables.
  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  ASSERT_OK(partitioner->ResetPartitionTables());

  // Ensure the final partition layout looks like we expect it to.
  // Non-Fuchsia partitions ought to have been preserved at their old offsets, and Fuchsia
  // partitions should be dynamically allocated in a first-fit manner.
  // For clarity they are listed in order of non-Fuchsia partitions followed by Fuchsia partitions,
  // but the order is not necessarily representative of the GPT partition table entries.
  const std::array<PartitionDescription, 12> partitions_at_end{
      // Preserved Non-Fuchsia partitions
      PartitionDescription{"efi", Uuid(kEfiType), 0x22, 0x1},
      PartitionDescription{"efi-system", Uuid(kEfiType), 0x23, 0x8000},
      PartitionDescription{"etc", etc_guid, 0x10031, 0x400},
      // Reallocated Fuchsia partitions
      PartitionDescription{GUID_BOOTLOADER_NAME, Uuid(kBootloaderType), 0x8023, 0x8000},
      PartitionDescription{GPT_ZIRCON_A_NAME, Uuid(kZirconType), 0x10431, 0x40000},
      PartitionDescription{GPT_ZIRCON_B_NAME, Uuid(kZirconType), 0x50431, 0x40000},
      PartitionDescription{GPT_ZIRCON_R_NAME, Uuid(kZirconType), 0x90431, 0x60000},
      PartitionDescription{GPT_VBMETA_A_NAME, Uuid(kVbMetaType), 0xf0431, 0x80},
      PartitionDescription{GPT_VBMETA_B_NAME, Uuid(kVbMetaType), 0xf04b1, 0x80},
      PartitionDescription{GPT_VBMETA_R_NAME, Uuid(kVbMetaType), 0xf0531, 0x80},
      PartitionDescription{GPT_DURABLE_BOOT_NAME, Uuid(kDurableBootType), 0x10023, 0x8},
      PartitionDescription{GPT_FVM_NAME, Uuid(kNewFvmType), 0xf05b1, 0x7000000},
  };
  ASSERT_NO_FATAL_FAILURE(EnsurePartitionsMatch(partitions_at_end));

  // Make sure we can find the important partitions.
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  EXPECT_OK(partitioner->FindPartition(
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager, paver::kOpaqueVolumeContentType)));

  // Check that we found the correct bootloader partition.
  auto status2 = partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA));
  EXPECT_OK(status2);

  auto status3 = status2->GetPartitionSize();
  EXPECT_OK(status3);
  EXPECT_EQ(status3.value(), 0x8000 * block_size_);
}

TEST_F(EfiDevicePartitionerTests, ResetPartitionTables) {
  ASSERT_NO_FATAL_FAILURE(ResetPartitionTablesTest());
}

using FixedDevicePartitionerTests = GptDevicePartitionerTests;

TEST_F(FixedDevicePartitionerTests, EmptyDisk) {
  std::unique_ptr<BlockDevice> dev;
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&dev));
  zx::result devices = CreateBlockDevices();
  ASSERT_OK(devices);
  auto status = paver::FixedDevicePartitioner::Initialize(*devices, {});
  ASSERT_OK(status);
}

TEST_F(FixedDevicePartitionerTests, FindPartitionTest) {
  std::unique_ptr<BlockDevice> gpt_dev;
  constexpr uint64_t kBlockCount = 0x748038;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                        {
                            {"bootloader", Uuid(kBootloaderType), 0x1000, 0x1000},
                            {"zircon_a", Uuid(kZirconAType), 0x2000, 0x1000},
                            {"zircon_b", Uuid(kZirconBType), 0x3000, 0x1000},
                            {"zircon_r", Uuid(kZirconRType), 0x4000, 0x1000},
                            {"vbmeta_a", Uuid(kVbMetaAType), 0x5000, 0x1000},
                            {"vbmeta_b", Uuid(kVbMetaBType), 0x6000, 0x1000},
                            {"vbmeta_r", Uuid(kVbMetaRType), 0x7000, 0x1000},
                            {"fvm", Uuid(kFvmType), 0x8000, 0x1000},
                        }));

  std::shared_ptr<paver::Context> context = std::make_shared<paver::Context>();
  zx::result devices = CreateBlockDevices();
  ASSERT_OK(devices);
  auto paver_config = paver::PaverConfig{
      .arch = paver::Arch::kArm64,
      .zvb_current_slot = "_a",
  };
  zx::result partitioner_result =
      paver::DevicePartitionerFactory::Create(*devices, kInvalidSvcRoot, paver_config, context);
  ASSERT_OK(partitioner_result);
  std::unique_ptr partitioner = std::move(partitioner_result.value());

  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
}

TEST_F(FixedDevicePartitionerTests, SupportsPartitionTest) {
  std::unique_ptr<BlockDevice> dev;
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&dev));
  zx::result devices = CreateBlockDevices();
  ASSERT_OK(devices);
  auto status = paver::FixedDevicePartitioner::Initialize(*devices, {});
  ASSERT_OK(status);
  auto& partitioner = status.value();

  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconR)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaR)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta)));
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));

  // Unsupported partition type.
  EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kUnknown)));

  // Unsupported content type.
  EXPECT_FALSE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA, "foo_type")));
}

class SherlockPartitionerTests : public GptDevicePartitionerTests {
 protected:
  SherlockPartitionerTests() : GptDevicePartitionerTests("sherlock") {}

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    fidl::ClientEnd<fuchsia_io::Directory> svc_root = RealmExposedDir();
    zx::result devices = CreateBlockDevices();
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kX64,
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    return paver::SherlockPartitioner::Initialize(*devices, svc_root, paver_config);
  }

  void FindPartitionNewGuidsTest() {
    std::unique_ptr<BlockDevice> gpt_dev;
    constexpr uint64_t kBlockCount = 0x748038;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                          // partition size / location is arbitrary
                          {
                              {GPT_DURABLE_BOOT_NAME, Uuid(kDurableBootType), 0x10400, 0x10000},
                              {GPT_VBMETA_A_NAME, Uuid(kVbMetaType), 0x20400, 0x10000},
                              {GPT_VBMETA_B_NAME, Uuid(kVbMetaType), 0x30400, 0x10000},
                              {GPT_VBMETA_R_NAME, Uuid(kVbMetaType), 0x40400, 0x10000},
                              {GPT_ZIRCON_A_NAME, Uuid(kZirconType), 0x50400, 0x10000},
                              {GPT_ZIRCON_B_NAME, Uuid(kZirconType), 0x60400, 0x10000},
                              {GPT_ZIRCON_R_NAME, Uuid(kZirconType), 0x70400, 0x10000},
                              {GPT_FVM_NAME, Uuid(kNewFvmType), 0x80400, 0x10000},
                          }));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // Make sure we can find the important partitions.
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  }

  void FindPartitionNewGuidsWithWrongTypeGUIDSTest() {
    // Due to a bootloader bug (b/173801312), the type GUID's may be reset in certain conditions.
    // This test verifies that the sherlock partitioner only looks at the partition name.
    constexpr uint64_t kBlockCount = 0x748038;
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                          {
                              {GPT_DURABLE_BOOT_NAME, Uuid(kStateLinuxGuid), 0x10400, 0x10000},
                              {GPT_VBMETA_A_NAME, Uuid(kStateLinuxGuid), 0x20400, 0x10000},
                              {GPT_VBMETA_B_NAME, Uuid(kStateLinuxGuid), 0x30400, 0x10000},
                              {GPT_VBMETA_R_NAME, Uuid(kStateLinuxGuid), 0x40400, 0x10000},
                              {GPT_ZIRCON_A_NAME, Uuid(kStateLinuxGuid), 0x50400, 0x10000},
                              {GPT_ZIRCON_B_NAME, Uuid(kStateLinuxGuid), 0x60400, 0x10000},
                              {GPT_ZIRCON_R_NAME, Uuid(kStateLinuxGuid), 0x70400, 0x10000},
                              {GPT_FVM_NAME, Uuid(kStateLinuxGuid), 0x80400, 0x10000},
                          }));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // Make sure we can find the important partitions.
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  }

  void FindPartitionSecondaryTest() {
    constexpr uint64_t kBlockCount = 0x748038;
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                          {
                              {GPT_DURABLE_BOOT_NAME, Uuid(kStateLinuxGuid), 0x10400, 0x10000},
                              {GPT_VBMETA_A_NAME, Uuid(kStateLinuxGuid), 0x20400, 0x10000},
                              {GPT_VBMETA_B_NAME, Uuid(kStateLinuxGuid), 0x30400, 0x10000},
                              // Removed vbmeta_r to validate that it is not found
                              {"boot", Uuid(kStateLinuxGuid), 0x50400, 0x10000},
                              {"system", Uuid(kStateLinuxGuid), 0x60400, 0x10000},
                              {"recovery", Uuid(kStateLinuxGuid), 0x70400, 0x10000},
                              {GPT_FVM_NAME, Uuid(kStateLinuxGuid), 0x80400, 0x10000},
                          }));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // Make sure we can find the important partitions.
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_NOT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  }

  void ShouldNotFindPartitionBootTest() {
    constexpr uint64_t kBlockCount = 0x748038;
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                          {
                              {"bootloader", Uuid(kStateLinuxGuid), 0x10400, 0x10000},
                          }));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // Make sure we can't find zircon_a, which is aliased to "boot". The GPT logic would
    // previously only check prefixes, so "boot" would match with "bootloader".
    EXPECT_NOT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
  }

  void FindBootloaderTest() {
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // No boot0/boot1 yet, we shouldn't be able to find the bootloader.
    ASSERT_NOT_OK(
        partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "skip_metadata")));

    std::unique_ptr<BlockDevice> boot0_dev, boot1_dev;
    ASSERT_NO_FATAL_FAILURE(CreateDisk(&boot0_dev, kBlockCount * kBlockSize, kBoot0Type));
    ASSERT_NO_FATAL_FAILURE(CreateDisk(&boot1_dev, kBlockCount * kBlockSize, kBoot1Type));

    // Now it should succeed.
    ASSERT_OK(
        partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "skip_metadata")));
  }

  void SupportsPartitionTest() {
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev, 64 * kMebibyte));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    EXPECT_TRUE(partitioner->SupportsPartition(
        PartitionSpec(paver::Partition::kBootloaderA, "skip_metadata")));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconR)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaR)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_TRUE(
        partitioner->SupportsPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));

    // Unsupported partition type.
    EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kUnknown)));

    // Unsupported content type.
    EXPECT_FALSE(
        partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA, "foo_type")));
  }
};

TEST_F(SherlockPartitionerTests, FindPartitionNewGuids) {
  ASSERT_NO_FATAL_FAILURE(FindPartitionNewGuidsTest());
}

TEST_F(SherlockPartitionerTests, FindPartitionNewGuidsWithWrongTypeGUIDS) {
  ASSERT_NO_FATAL_FAILURE(FindPartitionNewGuidsWithWrongTypeGUIDSTest());
}

TEST_F(SherlockPartitionerTests, FindPartitionSecondary) {
  ASSERT_NO_FATAL_FAILURE(FindPartitionSecondaryTest());
}

TEST_F(SherlockPartitionerTests, ShouldNotFindPartitionBoot) {
  ASSERT_NO_FATAL_FAILURE(ShouldNotFindPartitionBootTest());
}

TEST_F(SherlockPartitionerTests, FindBootloader) { ASSERT_NO_FATAL_FAILURE(FindBootloaderTest()); }

TEST_F(SherlockPartitionerTests, SupportsPartition) {
  ASSERT_NO_FATAL_FAILURE(SupportsPartitionTest());
}

class MoonflowerPartitionerTests : public GptDevicePartitionerTests {
 protected:
  MoonflowerPartitionerTests() : GptDevicePartitionerTests("sorrel") {}

  IsolatedDevmgr::Args BaseDevmgrArgs() override {
    IsolatedDevmgr::Args args = GptDevicePartitionerTests::BaseDevmgrArgs();
    args.fshost_config.emplace_back(
        component_testing::ConfigCapability{.name = "fuchsia.fshost.MergeSuperAndUserdata",
                                            .value = component_testing::ConfigValue::Bool(true)});
    return args;
  }

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    fidl::ClientEnd<fuchsia_io::Directory> svc_root = RealmExposedDir();
    zx::result devices = CreateBlockDevices();
    if (devices.is_error()) {
      return devices.take_error();
    }
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kArm64,
        .system_partition_names = {"super_and_userdata"},
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    return paver::MoonflowerPartitioner::Initialize(paver_config, *devices, svc_root);
  }
};

TEST_F(MoonflowerPartitionerTests, InitializeWithoutGptFails) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&gpt_dev));

  ASSERT_NOT_OK(CreatePartitioner());
}

TEST_F(MoonflowerPartitionerTests, InitializeWithoutFvmFails) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev, 32 * kGibibyte));

  ASSERT_NOT_OK(CreatePartitioner());
}

TEST_F(MoonflowerPartitionerTests, FindPartition) {
  constexpr uint64_t kBlockCount = 0x748038;
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                        {
                            {GUID_ABR_META_NAME, Uuid(kAbrMetaType), 0x10400, 0x10000},
                            {"dtbo_a", Uuid(kDummyType), 0x30400, 0x10000},
                            {"dtbo_b", Uuid(kDummyType), 0x40400, 0x10000},
                            {"boot_a", Uuid(kZirconAType), 0x50400, 0x10000},
                            {"boot_b", Uuid(kZirconBType), 0x60400, 0x10000},
                            {"system_a", Uuid(kDummyType), 0x70400, 0x10000},
                            {"system_b", Uuid(kDummyType), 0x80400, 0x10000},
                            {GPT_VBMETA_A_NAME, Uuid(kVbMetaAType), 0x90400, 0x10000},
                            {GPT_VBMETA_B_NAME, Uuid(kVbMetaBType), 0xa0400, 0x10000},
                            {"reserved_a", Uuid(kDummyType), 0xc0400, 0x10000},
                            {"reserved_b", Uuid(kDummyType), 0xd0400, 0x10000},
                            {"reserved_c", Uuid(kVbMetaRType), 0xe0400, 0x10000},
                            {"cache", Uuid(kZirconRType), 0xf0400, 0x10000},
                            {"super", Uuid(kFvmType), 0x100400, 0x10000},
                            {"userdata", Uuid(kDummyType), 0x110400, 0x10000},
                            {"vendor_boot_a", Uuid(kDummyType), 0x120400, 0x10000},
                            {"vendor_boot_b", Uuid(kDummyType), 0x130400, 0x10000},
                        }));
  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  // Make sure we can find the important partitions.
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "dtbo")));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderB, "dtbo")));
  EXPECT_OK(
      partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "recovery_zbi")));
  EXPECT_OK(
      partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderB, "recovery_zbi")));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));

  // We should not be able to find a kBootloader partition with unknown `content_type`.
  EXPECT_NOT_OK(
      partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "foo_type")));

  // We should not be able to find an unslotted kBootloader partition.
  EXPECT_NOT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "cache")));
}

TEST_F(MoonflowerPartitionerTests, SupportsPartition) {
  constexpr uint64_t kBlockCount = 0x748038;
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                        {
                            {GUID_ABR_META_NAME, Uuid(kAbrMetaType), 0x10400, 0x10000},
                            {"dtbo_a", Uuid(kDummyType), 0x30400, 0x10000},
                            {"dtbo_b", Uuid(kDummyType), 0x40400, 0x10000},
                            {"boot_a", Uuid(kZirconAType), 0x50400, 0x10000},
                            {"boot_b", Uuid(kZirconBType), 0x60400, 0x10000},
                            {"system_a", Uuid(kDummyType), 0x70400, 0x10000},
                            {"system_b", Uuid(kDummyType), 0x80400, 0x10000},
                            {GPT_VBMETA_A_NAME, Uuid(kVbMetaAType), 0x90400, 0x10000},
                            {GPT_VBMETA_B_NAME, Uuid(kVbMetaBType), 0xa0400, 0x10000},
                            {"reserved_a", Uuid(kDummyType), 0xc0400, 0x10000},
                            {"reserved_b", Uuid(kDummyType), 0xd0400, 0x10000},
                            {"reserved_c", Uuid(kVbMetaRType), 0xe0400, 0x10000},
                            {"cache", Uuid(kZirconRType), 0xf0400, 0x10000},
                            {"super", Uuid(kFvmType), 0x100400, 0x10000},
                            {"userdata", Uuid(kDummyType), 0x110400, 0x10000},
                            {"vendor_boot_a", Uuid(kDummyType), 0x120400, 0x10000},
                            {"vendor_boot_b", Uuid(kDummyType), 0x130400, 0x10000},
                        }));

  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  // We should support any kBootloader partitions with non-empty `content_type`.
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA, "dtbo")));
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderB, "dtbo")));
  EXPECT_TRUE(partitioner->SupportsPartition(
      PartitionSpec(paver::Partition::kBootloaderA, "recovery_zbi")));
  EXPECT_TRUE(partitioner->SupportsPartition(
      PartitionSpec(paver::Partition::kBootloaderB, "recovery_zbi")));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  EXPECT_TRUE(partitioner->SupportsPartition(
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager, paver::kOpaqueVolumeContentType)));

  // Unsupported partition type.
  EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kUnknown)));

  // Unsupported content type.
  EXPECT_FALSE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta, "foo_type")));
  EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA, "")));
}

class IrisPartitionerTests : public GptDevicePartitionerTests {
 protected:
  IrisPartitionerTests() : GptDevicePartitionerTests("iris") {}

  void SetUp() override {
    GptDevicePartitionerTests::SetUp();
    EXPECT_OK(loop_.StartThread("iris-devicepartitioner-tests-loop"));
    fake_ufs_svc_ = std::make_unique<FakeUfsSvc>(loop_.dispatcher(), RealmExposedDir());
  }

  void TearDown() override {
    fake_ufs_svc_.reset();
    loop_.Shutdown();
    GptDevicePartitionerTests::TearDown();
  }

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    zx::result svc_root = fake_ufs_svc_->svc();
    if (svc_root.is_error()) {
      return svc_root.take_error();
    }
    zx::result devices = CreateBlockDevices();
    if (devices.is_error()) {
      return devices.take_error();
    }
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kArm64,
        .system_partition_names = {"super"},
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    return paver::IrisPartitioner::Initialize(std::move(*devices), std::move(*svc_root),
                                              paver_config);
  }

  void CreateIrisFullGptDevice(std::unique_ptr<BlockDevice>* gpt_dev) {
    constexpr uint64_t kBlockCount = 0x748038;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(gpt_dev, kBlockCount * block_size_,
                          {
                              {"devinfo", Uuid(kAbrMetaType), 0x10400, 0x10000},
                              {"dtbo_a", Uuid(kDummyType), 0x30400, 0x10000},
                              {"dtbo_b", Uuid(kDummyType), 0x40400, 0x10000},
                              {"boot_a", Uuid(kZirconAType), 0x50400, 0x10000},
                              {"boot_b", Uuid(kZirconBType), 0x60400, 0x10000},
                              {"system_a", Uuid(kDummyType), 0x70400, 0x10000},
                              {"system_b", Uuid(kDummyType), 0x80400, 0x10000},
                              {"vbmeta_a", Uuid(kVbMetaAType), 0x90400, 0x10000},
                              {"vbmeta_b", Uuid(kVbMetaBType), 0xa0400, 0x10000},
                              {"reserved_a", Uuid(kDummyType), 0xc0400, 0x10000},
                              {"reserved_b", Uuid(kDummyType), 0xd0400, 0x10000},
                              {"reserved_c", Uuid(kVbMetaRType), 0xe0400, 0x10000},
                              {"cache", Uuid(kZirconRType), 0xf0400, 0x10000},
                              {"super", Uuid(kFvmType), 0x100400, 0x10000},
                              {"userdata", Uuid(kDummyType), 0x110400, 0x10000},
                              {"vendor_boot_a", Uuid(kDummyType), 0x120400, 0x10000},
                              {"vendor_boot_b", Uuid(kDummyType), 0x130400, 0x10000},
                          }));
  }

  std::shared_ptr<paver::Context> context_ = std::make_shared<paver::Context>();
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::unique_ptr<FakeUfsSvc> fake_ufs_svc_;
};

TEST_F(IrisPartitionerTests, InitializeWithoutGptFails) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&gpt_dev));

  ASSERT_NOT_OK(CreatePartitioner());
}

TEST_F(IrisPartitionerTests, InitializeWithoutFvmFails) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev, 32 * kGibibyte));

  ASSERT_NOT_OK(CreatePartitioner());
}

TEST_F(IrisPartitionerTests, FindPartition) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateIrisFullGptDevice(&gpt_dev));
  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "dtbo")));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderB, "dtbo")));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  // EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));

  EXPECT_NOT_OK(
      partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "foo_type")));
  EXPECT_NOT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "cache")));
}

TEST_F(IrisPartitionerTests, FindPartitionFromBlockFallback) {
  // Set up a fake /block directory containing "abl_a" and "devinfo" partitions.
  std::vector<block_server::PartitionInfo> fake_partitions;
  fake_partitions.emplace_back(block_server::PartitionInfo{
      .block_count = 512,
      .block_size = 512,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "abl_a",
  });
  fake_partitions.emplace_back(block_server::PartitionInfo{
      .block_count = 512,
      .block_size = 512,
      .type_guid = {9, 10, 11, 12},
      .instance_guid = {13, 14, 15, 16},
      .name = "devinfo",
  });
  FakeBlockDirectory fake_block_dir(loop_.dispatcher(), std::move(fake_partitions));

  // Initialize BlockDevices pointing directly to our fake directory FD.
  zx::result devices =
      paver::BlockDevices::CreateFromFshostBlockDir(fake_block_dir.DuplicateBlockDirFd());
  ASSERT_OK(devices);

  // We still need a GPT device to satisfy InitializeGpt / PartitionService dependencies.
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateIrisFullGptDevice(&gpt_dev));

  zx::result svc_root = fake_ufs_svc_->svc();
  ASSERT_OK(svc_root);

  auto paver_config = paver::PaverConfig{
      .arch = paver::Arch::kArm64,
      .system_partition_names = {"super"},
      .zvb_current_slot = "_a",
  };
  zx::result partitioner_res =
      paver::IrisPartitioner::Initialize(*devices, std::move(*svc_root), paver_config);
  ASSERT_OK(partitioner_res);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = partitioner_res.value();

  // FindPartition should successfully fall back to /block and resolve the partitions.
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "abl")));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
}

TEST_F(IrisPartitionerTests, SupportsPartition) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateIrisFullGptDevice(&gpt_dev));

  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA, "dtbo")));
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderB, "dtbo")));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta)));
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  EXPECT_TRUE(partitioner->SupportsPartition(
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager, paver::kOpaqueVolumeContentType)));

  EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kUnknown)));
  EXPECT_FALSE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta, "foo_type")));
  EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA, "")));
}

TEST_F(IrisPartitionerTests, CreateAbrClient) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateIrisFullGptDevice(&gpt_dev));

  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();
  EXPECT_OK(partitioner->CreateAbrClient());
}

TEST_F(IrisPartitionerTests, IrisAbrClientGetBootSlot) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateIrisFullGptDevice(&gpt_dev));

  std::vector<uint8_t> buffer(block_size_, 0);
  uint32_t magic = IRIS_DEVINFO_MAGIC;
  std::memcpy(buffer.data(), &magic, sizeof(magic));

  paver::iris_devinfo_ab_data_t ab_data = {};
  ab_data.slots[0].set_active(1);
  ab_data.slots[0].set_successful(1);
  ab_data.slots[0].retry_count = 3;
  ab_data.slots[0].set_fastboot_ok(1);
  ab_data.slots[0].set_bl1_bootable(1);

  ab_data.slots[1].set_active(0);
  ab_data.slots[1].set_successful(0);
  ab_data.slots[1].retry_count = 2;
  ab_data.slots[1].set_fastboot_ok(0);
  ab_data.slots[1].set_bl1_bootable(0);

  std::memcpy(buffer.data() + paver::kIrisAbrMetadataOffset, &ab_data, sizeof(ab_data));
  ASSERT_NO_FATAL_FAILURE(WriteBlocks(gpt_dev.get(), 0x10400, 1, buffer.data()));

  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  zx::result abr_client_res = partitioner->CreateAbrClient();
  ASSERT_OK(abr_client_res);
  std::unique_ptr<abr::Client>& abr_client = abr_client_res.value();

  bool is_slot_marked_successful = false;
  EXPECT_EQ(abr_client->GetBootSlot(false, &is_slot_marked_successful), kAbrSlotIndexA);
  EXPECT_TRUE(is_slot_marked_successful);

  zx::result<AbrSlotInfo> info_a = abr_client->GetSlotInfo(kAbrSlotIndexA);
  ASSERT_OK(info_a);
  EXPECT_TRUE(info_a->is_bootable);
  EXPECT_TRUE(info_a->is_active);
  EXPECT_TRUE(info_a->is_marked_successful);
  EXPECT_EQ(info_a->num_tries_remaining, 0);

  zx::result<AbrSlotInfo> info_b = abr_client->GetSlotInfo(kAbrSlotIndexB);
  ASSERT_OK(info_b);
  EXPECT_TRUE(info_b->is_bootable);
  EXPECT_FALSE(info_b->is_active);
  EXPECT_FALSE(info_b->is_marked_successful);
  EXPECT_EQ(info_b->num_tries_remaining, 2);
}

TEST_F(IrisPartitionerTests, IrisAbrClientMarkSlotActive) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateIrisFullGptDevice(&gpt_dev));

  std::vector<uint8_t> buffer(block_size_, 0);
  uint32_t magic = IRIS_DEVINFO_MAGIC;
  std::memcpy(buffer.data(), &magic, sizeof(magic));

  paver::iris_devinfo_ab_data_t ab_data = {};
  ab_data.slots[0].set_active(1);
  ab_data.slots[0].set_successful(1);
  ab_data.slots[0].retry_count = 2;
  ab_data.slots[0].set_fastboot_ok(1);
  ab_data.slots[0].set_bl1_bootable(1);
  ab_data.slots[0].set_unbootable(0);

  // Unbootable
  ab_data.slots[1].set_active(0);
  ab_data.slots[1].set_successful(0);
  ab_data.slots[1].retry_count = 0;
  ab_data.slots[1].set_fastboot_ok(0);
  ab_data.slots[1].set_bl1_bootable(0);
  ab_data.slots[1].set_unbootable(1);

  std::memcpy(buffer.data() + paver::kIrisAbrMetadataOffset, &ab_data, sizeof(ab_data));
  ASSERT_NO_FATAL_FAILURE(WriteBlocks(gpt_dev.get(), 0x10400, 1, buffer.data()));

  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  zx::result abr_client_res = partitioner->CreateAbrClient();
  ASSERT_OK(abr_client_res);
  std::unique_ptr<abr::Client>& abr_client = abr_client_res.value();

  // Test slot translation state from ABR's perspective
  auto info_b = abr_client->GetSlotInfo(kAbrSlotIndexB);
  ASSERT_OK(info_b);
  EXPECT_FALSE(info_b->is_bootable);
  EXPECT_FALSE(info_b->is_active);
  EXPECT_FALSE(info_b->is_marked_successful);
  EXPECT_EQ(info_b->num_tries_remaining, 0);

  ASSERT_OK(abr_client->MarkSlotActive(kAbrSlotIndexB));
  ASSERT_OK(abr_client->Flush());
  EXPECT_EQ(fake_ufs_svc_->fake_ufs().boot_lun_en_, 2u);

  std::vector<uint8_t> read_buffer(block_size_, 0);
  ASSERT_NO_FATAL_FAILURE(ReadBlocks(gpt_dev.get(), 0x10400, 1, read_buffer.data()));

  uint32_t read_magic = 0;
  std::memcpy(&read_magic, read_buffer.data(), sizeof(read_magic));
  EXPECT_EQ(read_magic, IRIS_DEVINFO_MAGIC);

  paver::iris_devinfo_ab_data_t updated_ab_data;
  std::memcpy(&updated_ab_data, read_buffer.data() + paver::kIrisAbrMetadataOffset,
              sizeof(updated_ab_data));

  // Slot A is inactive
  EXPECT_EQ(updated_ab_data.slots[0].active(), 0);
  // Verify other states (unbootable, fastboot_ok, bl1_bootable) are preserved
  EXPECT_EQ(updated_ab_data.slots[0].successful(), 1);
  // The internal ABR logic of clearing retry count to 0 for successful slot should not persist
  // to storage.
  EXPECT_EQ(updated_ab_data.slots[0].retry_count, 2);
  EXPECT_EQ(updated_ab_data.slots[0].fastboot_ok(), 1);
  EXPECT_EQ(updated_ab_data.slots[0].bl1_bootable(), 1);

  // Verify Slot B is now active
  EXPECT_EQ(updated_ab_data.slots[1].active(), 1);
  EXPECT_EQ(updated_ab_data.slots[1].successful(), 0);
  // Maximum retries clamped at 3.
  EXPECT_EQ(updated_ab_data.slots[1].retry_count, 3);
  // Unbootable state is reset.
  EXPECT_EQ(updated_ab_data.slots[1].unbootable(), 0);
  EXPECT_EQ(updated_ab_data.slots[1].fastboot_ok(), 0);
  EXPECT_EQ(updated_ab_data.slots[1].bl1_bootable(), 0);
}

TEST_F(IrisPartitionerTests, IrisAbrClientMarkSlotUnbootable) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateIrisFullGptDevice(&gpt_dev));

  std::vector<uint8_t> buffer(block_size_, 0);
  uint32_t magic = IRIS_DEVINFO_MAGIC;
  std::memcpy(buffer.data(), &magic, sizeof(magic));

  paver::iris_devinfo_ab_data_t ab_data = {};
  ab_data.slots[0].set_active(1);
  ab_data.slots[0].set_successful(1);
  ab_data.slots[0].retry_count = 2;
  ab_data.slots[0].set_fastboot_ok(1);
  ab_data.slots[0].set_bl1_bootable(1);
  ab_data.slots[0].set_unbootable(0);

  // Unbootable B slot
  ab_data.slots[1].set_active(0);
  ab_data.slots[1].set_successful(0);
  ab_data.slots[1].retry_count = 0;
  ab_data.slots[1].set_fastboot_ok(1);
  ab_data.slots[1].set_bl1_bootable(1);
  ab_data.slots[1].set_unbootable(1);

  std::memcpy(buffer.data() + paver::kIrisAbrMetadataOffset, &ab_data, sizeof(ab_data));
  ASSERT_NO_FATAL_FAILURE(WriteBlocks(gpt_dev.get(), 0x10400, 1, buffer.data()));

  zx::result status = CreatePartitioner();
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  zx::result abr_client_res = partitioner->CreateAbrClient();
  ASSERT_OK(abr_client_res);
  std::unique_ptr<abr::Client>& abr_client = abr_client_res.value();

  ASSERT_OK(abr_client->MarkSlotUnbootable(kAbrSlotIndexA));
  ASSERT_OK(abr_client->Flush());

  std::vector<uint8_t> read_buffer(block_size_, 0);
  ASSERT_NO_FATAL_FAILURE(ReadBlocks(gpt_dev.get(), 0x10400, 1, read_buffer.data()));

  uint32_t read_magic = 0;
  std::memcpy(&read_magic, read_buffer.data(), sizeof(read_magic));
  EXPECT_EQ(read_magic, IRIS_DEVINFO_MAGIC);

  paver::iris_devinfo_ab_data_t updated_ab_data;
  std::memcpy(&updated_ab_data, read_buffer.data() + paver::kIrisAbrMetadataOffset,
              sizeof(updated_ab_data));

  // Slot A is active because both unbootable slots default to A.
  EXPECT_EQ(updated_ab_data.slots[0].active(), 1);
  EXPECT_EQ(updated_ab_data.slots[0].successful(), 0);
  EXPECT_EQ(updated_ab_data.slots[0].unbootable(), 1);
  EXPECT_EQ(updated_ab_data.slots[0].retry_count, 0);
  EXPECT_EQ(updated_ab_data.slots[0].fastboot_ok(), 1);
  EXPECT_EQ(updated_ab_data.slots[0].bl1_bootable(), 1);
}

class LuisPartitionerTests : public GptDevicePartitionerTests {
 protected:
  LuisPartitionerTests() : GptDevicePartitionerTests("luis", 512, "_a") {}

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    zx::result devices = CreateBlockDevices();
    if (devices.is_error()) {
      return devices.take_error();
    }
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kArm64,
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    return paver::LuisPartitioner::Initialize(*devices, RealmExposedDir(), paver_config);
  }
};

TEST_F(LuisPartitionerTests, InitializeWithoutGptFails) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&gpt_dev));

  ASSERT_NOT_OK(CreatePartitioner());
}

TEST_F(LuisPartitionerTests, InitializeWithoutFvmFails) {
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev, 32 * kGibibyte));

  ASSERT_NOT_OK(CreatePartitioner());
}

TEST_F(LuisPartitionerTests, FindPartition) {
  // kBlockCount should be a value large enough to accommodate all partitions and blocks reserved
  // by gpt. The current value is copied from the case of sherlock. As of now, we assume they
  // have the same disk size requirement.
  constexpr uint64_t kBlockCount = 0x748038;
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                        {
                            {GPT_DURABLE_BOOT_NAME, Uuid(kDummyType), 0x10400, 0x10000},
                            {GPT_BOOTLOADER_A_NAME, Uuid(kDummyType), 0x30400, 0x10000},
                            {GPT_BOOTLOADER_B_NAME, Uuid(kDummyType), 0x40400, 0x10000},
                            {GPT_BOOTLOADER_R_NAME, Uuid(kDummyType), 0x50400, 0x10000},
                            {GPT_VBMETA_A_NAME, Uuid(kDummyType), 0x60400, 0x10000},
                            {GPT_VBMETA_B_NAME, Uuid(kDummyType), 0x70400, 0x10000},
                            {GPT_VBMETA_R_NAME, Uuid(kDummyType), 0x80400, 0x10000},
                            {GPT_ZIRCON_A_NAME, Uuid(kDummyType), 0x90400, 0x10000},
                            {GPT_ZIRCON_B_NAME, Uuid(kDummyType), 0xa0400, 0x10000},
                            {GPT_ZIRCON_R_NAME, Uuid(kDummyType), 0xb0400, 0x10000},
                            {GPT_FACTORY_NAME, Uuid(kDummyType), 0xc0400, 0x10000},
                            {GPT_FVM_NAME, Uuid(kDummyType), 0xe0400, 0x10000},
                        }));

  zx::result status = CreatePartitioner(gpt_dev.get());
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  EXPECT_NOT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA)));

  std::unique_ptr<BlockDevice> boot0_dev, boot1_dev;
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&boot0_dev, kBlockCount * kBlockSize, kBoot0Type));
  ASSERT_NO_FATAL_FAILURE(CreateDisk(&boot1_dev, kBlockCount * kBlockSize, kBoot1Type));

  // Make sure we can find the important partitions.
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderR)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
  EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
}

TEST_F(LuisPartitionerTests, CreateAbrClient) {
  constexpr uint64_t kBlockCount = 0x748038;
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                        {
                            {GPT_DURABLE_BOOT_NAME, Uuid(kDurableBootType), 0x10400, 0x10000},
                            {GPT_FVM_NAME, Uuid(kNewFvmType), 0x20400, 0x10000},
                        }));

  fidl::ClientEnd<fuchsia_io::Directory> svc_root = RealmExposedDir();
  std::shared_ptr<paver::Context> context;
  zx::result devices = CreateBlockDevices();
  ASSERT_OK(devices);
  auto paver_config = paver::PaverConfig{
      .arch = paver::Arch::kArm64,
      .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
  };
  zx::result partitioner =
      paver::LuisPartitionerFactory().New(*devices, svc_root, paver_config, context);
  ASSERT_OK(partitioner);
  EXPECT_OK(partitioner->CreateAbrClient());
}

TEST_F(LuisPartitionerTests, SupportsPartition) {
  constexpr uint64_t kBlockCount = 0x748038;
  std::unique_ptr<BlockDevice> gpt_dev;
  ASSERT_NO_FATAL_FAILURE(
      CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                        {
                            {GPT_DURABLE_BOOT_NAME, Uuid(kDurableBootType), 0x10400, 0x10000},
                            {GPT_FVM_NAME, Uuid(kNewFvmType), 0x20400, 0x10000},
                        }));
  zx::result status = CreatePartitioner(gpt_dev.get());
  ASSERT_OK(status);
  std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderR)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconR)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaA)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaB)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaR)));
  EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta)));
  EXPECT_TRUE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  // Unsupported partition type.
  EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kUnknown)));

  // Unsupported content type.
  EXPECT_FALSE(
      partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta, "foo_type")));
}

class NelsonPartitionerTests : public GptDevicePartitionerTests {
 protected:
  static constexpr size_t kNelsonBlockSize = 512;
  static constexpr size_t kTplSize = 1024;
  static constexpr size_t kBootloaderSize = paver::kNelsonBL2Size + kTplSize;
  static constexpr uint8_t kBL2ImageValue = 0x01;
  static constexpr uint8_t kTplImageValue = 0x02;
  static constexpr size_t kTplSlotAOffset = 0x3000;
  static constexpr size_t kTplSlotBOffset = 0x4000;
  static constexpr size_t kUserTplBlockCount = 0x1000;

  NelsonPartitionerTests() : GptDevicePartitionerTests("nelson", kNelsonBlockSize, "_a") {}

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    fidl::ClientEnd<fuchsia_io::Directory> svc_root = RealmExposedDir();
    zx::result devices = CreateBlockDevices();
    if (devices.is_error()) {
      return devices.take_error();
    }
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kArm64,
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    return paver::NelsonPartitioner::Initialize(*devices, svc_root, paver_config);
  }

  static void CreateBootloaderPayload(zx::vmo* out) {
    fzl::VmoMapper mapper;
    ASSERT_OK(
        mapper.CreateAndMap(kBootloaderSize, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, out));
    uint8_t* start = static_cast<uint8_t*>(mapper.start());
    memset(start, kBL2ImageValue, paver::kNelsonBL2Size);
    memset(start + paver::kNelsonBL2Size, kTplImageValue, kTplSize);
  }

  void TestBootloaderWrite(const PartitionSpec& spec, uint8_t tpl_a_expected,
                           uint8_t tpl_b_expected) {
    std::unique_ptr<BlockDevice> gpt_dev, boot0, boot1;
    ASSERT_NO_FATAL_FAILURE(InitializeBlockDeviceForBootloaderTest(&gpt_dev, &boot0, &boot1));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();
    {
      auto partition_client = partitioner->FindPartition(spec);
      ASSERT_OK(partition_client);

      zx::vmo bootloader_payload;
      ASSERT_NO_FATAL_FAILURE(CreateBootloaderPayload(&bootloader_payload));
      ASSERT_OK(partition_client->Write(bootloader_payload, kBootloaderSize));
    }
    const size_t bl2_blocks = paver::kNelsonBL2Size / block_size_;
    const size_t tpl_blocks = kTplSize / block_size_;

    // info block stays unchanged. assume that storage data initialized as 0.
    ASSERT_NO_FATAL_FAILURE(ValidateBlockContent(boot0.get(), 0, 1, 0));
    ASSERT_NO_FATAL_FAILURE(ValidateBlockContent(boot0.get(), 1, bl2_blocks, kBL2ImageValue));
    ASSERT_NO_FATAL_FAILURE(
        ValidateBlockContent(boot0.get(), 1 + bl2_blocks, tpl_blocks, kTplImageValue));

    // info block stays unchanged
    ASSERT_NO_FATAL_FAILURE(ValidateBlockContent(boot1.get(), 0, 1, 0));
    ASSERT_NO_FATAL_FAILURE(ValidateBlockContent(boot1.get(), 1, bl2_blocks, kBL2ImageValue));
    ASSERT_NO_FATAL_FAILURE(
        ValidateBlockContent(boot1.get(), 1 + bl2_blocks, tpl_blocks, kTplImageValue));

    ASSERT_NO_FATAL_FAILURE(
        ValidateBlockContent(gpt_dev.get(), kTplSlotAOffset, tpl_blocks, tpl_a_expected));
    ASSERT_NO_FATAL_FAILURE(
        ValidateBlockContent(gpt_dev.get(), kTplSlotBOffset, tpl_blocks, tpl_b_expected));
  }

  void TestBootloaderRead(const PartitionSpec& spec, uint8_t tpl_a_data, uint8_t tpl_b_data,
                          zx::result<>* out_status, uint8_t* out) {
    std::unique_ptr<BlockDevice> gpt_dev, boot0, boot1;
    ASSERT_NO_FATAL_FAILURE(InitializeBlockDeviceForBootloaderTest(&gpt_dev, &boot0, &boot1));

    const size_t bl2_blocks = paver::kNelsonBL2Size / block_size_;
    const size_t tpl_blocks = kTplSize / block_size_;

    // Setup initial storage data
    struct initial_storage_data {
      const BlockDevice* blk_dev;
      uint64_t start_block;
      uint64_t size_in_blocks;
      uint8_t data;
    } initial_storage[] = {
        {boot0.get(), 1, bl2_blocks, kBL2ImageValue},               // bl2 in boot0
        {boot1.get(), 1, bl2_blocks, kBL2ImageValue},               // bl2 in boot1
        {boot0.get(), 1 + bl2_blocks, tpl_blocks, kTplImageValue},  // tpl in boot0
        {boot1.get(), 1 + bl2_blocks, tpl_blocks, kTplImageValue},  // tpl in boot1
        {gpt_dev.get(), kTplSlotAOffset, tpl_blocks, tpl_a_data},   // tpl_a
        {gpt_dev.get(), kTplSlotBOffset, tpl_blocks, tpl_b_data},   // tpl_b
    };
    for (auto& info : initial_storage) {
      std::vector<uint8_t> data(info.size_in_blocks * block_size_, info.data);
      ASSERT_NO_FATAL_FAILURE(
          WriteBlocks(info.blk_dev, info.start_block, info.size_in_blocks, data.data()));
    }

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    fzl::OwnedVmoMapper read_buf;
    ASSERT_OK(read_buf.CreateAndMap(kBootloaderSize, "test-read-bootloader"));
    auto partition_client = partitioner->FindPartition(spec);
    ASSERT_OK(partition_client);
    *out_status = partition_client->Read(read_buf.vmo(), kBootloaderSize);
    memcpy(out, read_buf.start(), kBootloaderSize);
  }

  static void ValidateBootloaderRead(const uint8_t* buf, uint8_t expected_bl2,
                                     uint8_t expected_tpl) {
    for (size_t i = 0; i < paver::kNelsonBL2Size; i++) {
      ASSERT_EQ(buf[i], expected_bl2, "bl2 mismatch at idx: %zu", i);
    }

    for (size_t i = 0; i < kTplSize; i++) {
      ASSERT_EQ(buf[i + paver::kNelsonBL2Size], expected_tpl, "tpl mismatch at idx: %zu", i);
    }
  }

  void InitializeBlockDeviceForBootloaderTest(std::unique_ptr<BlockDevice>* gpt_dev,
                                              std::unique_ptr<BlockDevice>* boot0,
                                              std::unique_ptr<BlockDevice>* boot1) {
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(gpt_dev, 64 * kMebibyte,
                          {
                              {"tpl_a", Uuid(kDummyType), kTplSlotAOffset, kUserTplBlockCount},
                              {"tpl_b", Uuid(kDummyType), kTplSlotBOffset, kUserTplBlockCount},
                          }));

    ASSERT_NO_FATAL_FAILURE(CreateDisk(boot0, kUserTplBlockCount * kNelsonBlockSize, kBoot0Type));
    ASSERT_NO_FATAL_FAILURE(CreateDisk(boot1, kUserTplBlockCount * kNelsonBlockSize, kBoot1Type));
  }

  void InitializeWithoutFvmSucceedsTest() {
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev, 32 * kGibibyte));

    ASSERT_OK(CreatePartitioner());
  }

  void FindPartitionTest() {
    // kBlockCount should be a value large enough to accommodate all partitions and blocks reserved
    // by gpt. The current value is copied from the case of sherlock. The actual size of fvm
    // partition on nelson is yet to be finalized.
    constexpr uint64_t kBlockCount = 0x748038;
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                          {
                              // The initial gpt partitions are randomly chosen and does not
                              // necessarily reflect the actual gpt partition layout in product.
                              {GUID_ABR_META_NAME, Uuid(kAbrMetaType), 0x10400, 0x10000},
                              {"tpl_a", Uuid(kDummyType), 0x30400, 0x10000},
                              {"tpl_b", Uuid(kDummyType), 0x40400, 0x10000},
                              {"boot_a", Uuid(kZirconAType), 0x50400, 0x10000},
                              {"boot_b", Uuid(kZirconBType), 0x60400, 0x10000},
                              {"system_a", Uuid(kDummyType), 0x70400, 0x10000},
                              {"system_b", Uuid(kDummyType), 0x80400, 0x10000},
                              {GPT_VBMETA_A_NAME, Uuid(kVbMetaAType), 0x90400, 0x10000},
                              {GPT_VBMETA_B_NAME, Uuid(kVbMetaBType), 0xa0400, 0x10000},
                              {"reserved_a", Uuid(kDummyType), 0xc0400, 0x10000},
                              {"reserved_b", Uuid(kDummyType), 0xd0400, 0x10000},
                              {"reserved_c", Uuid(kVbMetaRType), 0xe0400, 0x10000},
                              {"cache", Uuid(kZirconRType), 0xf0400, 0x10000},
                              {"data", Uuid(kFvmType), 0x100400, 0x10000},
                          }));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    EXPECT_NOT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA)));

    std::unique_ptr<BlockDevice> boot0_dev, boot1_dev;
    ASSERT_NO_FATAL_FAILURE(CreateDisk(&boot0_dev, kBlockCount * kBlockSize, kBoot0Type));
    ASSERT_NO_FATAL_FAILURE(CreateDisk(&boot1_dev, kBlockCount * kBlockSize, kBoot1Type));

    // Make sure we can find the important partitions.
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "bl2")));
    EXPECT_OK(
        partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "bootloader")));
    EXPECT_OK(
        partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderB, "bootloader")));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "tpl")));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderB, "tpl")));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  }

  void CreateAbrClientTest() {
    constexpr uint64_t kBlockCount = 0x748038;
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                          {
                              {GUID_ABR_META_NAME, Uuid(kAbrMetaType), 0x10400, 0x10000},
                              {GPT_FVM_NAME, Uuid(kNewFvmType), 0x20400, 0x10000},
                          }));
    fidl::ClientEnd<fuchsia_io::Directory> svc_root = RealmExposedDir();
    std::shared_ptr<paver::Context> context;
    zx::result devices = CreateBlockDevices();
    ASSERT_OK(devices);
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kArm64,
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    zx::result partitioner =
        paver::NelsonPartitionerFactory().New(*devices, svc_root, paver_config, context);
    ASSERT_OK(partitioner);
    EXPECT_OK(partitioner->CreateAbrClient());
  }

  void SupportsPartitionTest() {
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev, 64 * kMebibyte));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    EXPECT_TRUE(
        partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA, "bl2")));
    EXPECT_TRUE(partitioner->SupportsPartition(
        PartitionSpec(paver::Partition::kBootloaderA, "bootloader")));
    EXPECT_TRUE(partitioner->SupportsPartition(
        PartitionSpec(paver::Partition::kBootloaderB, "bootloader")));
    EXPECT_TRUE(
        partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderA, "tpl")));
    EXPECT_TRUE(
        partitioner->SupportsPartition(PartitionSpec(paver::Partition::kBootloaderB, "tpl")));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kZirconR)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kVbMetaR)));
    EXPECT_TRUE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_TRUE(
        partitioner->SupportsPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
    // Unsupported partition type.
    EXPECT_FALSE(partitioner->SupportsPartition(PartitionSpec(paver::Partition::kUnknown)));

    // Unsupported content type.
    EXPECT_FALSE(
        partitioner->SupportsPartition(PartitionSpec(paver::Partition::kAbrMeta, "foo_type")));
  }

  void ValidatePayloadTest() {
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&gpt_dev, 64 * kMebibyte));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // Test invalid bootloader payload size.
    std::vector<uint8_t> payload_bl2_size(paver::kNelsonBL2Size);
    ASSERT_NOT_OK(
        partitioner->ValidatePayload(PartitionSpec(paver::Partition::kBootloaderA, "bootloader"),
                                     std::span<uint8_t>(payload_bl2_size)));
    ASSERT_NOT_OK(
        partitioner->ValidatePayload(PartitionSpec(paver::Partition::kBootloaderB, "bootloader"),
                                     std::span<uint8_t>(payload_bl2_size)));

    std::vector<uint8_t> payload_bl2_tpl_size(static_cast<size_t>(2) * 1024 * 1024);
    ASSERT_OK(
        partitioner->ValidatePayload(PartitionSpec(paver::Partition::kBootloaderA, "bootloader"),
                                     std::span<uint8_t>(payload_bl2_tpl_size)));
    ASSERT_OK(
        partitioner->ValidatePayload(PartitionSpec(paver::Partition::kBootloaderB, "bootloader"),
                                     std::span<uint8_t>(payload_bl2_tpl_size)));
  }

  void WriteBootloaderATest() {
    TestBootloaderWrite(PartitionSpec(paver::Partition::kBootloaderA, "bootloader"), kTplImageValue,
                        0x00);
  }

  void WriteBootloaderBTest() {
    TestBootloaderWrite(PartitionSpec(paver::Partition::kBootloaderB, "bootloader"), 0x00,
                        kTplImageValue);
  }

  void ReadBootloaderAFailTest() {
    auto spec = PartitionSpec(paver::Partition::kBootloaderA, "bootloader");
    std::vector<uint8_t> read_buf(kBootloaderSize);
    zx::result<> status = zx::ok();
    ASSERT_NO_FATAL_FAILURE(
        TestBootloaderRead(spec, 0x03, kTplImageValue, &status, read_buf.data()));
    ASSERT_NOT_OK(status);
  }

  void ReadBootloaderBFailTest() {
    auto spec = PartitionSpec(paver::Partition::kBootloaderB, "bootloader");
    std::vector<uint8_t> read_buf(kBootloaderSize);
    zx::result<> status = zx::ok();
    ASSERT_NO_FATAL_FAILURE(
        TestBootloaderRead(spec, kTplImageValue, 0x03, &status, read_buf.data()));
    ASSERT_NOT_OK(status);
  }

  void ReadBootloaderASucceedTest() {
    auto spec = PartitionSpec(paver::Partition::kBootloaderA, "bootloader");
    std::vector<uint8_t> read_buf(kBootloaderSize);
    zx::result<> status = zx::ok();
    ASSERT_NO_FATAL_FAILURE(
        TestBootloaderRead(spec, kTplImageValue, 0x03, &status, read_buf.data()));
    ASSERT_OK(status);
    ASSERT_NO_FATAL_FAILURE(
        ValidateBootloaderRead(read_buf.data(), kBL2ImageValue, kTplImageValue));
  }

  void ReadBootloaderBSucceedTest() {
    std::vector<uint8_t> read_buf(kBootloaderSize);
    auto spec = PartitionSpec(paver::Partition::kBootloaderB, "bootloader");
    zx::result<> status = zx::ok();
    ASSERT_NO_FATAL_FAILURE(
        TestBootloaderRead(spec, 0x03, kTplImageValue, &status, read_buf.data()));
    ASSERT_OK(status);
    ASSERT_NO_FATAL_FAILURE(
        ValidateBootloaderRead(read_buf.data(), kBL2ImageValue, kTplImageValue));
  }
};

TEST_F(NelsonPartitionerTests, InitializeWithoutFvmSucceeds) {
  ASSERT_NO_FATAL_FAILURE(InitializeWithoutFvmSucceedsTest());
}

TEST_F(NelsonPartitionerTests, FindPartition) { ASSERT_NO_FATAL_FAILURE(FindPartitionTest()); }

TEST_F(NelsonPartitionerTests, CreateAbrClient) { ASSERT_NO_FATAL_FAILURE(CreateAbrClientTest()); }

TEST_F(NelsonPartitionerTests, SupportsPartition) {
  ASSERT_NO_FATAL_FAILURE(SupportsPartitionTest());
}

TEST_F(NelsonPartitionerTests, ValidatePayload) { ASSERT_NO_FATAL_FAILURE(ValidatePayloadTest()); }

TEST_F(NelsonPartitionerTests, WriteBootloaderA) {
  ASSERT_NO_FATAL_FAILURE(WriteBootloaderATest());
}

TEST_F(NelsonPartitionerTests, WriteBootloaderB) {
  ASSERT_NO_FATAL_FAILURE(WriteBootloaderBTest());
}

TEST_F(NelsonPartitionerTests, ReadBootloaderAFail) {
  ASSERT_NO_FATAL_FAILURE(ReadBootloaderAFailTest());
}

TEST_F(NelsonPartitionerTests, ReadBootloaderBFail) {
  ASSERT_NO_FATAL_FAILURE(ReadBootloaderBFailTest());
}

TEST_F(NelsonPartitionerTests, ReadBootloaderASucceed) {
  ASSERT_NO_FATAL_FAILURE(ReadBootloaderASucceedTest());
}

TEST_F(NelsonPartitionerTests, ReadBootloaderBSucceed) {
  ASSERT_NO_FATAL_FAILURE(ReadBootloaderBSucceedTest());
}

class Vim3PartitionerTests : public GptDevicePartitionerTests {
 protected:
  static constexpr const char kDummyBootloaderHeader[] = "bootloader";

  Vim3PartitionerTests() : GptDevicePartitionerTests("vim3") {}

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    fidl::ClientEnd<fuchsia_io::Directory> svc_root = RealmExposedDir();
    zx::result devices = CreateBlockDevices();
    if (devices.is_error()) {
      return devices.take_error();
    }
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kArm64,
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    return paver::Vim3Partitioner::Initialize(*devices, svc_root, paver_config);
  }

  void CreateBootloaderDevices(std::unique_ptr<BlockDevice>* boot0,
                               std::unique_ptr<BlockDevice>* boot1) {
    zx::vmo vmo0;
    ASSERT_OK(zx::vmo::create(32 * 1024 * 1024, 0, &vmo0));
    // Write the first two blocks with a placeholder value we check later in VerifyBootloaderDevice.
    ASSERT_OK(vmo0.write(kDummyBootloaderHeader, 0, strlen(kDummyBootloaderHeader)));
    ASSERT_OK(vmo0.write(kDummyBootloaderHeader, block_size_, strlen(kDummyBootloaderHeader)));
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithContents(boot0, std::move(vmo0), kBoot0Type));

    zx::vmo vmo1;
    ASSERT_OK(zx::vmo::create(32 * 1024 * 1024, 0, &vmo1));
    ASSERT_OK(vmo1.write(kDummyBootloaderHeader, 0, strlen(kDummyBootloaderHeader)));
    ASSERT_OK(vmo1.write(kDummyBootloaderHeader, block_size_, strlen(kDummyBootloaderHeader)));
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithContents(boot1, std::move(vmo1), kBoot1Type));
  }

  void VerifyBootloaderDevice(const BlockDevice* device) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(block_size_, 0, &vmo));
    for (size_t block = 0; block < 2; ++block) {
      ASSERT_NO_FATAL_FAILURE(device->Read(vmo, block_size_, block, 0));
      std::vector<char> data(block_size_);
      ASSERT_OK(vmo.read(data.data(), 0, block_size_));
      EXPECT_EQ(std::string_view(data.data(), strlen(kDummyBootloaderHeader)),
                std::string_view(kDummyBootloaderHeader));
    }
  }

  void InitializeWithoutGptFailsTest() {
    std::unique_ptr<BlockDevice> boot0_dev;
    std::unique_ptr<BlockDevice> boot1_dev;
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(CreateBootloaderDevices(&boot0_dev, &boot1_dev));
    ASSERT_NO_FATAL_FAILURE(CreateDisk(&gpt_dev));

    ASSERT_NOT_OK(CreatePartitioner());
    ASSERT_NO_FATAL_FAILURE(VerifyBootloaderDevice(boot0_dev.get()));
    ASSERT_NO_FATAL_FAILURE(VerifyBootloaderDevice(boot1_dev.get()));
  }

  void InitializeTest() {
    std::unique_ptr<BlockDevice> boot0_dev;
    std::unique_ptr<BlockDevice> boot1_dev;
    std::unique_ptr<BlockDevice> gpt_dev;
    ASSERT_NO_FATAL_FAILURE(CreateBootloaderDevices(&boot0_dev, &boot1_dev));
    constexpr uint64_t kBlockCount = 0x748038;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&gpt_dev, kBlockCount * block_size_,
                          // partition size / location is arbitrary
                          {
                              {GPT_DURABLE_BOOT_NAME, Uuid(kDurableBootType), 0x10400, 0x10000},
                              {GPT_VBMETA_A_NAME, Uuid(kVbMetaType), 0x20400, 0x10000},
                              {GPT_VBMETA_B_NAME, Uuid(kVbMetaType), 0x30400, 0x10000},
                              {GPT_VBMETA_R_NAME, Uuid(kVbMetaType), 0x40400, 0x10000},
                              {GPT_ZIRCON_A_NAME, Uuid(kZirconType), 0x50400, 0x10000},
                              {GPT_ZIRCON_B_NAME, Uuid(kZirconType), 0x60400, 0x10000},
                              {GPT_ZIRCON_R_NAME, Uuid(kZirconType), 0x70400, 0x10000},
                              {GPT_FVM_NAME, Uuid(kNewFvmType), 0x80400, 0x10000},
                          }));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // Make sure we can find the important partitions.
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaR)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));

    ASSERT_NO_FATAL_FAILURE(VerifyBootloaderDevice(boot0_dev.get()));
    ASSERT_NO_FATAL_FAILURE(VerifyBootloaderDevice(boot1_dev.get()));
  }
};

TEST_F(Vim3PartitionerTests, InitializeWithoutGptFails) {
  ASSERT_NO_FATAL_FAILURE(InitializeWithoutGptFailsTest());
}

TEST_F(Vim3PartitionerTests, Initialize) { ASSERT_NO_FATAL_FAILURE(InitializeTest()); }

class AndroidPartitionerTests : public GptDevicePartitionerTests {
 protected:
  AndroidPartitionerTests() = default;

  IsolatedDevmgr::Args BaseDevmgrArgs() override {
    IsolatedDevmgr::Args args = GptDevicePartitionerTests::BaseDevmgrArgs();
    args.fshost_config.emplace_back(component_testing::ConfigCapability{
        .name = "fuchsia.fshost.GptAll", .value = component_testing::ConfigValue::Bool(true)});
    return args;
  }

  // Create a DevicePartition for a device.
  zx::result<std::unique_ptr<paver::DevicePartitioner>> CreatePartitioner(
      BlockDevice* gpt = nullptr) {
    fidl::ClientEnd<fuchsia_io::Directory> svc_root = RealmExposedDir();
    zx::result devices = CreateBlockDevices();
    if (devices.is_error()) {
      return devices.take_error();
    }
    auto paver_config = paver::PaverConfig{
        .arch = paver::Arch::kX64,
        .system_partition_names = {"super"},
        .zvb_current_slot = slot_suffix_.empty() ? "_a" : slot_suffix_,
    };
    return paver::AndroidDevicePartitioner::Initialize(*devices, svc_root, paver_config, {});
  }

  void InitializeTest() {
    std::unique_ptr<BlockDevice> primary_gpt_dev;
    std::unique_ptr<BlockDevice> other_gpt_dev;
    constexpr uint64_t kBlockCount = 0x748038;
    ASSERT_NO_FATAL_FAILURE(
        CreateDiskWithGpt(&primary_gpt_dev, kBlockCount * block_size_,
                          // partition size / location is arbitrary
                          {
                              {"misc", Uuid(kAbrMetaType), 0x10400, 0x10000},
                              {"vbmeta_a", Uuid(kVbMetaType), 0x20400, 0x10000},
                              {"vbmeta_b", Uuid(kVbMetaType), 0x30400, 0x10000},
                              {"boot_a", Uuid(kBootloaderType), 0x50400, 0x10000},
                              {"boot_b", Uuid(kBootloaderType), 0x60400, 0x10000},
                              {"vendor_boot_a", Uuid(kZirconType), 0x70400, 0x10000},
                              {"vendor_boot_b", Uuid(kZirconType), 0x80400, 0x10000},
                              {"super", Uuid(kNewFvmType), 0x90400, 0x10000},
                          }));
    ASSERT_NO_FATAL_FAILURE(CreateDiskWithGpt(&other_gpt_dev, 512 * block_size_,
                                              // partition size / location is arbitrary
                                              {
                                                  {"vbmeta", Uuid(kVbMetaType), 0x30, 0x1},
                                                  {"frp", Uuid::Generate(), 0x31, 0x1},
                                              }));

    zx::result status = CreatePartitioner();
    ASSERT_OK(status);
    std::unique_ptr<paver::DevicePartitioner>& partitioner = status.value();

    // Make sure we can find the important partitions.
    EXPECT_OK(
        partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderA, "boot_shim")));
    EXPECT_OK(
        partitioner->FindPartition(PartitionSpec(paver::Partition::kBootloaderB, "boot_shim")));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kZirconB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kAbrMeta)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaA)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kVbMetaB)));
    EXPECT_OK(partitioner->FindPartition(PartitionSpec(paver::Partition::kFuchsiaVolumeManager)));
  }
};

TEST_F(AndroidPartitionerTests, Initialize) { ASSERT_NO_FATAL_FAILURE(InitializeTest()); }

}  // namespace
