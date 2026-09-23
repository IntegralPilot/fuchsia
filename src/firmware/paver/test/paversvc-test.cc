// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fidl/fuchsia.sysinfo/cpp/wire_test_base.h>
#include <lib/abr/data.h>
#include <lib/abr/util.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/cksum.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/directory_watcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sysconfig/sync-client.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/vmo.h>
#include <sparse_format.h>

#include <numeric>
#include <optional>
// Clean up the unhelpful defines from sparse_format.h
#undef error

#include <zircon/hw/gpt.h>

#include <memory>

#include <fbl/algorithm.h>
#include <fbl/unique_fd.h>
#include <soc/aml-common/aml-guid.h>
#include <zxtest/zxtest.h>

#include "src/firmware/paver/abr-client.h"
#include "src/firmware/paver/astro.h"
#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/gpt.h"
#include "src/firmware/paver/luis.h"
#include "src/firmware/paver/moonflower.h"
#include "src/firmware/paver/nelson.h"
#include "src/firmware/paver/paver.h"
#include "src/firmware/paver/sherlock.h"
#include "src/firmware/paver/test/test-utils.h"
#include "src/firmware/paver/uefi.h"
#include "src/firmware/paver/vim3.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace {

namespace partition = fuchsia_storage_block;

using device_watcher::RecursiveWaitForFile;
using driver_integration_test::IsolatedDevmgr;
using uuid::Uuid;

constexpr std::string_view kFirmwareTypeBootloader;
constexpr std::string_view kFirmwareTypeBl2 = "bl2";
constexpr std::string_view kFirmwareTypeUnsupported = "unsupported_type";

// BL2 images must be exactly this size.
constexpr size_t kBl2ImageSize = 0x10000;
// Make sure we can use our page-based APIs to work with the BL2 image.
static_assert(kBl2ImageSize % kPageSize == 0);
constexpr size_t kBl2ImagePages = kBl2ImageSize / kPageSize;

constexpr uint32_t kBootloaderFirstBlock = 4;
constexpr uint32_t kBootloaderBlocks = 4;
constexpr uint32_t kBootloaderLastBlock = kBootloaderFirstBlock + kBootloaderBlocks - 1;
constexpr uint32_t kZirconAFirstBlock = kBootloaderLastBlock + 1;
constexpr uint32_t kZirconALastBlock = kZirconAFirstBlock + 1;
constexpr uint32_t kBl2FirstBlock = kNumBlocks - 1;
constexpr uint32_t kFvmFirstBlock = 18;

fuchsia_hardware_nand::wire::RamNandInfo BaseNandInfo() {
  return {
      .nand_info =
          {
              .page_size = kPageSize,
              .pages_per_block = kPagesPerBlock,
              .num_blocks = kNumBlocks,
              .ecc_bits = 8,
              .oob_size = kOobSize,
              .nand_class = fuchsia_hardware_nand::wire::Class::kPartmap,
              .partition_guid = {},
          },
      .partition_map =
          {
              .device_guid = {},
              .partition_count = 8,
              .partitions =
                  {
                      fuchsia_hardware_nand::wire::Partition{
                          .type_guid = {},
                          .unique_guid = {},
                          .first_block = 0,
                          .last_block = 3,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name = {},
                          .hidden = true,
                          .bbt = true,
                      },
                      {
                          .type_guid = GUID_BOOTLOADER_VALUE,
                          .unique_guid = {},
                          .first_block = kBootloaderFirstBlock,
                          .last_block = kBootloaderLastBlock,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name = {'b', 'o', 'o', 't', 'l', 'o', 'a', 'd', 'e', 'r'},
                          .hidden = false,
                          .bbt = false,
                      },
                      {
                          .type_guid = GUID_ZIRCON_A_VALUE,
                          .unique_guid = {},
                          .first_block = kZirconAFirstBlock,
                          .last_block = kZirconALastBlock,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name = {'z', 'i', 'r', 'c', 'o', 'n', '-', 'a'},
                          .hidden = false,
                          .bbt = false,
                      },
                      {
                          .type_guid = GUID_ZIRCON_B_VALUE,
                          .unique_guid = {},
                          .first_block = 10,
                          .last_block = 11,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name = {'z', 'i', 'r', 'c', 'o', 'n', '-', 'b'},
                          .hidden = false,
                          .bbt = false,
                      },
                      {
                          .type_guid = GUID_ZIRCON_R_VALUE,
                          .unique_guid = {},
                          .first_block = 12,
                          .last_block = 13,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name = {'z', 'i', 'r', 'c', 'o', 'n', '-', 'r'},
                          .hidden = false,
                          .bbt = false,
                      },
                      {
                          .type_guid = GUID_SYS_CONFIG_VALUE,
                          .unique_guid = {},
                          .first_block = 14,
                          .last_block = 17,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name = {'s', 'y', 's', 'c', 'o', 'n', 'f', 'i', 'g'},
                          .hidden = false,
                          .bbt = false,
                      },
                      {
                          .type_guid = GUID_FVM_VALUE,
                          .unique_guid = {},
                          .first_block = kFvmFirstBlock,
                          .last_block = kBl2FirstBlock - 1,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name = {'f', 'v', 'm'},
                          .hidden = false,
                          .bbt = false,
                      },
                      {
                          .type_guid = GUID_BL2_VALUE,
                          .unique_guid = {},
                          .first_block = kBl2FirstBlock,
                          .last_block = kBl2FirstBlock,
                          .copy_count = 0,
                          .copy_byte_offset = 0,
                          .name =
                              {
                                  'b',
                                  'l',
                                  '2',
                              },
                          .hidden = false,
                          .bbt = false,
                      },
                  },
          },
      .export_nand_config = true,
      .export_partition_map = true,
  };
}

// Creates a `Buffer` with payload of `data` repeating for `num_pages` pages.
void CreateBuffer(size_t num_pages, fuchsia_mem::wire::Buffer* out, uint8_t data = 0x4a) {
  zx::vmo vmo;
  fzl::VmoMapper mapper;
  const size_t size = kPageSize * num_pages;
  ASSERT_OK(mapper.CreateAndMap(size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));
  memset(mapper.start(), data, mapper.size());
  out->vmo = std::move(vmo);
  out->size = size;
}

// Creates a `Buffer` with the given data as the payload.
void CreateBuffer(std::span<const uint8_t> data, fuchsia_mem::wire::Buffer* out) {
  zx::vmo vmo;
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(data.size(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));
  memcpy(mapper.start(), data.data(), data.size());
  *out = {.vmo = std::move(vmo), .size = data.size()};
}

// Verifies that `buffer` contains exactly `data`.
void VerifyBufferContents(const fuchsia_mem::wire::Buffer& buffer, std::span<const uint8_t> data) {
  ASSERT_EQ(buffer.size, data.size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.Map(buffer.vmo, 0, buffer.size, ZX_VM_PERM_READ));
  ASSERT_BYTES_EQ(mapper.start(), data.data(), buffer.size);
}

class PaverServiceTest : public PaverTest {
 public:
  PaverServiceTest();
  ~PaverServiceTest() override;

  void SetUp() override {
    PaverTest::SetUp();
    // For convenience, call Init() in SetUp.  This will do for most tests, although some (e.g.
    // PaverServiceSkipBlockTest) need to call manually to inject varied boot args.
    ASSERT_NO_FATAL_FAILURE(Init(DevmgrArgs()));
  }

  void Init(IsolatedDevmgr::Args args) {
    ASSERT_OK(IsolatedDevmgr::Create(&args, &devmgr_));

    ASSERT_NO_FATAL_FAILURE(
        StartPaver(devmgr_.devfs_root().duplicate(), devmgr_.RealmExposedDir()));
  }

  virtual IsolatedDevmgr::Args DevmgrArgs() {
    IsolatedDevmgr::Args args;
    args.disable_block_watcher = false;
    return args;
  }

 protected:
  virtual paver::PaverConfig Config() { return {.zvb_current_slot = "_a"}; }

  void StartPaver(fbl::unique_fd devfs_root, fidl::ClientEnd<fuchsia_io::Directory> svc_root) {
    zx::result paver = paver::Paver::Create(Config(), std::move(devfs_root));
    ASSERT_OK(paver);
    paver_ = std::move(*paver);
    paver_->set_dispatcher(loop_.dispatcher());
    paver_->set_svc_root(std::move(svc_root));

    auto [client, server] = fidl::Endpoints<fuchsia_paver::Paver>::Create();
    client_ = fidl::WireSyncClient(std::move(client));
    fidl::BindServer(loop_.dispatcher(), std::move(server), paver_.get());
  }

  static constexpr size_t kKilobyte = 1 << 10;

  static void ValidateWritten(const fuchsia_mem::wire::Buffer& buf, size_t num_pages) {
    ASSERT_GE(buf.size, num_pages * kPageSize);
    fzl::VmoMapper mapper;
    ASSERT_OK(mapper.Map(buf.vmo, 0,
                         fbl::round_up(num_pages * kPageSize, zx_system_get_page_size()),
                         ZX_VM_PERM_READ));
    const uint8_t* start = reinterpret_cast<uint8_t*>(mapper.start());
    for (size_t i = 0; i < num_pages * kPageSize; i++) {
      ASSERT_EQ(start[i], 0x4a, "i = %zu", i);
    }
  }

  // Opens a paver `DataSink` and calls `WriteFirmware`.
  //
  // Call with `ASSERT_NO_FATAL_FAILURE()` to ensure expected behavior.
  //
  // Args:
  // * `firmware_type`: type string to pass to `WriteFirmware()`
  // * `configuration`: paver slot configuration to pass to `WriteFirmware()`
  // * `data`: data to copy into a VMO and pass to `WriteFirmware()`
  // * `result`: will be filled with the FIDL result
  void WriteFirmware(std::string_view firmware_type,
                     fuchsia_paver::wire::Configuration configuration, std::vector<uint8_t> data,
                     fuchsia_paver::wire::WriteFirmwareResult* result) {
    auto [local, remote] = fidl::Endpoints<fuchsia_paver::DataSink>::Create();
    ASSERT_OK(client_->FindDataSink(std::move(remote)));
    fidl::WireSyncClient data_sink{std::move(local)};

    fuchsia_mem::wire::Buffer wire_buffer;
    ASSERT_NO_FATAL_FAILURE(CreateBuffer(data, &wire_buffer));

    auto fidl_result = data_sink->WriteFirmware(
        configuration, fidl::StringView::FromExternal(firmware_type), std::move(wire_buffer));
    ASSERT_OK(fidl_result.status());
    *result = fidl_result->result;
  }

  IsolatedDevmgr devmgr_;
  std::unique_ptr<paver::Paver> paver_;
  fidl::WireSyncClient<fuchsia_paver::Paver> client_;
  async::Loop loop_;
};

PaverServiceTest::PaverServiceTest() : loop_(&kAsyncLoopConfigAttachToCurrentThread) {
  loop_.StartThread("paver-svc-test-loop");
}

PaverServiceTest::~PaverServiceTest() {
  loop_.Shutdown();
  paver_.reset();
}

// Common logic to test writing an asset to disk and reading it back.
//
// Args:
// * `block_device`: the block device object providing the disk.
// * `data_sink`: the paver data sink to make FIDL calls on.
// * `configuration`: configuration to write.
// * `asset`: asset type to write.
// * `partition_start_block`: the disk block number that we expect this data to start at.
void TestReadWriteAsset(BlockDevice& block_device,
                        fidl::WireSyncClient<fuchsia_paver::DynamicDataSink> data_sink,
                        fuchsia_paver::wire::Configuration configuration,
                        fuchsia_paver::wire::Asset asset, size_t partition_start_block) {
  // WriteAsset(Kernel) requires something that looks like a kernel.
  fbl::Array<uint8_t> data = CreateZbiHeader(paver::PaverConfig().arch, 1000, nullptr);

  // Use WriteAsset() FIDL to write a payload to disk.
  fuchsia_mem::wire::Buffer buffer;
  ASSERT_NO_FATAL_FAILURE(CreateBuffer(data, &buffer));
  auto write_result = data_sink->WriteAsset(configuration, asset, std::move(buffer));
  ASSERT_OK(write_result.status());
  ASSERT_OK(write_result.value().status);

  // Reset the buffer then read from disk directly to make sure the bytes were written correctly.
  // Block devices can only read full pages, so we need to round up here.
  ASSERT_NO_FATAL_FAILURE(CreateBuffer(fbl::round_up(data.size(), kBlockSize), &buffer, 0x00));
  ASSERT_NO_FATAL_FAILURE(block_device.Read(buffer.vmo, buffer.size, partition_start_block, 0));
  // Only verify up to the data we actually wrote.
  buffer.size = data.size();
  ASSERT_NO_FATAL_FAILURE(VerifyBufferContents(buffer, data));

  // Use ReadAsset() FIDL to make sure we get the expected data back.
  auto read_result = data_sink->ReadAsset(configuration, asset);
  ASSERT_OK(read_result.status());
  ASSERT_NO_FATAL_FAILURE(VerifyBufferContents(read_result->value()->asset, data));
}

