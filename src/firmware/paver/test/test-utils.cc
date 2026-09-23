// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/test/test-utils.h"

#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/zbi-format/partition.h>
#include <lib/zx/vmo.h>
#include <limits.h>

#include <memory>
#include <optional>

#include <fbl/algorithm.h>
#include <fbl/string.h>
#include <fbl/vector.h>
#include <gpt/gpt.h>
#include <zxtest/zxtest.h>

#include "src/lib/uuid/uuid.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/block_client/cpp/reader_writer.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace {

class RamdiskVolumeConnector : public paver::VolumeConnector {
 public:
  explicit RamdiskVolumeConnector(const ramdevice_client::Ramdisk& ramdisk) : ramdisk_(ramdisk) {}

  zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> Connect() const override {
    return ramdisk_.ConnectBlock();
  }

  zx::result<fidl::ClientEnd<fuchsia_storage_partitions::Partition>> PartitionManagement()
      const override {
    ZX_PANIC("Not implemented");
  }

 private:
  const ramdevice_client::Ramdisk& ramdisk_;
};

void CreateBadBlockMap(void* buffer) {
  // Set all entries in first BBT to be good blocks.
  constexpr uint8_t kBlockGood = 0;
  memset(buffer, kBlockGood, kPageSize);

  struct OobMetadata {
    uint32_t magic;
    int16_t program_erase_cycles;
    uint16_t generation;
  };

  constexpr size_t oob_offset{static_cast<size_t>(kPageSize) * kPagesPerBlock * kNumBlocks};
  auto* oob = reinterpret_cast<OobMetadata*>(reinterpret_cast<uintptr_t>(buffer) + oob_offset);
  oob->magic = 0x7462626E;  // "nbbt"
  oob->program_erase_cycles = 0;
  oob->generation = 1;
}

}  // namespace

zx::result<DeviceAndController> GetNewConnections(
    fidl::UnownedClientEnd<fuchsia_device::Controller> controller) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_device::Controller>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  if (fidl::OneWayError response =
          fidl::WireCall(controller)->ConnectToController(std::move(endpoints->server));
      !response.ok()) {
    return zx::error(response.status());
  }
  zx::result device_endpoints = fidl::CreateEndpoints<fuchsia_device::Controller>();
  if (device_endpoints.is_error()) {
    return device_endpoints.take_error();
  }
  if (fidl::OneWayError response =
          fidl::WireCall(controller)->ConnectToDeviceFidl(device_endpoints->server.TakeChannel());
      !response.ok()) {
    return zx::error(response.status());
  }
  return zx::ok(DeviceAndController{
      .device = device_endpoints->client.TakeChannel(),
      .controller = std::move(endpoints->client),
  });
}

void BlockDevice::Create(std::unique_ptr<BlockDevice>* device, const fbl::unique_fd& svc_root,
                         const uint8_t* guid, uint64_t block_count, uint32_t block_size) {
  ramdevice_client::Ramdisk::Options options;
  if (guid) {
    options.type_guid = {{}};
    std::copy(guid, guid + ZBI_PARTITION_GUID_LEN, options.type_guid->begin());
  }
  zx::result ramdisk =
      ramdevice_client::Ramdisk::Create(block_size, block_count, svc_root.get(), options);
  ASSERT_OK(ramdisk);
  zx::result block = ramdisk->ConnectBlock();
  ASSERT_OK(block);
  fidl::ClientEnd<fuchsia_storage_block::Block> volume(std::move(block)->TakeChannel());
  device->reset(new BlockDevice(std::move(*ramdisk), std::move(volume), block_count, block_size));
}

void BlockDevice::CreateFromVmo(std::unique_ptr<BlockDevice>* device,
                                const fbl::unique_fd& svc_root, const uint8_t* guid, zx::vmo vmo,
                                uint32_t block_size) {
  uint64_t block_count;
  ASSERT_OK(vmo.get_size(&block_count));
  block_count /= block_size;
  ramdevice_client::Ramdisk::Options options;
  if (guid) {
    options.type_guid = {{}};
    std::copy(guid, guid + ZBI_PARTITION_GUID_LEN, options.type_guid->begin());
  }
  zx::result ramdisk =
      ramdevice_client::Ramdisk::CreateWithVmo(std::move(vmo), block_size, svc_root.get(), options);
  ASSERT_OK(ramdisk);
  zx::result block = ramdisk->ConnectBlock();
  ASSERT_OK(block);
  fidl::ClientEnd<fuchsia_storage_block::Block> volume(std::move(block)->TakeChannel());
  device->reset(new BlockDevice(std::move(*ramdisk), std::move(volume), block_count, block_size));
}

