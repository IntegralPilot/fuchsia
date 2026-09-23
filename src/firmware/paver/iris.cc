// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/iris.h"

#include <dirent.h>
#include <fidl/fuchsia.hardware.ufs/cpp/wire.h>
#include <fidl/fuchsia.storage.partitions/cpp/wire_types.h>
#include <lib/abr/abr.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/hw/gpt.h>
#include <zircon/status.h>

#include <algorithm>
#include <iterator>
#include <set>
#include <string>

#include <fbl/unique_fd.h>
#include <gpt/gpt.h>

#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/boot_control_definition.h"
#include "src/firmware/paver/gpt.h"
#include "src/firmware/paver/libboot_control.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/firmware/paver/utils.h"
#include "src/firmware/paver/validation.h"
#include "src/lib/uuid/uuid.h"

namespace paver {

const std::set<std::string> kSupportedBoards{
    "iris",
};

namespace {

zx::result<fidl::ClientEnd<fuchsia_hardware_ufs::Ufs>> OpenUfsService(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root) {
  if (!svc_root) {
    ERROR("Svc root is not valid\n");
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  auto [client, server] = std::move(*endpoints);
  if (zx_status_t status = fdio_open3_at(svc_root.handle()->get(), "fuchsia.hardware.ufs.Service",
                                         static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                                         server.TakeChannel().release());
      status != ZX_OK) {
    ERROR("Failed to open fuchsia.hardware.ufs.Service: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }

  fbl::unique_fd ufs_svc_dir;
  if (zx_status_t status =
          fdio_fd_create(client.TakeChannel().release(), ufs_svc_dir.reset_and_get_address());
      status != ZX_OK) {
    ERROR("Failed to create fd for ufs service directory: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }

  DIR* dir = fdopendir(ufs_svc_dir.duplicate().release());
  if (dir == nullptr) {
    ERROR("Cannot inspect ufs service directory: %s\n", strerror(errno));
    return zx::error(ZX_ERR_INTERNAL);
  }
  const auto closer = fit::defer([dir]() { closedir(dir); });

  zx::result<fidl::ClientEnd<fuchsia_hardware_ufs::Ufs>> ufs_client = zx::error(ZX_ERR_NOT_FOUND);
  size_t instance_count = 0;
  struct dirent* de;
  while ((de = readdir(dir)) != nullptr) {
    if (std::string_view{de->d_name} == "." || std::string_view{de->d_name} == "..") {
      continue;
    }
    instance_count++;
    if (instance_count > 1) {
      ERROR("Expected single UFS service instance but found multiple\n");
      return zx::error(ZX_ERR_BAD_STATE);
    }
    std::string filename(de->d_name, strnlen(de->d_name, sizeof(de->d_name)));
    fbl::unique_fd instance_fd;
    if (zx_status_t status =
            fdio_open3_fd_at(ufs_svc_dir.get(), filename.c_str(),
                             static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                             instance_fd.reset_and_get_address());
        status != ZX_OK) {
      ERROR("Failed to open ufs service instance %s: %s\n", filename.c_str(),
            zx_status_get_string(status));
      return zx::error(status);
    }

    fdio_cpp::UnownedFdioCaller caller(instance_fd);
    zx::result ufs_endpoints = fidl::CreateEndpoints<fuchsia_hardware_ufs::Ufs>();
    if (ufs_endpoints.is_error()) {
      ERROR("Failed to create endpoints for ufs service instance %s: %s\n", filename.c_str(),
            ufs_endpoints.status_string());
      return ufs_endpoints.take_error();
    }
    if (zx_status_t status = fdio_service_connect_at(caller.borrow_channel(), "device",
                                                     ufs_endpoints->server.TakeChannel().release());
        status == ZX_OK) {
      LOG("Successfully connected to UFS service instance %s\n", filename.c_str());
      ufs_client = zx::ok(std::move(ufs_endpoints->client));
    }
  }

  if (ufs_client.is_error()) {
    ERROR("Failed to find and connect to Ufs service instance\n");
    return ufs_client.take_error();
  }

  LOG("Successfully opened connection to fuchsia.hardware.ufs.Ufs\n");

  // Perform a read of the BOOT_LUN_EN attribute and log the active boot slot it specifies.
  fidl::Arena arena;
  auto id = fuchsia_hardware_ufs::wire::Identifier::Builder(arena).index(0).selector(0).Build();
  auto attr = fuchsia_hardware_ufs::wire::Attribute::Builder(arena)
                  .type(fuchsia_hardware_ufs::wire::AttributeType::kBootLunEn)
                  .identifier(id)
                  .Build();
  const fidl::WireResult result = fidl::WireCall(ufs_client.value())->ReadAttribute(attr);
  if (!result.ok()) {
    ERROR("Failed to read BOOT_LUN_EN attribute (FIDL error): %s\n", result.status_string());
  } else if (result.value().is_error()) {
    ERROR("Failed to read BOOT_LUN_EN attribute (Query error code): %d\n",
          static_cast<uint32_t>(result.value().error_value()));
  } else {
    uint32_t boot_lun = result.value().value()->value;
    const char* boot_slot = (boot_lun == 2) ? "SLOT_B" : "SLOT_A";
    LOG("BOOT_LUN_EN active slot: %s\n", boot_slot);
  }

  return ufs_client;
}

zx::result<> SetActiveSlotInUfs(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                                AbrSlotIndex slot_index) {
  if (slot_index != kAbrSlotIndexA && slot_index != kAbrSlotIndexB) {
    ERROR("Unsupported AbrSlotIndex for UFS boot slot\n");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result ufs_client = OpenUfsService(svc_root);
  if (ufs_client.is_error()) {
    ERROR("Failed to open Ufs service for setting active slot\n");
    return ufs_client.take_error();
  }

  fidl::Arena arena;
  auto id = fuchsia_hardware_ufs::wire::Identifier::Builder(arena).index(0).selector(0).Build();
  auto attr = fuchsia_hardware_ufs::wire::Attribute::Builder(arena)
                  .type(fuchsia_hardware_ufs::wire::AttributeType::kBootLunEn)
                  .identifier(id)
                  .Build();
  uint32_t val = (slot_index == kAbrSlotIndexB) ? 2 : 1;
  const fidl::WireResult result = fidl::WireCall(ufs_client.value())->WriteAttribute(attr, val);
  if (!result.ok()) {
    ERROR("Failed to write bBootLunEn attribute (FIDL error): %s\n", result.status_string());
    return zx::error(result.status());
  }
  if (result.value().is_error()) {
    ERROR("Failed to write bBootLunEn attribute (Query error code): %d\n",
          static_cast<uint32_t>(result.value().error_value()));
    return zx::error(ZX_ERR_BAD_STATE);
  }

  LOG("Successfully set active slot in UFS bBootLunEn attribute\n");
  return zx::ok();
}

}  // namespace

zx::result<std::unique_ptr<DevicePartitioner>> IrisPartitioner::Initialize(
    const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config) {
  zx::result<std::string> board_name = GetBoardName(svc_root);
  if (board_name.is_error()) {
    return board_name.take_error();
  }

  if (!kSupportedBoards.contains(board_name.value())) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  auto gpt = GptDevicePartitioner::InitializeGpt(devices, svc_root, config);
  if (gpt.is_error()) {
    return gpt.take_error();
  }
  if (gpt->initialize_partition_tables) {
    LOG("Found GPT but it was missing expected partitions. The device should be re-initialized via fastboot.\n");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  auto partitioner =
      WrapUnique(new IrisPartitioner(devices.Duplicate(), component::MaybeClone(svc_root)));

  LOG("Successfully initialized Iris Device Partitioner\n");
  return zx::ok(std::move(partitioner));
}

bool IrisPartitioner::SupportsPartition(const PartitionSpec& spec) const {
  if (spec.partition == Partition::kBootloaderA || spec.partition == Partition::kBootloaderB) {
    // TODO(b/515134439): Support recover_zbi once recovery image is supported.
    return !spec.content_type.empty() && spec.content_type != "recovery_zbi";
  }

  constexpr PartitionSpec non_bootloader_specs[] = {
      PartitionSpec(paver::Partition::kZirconA),
      PartitionSpec(paver::Partition::kZirconB),
      PartitionSpec(paver::Partition::kVbMetaA),
      PartitionSpec(paver::Partition::kVbMetaB),
      PartitionSpec(paver::Partition::kAbrMeta),
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager),
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager, kOpaqueVolumeContentType),
  };
  return std::any_of(std::cbegin(non_bootloader_specs), std::cend(non_bootloader_specs),
                     [&](const PartitionSpec& supported) { return SpecMatches(spec, supported); });
}

zx::result<std::unique_ptr<PartitionClient>> IrisPartitioner::FindPartition(
    const PartitionSpec& spec) const {
  if (!SupportsPartition(spec)) {
    ERROR("Unsupported partition %s\n", spec.ToString().c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  auto gpt = GptDevicePartitioner::InitializeGpt(devices_, svc_root_, PaverConfig{});
  if (gpt.is_error()) {
    return gpt.take_error();
  }

  std::string part_name;
  switch (spec.partition) {
    case Partition::kBootloaderA:
    case Partition::kBootloaderB:
      part_name = spec.content_type;
      part_name += spec.partition == Partition::kBootloaderA ? "_a" : "_b";
      break;
    case Partition::kZirconA:
      part_name = "boot_a";
      break;
    case Partition::kZirconB:
      part_name = "boot_b";
      break;
    case Partition::kVbMetaA:
      part_name = "vbmeta_a";
      break;
    case Partition::kVbMetaB:
      part_name = "vbmeta_b";
      break;
    case Partition::kAbrMeta:
      part_name = "devinfo";
      break;
    case Partition::kFuchsiaVolumeManager:
      part_name = "super";
      break;
    default:
      ERROR("Iris partitioner cannot find unknown partition type\n");
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // First check the GPT, but this currently only serves the partitions that exist in the first
  // GPT (LUN0). These partitions are all exposed by default.
  if (auto client = gpt->gpt->FindPartition(
          [&part_name](const GptPartitionMetadata& part) { return FilterByName(part, part_name); });
      client.is_ok()) {
    return client;
  }

  // Next check partitions that have been exposed as block devices (LUN1-3). These partitions
  // must be explicitly declared in the build file `fuchsia_board_configuration.filesystems` in
  // order to be visible to the paver.
  if (auto client = OpenPartitionFromBlockDevices(part_name); client.is_ok()) {
    return client;
  }

  return zx::error(ZX_ERR_NOT_FOUND);
}

zx::result<std::unique_ptr<PartitionClient>> IrisPartitioner::OpenPartitionFromBlockDevices(
    std::string_view name) const {
  // TODO(b/512994030): Switch to an explicit synchronous API.
  auto part_connector = OpenBlockPartition(devices_, std::nullopt, std::nullopt, name, ZX_SEC(5));
  if (part_connector.is_error()) {
    ERROR("Failed to open block partition %.*s via OpenBlockPartition: %s\n",
          static_cast<int>(name.size()), name.data(), part_connector.status_string());
    return part_connector.take_error();
  }
  auto client = BlockPartitionClient::Create(std::move(part_connector.value()));
  if (client.is_error()) {
    ERROR("Failed to create BlockPartitionClient %.*s via OpenBlockPartition: %s\n",
          static_cast<int>(name.size()), name.data(), client.status_string());
    return client.take_error();
  }
  LOG("Successfully found %.*s on raw block device via OpenBlockPartition\n",
      static_cast<int>(name.size()), name.data());
  return zx::ok(std::move(client.value()));
}

zx::result<> IrisPartitioner::ResetPartitionTables() const {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> IrisPartitioner::ValidatePayload(const PartitionSpec& spec,
                                              std::span<const uint8_t> data) const {
  if (!SupportsPartition(spec)) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok();
}

const paver::BlockDevices& IrisPartitioner::Devices() const { return devices_; }

fidl::UnownedClientEnd<fuchsia_io::Directory> IrisPartitioner::SvcRoot() const {
  return svc_root_.borrow();
}

zx::result<std::unique_ptr<DevicePartitioner>> IrisPartitionerFactory::New(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config, std::shared_ptr<Context> context) {
  return IrisPartitioner::Initialize(devices, svc_root, config);
}

namespace {

using uuid::Uuid;

class IrisAbrClient : public abr::Client {
 public:
  static zx::result<std::unique_ptr<abr::Client>> Create(
      std::unique_ptr<paver::PartitionClient> partition,
      fidl::ClientEnd<fuchsia_io::Directory> svc_root) {
    zx::vmo vmo;
    if (auto status = zx::make_result(zx::vmo::create(kIrisDevinfoSize, 0, &vmo));
        status.is_error()) {
      ERROR("Failed to create vmo\n");
      return status.take_error();
    }
    return zx::ok(new IrisAbrClient(std::move(partition), std::move(vmo), std::move(svc_root)));
  }

 private:
  IrisAbrClient(std::unique_ptr<paver::PartitionClient> partition, zx::vmo vmo,
                fidl::ClientEnd<fuchsia_io::Directory> svc_root)
      : Client(/*custom = */ true),
        partition_(std::move(partition)),
        vmo_(std::move(vmo)),
        svc_root_(std::move(svc_root)) {}

  zx::result<> Read(uint8_t* buffer, size_t size) override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  zx::result<> Write(const uint8_t* buffer, size_t size) override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  iris_devinfo_ab_slot_data_t ToIris(const AbrSlotData& src, bool active,
                                     const iris_devinfo_ab_slot_data_t& disk) {
    iris_devinfo_ab_slot_data_t slot_metadata = disk;
    slot_metadata.set_successful(src.successful_boot);
    // ABR retry count is only in effect when not successful otherwise it's cleared to 0.
    if (!slot_metadata.successful()) {
      slot_metadata.retry_count = std::min<uint8_t>(src.tries_remaining, 3);
    }
    slot_metadata.set_unbootable(AbrGetNormalizedPriority(&src) == 0 ? 1 : 0);
    slot_metadata.set_active(active ? 1 : 0);
    return slot_metadata;
  }

  AbrSlotData ToFuchsia(const iris_devinfo_ab_slot_data_t& slot_metadata) {
    AbrSlotData abr_slot_data = {};
    abr_slot_data.successful_boot = slot_metadata.successful();
    abr_slot_data.tries_remaining = slot_metadata.retry_count;
    abr_slot_data.priority = slot_metadata.active() ? 2 : 1;
    if (abr_slot_data.successful_boot) {
      // ABR requires that successful boot must have a retry of 0.
      abr_slot_data.tries_remaining = 0;
    } else if (slot_metadata.unbootable()) {
      abr_slot_data.priority = 0;
    }
    return abr_slot_data;
  }

  zx::result<> ReadCustom(AbrSlotData* a, AbrSlotData* b, uint8_t* one_shot_recovery) override {
    auto devinfo_res = ReadDevinfoAb();
    if (devinfo_res.is_error()) {
      return devinfo_res.take_error();
    }
    *a = ToFuchsia(devinfo_res->slots[0]);
    *b = ToFuchsia(devinfo_res->slots[1]);
    *one_shot_recovery = 0;
    return zx::ok();
  }

  zx::result<> WriteCustom(const AbrSlotData* a, const AbrSlotData* b,
                           uint8_t one_shot_recovery) override {
    auto devinfo_res = ReadDevinfoAb();
    if (devinfo_res.is_error()) {
      return devinfo_res.take_error();
    }
    iris_devinfo_ab_data_t ab_data = *devinfo_res;

    bool b_active = AbrGetActiveSlotFromData(a, b) == kAbrSlotIndexB;
    // In case of both slot unbootable, default to active a slot.
    bool a_active = !b_active;
    ab_data.slots[0] = ToIris(*a, a_active, ab_data.slots[0]);
    ab_data.slots[1] = ToIris(*b, b_active, ab_data.slots[1]);

    if (auto status =
            SetActiveSlotInUfs(svc_root_.borrow(), b_active ? kAbrSlotIndexB : kAbrSlotIndexA);
        status.is_error()) {
      ERROR("Failed to set active slot in UFS attributes\n");
      return status.take_error();
    }

    // Not supported on Iris
    (void)one_shot_recovery;

    if (auto status =
            zx::make_result(vmo_.write(&ab_data, kIrisAbrMetadataOffset, sizeof(ab_data)));
        status.is_error()) {
      ERROR("Failed to write ab_data to vmo\n");
      return status;
    }

    if (auto status = partition_->Write(vmo_, kIrisDevinfoSize); status.is_error()) {
      ERROR("Failed to write to partition\n");
      return status.take_error();
    }
    return partition_->Flush();
  }

  zx::result<iris_devinfo_ab_data_t> ReadDevinfoAb() const {
    if (auto status = partition_->Read(vmo_, kIrisDevinfoSize); status.is_error()) {
      ERROR("Failed to read from partition\n");
      return status.take_error();
    }

    uint32_t magic = 0;
    if (auto status = zx::make_result(vmo_.read(&magic, 0, sizeof(magic))); status.is_error()) {
      ERROR("Failed to read magic from vmo\n");
      return status.take_error();
    }

    // TODO(b/512994030): Remove this check once we have a way to initialize the devinfo partition.
    if (magic != IRIS_DEVINFO_MAGIC) {
      ERROR("Invalid devinfo magic: 0x%08x\n", magic);
      return zx::error(ZX_ERR_BAD_STATE);
    }

    iris_devinfo_ab_data_t ab_data;
    if (auto status = zx::make_result(vmo_.read(&ab_data, kIrisAbrMetadataOffset, sizeof(ab_data)));
        status.is_error()) {
      ERROR("Failed to read ab_data from vmo\n");
      return status.take_error();
    }

    return zx::ok(ab_data);
  }

  zx::result<> Flush() override { return zx::ok(); }

  std::unique_ptr<paver::PartitionClient> partition_;
  zx::vmo vmo_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;
};

}  // namespace

zx::result<std::unique_ptr<abr::Client>> IrisPartitioner::CreateAbrClient() const {
  zx::result partition = FindPartition(paver::PartitionSpec(paver::Partition::kAbrMeta));
  if (partition.is_error()) {
    ERROR("Failed to find abr partition\n");
    return partition.take_error();
  }

  return IrisAbrClient::Create(std::move(partition.value()), component::MaybeClone(SvcRoot()));
}

}  // namespace paver