class PaverServiceSkipBlockTest : public PaverServiceTest {
 public:
  void StartFixture(std::string boot_slot = "-a", bool astro_sysconfig_abr_wear_leveling = false) {
    IsolatedDevmgr::Args args = DevmgrArgs();

    paver_config_ = paver::PaverConfig{
        .arch = paver::Arch::kX64,
        .astro_sysconfig_abr_wear_leveling = astro_sysconfig_abr_wear_leveling,
        .zvb_current_slot = std::move(boot_slot),
    };
    ASSERT_NO_FATAL_FAILURE(PaverServiceTest::Init(std::move(args)));
    ASSERT_NO_FATAL_FAILURE(
        SkipBlockDevice::Create(devmgr_.devfs_root().duplicate(), NandInfo(), &device_));
    ASSERT_NO_FATAL_FAILURE(WaitForDevices());
  }

  paver::PaverConfig Config() override { return paver_config_; }

  void SetUp() override {
    // Call PaverTest::SetUp, *not* PaverServiceTest::SetUp; we manually start the fixture
    PaverTest::SetUp();
  }

  IsolatedDevmgr::Args DevmgrArgs() override {
    IsolatedDevmgr::Args args = PaverServiceTest::DevmgrArgs();
    args.board_name = "astro";
    return args;
  }

  void WaitForDevices() {
    auto svc_dir = devmgr_.RealmExposedDir();
    component::SyncDirectoryWatcher skipblock_watcher(svc_dir.borrow(),
                                                      "fuchsia.hardware.skipblock.Service");
    zx::result skipblock = skipblock_watcher.GetNextEntry(false, zx::deadline_after(zx::sec(30)));
    ASSERT_OK(skipblock.status_value());

    component::SyncDirectoryWatcher volume_watcher(svc_dir.borrow(),
                                                   "fuchsia.hardware.block.volume.Service");
    zx::result volume = volume_watcher.GetNextEntry(false, zx::deadline_after(zx::sec(30)));
    ASSERT_OK(volume.status_value());
  }

  virtual fuchsia_hardware_nand::wire::RamNandInfo NandInfo() { return BaseNandInfo(); }

  void FindBootManager() {
    auto [local, remote] = fidl::Endpoints<fuchsia_paver::BootManager>::Create();

    auto result = client_->FindBootManager(std::move(remote));
    ASSERT_OK(result.status());
    boot_manager_ = fidl::WireSyncClient(std::move(local));
  }

  void FindDataSink() {
    auto [local, remote] = fidl::Endpoints<fuchsia_paver::DataSink>::Create();

    auto result = client_->FindDataSink(std::move(remote));
    ASSERT_OK(result.status());
    data_sink_ = fidl::WireSyncClient(std::move(local));
  }

  void FindSysconfig() {
    auto [local, remote] = fidl::Endpoints<fuchsia_paver::Sysconfig>::Create();

    auto result = client_->FindSysconfig(std::move(remote));
    ASSERT_OK(result.status());
    sysconfig_ = fidl::WireSyncClient(std::move(local));
  }

  void SetAbr(const AbrData& data) {
    auto* buf = reinterpret_cast<uint8_t*>(device_->mapper().start()) +
                (static_cast<size_t>(14) * kSkipBlockSize) + (static_cast<size_t>(60) * kKilobyte);
    *reinterpret_cast<AbrData*>(buf) = data;
  }

  AbrData GetAbr() {
    auto* buf = reinterpret_cast<uint8_t*>(device_->mapper().start()) +
                (static_cast<size_t>(14) * kSkipBlockSize) + (static_cast<size_t>(60) * kKilobyte);
    return *reinterpret_cast<AbrData*>(buf);
  }

  const uint8_t* SysconfigStart() {
    return reinterpret_cast<uint8_t*>(device_->mapper().start()) +
           (static_cast<size_t>(14) * kSkipBlockSize);
  }

  sysconfig_header GetSysconfigHeader() {
    const uint8_t* sysconfig_start = SysconfigStart();
    sysconfig_header ret;
    memcpy(&ret, sysconfig_start, sizeof(ret));
    return ret;
  }

  // Equivalence of GetAbr() in the context of abr wear-leveling.
  // Since there can be multiple pages in abr sub-partition that may have valid abr data,
  // argument |copy_index| is used to read a specific one.
  AbrData GetAbrInWearLeveling(const sysconfig_header& header, size_t copy_index) {
    auto* buf = SysconfigStart() + header.abr_metadata.offset + copy_index * 4 * kKilobyte;
    AbrData ret;
    memcpy(&ret, buf, sizeof(ret));
    return ret;
  }

  using PaverServiceTest::ValidateWritten;

  // Checks that the device mapper contains |expected| at each byte in the given
  // range. Uses ASSERT_EQ() per-byte to give a helpful message on failure.
  void AssertContents(size_t offset, size_t length, uint8_t expected) {
    const uint8_t* contents = static_cast<uint8_t*>(device_->mapper().start()) + offset;
    for (size_t i = 0; i < length; i++) {
      ASSERT_EQ(expected, contents[i], "i = %zu", i);
    }
  }

  void ValidateWritten(uint32_t block, size_t num_blocks) {
    AssertContents(static_cast<size_t>(block) * kSkipBlockSize, num_blocks * kSkipBlockSize, 0x4A);
  }

  void ValidateUnwritten(uint32_t block, size_t num_blocks) {
    AssertContents(static_cast<size_t>(block) * kSkipBlockSize, num_blocks * kSkipBlockSize, 0xFF);
  }

  void ValidateWrittenPages(uint32_t page, size_t num_pages) {
    AssertContents(static_cast<size_t>(page) * kPageSize, num_pages * kPageSize, 0x4A);
  }

  void ValidateUnwrittenPages(uint32_t page, size_t num_pages) {
    AssertContents(static_cast<size_t>(page) * kPageSize, num_pages * kPageSize, 0xFF);
  }

  void ValidateWrittenBytes(size_t offset, size_t num_bytes) {
    AssertContents(offset, num_bytes, 0x4A);
  }

  void ValidateUnwrittenBytes(size_t offset, size_t num_bytes) {
    AssertContents(offset, num_bytes, 0xFF);
  }

  void WriteData(uint32_t page, size_t num_pages, uint8_t data) {
    WriteDataBytes(page * kPageSize, num_pages * kPageSize, data);
  }

  void WriteDataBytes(uint32_t start, size_t num_bytes, uint8_t data) {
    memset(static_cast<uint8_t*>(device_->mapper().start()) + start, data, num_bytes);
  }

  void WriteDataBytes(uint32_t start, void* data, size_t num_bytes) {
    memcpy(static_cast<uint8_t*>(device_->mapper().start()) + start, data, num_bytes);
  }

  void TestSysconfigWriteBufferedClient(uint32_t offset_in_pages, uint32_t sysconfig_pages);

  void TestSysconfigWipeBufferedClient(uint32_t offset_in_pages, uint32_t sysconfig_pages);

  void TestQueryConfigurationLastSetActive(fuchsia_paver::wire::Configuration this_slot,
                                           fuchsia_paver::wire::Configuration other_slot);

  void TestQueryConfigurationStatus(AbrData abr_data,
                                    fuchsia_paver::wire::Configuration configuration,
                                    fuchsia_paver::wire::ConfigurationStatus expected_status);

  void TestQueryConfigurationStatusAndBootAttempts(
      AbrData abr_data, fuchsia_paver::wire::Configuration configuration,
      fuchsia_paver::wire::ConfigurationStatus expected_status,
      std::optional<uint8_t> expected_boot_attempts,
      std::optional<fuchsia_paver::wire::UnbootableReason> expected_unbootable_reason,
      std::string boot_slot = "_a");

  paver::PaverConfig paver_config_;

  fidl::WireSyncClient<fuchsia_paver::BootManager> boot_manager_;
  fidl::WireSyncClient<fuchsia_paver::DataSink> data_sink_;
  fidl::WireSyncClient<fuchsia_paver::Sysconfig> sysconfig_;

  std::unique_ptr<SkipBlockDevice> device_;
};

constexpr AbrData kAbrDataAUnbootableBSuccessful = {
    .magic = {'\0', 'A', 'B', '0'},
    .version_major = 2,
    .version_minor = 3,
    .reserved1 = {},
    .slot_data =
        {
            {
                .priority = 0,
                .tries_remaining = 0,
                .successful_boot = 0,
                .unbootable_reason = kAbrUnbootableReasonNone,
            },
            {
                .priority = 1,
                .tries_remaining = 0,
                .successful_boot = 1,
                .unbootable_reason = kAbrUnbootableReasonNone,
            },
        },
    .one_shot_flags = kAbrDataOneShotFlagNone,
    .reserved2 = {},
    .crc32 = {},
};

// Returns AbrData that has both slots unbootable with |reason|, and A higher priority.
AbrData AbrDataBothUnbootable(uint8_t reason) {
  return {
      .magic = {'\0', 'A', 'B', '0'},
      .version_major = 2,
      .version_minor = 3,
      .reserved1 = {},
      .slot_data =
          {
              {
                  .priority = 15,
                  .tries_remaining = 0,
                  .successful_boot = 0,
                  .unbootable_reason = reason,
              },
              {
                  .priority = 14,
                  .tries_remaining = 0,
                  .successful_boot = 0,
                  .unbootable_reason = reason,
              },
          },
      .one_shot_flags = kAbrDataOneShotFlagNone,
      .reserved2 = {},
      .crc32 = {},
  };
}

void ComputeCrc(AbrData* data) {
  data->crc32 = htobe32(crc32(0, reinterpret_cast<const uint8_t*>(data), offsetof(AbrData, crc32)));
}

TEST_F(PaverServiceSkipBlockTest, InitializeAbr) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = {};
  memset(&abr_data, 0x3d, sizeof(abr_data));
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryActiveConfiguration();
  ASSERT_OK(result.status());
}

TEST_F(PaverServiceSkipBlockTest, InitializeAbrAlreadyValid) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryActiveConfiguration();
  ASSERT_OK(result.status());
}

TEST_F(PaverServiceSkipBlockTest, QueryActiveConfigurationInvalidAbr) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = {};
  memset(&abr_data, 0x3d, sizeof(abr_data));
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryActiveConfiguration();
  ASSERT_OK(result.status());
}

TEST_F(PaverServiceSkipBlockTest, QueryActiveConfigurationBothPriority0) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].priority = 0;
  abr_data.slot_data[1].priority = 0;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryActiveConfiguration();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_error());
  ASSERT_STATUS(result->error_value(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(PaverServiceSkipBlockTest, QueryActiveConfigurationSlotB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryActiveConfiguration();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kB);
}

TEST_F(PaverServiceSkipBlockTest, QueryActiveConfigurationSlotA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].priority = 2;
  abr_data.slot_data[0].successful_boot = 1;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryActiveConfiguration();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kA);
}