void BlockDevice::CreateWithGpt(const fbl::unique_fd& svc_root, uint64_t block_count,
                                uint32_t block_size,
                                const std::vector<PartitionDescription>& init_partitions,
                                std::unique_ptr<BlockDevice>* device) {
  auto dev = std::make_unique<block_client::FakeBlockDevice>(block_count, block_size);
  zx::result contents = dev->VmoChildReference();
  ASSERT_OK(contents);
  zx::result gpt_result = gpt::GptDevice::Create(std::move(dev), block_size, block_count);
  ASSERT_OK(gpt_result);
  std::unique_ptr<gpt::GptDevice> gpt = std::move(*gpt_result);
  ASSERT_OK(gpt->Sync());

  for (auto& part : init_partitions) {
    uuid::Uuid instance = part.instance.value_or(uuid::Uuid::Generate());
    ASSERT_OK(gpt->AddPartition(part.name.c_str(), part.type.bytes(), instance.bytes(), part.start,
                                part.length, 0),
              "%s", part.name.c_str());
  }
  ASSERT_OK(gpt->Sync());

  constexpr const uint8_t kEmptyGuid[ZBI_PARTITION_GUID_LEN] = {0};
  ASSERT_NO_FATAL_FAILURE(
      CreateFromVmo(device, svc_root, kEmptyGuid, std::move(*contents), block_size));
}

void BlockDevice::Read(const zx::vmo& vmo, size_t size, size_t dev_offset,
                       size_t vmo_offset) const {
  zx::result block_client = block_client::RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>(Connect().TakeChannel()));
  ASSERT_OK(block_client);
  ASSERT_OK(block_client::ReaderWriter(**block_client)
                .Read(dev_offset * block_size_, size, vmo, vmo_offset));
}

void BlockDevice::Write(const zx::vmo& vmo, size_t size, size_t dev_offset,
                        size_t vmo_offset) const {
  zx::result block_client = block_client::RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>(Connect().TakeChannel()));
  ASSERT_OK(block_client);
  ASSERT_OK(block_client::ReaderWriter(**block_client)
                .Write(dev_offset * block_size_, size, vmo, vmo_offset));
}

void SkipBlockDevice::Create(fbl::unique_fd devfs_root,
                             fuchsia_hardware_nand::wire::RamNandInfo nand_info,
                             std::unique_ptr<SkipBlockDevice>* device) {
  fzl::VmoMapper mapper;
  zx::vmo vmo;
  ASSERT_OK(
      mapper.CreateAndMap(static_cast<size_t>(kPageSize + kOobSize) * kPagesPerBlock * kNumBlocks,
                          ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));
  memset(mapper.start(), 0xff, mapper.size());
  CreateBadBlockMap(mapper.start());
  vmo.op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, 0, mapper.size(), nullptr, 0);
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &nand_info.vmo));

  std::unique_ptr<ramdevice_client_test::RamNandCtl> ctl;
  ASSERT_OK(ramdevice_client_test::RamNandCtl::Create(std::move(devfs_root), &ctl));
  std::optional<ramdevice_client::RamNand> ram_nand;
  ASSERT_OK(ctl->CreateRamNand(std::move(nand_info), &ram_nand));
  device->reset(new SkipBlockDevice(std::move(ctl), *std::move(ram_nand), std::move(mapper)));
}

FakePartitionClient::FakePartitionClient(size_t block_count, size_t block_size)
    : block_size_(block_size) {
  partition_size_ = block_count * block_size;
  zx_status_t status = zx::vmo::create(partition_size_, ZX_VMO_RESIZABLE, &partition_);
  if (status != ZX_OK) {
    partition_size_ = 0;
  }
}

FakePartitionClient::FakePartitionClient(size_t block_count)
    : FakePartitionClient(block_count, zx_system_get_page_size()) {}

zx::result<size_t> FakePartitionClient::GetBlockSize() { return zx::ok(block_size_); }

zx::result<size_t> FakePartitionClient::GetPartitionSize() { return zx::ok(partition_size_); }