void PaverServiceSkipBlockTest::TestQueryConfigurationLastSetActive(
    fuchsia_paver::wire::Configuration this_slot, fuchsia_paver::wire::Configuration other_slot) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  // Set both slots to the active state.
  {
    auto result = boot_manager_->SetConfigurationActive(other_slot);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->SetConfigurationActive(this_slot);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  // Marking the slot successful shall not change the result.
  {
    auto result = boot_manager_->SetConfigurationHealthy(this_slot);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);

    auto get_result = boot_manager_->QueryConfigurationLastSetActive();
    ASSERT_OK(get_result.status());
    ASSERT_TRUE(get_result->is_ok());
    ASSERT_EQ(get_result->value()->configuration, this_slot);
  }

  // Marking the slot unbootable shall not change the result.
  {
    auto result = boot_manager_->SetConfigurationUnbootable(this_slot);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);

    auto get_result = boot_manager_->QueryConfigurationLastSetActive();
    ASSERT_OK(get_result.status());
    ASSERT_TRUE(get_result->is_ok());
    ASSERT_EQ(get_result->value()->configuration, this_slot);
  }

  // Marking the other slot successful shall not change the result.
  {
    auto result = boot_manager_->SetConfigurationHealthy(other_slot);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);

    auto get_result = boot_manager_->QueryConfigurationLastSetActive();
    ASSERT_OK(get_result.status());
    ASSERT_TRUE(get_result->is_ok());
    ASSERT_EQ(get_result->value()->configuration, this_slot);
  }

  // Marking the other slot unbootable shall not change the result.
  {
    auto result = boot_manager_->SetConfigurationUnbootable(other_slot);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);

    auto get_result = boot_manager_->QueryConfigurationLastSetActive();
    ASSERT_OK(get_result.status());
    ASSERT_TRUE(get_result->is_ok());
    ASSERT_EQ(get_result->value()->configuration, this_slot);
  }

  // Marking the other slot active does change the result.
  {
    auto result = boot_manager_->SetConfigurationActive(other_slot);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);

    auto get_result = boot_manager_->QueryConfigurationLastSetActive();
    ASSERT_OK(get_result.status());
    ASSERT_TRUE(get_result->is_ok());
    ASSERT_EQ(get_result->value()->configuration, other_slot);
  }
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationLastSetActiveSlotA) {
  TestQueryConfigurationLastSetActive(fuchsia_paver::wire::Configuration::kA,
                                      fuchsia_paver::wire::Configuration::kB);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationLastSetActiveSlotB) {
  TestQueryConfigurationLastSetActive(fuchsia_paver::wire::Configuration::kB,
                                      fuchsia_paver::wire::Configuration::kA);
}

TEST_F(PaverServiceSkipBlockTest, QueryCurrentConfigurationSlotA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryCurrentConfiguration();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kA);
}

TEST_F(PaverServiceSkipBlockTest, QueryCurrentConfigurationSlotB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture("-b"));

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryCurrentConfiguration();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kB);
}

TEST_F(PaverServiceSkipBlockTest, QueryCurrentConfigurationSlotR) {
  ASSERT_NO_FATAL_FAILURE(StartFixture("-r"));

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryCurrentConfiguration();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kRecovery);
}

TEST_F(PaverServiceSkipBlockTest, QueryCurrentConfigurationSlotInvalid) {
  ASSERT_NO_FATAL_FAILURE(StartFixture("asdf"));

  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryCurrentConfiguration();
  ASSERT_STATUS(result, ZX_ERR_PEER_CLOSED);
}

// Registers the given `abr_data`, calls `BootManager::QueryConfigurationStatus`, and checks that
// the resulting status matches `expected_status`.
void PaverServiceSkipBlockTest::TestQueryConfigurationStatus(
    AbrData abr_data, fuchsia_paver::wire::Configuration configuration,
    fuchsia_paver::wire::ConfigurationStatus expected_status) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryConfigurationStatus(configuration);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ((*result)->status, expected_status);
}

// Common test logic for `QueryConfigurationStatusAndBootAttempts`.
//
// Args:
// * abr_data: A/B/R metadata to set; CRC will be updated by this function.
// * configuration: which `Configuration` slot to query.
// * expected_status: the expected returned configuration status.
// * expected_boot_attempts: the expected reported boot attempts.
// * expected_unbootable_reason: the expected reported unbootable reason.
void PaverServiceSkipBlockTest::TestQueryConfigurationStatusAndBootAttempts(
    AbrData abr_data, fuchsia_paver::wire::Configuration configuration,
    fuchsia_paver::wire::ConfigurationStatus expected_status,
    std::optional<uint8_t> expected_boot_attempts,
    std::optional<fuchsia_paver::wire::UnbootableReason> expected_unbootable_reason,
    std::string boot_slot) {
  ASSERT_NO_FATAL_FAILURE(StartFixture(std::move(boot_slot)));

  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result = boot_manager_->QueryConfigurationStatusAndBootAttempts(configuration);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());

  ASSERT_TRUE((*result)->has_status());
  ASSERT_EQ((*result)->status(), expected_status);

  if (expected_boot_attempts.has_value()) {
    ASSERT_TRUE((*result)->has_boot_attempts());
    ASSERT_EQ((*result)->boot_attempts(), expected_boot_attempts.value());
  } else {
    ASSERT_FALSE((*result)->has_boot_attempts());
  }

  if (expected_unbootable_reason.has_value()) {
    ASSERT_TRUE((*result)->has_unbootable_reason());
    ASSERT_EQ((*result)->unbootable_reason(), expected_unbootable_reason.value());
  } else {
    ASSERT_FALSE((*result)->has_unbootable_reason());
  }
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusHealthy) {
  TestQueryConfigurationStatus(kAbrDataAUnbootableBSuccessful,
                               fuchsia_paver::wire::Configuration::kB,
                               fuchsia_paver::wire::ConfigurationStatus::kHealthy);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsHealthy) {
  TestQueryConfigurationStatusAndBootAttempts(
      kAbrDataAUnbootableBSuccessful, fuchsia_paver::wire::Configuration::kB,
      fuchsia_paver::wire::ConfigurationStatus::kHealthy, std::nullopt, std::nullopt);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusPending) {
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[1].successful_boot = 0;
  abr_data.slot_data[1].tries_remaining = 1;

  TestQueryConfigurationStatus(abr_data, fuchsia_paver::wire::Configuration::kB,
                               fuchsia_paver::wire::ConfigurationStatus::kPending);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsPendingNoAttempts) {
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[1].successful_boot = 0;
  abr_data.slot_data[1].tries_remaining = kAbrMaxTriesRemaining;

  TestQueryConfigurationStatusAndBootAttempts(abr_data, fuchsia_paver::wire::Configuration::kB,
                                              fuchsia_paver::wire::ConfigurationStatus::kPending, 0,
                                              std::nullopt);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsPendingSomeAttempts) {
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[1].successful_boot = 0;
  abr_data.slot_data[1].tries_remaining = 1;

  TestQueryConfigurationStatusAndBootAttempts(abr_data, fuchsia_paver::wire::Configuration::kB,
                                              fuchsia_paver::wire::ConfigurationStatus::kPending,
                                              kAbrMaxTriesRemaining - 1, std::nullopt);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsFinalBootA) {
  // The current boot slot should interpret "no more tries" as "last attempt".
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonNoMoreTries),
      fuchsia_paver::wire::Configuration::kA, fuchsia_paver::wire::ConfigurationStatus::kPending,
      kAbrMaxTriesRemaining, std::nullopt);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsFinalBootB) {
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonNoMoreTries),
      fuchsia_paver::wire::Configuration::kB, fuchsia_paver::wire::ConfigurationStatus::kPending,
      kAbrMaxTriesRemaining, std::nullopt, "_b");
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsFinalBootLegacyReason) {
  // The current boot slot should also interpret "unknown reason" as "last attempt" to support
  // bootloaders that haven't been updated yet to include the reboot reason.
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonNone), fuchsia_paver::wire::Configuration::kA,
      fuchsia_paver::wire::ConfigurationStatus::kPending, kAbrMaxTriesRemaining, std::nullopt);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsFinalBootAQueryB) {
  // When it's not the current boot slot, "no more tries" really does mean unbootable.
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonNoMoreTries),
      fuchsia_paver::wire::Configuration::kB, fuchsia_paver::wire::ConfigurationStatus::kUnbootable,
      std::nullopt, fuchsia_paver::wire::UnbootableReason::kNoMoreTries);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsFinalBootBQueryA) {
  // When it's not the current boot slot, "no more tries" really does mean unbootable.
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonNoMoreTries),
      fuchsia_paver::wire::Configuration::kA, fuchsia_paver::wire::ConfigurationStatus::kUnbootable,
      std::nullopt, fuchsia_paver::wire::UnbootableReason::kNoMoreTries, "_b");
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusUnbootable) {
  TestQueryConfigurationStatus(AbrDataBothUnbootable(kAbrUnbootableReasonOsRequested),
                               fuchsia_paver::wire::Configuration::kA,
                               fuchsia_paver::wire::ConfigurationStatus::kUnbootable);
}

// This function is just a compile-time check to trigger a breakage if any new enum variants are
// added, so that we can be sure to add them to the paver as well.
//
// If this function starts failing to compile:
// 1. Update this switch statements to include the new enum variants.
// 2. Add a unittest below to verify the libabr -> paver variant translation.
[[maybe_unused]] void UnbootableReasonEnums(AbrUnbootableReason abr_reason) {
  switch (abr_reason) {
    case kAbrUnbootableReasonNone:
      break;
    case kAbrUnbootableReasonNoMoreTries:
      break;
    case kAbrUnbootableReasonOsRequested:
      break;
    case kAbrUnbootableReasonVerificationFailure:
      break;
      // Do not add default - the whole point is to compile-time catch any missing variants.
  }
}

// kAbrUnbootableReasonNone -> UnbootableReason::kNone
TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsUnbootableReasonNone) {
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonNone), fuchsia_paver::wire::Configuration::kB,
      fuchsia_paver::wire::ConfigurationStatus::kUnbootable, std::nullopt,
      fuchsia_paver::wire::UnbootableReason::kNone);
}

// kAbrUnbootableReasonNoMoreTries -> UnbootableReason::kNoMoreTries
TEST_F(PaverServiceSkipBlockTest,
       QueryConfigurationStatusAndBootAttemptsUnbootableReasonNoMoreTries) {
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonNoMoreTries),
      fuchsia_paver::wire::Configuration::kB, fuchsia_paver::wire::ConfigurationStatus::kUnbootable,
      std::nullopt, fuchsia_paver::wire::UnbootableReason::kNoMoreTries);
}

// kAbrUnbootableReasonOsRequested -> UnbootableReason::kOsRequested
TEST_F(PaverServiceSkipBlockTest,
       QueryConfigurationStatusAndBootAttemptsUnbootableReasonOsRequested) {
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonOsRequested),
      fuchsia_paver::wire::Configuration::kB, fuchsia_paver::wire::ConfigurationStatus::kUnbootable,
      std::nullopt, fuchsia_paver::wire::UnbootableReason::kOsRequested);
}

// kAbrUnbootableReasonVerificationFailure -> UnbootableReason::kVerificationFailure
TEST_F(PaverServiceSkipBlockTest,
       QueryConfigurationStatusAndBootAttemptsUnbootableReasonVerificationFailure) {
  TestQueryConfigurationStatusAndBootAttempts(
      AbrDataBothUnbootable(kAbrUnbootableReasonVerificationFailure),
      fuchsia_paver::wire::Configuration::kB, fuchsia_paver::wire::ConfigurationStatus::kUnbootable,
      std::nullopt, fuchsia_paver::wire::UnbootableReason::kVerificationFailure);
}

TEST_F(PaverServiceSkipBlockTest, QueryConfigurationStatusAndBootAttemptsInvalidBootAttempts) {
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[1].successful_boot = 0;
  abr_data.slot_data[1].tries_remaining = kAbrMaxTriesRemaining + 1;  // Invalid tries remaining.

  // The A/B/R data gets fixed up on load, so even though the on-disk data was invalid it should now
  // be snapped into the valid range.
  TestQueryConfigurationStatusAndBootAttempts(abr_data, fuchsia_paver::wire::Configuration::kB,
                                              fuchsia_paver::wire::ConfigurationStatus::kPending, 0,
                                              std::nullopt);
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationActive) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  abr_data.slot_data[0].priority = kAbrMaxPriority;
  abr_data.slot_data[0].tries_remaining = kAbrMaxTriesRemaining;
  abr_data.slot_data[0].successful_boot = 0;
  ComputeCrc(&abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationActive(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationActiveRollover) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[1].priority = kAbrMaxPriority;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  abr_data.slot_data[1].priority = kAbrMaxPriority - 1;
  abr_data.slot_data[0].priority = kAbrMaxPriority;
  abr_data.slot_data[0].tries_remaining = kAbrMaxTriesRemaining;
  abr_data.slot_data[0].successful_boot = 0;
  ComputeCrc(&abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationActive(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }
  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationUnbootableSlotA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].priority = 2;
  abr_data.slot_data[0].tries_remaining = 3;
  abr_data.slot_data[0].successful_boot = 0;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  abr_data.slot_data[0].tries_remaining = 0;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[0].unbootable_reason = kAbrUnbootableReasonOsRequested;
  ComputeCrc(&abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationUnbootable(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationUnbootableSlotB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[1].tries_remaining = 3;
  abr_data.slot_data[1].successful_boot = 0;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].successful_boot = 0;
  abr_data.slot_data[1].unbootable_reason = kAbrUnbootableReasonOsRequested;
  ComputeCrc(&abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationUnbootable(fuchsia_paver::wire::Configuration::kB);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationHealthySlotA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].priority = kAbrMaxPriority;
  abr_data.slot_data[0].tries_remaining = 0;
  abr_data.slot_data[0].successful_boot = 1;
  abr_data.slot_data[1].priority = 0;
  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].successful_boot = 0;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationHealthySlotB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ComputeCrc(&abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kB);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationHealthySlotR) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  auto result =
      boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kRecovery);
  ASSERT_OK(result.status());
  ASSERT_EQ(result.value().status, ZX_ERR_INVALID_ARGS);
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationHealthyBothUnknown) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].priority = kAbrMaxPriority;
  abr_data.slot_data[0].tries_remaining = 3;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[1].priority = kAbrMaxPriority - 1;
  abr_data.slot_data[1].tries_remaining = 3;
  abr_data.slot_data[1].successful_boot = 0;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  abr_data.slot_data[0].tries_remaining = 0;
  abr_data.slot_data[0].successful_boot = 1;
  abr_data.slot_data[1].tries_remaining = kAbrMaxTriesRemaining;
  ComputeCrc(&abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetConfigurationHealthyOtherHealthy) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].priority = kAbrMaxPriority - 1;
  abr_data.slot_data[0].tries_remaining = 0;
  abr_data.slot_data[0].successful_boot = 1;
  abr_data.slot_data[1].priority = kAbrMaxPriority;
  abr_data.slot_data[1].tries_remaining = 3;
  abr_data.slot_data[1].successful_boot = 0;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  abr_data.slot_data[0].tries_remaining = kAbrMaxTriesRemaining;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].successful_boot = 1;
  ComputeCrc(&abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kB);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetUnbootableConfigurationHealthyFails) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = AbrDataBothUnbootable(kAbrUnbootableReasonOsRequested);
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_EQ(result.value().status, ZX_ERR_INVALID_ARGS);
  }

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kB);
    ASSERT_OK(result.status());
    ASSERT_EQ(result.value().status, ZX_ERR_INVALID_ARGS);
  }

  // A/B/R metadata should not have changed.
  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }
  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, SetUnbootableConfigurationHealthyLastBootAttemptSucceeds) {
  // If we're on the last boot attempt, we should still be able to set the configuration healthy.
  // Here we set B to be the current slot on its last boot attempt, so A should still refuse but
  // B should now be allowed to be marked healthy.
  ASSERT_NO_FATAL_FAILURE(StartFixture("_b"));
  AbrData abr_data = AbrDataBothUnbootable(kAbrUnbootableReasonNoMoreTries);
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_EQ(result.value().status, ZX_ERR_INVALID_ARGS);
  }
  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kB);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  // Make sure the A/B/R metadata was updated as we expect.
  abr_data.slot_data[1].successful_boot = 1;
  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].unbootable_reason = kAbrUnbootableReasonNone;
  ComputeCrc(&abr_data);
  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

TEST_F(PaverServiceSkipBlockTest, BootManagerBuffered) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  // Successful slot b, active slot a. Like what happen after a reboot following an OTA.
  abr_data.slot_data[0].tries_remaining = 3;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[0].priority = 1;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->QueryActiveConfiguration();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
    ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kA);
  }

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = boot_manager_->SetConfigurationUnbootable(fuchsia_paver::wire::Configuration::kB);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  // haven't flushed yet, storage shall stay the same.
  auto abr = GetAbr();
  ASSERT_BYTES_EQ(&abr, &abr_data, sizeof(abr));

  {
    auto result = boot_manager_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  abr_data.slot_data[0].tries_remaining = 0;
  abr_data.slot_data[0].successful_boot = 1;
  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].successful_boot = 0;
  abr_data.slot_data[1].unbootable_reason = kAbrUnbootableReasonOsRequested;
  ComputeCrc(&abr_data);

  abr = GetAbr();
  ASSERT_BYTES_EQ(&abr, &abr_data, sizeof(abr));
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetKernelConfigA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(2) * kPagesPerBlock, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kA,
                                       fuchsia_paver::wire::Asset::kKernel, std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
  ValidateWritten(8, 2);
  ValidateUnwritten(10, 4);
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetKernelConfigB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(2) * kPagesPerBlock, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kB,
                                       fuchsia_paver::wire::Asset::kKernel, std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
  ValidateUnwritten(8, 2);
  ValidateWritten(10, 2);
  ValidateUnwritten(12, 2);
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetKernelConfigRecovery) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(2) * kPagesPerBlock, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kRecovery,
                                       fuchsia_paver::wire::Asset::kKernel, std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
  ValidateUnwritten(8, 4);
  ValidateWritten(12, 2);
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetVbMetaConfigA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(32, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result =
      data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kA,
                             fuchsia_paver::wire::Asset::kVerifiedBootMetadata, std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);

  auto sync_result = data_sink_->Flush();
  ASSERT_OK(sync_result.status());
  ASSERT_OK(sync_result.value().status);

  ValidateWrittenPages(14 * kPagesPerBlock + 32, 32);
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetVbMetaConfigB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(32, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result =
      data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kB,
                             fuchsia_paver::wire::Asset::kVerifiedBootMetadata, std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);

  auto sync_result = data_sink_->Flush();
  ASSERT_OK(sync_result.status());
  ASSERT_OK(sync_result.value().status);

  ValidateWrittenPages(14 * kPagesPerBlock + 64, 32);
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetVbMetaConfigRecovery) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(32, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result =
      data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kRecovery,
                             fuchsia_paver::wire::Asset::kVerifiedBootMetadata, std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);

  auto sync_result = data_sink_->Flush();
  ASSERT_OK(sync_result.status());
  ASSERT_OK(sync_result.value().status);

  ValidateWrittenPages(14 * kPagesPerBlock + 96, 32);
}

TEST_F(PaverServiceSkipBlockTest, AbrWearLevelingLayoutNotUpdated) {
  // Enable write-caching + abr metadata wear-leveling
  ASSERT_NO_FATAL_FAILURE(StartFixture("-a", true));

  // Active slot b
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].tries_remaining = 3;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[0].priority = 0;
  abr_data.slot_data[1].tries_remaining = 3;
  abr_data.slot_data[1].successful_boot = 0;
  abr_data.slot_data[1].priority = 1;
  ComputeCrc(&abr_data);
  SetAbr(abr_data);

  // Layout will not be updated as A/B state does not meet requirement.
  // (one successful slot + one unbootable slot)
  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->QueryActiveConfiguration();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
    ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kB);
  }

  {
    auto result = boot_manager_->SetConfigurationHealthy(fuchsia_paver::wire::Configuration::kB);
    ASSERT_OK(result.status());
  }

  {
    // The query result will come from the cache as flushed is not called.
    // Validate that it is correct.
    auto result = boot_manager_->QueryActiveConfiguration();
    ASSERT_OK(result.status());
    ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kB);
  }

  {
    // Mark the old slot A as unbootable.
    auto set_unbootable_result =
        boot_manager_->SetConfigurationUnbootable(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(set_unbootable_result.status());
  }

  // Haven't flushed yet. abr data in storage should stayed the same.
  auto actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));

  {
    auto result_sync = boot_manager_->Flush();
    ASSERT_OK(result_sync.status());
    ASSERT_OK(result_sync.value().status);
  }

  // Expected result: unbootable slot a, successful active slot b
  abr_data.slot_data[0].tries_remaining = 0;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[0].priority = 0;
  abr_data.slot_data[0].unbootable_reason = kAbrUnbootableReasonOsRequested;
  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].successful_boot = 1;
  abr_data.slot_data[1].priority = 1;
  ComputeCrc(&abr_data);

  // Validate that new abr data is flushed to memory.
  // Since layout is not updated, Abr metadata is expected to be at the traditional position
  // (16th page).
  actual = GetAbr();
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));
}

AbrData GetAbrWearlevelingSupportingLayout() {
  // Unbootable slot a, successful active slot b
  AbrData abr_data = kAbrDataAUnbootableBSuccessful;
  abr_data.slot_data[0].tries_remaining = 0;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[0].priority = 0;
  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].successful_boot = 1;
  abr_data.slot_data[1].priority = 1;
  ComputeCrc(&abr_data);
  return abr_data;
}

TEST_F(PaverServiceSkipBlockTest, AbrWearLevelingLayoutUpdated) {
  // Enable write-caching + abr metadata wear-leveling
  ASSERT_NO_FATAL_FAILURE(StartFixture("-a", true));

  // Unbootable slot a, successful active slot b
  auto abr_data = GetAbrWearlevelingSupportingLayout();
  SetAbr(abr_data);

  // Layout will be updated. Since A/B state is one successful + one unbootable
  ASSERT_NO_FATAL_FAILURE(FindBootManager());

  {
    auto result = boot_manager_->QueryActiveConfiguration();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
    ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kB);
  }

  {
    auto result = boot_manager_->SetConfigurationActive(fuchsia_paver::wire::Configuration::kA);
    ASSERT_OK(result.status());
  }

  {
    // The query result will come from the cache as we haven't flushed.
    // Validate that it is correct.
    auto result = boot_manager_->QueryActiveConfiguration();
    ASSERT_OK(result.status());
    ASSERT_EQ(result->value()->configuration, fuchsia_paver::wire::Configuration::kA);
  }

  // Haven't flushed yet. abr data in storage should stayed the same.
  // Since layout changed, use the updated layout to find abr.
  auto header = sysconfig::SyncClientAbrWearLeveling::GetAbrWearLevelingSupportedLayout();
  auto actual = GetAbrInWearLeveling(header, 0);
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));

  {
    auto result_sync = boot_manager_->Flush();
    ASSERT_OK(result_sync.status());
    ASSERT_OK(result_sync.value().status);
  }

  // Expected result: successful slot a, active slot b with max tries and priority.
  abr_data.slot_data[0].tries_remaining = kAbrMaxTriesRemaining;
  abr_data.slot_data[0].successful_boot = 0;
  abr_data.slot_data[0].priority = kAbrMaxPriority;
  abr_data.slot_data[1].tries_remaining = 0;
  abr_data.slot_data[1].successful_boot = 1;
  abr_data.slot_data[1].priority = 1;
  ComputeCrc(&abr_data);

  // Validate that new abr data is flushed to memory.
  // The first page (page 0) in the abr sub-partition is occupied by the initial abr data.
  // Thus, the new abr metadata is expected to be appended at the 2nd page (page 1).
  actual = GetAbrInWearLeveling(header, 1);
  ASSERT_BYTES_EQ(&abr_data, &actual, sizeof(abr_data));

  // Validate that header is updated.
  const sysconfig_header actual_header = GetSysconfigHeader();
  ASSERT_BYTES_EQ(&header, &actual_header, sizeof(sysconfig_header));
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetBuffered) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  fuchsia_paver::wire::Configuration configs[] = {fuchsia_paver::wire::Configuration::kA,
                                                  fuchsia_paver::wire::Configuration::kB,
                                                  fuchsia_paver::wire::Configuration::kRecovery};

  for (auto config : configs) {
    fuchsia_mem::wire::Buffer payload;
    CreateBuffer(32, &payload);
    auto result = data_sink_->WriteAsset(config, fuchsia_paver::wire::Asset::kVerifiedBootMetadata,
                                         std::move(payload));
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }
  ValidateUnwrittenPages(14 * kPagesPerBlock + 32, 96);

  auto sync_result = data_sink_->Flush();
  ASSERT_OK(sync_result.status());
  ASSERT_OK(sync_result.value().status);
  ValidateWrittenPages(14 * kPagesPerBlock + 32, 96);
}