zx::result<> FakePartitionClient::Read(const zx::vmo& vmo, size_t size) {
  if (partition_size_ == 0) {
    return zx::ok();
  }

  fzl::VmoMapper mapper;
  if (auto status = mapper.Map(vmo, 0, size, ZX_VM_PERM_WRITE); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::make_result(partition_.read(mapper.start(), 0, size));
}

zx::result<> FakePartitionClient::Write(const zx::vmo& vmo, size_t size) {
  if (size > partition_size_) {
    size_t new_size = fbl::round_up(size, block_size_);
    zx_status_t status = partition_.set_size(new_size);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    partition_size_ = new_size;
  }

  fzl::VmoMapper mapper;
  if (auto status = mapper.Map(vmo, 0, size, ZX_VM_PERM_READ | ZX_VM_ALLOW_FAULTS);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::make_result(partition_.write(mapper.start(), 0, size));
}

zx::result<> FakePartitionClient::Trim() {
  zx_status_t status = partition_.set_size(0);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  partition_size_ = 0;
  return zx::ok();
}

zx::result<> FakePartitionClient::Flush() { return zx::ok(); }

paver::PaverConfig FakePaverConfig(std::string slot_suffix) {
  paver::PaverConfig config;
  config.zvb_current_slot = std::move(slot_suffix);
  return config;
}

fbl::Array<uint8_t> CreateZbiHeader(paver::Arch arch, size_t payload_size,
                                    ZbiKernelImage** result_header, std::span<uint8_t>* span) {
  // Allocate raw memory.
  const size_t data_size = sizeof(ZbiKernelImage) + payload_size;
  auto data = fbl::Array<uint8_t>(new uint8_t[data_size], data_size);
  memset(data.get(), 0xee, data_size);

  // Set up header for outer ZBI header.
  auto header = reinterpret_cast<ZbiKernelImage*>(data.get());
  header->hdr_file.type = ZBI_TYPE_CONTAINER;
  header->hdr_file.extra = ZBI_CONTAINER_MAGIC;
  header->hdr_file.magic = ZBI_ITEM_MAGIC;
  header->hdr_file.flags = ZBI_FLAGS_VERSION;
  header->hdr_file.crc32 = ZBI_ITEM_NO_CRC32;
  header->hdr_file.length =
      static_cast<uint32_t>(sizeof(zbi_header_t) + sizeof(zbi_kernel_t) + payload_size);

  // Set up header for inner ZBI header.
  header->hdr_kernel.type =
      (arch == paver::Arch::kX64) ? ZBI_TYPE_KERNEL_X64 : ZBI_TYPE_KERNEL_ARM64;
  header->hdr_kernel.magic = ZBI_ITEM_MAGIC;
  header->hdr_kernel.flags = ZBI_FLAGS_VERSION;
  header->hdr_kernel.crc32 = ZBI_ITEM_NO_CRC32;
  header->hdr_kernel.length = static_cast<uint32_t>(sizeof(zbi_kernel_t) + payload_size);

  if (span != nullptr) {
    *span = std::span<uint8_t>(data.get(), data_size);
  }
  if (result_header != nullptr) {
    *result_header = reinterpret_cast<ZbiKernelImage*>(data.get());
  }

  return data;
}

std::unique_ptr<paver::VolumeConnector> BlockDevice::GetConnector() const {
  return std::make_unique<RamdiskVolumeConnector>(ramdisk_);
}

FakeBlockDirectory::FakeBlockDirectory(async_dispatcher_t* dispatcher,
                                       std::vector<block_server::PartitionInfo> partitions)
    : root_dir_(fbl::MakeRefCounted<fs::PseudoDir>()), vfs_(dispatcher) {
  for (size_t i = 0; i < partitions.size(); ++i) {
    servers_.push_back(std::make_unique<block_server::FakeServer>(std::move(partitions[i])));
    auto device_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    ZX_ASSERT(root_dir_->AddEntry("block-" + std::to_string(i), device_dir) == ZX_OK);
    ZX_ASSERT(device_dir->AddEntry("fuchsia.storage.block.Block",
                                   fbl::MakeRefCounted<fs::Service>([this, i](zx::channel channel) {
                                     fidl::ServerEnd<fuchsia_storage_block::Block> request(
                                         std::move(channel));
                                     this->servers_[i]->Serve(std::move(request));
                                     return ZX_OK;
                                   })) == ZX_OK);
  }

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  ZX_ASSERT(vfs_.ServeDirectory(root_dir_, std::move(server)) == ZX_OK);

  fbl::unique_fd fd;
  ZX_ASSERT(fdio_fd_create(client.TakeChannel().release(), fd.reset_and_get_address()) == ZX_OK);
  block_dir_fd_ = std::move(fd);
}

fbl::unique_fd FakeBlockDirectory::DuplicateBlockDirFd() {
  return fbl::unique_fd(dup(block_dir_fd_.get()));
}