TEST_F(PaverServiceSkipBlockTest, WriteAssetTwice) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(2) * kPagesPerBlock, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  {
    auto result = data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kA,
                                         fuchsia_paver::wire::Asset::kKernel, std::move(payload));
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    CreateBuffer(static_cast<size_t>(2) * kPagesPerBlock, &payload);
    ValidateWritten(8, 2);
    ValidateUnwritten(10, 4);
  }
  {
    auto result = data_sink_->WriteAsset(fuchsia_paver::wire::Configuration::kA,
                                         fuchsia_paver::wire::Asset::kKernel, std::move(payload));
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ValidateWritten(8, 2);
    ValidateUnwritten(10, 4);
  }
}

TEST_F(PaverServiceSkipBlockTest, ReadFirmwareConfigA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(kBootloaderFirstBlock * kPagesPerBlock,
            static_cast<size_t>(kBootloaderBlocks) * kPagesPerBlock, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadFirmware(fuchsia_paver::wire::Configuration::kA,
                                         fidl::StringView::FromExternal(kFirmwareTypeBootloader));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().is_ok());
  ValidateWritten(result.value().value()->firmware,
                  static_cast<size_t>(kBootloaderBlocks) * kPagesPerBlock);
}

TEST_F(PaverServiceSkipBlockTest, ReadFirmwareUnsupportedConfigBFallBackToA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(kBootloaderFirstBlock * kPagesPerBlock,
            static_cast<size_t>(kBootloaderBlocks) * kPagesPerBlock, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadFirmware(fuchsia_paver::wire::Configuration::kB,
                                         fidl::StringView::FromExternal(kFirmwareTypeBootloader));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().is_ok());
  ValidateWritten(result.value().value()->firmware,
                  static_cast<size_t>(kBootloaderBlocks) * kPagesPerBlock);
}

TEST_F(PaverServiceSkipBlockTest, ReadFirmwareUnsupportedConfigR) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadFirmware(fuchsia_paver::wire::Configuration::kRecovery,
                                         fidl::StringView::FromExternal(kFirmwareTypeBootloader));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().is_error());
}

TEST_F(PaverServiceSkipBlockTest, ReadFirmwareUnsupportedType) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadFirmware(fuchsia_paver::wire::Configuration::kA,
                                         fidl::StringView::FromExternal(kFirmwareTypeUnsupported));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().is_error());
}

TEST_F(PaverServiceSkipBlockTest, WriteFirmwareConfigASupported) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(4) * kPagesPerBlock, &payload);
  auto result = data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kA,
                                          fidl::StringView::FromExternal(kFirmwareTypeBootloader),
                                          std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().result.is_status());
  ASSERT_OK(result.value().result.status());
  ValidateWritten(kBootloaderFirstBlock, 4);
  WriteData(kBootloaderFirstBlock, static_cast<size_t>(4) * kPagesPerBlock, 0xff);
}

TEST_F(PaverServiceSkipBlockTest, WriteFirmwareUnsupportedConfigBFallBackToA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(4) * kPagesPerBlock, &payload);
  auto result = data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kB,
                                          fidl::StringView::FromExternal(kFirmwareTypeBootloader),
                                          std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().result.is_status());
  ASSERT_OK(result.value().result.status());
  ValidateWritten(kBootloaderFirstBlock, 4);
  WriteData(kBootloaderFirstBlock, static_cast<size_t>(4) * kPagesPerBlock, 0xff);
}

TEST_F(PaverServiceSkipBlockTest, WriteFirmwareUnsupportedConfigR) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(4) * kPagesPerBlock, &payload);
  auto result = data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kRecovery,
                                          fidl::StringView::FromExternal(kFirmwareTypeBootloader),
                                          std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().result.is_unsupported());
  ASSERT_TRUE(result.value().result.unsupported());
  ValidateUnwritten(kBootloaderFirstBlock, 4);
}

TEST_F(PaverServiceSkipBlockTest, WriteFirmwareBl2ConfigASupported) {
  // BL2 special handling: we should always leave the first 4096 bytes intact.
  constexpr size_t kBl2StartByte{static_cast<size_t>(kBl2FirstBlock) * kPageSize * kPagesPerBlock};
  constexpr size_t kBl2SkipLength{4096};

  ASSERT_NO_FATAL_FAILURE(StartFixture());
  ASSERT_NO_FATAL_FAILURE(FindDataSink());

  WriteDataBytes(kBl2StartByte, kBl2SkipLength, 0xC6);
  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(kBl2ImagePages, &payload);
  auto result = data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kA,
                                          fidl::StringView::FromExternal(kFirmwareTypeBl2),
                                          std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().result.is_status());
  ASSERT_OK(result.value().result.status());
}

TEST_F(PaverServiceSkipBlockTest, WriteFirmwareBl2UnsupportedConfigBFallBackToA) {
  // BL2 special handling: we should always leave the first 4096 bytes intact.
  constexpr size_t kBl2StartByte{static_cast<size_t>(kBl2FirstBlock) * kPageSize * kPagesPerBlock};
  constexpr size_t kBl2SkipLength{4096};

  ASSERT_NO_FATAL_FAILURE(StartFixture());
  WriteDataBytes(kBl2StartByte, kBl2SkipLength, 0xC6);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(kBl2ImagePages, &payload);
  auto result = data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kB,
                                          fidl::StringView::FromExternal(kFirmwareTypeBl2),
                                          std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().result.is_status());
  ASSERT_OK(result.value().result.status());
}

TEST_F(PaverServiceSkipBlockTest, WriteFirmwareBl2UnsupportedConfigR) {
  // BL2 special handling: we should always leave the first 4096 bytes intact.
  constexpr size_t kBl2StartByte{static_cast<size_t>(kBl2FirstBlock) * kPageSize * kPagesPerBlock};
  constexpr size_t kBl2SkipLength{4096};

  ASSERT_NO_FATAL_FAILURE(StartFixture());
  WriteDataBytes(kBl2StartByte, kBl2SkipLength, 0xC6);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(kBl2ImagePages, &payload);
  auto result = data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kRecovery,
                                          fidl::StringView::FromExternal(kFirmwareTypeBl2),
                                          std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().result.is_unsupported());
  ASSERT_TRUE(result.value().result.unsupported());
}

TEST_F(PaverServiceSkipBlockTest, WriteFirmwareUnsupportedType) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  constexpr fuchsia_paver::wire::Configuration kAllConfigs[] = {
      fuchsia_paver::wire::Configuration::kA,
      fuchsia_paver::wire::Configuration::kB,
      fuchsia_paver::wire::Configuration::kRecovery,
  };

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  for (auto config : kAllConfigs) {
    fuchsia_mem::wire::Buffer payload;
    CreateBuffer(static_cast<size_t>(4) * kPagesPerBlock, &payload);
    auto result = data_sink_->WriteFirmware(
        config, fidl::StringView::FromExternal(kFirmwareTypeUnsupported), std::move(payload));
    ASSERT_OK(result.status());
    ASSERT_TRUE(result.value().result.is_unsupported());
    ASSERT_TRUE(result.value().result.unsupported());
    ValidateUnwritten(kBootloaderFirstBlock, 4);
    ValidateUnwritten(kBl2FirstBlock, 1);
  }
}

struct PaverServiceSkipBlockWithNoBootloaderTest : public PaverServiceSkipBlockTest {
 public:
  fuchsia_hardware_nand::wire::RamNandInfo NandInfo() override {
    // Make a RAM NAND device without a visible "bootloader" partition so that
    // the partitioner initializes properly but then fails when trying to find it.
    fuchsia_hardware_nand::wire::RamNandInfo info = PaverServiceSkipBlockTest::NandInfo();
    info.partition_map.partitions[1].hidden = true;
    return info;
  }
};

TEST_F(PaverServiceSkipBlockWithNoBootloaderTest, WriteFirmwareError) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(4) * kPagesPerBlock, &payload);
  auto result = data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kA,
                                          fidl::StringView::FromExternal(kFirmwareTypeBootloader),
                                          std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().result.is_status());
  ASSERT_NOT_OK(result.value().result.status());
  ValidateUnwritten(kBootloaderFirstBlock, 4);
}

TEST_F(PaverServiceSkipBlockTest, ReadAssetKernelConfigA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(kZirconAFirstBlock * kPagesPerBlock, static_cast<size_t>(2) * kPagesPerBlock, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadAsset(fuchsia_paver::wire::Configuration::kA,
                                      fuchsia_paver::wire::Asset::kKernel);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ValidateWritten(result->value()->asset, static_cast<size_t>(2) * kPagesPerBlock);
}

TEST_F(PaverServiceSkipBlockTest, ReadAssetKernelConfigB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(10 * kPagesPerBlock, static_cast<size_t>(2) * kPagesPerBlock, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadAsset(fuchsia_paver::wire::Configuration::kB,
                                      fuchsia_paver::wire::Asset::kKernel);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ValidateWritten(result->value()->asset, static_cast<size_t>(2) * kPagesPerBlock);
}

TEST_F(PaverServiceSkipBlockTest, ReadAssetKernelConfigRecovery) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(12 * kPagesPerBlock, static_cast<size_t>(2) * kPagesPerBlock, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadAsset(fuchsia_paver::wire::Configuration::kRecovery,
                                      fuchsia_paver::wire::Asset::kKernel);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ValidateWritten(result->value()->asset, static_cast<size_t>(2) * kPagesPerBlock);
}

TEST_F(PaverServiceSkipBlockTest, ReadAssetVbMetaConfigA) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(14 * kPagesPerBlock + 32, 32, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadAsset(fuchsia_paver::wire::Configuration::kA,
                                      fuchsia_paver::wire::Asset::kVerifiedBootMetadata);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ValidateWritten(result->value()->asset, 32);
}

TEST_F(PaverServiceSkipBlockTest, ReadAssetVbMetaConfigB) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(14 * kPagesPerBlock + 64, 32, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadAsset(fuchsia_paver::wire::Configuration::kB,
                                      fuchsia_paver::wire::Asset::kVerifiedBootMetadata);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ValidateWritten(result->value()->asset, 32);
}

TEST_F(PaverServiceSkipBlockTest, ReadAssetVbMetaConfigRecovery) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  WriteData(14 * kPagesPerBlock + 96, 32, 0x4a);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadAsset(fuchsia_paver::wire::Configuration::kRecovery,
                                      fuchsia_paver::wire::Asset::kVerifiedBootMetadata);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ValidateWritten(result->value()->asset, 32);
}

TEST_F(PaverServiceSkipBlockTest, ReadAssetZbi) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  zbi_header_t container;
  // Currently our ZBI checker only validates the container header so the data can be anything.
  uint8_t data[8] = {10, 20, 30, 40, 50, 60, 70, 80};
  container.type = ZBI_TYPE_CONTAINER;
  container.extra = ZBI_CONTAINER_MAGIC;
  container.magic = ZBI_ITEM_MAGIC;
  container.flags = ZBI_FLAGS_VERSION;
  container.crc32 = ZBI_ITEM_NO_CRC32;
  container.length = sizeof(data);  // Contents size only, does not include header size.

  constexpr uint32_t kZirconAStartByte = kZirconAFirstBlock * kPagesPerBlock * kPageSize;
  WriteDataBytes(kZirconAStartByte, &container, sizeof(container));
  WriteDataBytes(kZirconAStartByte + sizeof(container), &data, sizeof(data));

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result = data_sink_->ReadAsset(fuchsia_paver::wire::Configuration::kA,
                                      fuchsia_paver::wire::Asset::kKernel);
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ(result->value()->asset.size, sizeof(container) + sizeof(data));

  fzl::VmoMapper mapper;
  ASSERT_OK(
      mapper.Map(result->value()->asset.vmo, 0, result->value()->asset.size, ZX_VM_PERM_READ));
  const uint8_t* read_data = static_cast<const uint8_t*>(mapper.start());
  ASSERT_EQ(0, memcmp(read_data, &container, sizeof(container)));
  ASSERT_EQ(0, memcmp(read_data + sizeof(container), &data, sizeof(data)));
}

TEST_F(PaverServiceSkipBlockTest, WriteBootloader) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(4) * kPagesPerBlock, &payload);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result =
      data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kA, "", std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().result.status());
  ValidateWritten(4, 4);
}

// We prefill the bootloader partition with the expected data, leaving the last block as 0xFF.
// Normally the last page would be overwritten with 0s, but because the actual payload is identical,
// we don't actually pave the image, so the extra page stays as 0xFF.
TEST_F(PaverServiceSkipBlockTest, WriteBootloaderNotAligned) {
  ASSERT_NO_FATAL_FAILURE(StartFixture());

  fuchsia_mem::wire::Buffer payload;
  CreateBuffer(static_cast<size_t>(4) * kPagesPerBlock - 1, &payload);

  WriteData(4 * kPagesPerBlock, static_cast<size_t>(4) * kPagesPerBlock - 1, 0x4a);
  WriteData(8 * kPagesPerBlock - 1, 1, 0xff);

  ASSERT_NO_FATAL_FAILURE(FindDataSink());
  auto result =
      data_sink_->WriteFirmware(fuchsia_paver::wire::Configuration::kA, "", std::move(payload));
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().result.status());
  ValidateWrittenPages(4 * kPagesPerBlock, static_cast<size_t>(4) * kPagesPerBlock - 1);
  ValidateUnwrittenPages(8 * kPagesPerBlock - 1, 1);
}

void PaverServiceSkipBlockTest::TestSysconfigWriteBufferedClient(uint32_t offset_in_pages,
                                                                 uint32_t sysconfig_pages) {
  {
    auto result = sysconfig_->GetPartitionSize();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
    ASSERT_EQ(result->value()->size, sysconfig_pages * kPageSize);
  }

  {
    fuchsia_mem::wire::Buffer payload;
    CreateBuffer(sysconfig_pages, &payload);
    auto result = sysconfig_->Write(std::move(payload));
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    // Without flushing, data in the storage should remain unchanged.
    ASSERT_NO_FATAL_FAILURE(
        ValidateUnwrittenPages(14 * kPagesPerBlock + offset_in_pages, sysconfig_pages));
  }

  {
    auto result = sysconfig_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ASSERT_NO_FATAL_FAILURE(
        ValidateWrittenPages(14 * kPagesPerBlock + offset_in_pages, sysconfig_pages));
  }

  {
    // Validate read.
    auto result = sysconfig_->Read();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());
    ASSERT_NO_FATAL_FAILURE(ValidateWritten(result->value()->data, sysconfig_pages));
  }
}

TEST_F(PaverServiceSkipBlockTest, SysconfigWriteWithBufferredClientLayoutNotUpdated) {
  // Enable write-caching + abr metadata wear-leveling
  ASSERT_NO_FATAL_FAILURE(StartFixture("-a", true));

  ASSERT_NO_FATAL_FAILURE(FindSysconfig());

  ASSERT_NO_FATAL_FAILURE(TestSysconfigWriteBufferedClient(0, 15 * 2));
}

TEST_F(PaverServiceSkipBlockTest, SysconfigWriteWithBufferredClientLayoutUpdated) {
  // Enable write-caching + abr metadata wear-leveling
  ASSERT_NO_FATAL_FAILURE(StartFixture("-a", true));

  auto abr_data = GetAbrWearlevelingSupportingLayout();
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindSysconfig());

  ASSERT_NO_FATAL_FAILURE(TestSysconfigWriteBufferedClient(2, 5 * 2));
}

void PaverServiceSkipBlockTest::TestSysconfigWipeBufferedClient(uint32_t offset_in_pages,
                                                                uint32_t sysconfig_pages) {
  {
    auto result = sysconfig_->Wipe();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    // Without flushing, data in the storage should remain unchanged.
    ASSERT_NO_FATAL_FAILURE(
        ValidateUnwrittenPages(14 * kPagesPerBlock + offset_in_pages, sysconfig_pages));
  }

  {
    auto result = sysconfig_->Flush();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ASSERT_NO_FATAL_FAILURE(AssertContents(
        static_cast<size_t>(14) * kSkipBlockSize + offset_in_pages * static_cast<size_t>(kPageSize),
        sysconfig_pages * static_cast<size_t>(kPageSize), 0));
  }
}

TEST_F(PaverServiceSkipBlockTest, SysconfigWipeWithBufferredClientLayoutNotUpdated) {
  // Enable write-caching + abr metadata wear-leveling
  ASSERT_NO_FATAL_FAILURE(StartFixture("-a", true));

  ASSERT_NO_FATAL_FAILURE(FindSysconfig());

  ASSERT_NO_FATAL_FAILURE(TestSysconfigWipeBufferedClient(0, 15 * 2));
}

TEST_F(PaverServiceSkipBlockTest, SysconfigWipeWithBufferredClientLayoutUpdated) {
  // Enable write-caching + abr metadata wear-leveling
  ASSERT_NO_FATAL_FAILURE(StartFixture("-a", true));

  auto abr_data = GetAbrWearlevelingSupportingLayout();
  SetAbr(abr_data);

  ASSERT_NO_FATAL_FAILURE(FindSysconfig());

  ASSERT_NO_FATAL_FAILURE(TestSysconfigWipeBufferedClient(2, 5 * 2));
}

class PaverServiceUefiTest : public PaverServiceTest {
 protected:
  IsolatedDevmgr::Args DevmgrArgs() override {
    IsolatedDevmgr::Args args = PaverServiceTest::DevmgrArgs();
    args.fshost_config.emplace_back(component_testing::ConfigCapability{
        .name = "fuchsia.fshost.GptAll", .value = component_testing::ConfigValue::Bool(true)});
    return args;
  }

  // Installs a UEFI-compatible GPT to `devmgr_` using the given `scheme`.
  //
  // Partition start blocks and sizes are given in the constants below.
  //
  // Returns nullptr and logs a test failure on error.
  std::unique_ptr<BlockDevice> InstallUefiGpt(paver::PartitionScheme scheme);

  // Tests writing then reading back a single asset.
  //
  // Args:
  // `scheme`: GPT partition scheme to test against.
  // `configuration`: which configuration to write.
  // `asset`: asset type to write.
  // `block_start`: disk block where the data is expected to be written.
  void AssetTest(paver::PartitionScheme scheme, fuchsia_paver::wire::Configuration configuration,
                 fuchsia_paver::wire::Asset asset, size_t block_start);

  static constexpr uint8_t kEmptyType[GPT_GUID_LEN] = GUID_EMPTY_VALUE;
  static constexpr size_t kEfiBlockStart = 0x20400;
  static constexpr size_t kEfiBlockSize = 0x10000;
  static constexpr size_t kZirconABlockStart = kEfiBlockStart + kEfiBlockSize;
  static constexpr size_t kZirconABlockSize = 0x10000;
  static constexpr size_t kZirconBBlockStart = kZirconABlockStart + kZirconABlockSize;
  static constexpr size_t kZirconBBlockSize = 0x10000;
  static constexpr size_t kZirconRBlockStart = kZirconBBlockStart + kZirconBBlockSize;
  static constexpr size_t kZirconRBlockSize = 0x10000;
  static constexpr size_t kVbmetaABlockStart = kZirconRBlockStart + kZirconRBlockSize;
  static constexpr size_t kVbmetaABlockSize = 0x10000;
  static constexpr size_t kVbmetaBBlockStart = kVbmetaABlockStart + kVbmetaABlockSize;
  static constexpr size_t kVbmetaBBlockSize = 0x10000;
  static constexpr size_t kVbmetaRBlockStart = kVbmetaBBlockStart + kVbmetaBBlockSize;
  static constexpr size_t kVbmetaRBlockSize = 0x10000;
  static constexpr size_t kFvmBlockStart = kVbmetaRBlockStart + kVbmetaRBlockSize;
  static constexpr size_t kFvmBlockSize = 0x10000;
};

std::unique_ptr<BlockDevice> PaverServiceUefiTest::InstallUefiGpt(paver::PartitionScheme scheme) {
  std::unique_ptr<BlockDevice> gpt_dev;
  constexpr uint64_t block_count = (64LU << 30) / kBlockSize;  // 64 GiB disk.
  bool legacy = (scheme == paver::PartitionScheme::kLegacy);
  std::vector<PartitionDescription> init_partitions = {
      {.name = legacy ? "efi-system" : GUID_EFI_NAME,
       .type = GUID_EFI_VALUE,  // Same for both schemes.
       .start = kEfiBlockStart,
       .length = kEfiBlockSize},
      {.name = legacy ? GUID_ZIRCON_A_NAME : GPT_ZIRCON_A_NAME,
       .type = legacy ? uuid::Uuid(GUID_ZIRCON_A_VALUE) : uuid::Uuid(GPT_ZIRCON_ABR_TYPE_GUID),
       .start = kZirconABlockStart,
       .length = kZirconABlockSize},
      {.name = legacy ? GUID_ZIRCON_B_NAME : GPT_ZIRCON_B_NAME,
       .type = legacy ? uuid::Uuid(GUID_ZIRCON_B_VALUE) : uuid::Uuid(GPT_ZIRCON_ABR_TYPE_GUID),
       .start = kZirconBBlockStart,
       .length = kZirconBBlockSize},
      {.name = legacy ? GUID_ZIRCON_R_NAME : GPT_ZIRCON_R_NAME,
       .type = legacy ? uuid::Uuid(GUID_ZIRCON_R_VALUE) : uuid::Uuid(GPT_ZIRCON_ABR_TYPE_GUID),
       .start = kZirconRBlockStart,
       .length = kZirconRBlockSize},
      {.name = legacy ? GUID_VBMETA_A_NAME : GPT_VBMETA_A_NAME,
       .type = legacy ? uuid::Uuid(GUID_VBMETA_A_VALUE) : uuid::Uuid(GPT_VBMETA_ABR_TYPE_GUID),
       .start = kVbmetaABlockStart,
       .length = kVbmetaABlockSize},
      {.name = legacy ? GUID_VBMETA_B_NAME : GPT_VBMETA_B_NAME,
       .type = legacy ? uuid::Uuid(GUID_VBMETA_B_VALUE) : uuid::Uuid(GPT_VBMETA_ABR_TYPE_GUID),
       .start = kVbmetaBBlockStart,
       .length = kVbmetaBBlockSize},
      {.name = legacy ? GUID_VBMETA_R_NAME : GPT_VBMETA_R_NAME,
       .type = legacy ? uuid::Uuid(GUID_VBMETA_R_VALUE) : uuid::Uuid(GPT_VBMETA_ABR_TYPE_GUID),
       .start = kVbmetaRBlockStart,
       .length = kVbmetaRBlockSize},
      {.name = legacy ? GUID_FVM_NAME : GPT_FVM_NAME,
       .type = legacy ? uuid::Uuid(GUID_FVM_VALUE) : uuid::Uuid(GPT_FVM_TYPE_GUID),
       .start = kFvmBlockStart,
       .length = kFvmBlockSize},
  };
  fidl::ClientEnd svc_root = devmgr_.RealmExposedDir();
  fbl::unique_fd fd;
  EXPECT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
  BlockDevice::CreateWithGpt(fd, block_count, kBlockSize, init_partitions, &gpt_dev);

  return gpt_dev;
}

void PaverServiceUefiTest::AssetTest(paver::PartitionScheme scheme,
                                     fuchsia_paver::wire::Configuration configuration,
                                     fuchsia_paver::wire::Asset asset, size_t block_start) {
  std::unique_ptr<BlockDevice> gpt_dev = InstallUefiGpt(scheme);
  ASSERT_NOT_NULL(gpt_dev.get());

  auto [client, server] = fidl::Endpoints<fuchsia_paver::DynamicDataSink>::Create();
  ASSERT_OK(client_->FindPartitionTableManager(std::move(server)));
  fidl::WireSyncClient data_sink{std::move(client)};

  ASSERT_NO_FATAL_FAILURE(
      TestReadWriteAsset(*gpt_dev, std::move(data_sink), configuration, asset, block_start));
}

// PaverServiceUefiTest but with a fshost ramdisk image, which is necessary to allow modification
// of the GPT at runtime. Non-ramdisk boots actively use the GPT so updating it at runtime would
// cause all sorts of unpredictable things to happen.
class PaverServiceUefiTestWithRamdisk : public PaverServiceUefiTest {
  IsolatedDevmgr::Args DevmgrArgs() override {
    IsolatedDevmgr::Args args = PaverServiceUefiTest::DevmgrArgs();
    args.fshost_config.emplace_back(
        component_testing::ConfigCapability{.name = "fuchsia.fshost.RamdiskImage",
                                            .value = component_testing::ConfigValue::Bool(true)});
    return args;
  }
};

TEST_F(PaverServiceUefiTestWithRamdisk, InitializePartitionTables) {
  std::unique_ptr<BlockDevice> gpt_dev;
  // 64GiB disk.
  constexpr uint64_t block_count = (64LU << 30) / kBlockSize;
  fidl::ClientEnd svc_root = devmgr_.RealmExposedDir();
  fbl::unique_fd fd;
  ASSERT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
  ASSERT_NO_FATAL_FAILURE(BlockDevice::CreateWithGpt(
      fd, block_count, kBlockSize,
      {
          {GUID_EFI_NAME, uuid::Uuid(GUID_EFI_VALUE), kEfiBlockStart, kEfiBlockSize},
          {GUID_FVM_NAME, uuid::Uuid(GUID_FVM_VALUE), kFvmBlockStart, kFvmBlockSize},
      },
      &gpt_dev));

  auto [data_sink, server] = fidl::Endpoints<fuchsia_paver::DynamicDataSink>::Create();
  fidl::OneWayStatus find_result = client_->FindPartitionTableManager(std::move(server));
  ASSERT_OK(find_result.status());

  fidl::WireResult result = fidl::WireCall(data_sink)->InitializePartitionTables();
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
}

TEST_F(PaverServiceUefiTestWithRamdisk, InitializePartitionTablesMultipleDevicesOneGpt) {
  std::unique_ptr<BlockDevice> gpt_dev1, gpt_dev2;
  // 64GiB disk.
  constexpr uint64_t block_count = (64LU << 30) / kBlockSize;
  fidl::ClientEnd svc_root = devmgr_.RealmExposedDir();
  fbl::unique_fd fd;
  ASSERT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
  ASSERT_NO_FATAL_FAILURE(BlockDevice::CreateWithGpt(
      fd, block_count, kBlockSize,
      {
          {GUID_EFI_NAME, uuid::Uuid(GUID_EFI_VALUE), kEfiBlockStart, kEfiBlockSize},
          {GUID_FVM_NAME, uuid::Uuid(GUID_FVM_VALUE), kFvmBlockStart, kFvmBlockSize},
      },
      &gpt_dev1));
  ASSERT_NO_FATAL_FAILURE(BlockDevice::Create(&gpt_dev2, fd, kEmptyType, block_count));

  auto [data_sink, server] = fidl::Endpoints<fuchsia_paver::DynamicDataSink>::Create();
  fidl::OneWayStatus find_result = client_->FindPartitionTableManager(std::move(server));
  ASSERT_OK(find_result.status());

  fidl::WireResult result = fidl::WireCall(data_sink)->InitializePartitionTables();
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
}

// Test a variety of asset read/write using both new and legacy partition schemes.
TEST_F(PaverServiceUefiTest, AssetZirconANew) {
  AssetTest(paver::PartitionScheme::kNew, fuchsia_paver::wire::Configuration::kA,
            fuchsia_paver::wire::Asset::kKernel, kZirconABlockStart);
}

TEST_F(PaverServiceUefiTest, AssetZirconBLegacy) {
  AssetTest(paver::PartitionScheme::kLegacy, fuchsia_paver::wire::Configuration::kB,
            fuchsia_paver::wire::Asset::kKernel, kZirconBBlockStart);
}

TEST_F(PaverServiceUefiTest, AssetZirconRNew) {
  AssetTest(paver::PartitionScheme::kNew, fuchsia_paver::wire::Configuration::kRecovery,
            fuchsia_paver::wire::Asset::kKernel, kZirconRBlockStart);
}

TEST_F(PaverServiceUefiTest, AssetVbmetaALegacy) {
  AssetTest(paver::PartitionScheme::kLegacy, fuchsia_paver::wire::Configuration::kA,
            fuchsia_paver::wire::Asset::kVerifiedBootMetadata, kVbmetaABlockStart);
}

TEST_F(PaverServiceUefiTest, AssetVbmetaBNew) {
  AssetTest(paver::PartitionScheme::kNew, fuchsia_paver::wire::Configuration::kB,
            fuchsia_paver::wire::Asset::kVerifiedBootMetadata, kVbmetaBBlockStart);
}

TEST_F(PaverServiceUefiTest, AssetVbmetaRLegacy) {
  AssetTest(paver::PartitionScheme::kLegacy, fuchsia_paver::wire::Configuration::kRecovery,
            fuchsia_paver::wire::Asset::kVerifiedBootMetadata, kVbmetaRBlockStart);
}

class PaverServiceGptDeviceTest : public PaverServiceTest {
 protected:
  void InitializeGptDevice(uint64_t block_count, uint32_t block_size,
                           const std::vector<PartitionDescription>& partitions) {
    block_count_ = block_count;
    block_size_ = block_size;
    fidl::ClientEnd svc_root = devmgr_.RealmExposedDir();
    fbl::unique_fd fd;
    EXPECT_OK(fdio_fd_create(svc_root.TakeHandle().release(), fd.reset_and_get_address()));
    ASSERT_NO_FATAL_FAILURE(
        BlockDevice::CreateWithGpt(fd, block_count, block_size, partitions, &gpt_dev_));
  }

  // Reads `count` blocks from disk starting at the given block `offset`.
  std::vector<uint8_t> ReadBlocks(size_t offset, size_t count) {
    size_t num_bytes = count * block_size_;
    zx::vmo vmo;
    fzl::VmoMapper vmo_mapper;
    EXPECT_OK(vmo_mapper.CreateAndMap(num_bytes, ZX_VM_PERM_READ, nullptr, &vmo));
    gpt_dev_->Read(vmo, num_bytes, offset, 0);

    std::vector<uint8_t> data(num_bytes);
    memcpy(data.data(), vmo_mapper.start(), num_bytes);
    return data;
  }

  std::unique_ptr<BlockDevice> gpt_dev_;
  uint64_t block_count_;
  uint64_t block_size_;
};

class PaverServiceLuisTest : public PaverServiceGptDeviceTest {
 public:
  static constexpr size_t kDurableBootStart = 0x10400;
  static constexpr size_t kDurableBootSize = 0x10000;
  static constexpr size_t kFvmBlockStart = 0x20400;
  static constexpr size_t kFvmBlockSize = 0x10000;

  IsolatedDevmgr::Args DevmgrArgs() override {
    IsolatedDevmgr::Args args = PaverServiceGptDeviceTest::DevmgrArgs();
    args.board_name = "luis";
    return args;
  }

  void SetUp() override {
    ASSERT_NO_FATAL_FAILURE(PaverServiceGptDeviceTest::SetUp());
    ASSERT_NO_FATAL_FAILURE(InitializeGptDevice(
        0x748038, 512,
        {
            {GPT_DURABLE_BOOT_NAME, uuid::Uuid(GUID_ZIRCON_A_VALUE), kDurableBootStart,
             kDurableBootSize},
            {GPT_FVM_NAME, uuid::Uuid(GUID_FVM_VALUE), kFvmBlockStart, kFvmBlockSize},
        }));
  }
};

TEST_F(PaverServiceLuisTest, SysconfigNotSupportedAndFailWithPeerClosed) {
  auto [local, remote] = fidl::Endpoints<fuchsia_paver::Sysconfig>::Create();
  auto result = client_->FindSysconfig(std::move(remote));
  ASSERT_OK(result.status());

  fidl::WireSyncClient sysconfig(std::move(local));
  auto wipe_result = sysconfig->Wipe();
  ASSERT_EQ(wipe_result.status(), ZX_ERR_PEER_CLOSED);
}

TEST_F(PaverServiceLuisTest, WriteOpaqueVolume) {
  // TODO(b/217597389): Consider also adding an e2e test for this interface.
  auto [local, remote] = fidl::Endpoints<fuchsia_paver::DynamicDataSink>::Create();
  ASSERT_OK(client_->FindPartitionTableManager(std::move(remote)));
  fidl::WireSyncClient data_sink{std::move(local)};

  // Create a payload
  constexpr size_t kPayloadSize = 2048;
  std::vector<uint8_t> payload(kPayloadSize, 0x4a);

  fuchsia_mem::wire::Buffer payload_wire_buffer;
  zx::vmo payload_vmo;
  fzl::VmoMapper payload_vmo_mapper;
  ASSERT_OK(payload_vmo_mapper.CreateAndMap(kPayloadSize, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                            nullptr, &payload_vmo));
  memcpy(payload_vmo_mapper.start(), payload.data(), kPayloadSize);
  payload_wire_buffer.vmo = std::move(payload_vmo);
  payload_wire_buffer.size = kPayloadSize;

  // Write the payload as opaque volume
  auto result = data_sink->WriteOpaqueVolume(std::move(payload_wire_buffer));
  ASSERT_OK(result.status());

  // Create a block partition client to read the written content directly.
  zx::result block_client = paver::BlockPartitionClient::Create(gpt_dev_->GetConnector());
  ASSERT_OK(block_client);

  // Read the partition directly from block and verify.
  zx::vmo block_read_vmo;
  fzl::VmoMapper block_read_vmo_mapper;
  ASSERT_OK(
      block_read_vmo_mapper.CreateAndMap(kPayloadSize, ZX_VM_PERM_READ, nullptr, &block_read_vmo));
  ASSERT_OK(block_client->Read(block_read_vmo, kPayloadSize, kFvmBlockStart, 0));

  // Verify the written data against the payload
  ASSERT_BYTES_EQ(block_read_vmo_mapper.start(), payload.data(), kPayloadSize);
}

struct SparseImageResult {
  std::vector<uint8_t> sparse;
  std::vector<uint32_t> raw_data;
  // image_length can be > raw_data.size(), simulating an image with sparse padding at the end.
  size_t image_length;
};

class Chunk {
 public:
  enum class ChunkType {
    kUnknown = 0,
    kRaw = CHUNK_TYPE_RAW,
    kFill = CHUNK_TYPE_FILL,
    kDontCare = CHUNK_TYPE_DONT_CARE,
    kCrc32 = CHUNK_TYPE_CRC32,
  };

  constexpr Chunk(ChunkType type, uint32_t payload, size_t output_blocks, size_t block_size)
      : type_(type),
        payload_(payload),
        output_blocks_(output_blocks),
        block_size_bytes_(block_size) {}

  constexpr chunk_header_t GenerateHeader() const {
    return chunk_header_t{
        .chunk_type = static_cast<uint16_t>(type_),
        .reserved1 = 0,
        .chunk_sz = static_cast<uint32_t>(output_blocks_),
        .total_sz = static_cast<uint32_t>(SizeInImage()),
    };
  }

  constexpr size_t SizeInImage() const {
    switch (type_) {
      case ChunkType::kRaw:
        return sizeof(chunk_header_t) + output_blocks_ * block_size_bytes_;
      case ChunkType::kCrc32:
      case ChunkType::kFill:
        return sizeof(chunk_header_t) + sizeof(payload_);
      case ChunkType::kUnknown:
      case ChunkType::kDontCare:
        return sizeof(chunk_header_t);
    }
  }

  constexpr size_t OutputSize() const {
    switch (type_) {
      case ChunkType::kRaw:
      case ChunkType::kFill:
      case ChunkType::kDontCare:
        return output_blocks_ * block_size_bytes_;
      case ChunkType::kUnknown:
      case ChunkType::kCrc32:
        return 0;
    }
  }

  constexpr size_t OutputBlocks() const { return output_blocks_; }

  void AppendImageBytes(std::vector<uint8_t>& sparse_image) const {
    chunk_header_t hdr = GenerateHeader();
    const uint8_t* hdr_bytes = reinterpret_cast<const uint8_t*>(&hdr);
    sparse_image.insert(sparse_image.end(), hdr_bytes, hdr_bytes + sizeof(hdr));

    uint32_t tmp = payload_;
    // Make the payload an ascending counter for the raw case to disambiguate with fill.
    uint32_t increment = type_ == ChunkType::kRaw ? 1 : 0;
    for (size_t i = 0; i < (SizeInImage() - sizeof(hdr)) / sizeof(tmp); i++, tmp += increment) {
      const uint8_t* tmp_bytes = reinterpret_cast<const uint8_t*>(&tmp);
      sparse_image.insert(sparse_image.end(), tmp_bytes, tmp_bytes + sizeof(tmp));
    }
  }

  void AppendExpectedBytes(std::vector<uint32_t>& image) const {
    // Make the payload an ascending counter for the raw case to disambiguate with fill.
    uint32_t increment = type_ == ChunkType::kRaw ? 1 : 0;
    uint32_t tmp = payload_;
    switch (type_) {
      case ChunkType::kRaw:
      case ChunkType::kFill:
        for (size_t i = 0; i < output_blocks_ * block_size_bytes_ / sizeof(uint32_t);
             i++, tmp += increment) {
          image.push_back(tmp);
        }
        break;
      case ChunkType::kDontCare:
        for (size_t i = 0; i < output_blocks_ * block_size_bytes_ / sizeof(uint32_t); i++) {
          // A DONT_CARE chunk still has an impact on the output image
          image.push_back(0);
        }
        break;
      case ChunkType::kUnknown:
      case ChunkType::kCrc32:
        break;
    }
  }

 private:
  ChunkType type_;
  uint32_t payload_;
  size_t output_blocks_;
  size_t block_size_bytes_;
};

SparseImageResult CreateSparseImage() {
  constexpr size_t kBlockSize = 512;
  std::vector<uint32_t> raw;
  std::vector<uint8_t> sparse;

  constexpr Chunk chunks[] = {
      Chunk(Chunk::ChunkType::kRaw, 0x55555555, 1, kBlockSize),
      Chunk(Chunk::ChunkType::kDontCare, 0, 2, kBlockSize),
      Chunk(Chunk::ChunkType::kFill, 0xCAFED00D, 3, kBlockSize),
  };
  size_t total_blocks =
      std::reduce(std::cbegin(chunks), std::cend(chunks), 0,
                  [](size_t sum, const Chunk& c) { return sum + c.OutputBlocks(); });
  size_t image_length =
      std::reduce(std::cbegin(chunks), std::cend(chunks), 0,
                  [](size_t sum, const Chunk& c) { return sum + c.OutputSize(); });
  sparse_header_t header = {
      .magic = SPARSE_HEADER_MAGIC,
      .major_version = 1,
      .file_hdr_sz = sizeof(sparse_header_t),
      .chunk_hdr_sz = sizeof(chunk_header_t),
      .blk_sz = kBlockSize,
      .total_blks = static_cast<uint32_t>(total_blocks),
      .total_chunks = static_cast<uint32_t>(std::size(chunks)),
      .image_checksum = 0xDEADBEEF  // We don't do crc validation as of 2023-07-05
  };
  const uint8_t* header_bytes = reinterpret_cast<const uint8_t*>(&header);
  sparse.insert(sparse.end(), header_bytes, header_bytes + sizeof(header));
  for (const Chunk& chunk : chunks) {
    chunk.AppendImageBytes(sparse);
    chunk.AppendExpectedBytes(raw);
  }

  return SparseImageResult{
      .sparse = std::move(sparse),
      .raw_data = std::move(raw),
      .image_length = image_length,
  };
}

TEST_F(PaverServiceLuisTest, WriteSparseVolume) {
  auto [local, remote] = fidl::Endpoints<fuchsia_paver::DynamicDataSink>::Create();
  ASSERT_OK(client_->FindPartitionTableManager(std::move(remote)));
  fidl::WireSyncClient data_sink{std::move(local)};

  SparseImageResult image = CreateSparseImage();

  fuchsia_mem::wire::Buffer payload_wire_buffer;
  zx::vmo payload_vmo;
  fzl::VmoMapper payload_vmo_mapper;
  ASSERT_OK(payload_vmo_mapper.CreateAndMap(image.sparse.size(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                            nullptr, &payload_vmo));
  std::copy(image.sparse.cbegin(), image.sparse.cend(),
            static_cast<uint8_t*>(payload_vmo_mapper.start()));
  payload_wire_buffer.vmo = std::move(payload_vmo);
  payload_wire_buffer.size = image.sparse.size();

  auto result = data_sink->WriteSparseVolume(std::move(payload_wire_buffer));
  ASSERT_OK(result.status());

  // Create a block partition client to read the written content directly.
  zx::result block_client = paver::BlockPartitionClient::Create(gpt_dev_->GetConnector());
  ASSERT_OK(block_client);

  // Read the partition directly from block and verify.  Read `image.image_length` bytes so we know
  // the image was paved to the desired length, although we only verify the bytes up to the size of
  // `image.raw_data`.
  zx::vmo block_read_vmo;
  fzl::VmoMapper block_read_vmo_mapper;
  ASSERT_OK(block_read_vmo_mapper.CreateAndMap(image.image_length, ZX_VM_PERM_READ, nullptr,
                                               &block_read_vmo));
  ASSERT_OK(block_client->Read(block_read_vmo, image.image_length, kFvmBlockStart, 0));

  // Verify the written data against the unsparsed payload
  std::span<const uint8_t> raw_as_bytes = {reinterpret_cast<const uint8_t*>(image.raw_data.data()),
                                           image.raw_data.size() * sizeof(uint32_t)};
  ASSERT_BYTES_EQ(block_read_vmo_mapper.start(), raw_as_bytes.data(), raw_as_bytes.size());
}

TEST_F(PaverServiceLuisTest, OneShotRecovery) {
  // TODO(b/255567130): There's an discussion whether use one-shot-recovery to implement
  // RebootToRecovery in power-manager. If the approach is taken, paver e2e test will
  // cover this.
  auto [local, remote] = fidl::Endpoints<fuchsia_paver::BootManager>::Create();

  auto result = client_->FindBootManager(std::move(remote));
  ASSERT_OK(result.status());
  auto boot_manager = fidl::WireSyncClient(std::move(local));

  auto set_one_shot_recovery_result = boot_manager->SetOneShotRecovery();
  ASSERT_OK(set_one_shot_recovery_result.status());

  // Read the abr data directly from block and verify.
  zx::vmo block_read_vmo;
  fzl::VmoMapper block_read_vmo_mapper;
  ASSERT_OK(block_read_vmo_mapper.CreateAndMap(kDurableBootSize * kBlockSize, ZX_VM_PERM_READ,
                                               nullptr, &block_read_vmo));
  gpt_dev_->Read(block_read_vmo, kDurableBootSize, kDurableBootStart, 0);

  AbrData disk_abr_data;
  memcpy(&disk_abr_data, block_read_vmo_mapper.start(), sizeof(disk_abr_data));
  ASSERT_TRUE(AbrIsOneShotRecoveryBoot(&disk_abr_data));
}

// Randomly generated. Moonflower A/B does care about GUIDs, but writing images doesn't, and
// currently that's all we're testing here.
constexpr Uuid kTestGuid = Uuid(0x8b, 0xa2, 0x50, 0x03, 0xc6, 0x0d, 0x4c, 0x9b, 0x85, 0xe7, 0x5c,
                                0x4f, 0x71, 0xaf, 0x37, 0x10);

// Creates a chunk of test data with incrementally increasing bytes rolling over at 0xFF->0x00.
std::vector<uint8_t> TestData(size_t size) {
  std::vector<uint8_t> data(size);
  for (size_t i = 0; i < data.size(); ++i) {
    data[i] = static_cast<uint8_t>(i);
  }
  return data;
}

class PaverServiceMoonflowerTest : public PaverServiceGptDeviceTest {
 public:
  paver::PaverConfig Config() override {
    return {
        .system_partition_names = {"super_and_userdata"},
        .zvb_current_slot = "_a",
    };
  }

  IsolatedDevmgr::Args DevmgrArgs() override {
    IsolatedDevmgr::Args args = PaverServiceGptDeviceTest::DevmgrArgs();
    args.board_name = "sorrel";  // Any moonflower board name is fine here
    args.fshost_config.emplace_back(
        component_testing::ConfigCapability{.name = "fuchsia.fshost.MergeSuperAndUserdata",
                                            .value = component_testing::ConfigValue::Bool(true)});
    return args;
  }

  void SetUp() override {
    ASSERT_NO_FATAL_FAILURE(PaverServiceGptDeviceTest::SetUp());
    ASSERT_NO_FATAL_FAILURE(
        InitializeGptDevice(0x20000, 512,
                            {
                                {"boot_a", kTestGuid, 0x1000, 0x1000},
                                {"boot_b", kTestGuid, 0x2000, 0x1000},
                                {"vendor_boot_a", kTestGuid, 0x3000, 0x1000},
                                {"vendor_boot_b", kTestGuid, 0x4000, 0x1000},
                                {"vendor_kernel_boot_a", kTestGuid, 0x5000, 0x1000},
                                {"vendor_kernel_boot_b", kTestGuid, 0x6000, 0x1000},
                                {"super", kTestGuid, 0x7000, 0x1},
                                {"userdata", kTestGuid, 0x7001, 0xfff},
                                {"dtbo_a", kTestGuid, 0x8000, 0x1000},
                                {"dtbo_b", kTestGuid, 0x9000, 0x1000},
                            }));
  }
};

// `WriteFirmware()` with a named partition.
TEST_F(PaverServiceMoonflowerTest, WriteFirmwareWithPartitionName) {
  std::vector<uint8_t> test_data = TestData(block_size_ * 3);

  fuchsia_paver::wire::WriteFirmwareResult result;
  ASSERT_NO_FATAL_FAILURE(WriteFirmware(
      "vendor_kernel_boot", fuchsia_paver::wire::Configuration::kA, test_data, &result));
  ASSERT_TRUE(result.is_status());
  ASSERT_OK(result.status());

  // Read it back and make sure it matches what we wrote.
  std::vector<uint8_t> actual = ReadBlocks(0x5000, 3);
  ASSERT_EQ(test_data, actual);
}

// `WriteFirmware()` with the "recovery_zbi" -> "vendor_boot" mapping.
TEST_F(PaverServiceMoonflowerTest, WriteFirmwareWithRecoveryZbi) {
  std::vector<uint8_t> test_data = TestData(block_size_ * 3);

  fuchsia_paver::wire::WriteFirmwareResult result;
  ASSERT_NO_FATAL_FAILURE(
      WriteFirmware("recovery_zbi", fuchsia_paver::wire::Configuration::kB, test_data, &result));
  ASSERT_TRUE(result.is_status());
  ASSERT_OK(result.status());

  // Read it back and make sure it matches what we wrote.
  std::vector<uint8_t> actual = ReadBlocks(0x4000, 3);
  ASSERT_EQ(test_data, actual);
}

// `WriteFirmware()` should fail when the given partition name doesn't exist.
TEST_F(PaverServiceMoonflowerTest, WriteFirmwareWithNonExistentPartitionFails) {
  std::vector<uint8_t> test_data = TestData(block_size_ * 3);

  fuchsia_paver::wire::WriteFirmwareResult result;
  ASSERT_NO_FATAL_FAILURE(
      WriteFirmware("not_a_partition", fuchsia_paver::wire::Configuration::kA, test_data, &result));
  ASSERT_TRUE(result.is_status());
  ASSERT_NOT_OK(result.status());
}

// `WriteFirmware()` should fail when the given a non-slotted partition.
TEST_F(PaverServiceMoonflowerTest, WriteFirmwareWithNonSlottedPartitionFails) {
  std::vector<uint8_t> test_data = TestData(block_size_ * 3);

  fuchsia_paver::wire::WriteFirmwareResult result;
  ASSERT_NO_FATAL_FAILURE(
      WriteFirmware("userdata", fuchsia_paver::wire::Configuration::kA, test_data, &result));
  ASSERT_TRUE(result.is_status());
  ASSERT_NOT_OK(result.status());
}

TEST_F(PaverServiceMoonflowerTest, WriteOpaqueVolume) {
  // TODO(b/217597389): Consider also adding an e2e test for this interface.
  auto [local, remote] = fidl::Endpoints<fuchsia_paver::DynamicDataSink>::Create();
  ASSERT_OK(client_->FindPartitionTableManager(std::move(remote)));
  fidl::WireSyncClient data_sink{std::move(local)};

  // Create a payload that is intentionally big enough to spill over super and into userdata
  constexpr size_t kPayloadSize = 16384;
  std::vector<uint8_t> payload(kPayloadSize, 0x4a);

  fuchsia_mem::wire::Buffer payload_wire_buffer;
  zx::vmo payload_vmo;
  fzl::VmoMapper payload_vmo_mapper;
  ASSERT_OK(payload_vmo_mapper.CreateAndMap(kPayloadSize, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                            nullptr, &payload_vmo));
  memcpy(payload_vmo_mapper.start(), payload.data(), kPayloadSize);
  payload_wire_buffer.vmo = std::move(payload_vmo);
  payload_wire_buffer.size = kPayloadSize;

  // Write the payload as opaque volume
  auto result = data_sink->WriteOpaqueVolume(std::move(payload_wire_buffer));
  ASSERT_OK(result.status());
}

}  // namespace
