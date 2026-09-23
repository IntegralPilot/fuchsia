// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/paver.h"

#include <dirent.h>
#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <lib/abr/data.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/epitaph.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <libgen.h>
#include <stddef.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstdarg>
#include <memory>
#include <optional>
#include <string_view>
#include <utility>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/unique_fd.h>

#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/firmware/paver/sparse.h"
#include "src/firmware/paver/sysconfig-fidl.h"
#include "src/firmware/paver/validation.h"
#include "src/storage/lib/buffer/owned_vmoid.h"
#include "sysconfig-fidl.h"

namespace paver {
namespace {

using fuchsia_paver::wire::Asset;
using fuchsia_paver::wire::Configuration;
using fuchsia_paver::wire::ConfigurationStatus;
using fuchsia_paver::wire::kMaxPendingBootAttempts;
using fuchsia_paver::wire::UnbootableReason;
using fuchsia_paver::wire::WriteFirmwareResult;

Partition PartitionType(Configuration configuration, Asset asset) {
  switch (asset) {
    case Asset::kKernel: {
      switch (configuration) {
        case Configuration::kA:
          return Partition::kZirconA;
        case Configuration::kB:
          return Partition::kZirconB;
        case Configuration::kRecovery:
          return Partition::kZirconR;
      };
      break;
    }
    case Asset::kVerifiedBootMetadata: {
      switch (configuration) {
        case Configuration::kA:
          return Partition::kVbMetaA;
        case Configuration::kB:
          return Partition::kVbMetaB;
        case Configuration::kRecovery:
          return Partition::kVbMetaR;
      };
      break;
    }
  };
  return Partition::kUnknown;
}

// Best effort attempt to see if payload contents match what is already inside
// of the partition.
bool CheckIfSame(PartitionClient* partition, const zx::vmo& vmo, size_t payload_size,
                 size_t block_size) {
  const size_t payload_size_aligned = fbl::round_up(payload_size, block_size);
  zx::vmo read_vmo;
  auto status =
      zx::vmo::create(fbl::round_up(payload_size_aligned, zx_system_get_page_size()), 0, &read_vmo);
  if (status != ZX_OK) {
    ERROR("Failed to create VMO: %s\n", zx_status_get_string(status));
    return false;
  }

  if (auto status = partition->Read(read_vmo, payload_size_aligned); status.is_error()) {
    return false;
  }

  fzl::VmoMapper first_mapper;
  fzl::VmoMapper second_mapper;

  // The payload VMO can be pager-backed, mapping which requires ZX_VM_ALLOW_FAULTS. Otherwise, the
  // ZX_VM_ALLOW_FAULTS flag is a no-op.
  status = first_mapper.Map(vmo, 0, 0, ZX_VM_PERM_READ | ZX_VM_ALLOW_FAULTS);
  if (status != ZX_OK) {
    ERROR("Error mapping vmo: %s\n", zx_status_get_string(status));
    return false;
  }

  status = second_mapper.Map(read_vmo, 0, 0, ZX_VM_PERM_READ);
  if (status != ZX_OK) {
    ERROR("Error mapping vmo: %s\n", zx_status_get_string(status));
    return false;
  }
  return memcmp(first_mapper.start(), second_mapper.start(), payload_size) == 0;
}

// Reads an image from disk into a vmo.
zx::result<fuchsia_mem::wire::Buffer> PartitionRead(const DevicePartitioner& partitioner,
                                                    const PartitionSpec& spec) {
  LOG("Reading partition \"%s\".\n", spec.ToString().c_str());

  auto status = partitioner.FindPartition(spec);
  if (status.is_error()) {
    ERROR("Could not find \"%s\" Partition on device: %s\n", spec.ToString().c_str(),
          status.status_string());
    return status.take_error();
  }
  std::unique_ptr<PartitionClient>& partition = status.value();

  auto status2 = partition->GetPartitionSize();
  if (status2.is_error()) {
    ERROR("Error getting partition \"%s\" size: %s\n", spec.ToString().c_str(),
          status2.status_string());
    return status2.take_error();
  }
  const uint64_t partition_size = status2.value();

  zx::vmo vmo;
  if (auto status = zx::make_result(
          zx::vmo::create(fbl::round_up(partition_size, zx_system_get_page_size()), 0, &vmo));
      status.is_error()) {
    ERROR("Error creating vmo for \"%s\": %s\n", spec.ToString().c_str(), status.status_string());
    return status.take_error();
  }

  if (auto status = partition->Read(vmo, static_cast<size_t>(partition_size)); status.is_error()) {
    ERROR("Error writing partition data for \"%s\": %s\n", spec.ToString().c_str(),
          status.status_string());
    return status.take_error();
  }

  size_t asset_size = static_cast<size_t>(partition_size);
  // Try to find ZBI size if asset is a ZBI. This won't work on signed ZBI, nor vbmeta assets.
  fzl::VmoMapper mapper;
  if (zx::make_result(mapper.Map(vmo, 0, partition_size, ZX_VM_PERM_READ)).is_ok()) {
    auto data = std::span(static_cast<uint8_t*>(mapper.start()), mapper.size());
    const zbi_header_t* container_header;
    std::span<const uint8_t> container_data;
    if (ExtractZbiPayload(data, &container_header, &container_data)) {
      asset_size = sizeof(*container_header) + container_data.size();
    }
  }

  LOG("Completed successfully\n");
  return zx::ok(fuchsia_mem::wire::Buffer{std::move(vmo), asset_size});
}

zx::result<> ValidatePartitionPayload(const DevicePartitioner& partitioner,
                                      const zx::vmo& payload_vmo, size_t payload_size,
                                      const PartitionSpec& spec, bool sparse) {
  fzl::VmoMapper payload_mapper;
  // The payload VMO can be pager-backed, mapping which requires ZX_VM_ALLOW_FAULTS. Otherwise, the
  // ZX_VM_ALLOW_FAULTS flag is a no-op.
  auto status =
      zx::make_result(payload_mapper.Map(payload_vmo, 0, 0, ZX_VM_PERM_READ | ZX_VM_ALLOW_FAULTS));
  if (status.is_error()) {
    ERROR("Could not map payload into memory: %s\n", status.status_string());
    return status.take_error();
  }
  ZX_ASSERT(payload_mapper.size() >= payload_size);

  // Pass an empty payload for the sparse image; if any of the validators need to look at contents,
  // they will simply fail.
  // At this time none of them do so in a context where the sparse format is used.
  auto payload = sparse ? std::span<const uint8_t>()
                        : std::span<const uint8_t>(
                              static_cast<const uint8_t*>(payload_mapper.start()), payload_size);
  return partitioner.ValidatePayload(spec, payload);
}

zx::result<> WriteOpaque(PartitionClient& partition, const PartitionSpec& spec, zx::vmo payload_vmo,
                         size_t payload_size) {
  zx::result block_size = partition.GetBlockSize();
  if (block_size.is_error()) {
    ERROR("Couldn't get partition \"%s\" block size\n", spec.ToString().c_str());
    return block_size.take_error();
  }
  const size_t block_size_bytes = block_size.value();

  if (CheckIfSame(&partition, payload_vmo, payload_size, block_size_bytes)) {
    LOG("Skipping write as partition \"%s\" contents match payload.\n", spec.ToString().c_str());
    return zx::ok();
  }

  // Pad payload with 0s to make it block size aligned.
  if (payload_size % block_size_bytes != 0) {
    const size_t remaining_bytes = block_size_bytes - (payload_size % block_size_bytes);
    size_t vmo_size;
    if (zx::result status = zx::make_result(payload_vmo.get_size(&vmo_size)); status.is_error()) {
      ERROR("Couldn't get vmo size for \"%s\"\n", spec.ToString().c_str());
      return status.take_error();
    }
    // Grow VMO if it's too small.
    if (vmo_size < payload_size + remaining_bytes) {
      const size_t new_size =
          fbl::round_up(payload_size + remaining_bytes, zx_system_get_page_size());
      // First attempt to just resize the payload VMO.
      zx::result status = zx::make_result(payload_vmo.set_size(new_size));
      if (status.is_error()) {
        // If the resize failed then try and create a clone instead.
        zx::vmo clone;
        if (status = zx::make_result(payload_vmo.create_child(
                ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, new_size, &clone));
            status.is_error()) {
          ERROR("Couldn't create clone for \"%s\"\n", spec.ToString().c_str());
          return status.take_error();
        }
        payload_vmo = std::move(clone);
      }
    }
    auto buffer = std::make_unique<uint8_t[]>(remaining_bytes);
    memset(buffer.get(), 0, remaining_bytes);
    zx::result status =
        zx::make_result(payload_vmo.write(buffer.get(), payload_size, remaining_bytes));
    if (status.is_error()) {
      ERROR("Failed to write padding to vmo for \"%s\"\n", spec.ToString().c_str());
      return status.take_error();
    }
    payload_size += remaining_bytes;
  }

  if (zx::result status = partition.Write(payload_vmo, payload_size); status.is_error()) {
    if (status.error_value() == ZX_ERR_OUT_OF_RANGE) {
      ERROR("Write failed with ZX_ERR_OUT_OF_RANGE: image likely exceeds partition size.\n");
    }
    ERROR("Error writing partition \"%s\" data: %s\n", spec.ToString().c_str(),
          status.status_string());
    return status.take_error();
  }
  return zx::ok();
}

// Paves an image onto the disk.  If `sparse` is set, the image is treated as an Android Sparse
// image and unpacked.
zx::result<> PartitionPave(const DevicePartitioner& partitioner, zx::vmo payload_vmo,
                           size_t payload_size, const PartitionSpec& spec, bool sparse) {
  if (sparse) {
    LOG("Paving sparse partition \"%s\".\n", spec.ToString().c_str());
  } else {
    LOG("Paving partition \"%s\".\n", spec.ToString().c_str());
  }

  // The payload_vmo might be pager-backed. Commit its pages first before using it for
  // block writes below, to avoid deadlocks in the block server. If all the pages of the
  // payload_vmo are not in memory, the block server might see a read fault in the midst of a write.
  // Read faults need to be fulfilled by the block server itself, so it will deadlock.
  //
  // Note that these pages would be committed anyway when the block server pins them for the write.
  // We're simply committing a little early here.
  //
  // If payload_vmo is pager-backed, committing its pages guarantees that they will remain in memory
  // and not be evicted only if it's a clone of a pager-backed VMO, and not a root pager-backed VMO
  // (directly backed by a pager source). To ensure this is the case we create our own local clone
  // of the payload_vmo, guaranteeing it is not the root, or a reference to the root.
  zx_info_vmo_t info;
  zx::result status =
      zx::make_result(payload_vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  if (status.is_error()) {
    ERROR("Failed to get info for payload VMO for partition \"%s\": %s\n", spec.ToString().c_str(),
          status.status_string());
    return status.take_error();
  }
  // If payload_vmo is pager-backed then create a clone.
  if (info.flags & ZX_INFO_VMO_PAGER_BACKED) {
    zx::vmo clone;
    status = zx::make_result(
        payload_vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, payload_size, &clone));
    if (status.is_error()) {
      ERROR("Failed to create clone of paylod VMO for partition \"%s\": %s\n",
            spec.ToString().c_str(), status.status_string());
      return status.take_error();
    }
    payload_vmo = std::move(clone);
  }

  status = zx::make_result(payload_vmo.op_range(ZX_VMO_OP_COMMIT, 0, payload_size, nullptr, 0));
  if (status.is_error()) {
    ERROR("Failed to commit payload VMO for partition \"%s\": %s\n", spec.ToString().c_str(),
          status.status_string());
    return status.take_error();
  }

  // Perform basic safety checking on the partition before we attempt to write it.
  status = ValidatePartitionPayload(partitioner, payload_vmo, payload_size, spec, sparse);
  if (status.is_error()) {
    ERROR("Failed to validate partition \"%s\": %s\n", spec.ToString().c_str(),
          status.status_string());
    return status.take_error();
  }

  // Find or create the appropriate partition.
  std::unique_ptr<PartitionClient> partition;
  if (auto status = partitioner.FindPartition(spec); status.is_ok()) {
    LOG("Partition \"%s\" already exists\n", spec.ToString().c_str());
    partition = std::move(status.value());
  } else {
    if (status.error_value() != ZX_ERR_NOT_FOUND) {
      ERROR("Failure looking for partition \"%s\": %s\n", spec.ToString().c_str(),
            status.status_string());
      return status.take_error();
    }
    ERROR("Could not find \"%s\" Partition on device.  The device may need to be re-initialized.\n",
          spec.ToString().c_str());
    return status.take_error();
  }

  status = sparse ? WriteSparse(*partition, spec, std::move(payload_vmo), payload_size)
                  : WriteOpaque(*partition, spec, std::move(payload_vmo), payload_size);
  if (status.is_error()) {
    return status.take_error();
  }

  LOG("Completed paving partition \"%s\" successfully\n", spec.ToString().c_str());
  return zx::ok();
}

Configuration SlotIndexToConfiguration(AbrSlotIndex slot_index) {
  switch (slot_index) {
    case kAbrSlotIndexA:
      return Configuration::kA;
    case kAbrSlotIndexB:
      return Configuration::kB;
    case kAbrSlotIndexR:
      return Configuration::kRecovery;
  }
  ERROR("Unknown Abr slot index %d\n", static_cast<int>(slot_index));
  ZX_ASSERT(false);  // Unreachable
}

std::optional<AbrSlotIndex> ConfigurationToSlotIndex(Configuration config) {
  switch (config) {
    case Configuration::kA:
      return kAbrSlotIndexA;
    case Configuration::kB:
      return kAbrSlotIndexB;
    case Configuration::kRecovery:
      return kAbrSlotIndexR;
  }
  ERROR("Unknown configuration %d\n", static_cast<int>(config));
  return std::nullopt;
}

std::optional<Configuration> GetActiveConfiguration(const abr::Client& abr_client) {
  auto slot_index = abr_client.GetBootSlot(false, nullptr);
  if (slot_index == kAbrSlotIndexR) {
    return std::nullopt;
  }
  return SlotIndexToConfiguration(slot_index);
}

// Helper to wrap a std::variant with a WriteFirmwareResult union.
//
// This can go away once llcpp unions support owning memory, but until then we
// need the variant to own the underlying data.
//
// |variant| must outlive the returned WriteFirmwareResult.
WriteFirmwareResult CreateWriteFirmwareResult(std::variant<zx_status_t, bool>* variant) {
  if (std::holds_alternative<zx_status_t>(*variant)) {
    return WriteFirmwareResult::WithStatus(std::get<zx_status_t>(*variant));
  }
  return WriteFirmwareResult::WithUnsupported(std::get<bool>(*variant));
}

}  // namespace

zx::result<std::unique_ptr<Paver>> Paver::Create(PaverConfig config, fbl::unique_fd devfs_root) {
  auto [svc, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  if (zx_status_t status =
          fdio_open3("/svc", static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                     server.TakeChannel().release());
      status != ZX_OK) {
    return zx::error(status);
  }
  zx::result devices = BlockDevices::CreateFromFshostBlockDir(std::move(devfs_root));
  if (devices.is_error()) {
    ERROR("Failed to open device source: %s\n", devices.status_string());
    return devices.take_error();
  }
  return zx::ok(std::make_unique<Paver>(config, std::move(*devices), std::move(svc)));
}

void Paver::FindDataSink(FindDataSinkRequestView request, FindDataSinkCompleter::Sync& _completer) {
  DataSink::Bind(dispatcher_, devices_.Duplicate(), svc_root_.borrow(), config_,
                 std::move(request->data_sink), context_);
}

void Paver::FindPartitionTableManager(FindPartitionTableManagerRequestView request,
                                      FindPartitionTableManagerCompleter::Sync& _completer) {
  DynamicDataSink::Bind(dispatcher_, devices_.Duplicate(), svc_root_.borrow(), config_,
                        std::move(request->data_sink), context_);
}

void Paver::FindBootManager(FindBootManagerRequestView request,
                            FindBootManagerCompleter::Sync& _completer) {
  BootManager::Bind(dispatcher_, devices_.Duplicate(), component::MaybeClone(svc_root_), config_,
                    context_, std::move(request->boot_manager));
}

void DataSink::ReadAsset(ReadAssetRequestView request, ReadAssetCompleter::Sync& completer) {
  auto status = sink_.ReadAsset(request->configuration, request->asset);
  if (status.is_ok()) {
    completer.ReplySuccess(std::move(status.value()));
  } else {
    completer.ReplyError(status.error_value());
  }
}

void DataSink::WriteFirmware(WriteFirmwareRequestView request,
                             WriteFirmwareCompleter::Sync& completer) {
  auto variant =
      sink_.WriteFirmware(request->configuration, request->type, std::move(request->payload));
  completer.Reply(CreateWriteFirmwareResult(&variant));
}

void DataSink::ReadFirmware(ReadFirmwareRequestView request,
                            ReadFirmwareCompleter::Sync& completer) {
  auto status = sink_.ReadFirmware(request->configuration, request->type);
  if (status.is_ok()) {
    completer.ReplySuccess(std::move(status.value()));
  } else {
    completer.ReplyError(status.error_value());
  }
}

zx::result<fuchsia_mem::wire::Buffer> DataSinkImpl::ReadAsset(Configuration configuration,
                                                              Asset asset) {
  // No assets support content types yet, use the PartitionSpec default.
  PartitionSpec spec(PartitionType(configuration, asset));

  // Important: if we ever do pass a content type here, do NOT just return
  // ZX_ERR_NOT_SUPPORTED directly - the caller needs to be able to distinguish
  // between unknown asset types (which should be ignored) and actual errors
  // that happen to return this same status code.
  if (!partitioner_->SupportsPartition(spec)) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  auto status = PartitionRead(*partitioner_, spec);
  if (status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(status.value()));
}

zx::result<> DataSinkImpl::WriteOpaqueVolume(fuchsia_mem::wire::Buffer payload) {
  PartitionSpec spec(Partition::kFuchsiaVolumeManager, kOpaqueVolumeContentType);
  if (!partitioner_->SupportsPartition(spec)) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  return PartitionPave(*partitioner_, std::move(payload.vmo), payload.size, spec, false);
}

zx::result<> DataSinkImpl::WriteSparseVolume(fuchsia_mem::wire::Buffer payload) {
  PartitionSpec spec(Partition::kFuchsiaVolumeManager, kOpaqueVolumeContentType);
  if (!partitioner_->SupportsPartition(spec)) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  return PartitionPave(*partitioner_, std::move(payload.vmo), payload.size, spec, true);
}

zx::result<> DataSinkImpl::WriteAsset(Configuration configuration, Asset asset,
                                      fuchsia_mem::wire::Buffer payload) {
  // No assets support content types yet, use the PartitionSpec default.
  PartitionSpec spec(PartitionType(configuration, asset));

  // Important: if we ever do pass a content type here, do NOT just return
  // ZX_ERR_NOT_SUPPORTED directly - the caller needs to be able to distinguish
  // between unknown asset types (which should be ignored) and actual errors
  // that happen to return this same status code.
  if (!partitioner_->SupportsPartition(spec)) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  return PartitionPave(*partitioner_, std::move(payload.vmo), payload.size, spec, false);
}

std::optional<PartitionSpec> DataSinkImpl::GetFirmwarePartitionSpec(Configuration configuration,
                                                                    fidl::StringView type) {
  // Currently all our supported firmware lives in Partition::kBootloaderA/B/R.
  Partition part_type;
  switch (configuration) {
    case Configuration::kA:
      part_type = Partition::kBootloaderA;
      break;
    case Configuration::kB:
      part_type = Partition::kBootloaderB;
      break;
    case Configuration::kRecovery:
      part_type = Partition::kBootloaderR;
      break;
  }
  PartitionSpec spec = PartitionSpec(part_type, std::string_view(type.data(), type.size()));

  bool supported = partitioner_->SupportsPartition(spec);
  if (!supported && part_type == Partition::kBootloaderB) {
    // It's possible that the device does not support bootloader A/B. In this case,
    // try writing to configuration A, which is always supported for some expected firmware
    // type.
    LOG("Device may not support firmware A/B. Attempt to write to slot A\n")
    spec.partition = Partition::kBootloaderA;
    supported = partitioner_->SupportsPartition(spec);
  }

  return supported ? std::optional{spec} : std::nullopt;
}

std::variant<zx_status_t, bool> DataSinkImpl::WriteFirmware(Configuration configuration,
                                                            fidl::StringView type,
                                                            fuchsia_mem::wire::Buffer payload) {
  std::optional<PartitionSpec> spec = GetFirmwarePartitionSpec(configuration, type);
  if (spec) {
    return PartitionPave(*partitioner_, std::move(payload.vmo), payload.size, *spec, false)
        .status_value();
  }

  // unsupported_type = true.
  return true;
}

zx::result<fuchsia_mem::wire::Buffer> DataSinkImpl::ReadFirmware(Configuration configuration,
                                                                 fidl::StringView type) {
  std::optional<PartitionSpec> spec = GetFirmwarePartitionSpec(configuration, type);
  if (spec) {
    auto status = PartitionRead(*partitioner_, *spec);
    if (status.is_error()) {
      return status.take_error();
    }
    return zx::ok(std::move(status.value()));
  }
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void DataSink::Bind(async_dispatcher_t* dispatcher, BlockDevices devices,
                    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                    const PaverConfig& config, fidl::ServerEnd<fuchsia_paver::DataSink> server,
                    std::shared_ptr<Context> context) {
  zx::result partitioner =
      DevicePartitionerFactory::Create(devices, svc_root, config, std::move(context));
  if (partitioner.is_error()) {
    ERROR("Unable to initialize a partitioner: %s.\n", partitioner.status_string());
    fidl_epitaph_write(server.channel().get(), ZX_ERR_BAD_STATE);
    return;
  }
  std::unique_ptr data_sink =
      std::make_unique<DataSink>(std::move(devices), std::move(partitioner.value()));
  fidl::BindServer(dispatcher, std::move(server), std::move(data_sink));
}

void DynamicDataSink::Bind(async_dispatcher_t* dispatcher, BlockDevices devices,
                           fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                           const PaverConfig& config,
                           fidl::ServerEnd<fuchsia_paver::DynamicDataSink> server,
                           std::shared_ptr<Context> context) {
  zx::result partitioner =
      DevicePartitionerFactory::Create(devices, svc_root, config, std::move(context));
  if (partitioner.is_error()) {
    ERROR("Unable to initialize a partitioner: %s.\n", partitioner.status_string());
    fidl_epitaph_write(server.channel().get(), ZX_ERR_BAD_STATE);
    return;
  }
  std::unique_ptr data_sink =
      std::make_unique<DynamicDataSink>(std::move(devices), std::move(partitioner.value()));
  fidl::BindServer(dispatcher, std::move(server), std::move(data_sink));
}

void DynamicDataSink::InitializePartitionTables(
    InitializePartitionTablesCompleter::Sync& completer) {
  completer.Reply(sink_.partitioner()->ResetPartitionTables().status_value());
}

void DynamicDataSink::WipePartitionTables(WipePartitionTablesCompleter::Sync& completer) {
  completer.Reply(ZX_ERR_NOT_SUPPORTED);
}

void DynamicDataSink::ReadAsset(ReadAssetRequestView request, ReadAssetCompleter::Sync& completer) {
  auto status = sink_.ReadAsset(request->configuration, request->asset);
  if (status.is_ok()) {
    completer.ReplySuccess(std::move(status.value()));
  } else {
    completer.ReplyError(status.error_value());
  }
}

void DynamicDataSink::WriteFirmware(WriteFirmwareRequestView request,
                                    WriteFirmwareCompleter::Sync& completer) {
  auto variant =
      sink_.WriteFirmware(request->configuration, request->type, std::move(request->payload));
  completer.Reply(CreateWriteFirmwareResult(&variant));
}

void DynamicDataSink::ReadFirmware(ReadFirmwareRequestView request,
                                   ReadFirmwareCompleter::Sync& completer) {
  auto status = sink_.ReadFirmware(request->configuration, request->type);
  if (status.is_ok()) {
    completer.ReplySuccess(std::move(status.value()));
  } else {
    completer.ReplyError(status.error_value());
  }
}

void BootManager::Bind(async_dispatcher_t* dispatcher, BlockDevices devices,
                       fidl::ClientEnd<fuchsia_io::Directory> svc_root, const PaverConfig& config,
                       std::shared_ptr<Context> context,
                       fidl::ServerEnd<fuchsia_paver::BootManager> server) {
  zx::result supports_abr = abr::SupportsVerifiedBoot(devices, config);
  if (supports_abr.is_error()) {
    ERROR("Failed to check if system supports verified boot: %s\n", supports_abr.status_string());
    fidl_epitaph_write(server.channel().get(), supports_abr.error_value());
    return;
  } else if (!*supports_abr) {
    LOG("System doesn't support verified boot; not creating BootManager\n");
    fidl_epitaph_write(server.channel().get(), ZX_ERR_NOT_SUPPORTED);
    return;
  }

  zx::result partitioner =
      DevicePartitionerFactory::Create(devices, svc_root, config, std::move(context));
  if (partitioner.is_error()) {
    ERROR("Unable to initialize a partitioner: %s.\n", partitioner.status_string());
    fidl_epitaph_write(server.channel().get(), partitioner.error_value());
    return;
  }
  zx::result abr = partitioner->CreateAbrClient();
  if (abr.is_error()) {
    ERROR("Failed to get ABR client: %s\n", abr.status_string());
    fidl_epitaph_write(server.channel().get(), abr.error_value());
    return;
  }

  auto boot_manager =
      std::make_unique<BootManager>(std::move(*partitioner), std::move(*abr), config);
  fidl::BindServer(dispatcher, std::move(server), std::move(boot_manager));
}

void BootManager::QueryCurrentConfiguration(QueryCurrentConfigurationCompleter::Sync& completer) {
  zx::result<Configuration> status = abr::QueryBootConfig(partitioner_->Devices(), config_);
  if (status.is_error()) {
    completer.ReplyError(status.status_value());
    return;
  }
  completer.ReplySuccess(status.value());
}

void BootManager::QueryActiveConfiguration(QueryActiveConfigurationCompleter::Sync& completer) {
  std::optional<Configuration> config = GetActiveConfiguration(*abr_client_);
  if (!config) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  completer.ReplySuccess(config.value());
}

void BootManager::QueryConfigurationLastSetActive(
    QueryConfigurationLastSetActiveCompleter::Sync& completer) {
  auto status = abr_client_->GetSlotLastMarkedActive();
  if (status.is_error()) {
    ERROR("Failed to get slot most recently marked active\n");
    completer.ReplyError(status.error_value());
    return;
  }

  completer.ReplySuccess(SlotIndexToConfiguration(status.value()));
}

// libabr is primarily a firmware library, so generally reports the *next* boot attempt state rather
// than the current boot attempt. Usually this doesn't matter, but on the final boot attempt libabr
// has already set the attempts remaining to 0, which makes the slot look like it's unbootable when
// in fact it's still pending.
//
// This function detects that case, returning true if |configuration| is currently running its final
// boot attempt.
bool BootManager::IsFinalBootAttempt(const AbrSlotInfo& slot_info, Configuration configuration) {
  // This is the final boot attempt if:
  //   1. |slot_index| is our currently booted slot
  //   2. The slot reports unbootable
  //   3. The unbootable reason is "no more tries" (or "none" for backwards-compatibility with
  //      older bootloaders)
  //
  // This works because we trust the bootloader to respect the A/B/R metadata, so we never would
  // have gotten into this slot unless it was bootable at the time. So it either:
  //   * was set unbootable by the bootloader just before boot because it's the final attempt
  //   * has been set unbootable during this attempt via SetConfigurationUnbootable()
  // and we can check for the first by filtering the reboot reason.
  //
  // This cannot be done entirely in libabr because the current boot slot is not always possible to
  // determine based on the A/B/R metadata, so we have to do it in the paver where we do know the
  // current slot.

  // Check the slot info first because it's the quickest, since all the data is local.
  if (slot_info.is_bootable || (slot_info.unbootable_reason != kAbrUnbootableReasonNoMoreTries &&
                                slot_info.unbootable_reason != kAbrUnbootableReasonNone)) {
    return false;
  }

  // If the slot in question is our current boot slot, we're on our last attempt.
  zx::result<Configuration> current_configuration =
      abr::QueryBootConfig(partitioner_->Devices(), config_);
  return current_configuration.is_ok() && *current_configuration == configuration;
}

// Returns the status for the given `configuration`.
//
// Returns a pair containing:
//   1. The `ConfigurationStatus`
//   2. If status is pending, the number of boots attempted; otherwise, nullopt.
zx::result<BootManager::ConfigurationState> BootManager::GetConfigurationState(
    fuchsia_paver::wire::Configuration configuration) {
  auto slot_index = ConfigurationToSlotIndex(configuration);
  if (!slot_index) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto slot_info = abr_client_->GetSlotInfo(*slot_index);
  if (slot_info.is_error()) {
    ERROR("Failed to get slot info %d\n", static_cast<uint32_t>(configuration));
    return slot_info.take_error();
  }

  if (!slot_info->is_bootable) {
    // If this is the final boot attempt, we aren't actually unbootable yet.
    if (IsFinalBootAttempt(*slot_info, configuration)) {
      return zx::ok(ConfigurationState{.status = ConfigurationStatus::kPending,
                                       .boot_attempts = kMaxPendingBootAttempts,
                                       .unbootable_reason = std::nullopt});
    }
    return zx::ok(
        ConfigurationState{.status = ConfigurationStatus::kUnbootable,
                           .boot_attempts = std::nullopt,
                           .unbootable_reason = UnbootableReason(slot_info->unbootable_reason)});
  }

  if (slot_info->is_marked_successful) {
    return zx::ok(ConfigurationState{.status = ConfigurationStatus::kHealthy,
                                     .boot_attempts = std::nullopt,
                                     .unbootable_reason = std::nullopt});
  }

  // Bootable but not successful = pending, and we also provide boot attempts.
  static_assert(kAbrMaxTriesRemaining == kMaxPendingBootAttempts,
                "Paver max attempts constant does not match libabr");
  if (slot_info->num_tries_remaining > kMaxPendingBootAttempts) {
    // Error in libabr; num_tries_remaining should never exceed the max attempts.
    ERROR("Invalid slot %d num_tries_remaining: %u\n", configuration,
          slot_info->num_tries_remaining);
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok(
      ConfigurationState{.status = ConfigurationStatus::kPending,
                         .boot_attempts = kMaxPendingBootAttempts - slot_info->num_tries_remaining,
                         .unbootable_reason = std::nullopt});
}

void BootManager::QueryConfigurationStatus(QueryConfigurationStatusRequestView request,
                                           QueryConfigurationStatusCompleter::Sync& completer) {
  auto result = GetConfigurationState(request->configuration);

  if (result.is_error()) {
    completer.ReplyError(result.error_value());
  } else {
    completer.ReplySuccess(result->status);
  }
}

void BootManager::QueryConfigurationStatusAndBootAttempts(
    QueryConfigurationStatusAndBootAttemptsRequestView request,
    QueryConfigurationStatusAndBootAttemptsCompleter::Sync& completer) {
  auto result = GetConfigurationState(request->configuration);

  if (result.is_error()) {
    completer.ReplyError(result.error_value());
  } else {
    fidl::Arena arena;
    auto builder =
        fuchsia_paver::wire::BootManagerQueryConfigurationStatusAndBootAttemptsResponse::Builder(
            arena);

    builder.status(result->status);
    if (result->boot_attempts) {
      builder.boot_attempts(*result->boot_attempts);
    }
    if (result->unbootable_reason) {
      builder.unbootable_reason(*result->unbootable_reason);
    }
    completer.ReplySuccess(builder.Build());
  }
}

void BootManager::SetConfigurationActive(SetConfigurationActiveRequestView request,
                                         SetConfigurationActiveCompleter::Sync& completer) {
  LOG("Setting configuration %d as active\n", static_cast<uint32_t>(request->configuration));

  auto slot_index = ConfigurationToSlotIndex(request->configuration);
  auto status =
      slot_index ? abr_client_->MarkSlotActive(*slot_index) : zx::error(ZX_ERR_INVALID_ARGS);
  if (status.is_error()) {
    ERROR("Failed to set configuration: %d active\n",
          static_cast<uint32_t>(request->configuration));
    completer.Reply(status.error_value());
    return;
  }

  LOG("Set active configuration to %d\n", static_cast<uint32_t>(request->configuration));

  completer.Reply(ZX_OK);
}

void BootManager::SetConfigurationUnbootable(SetConfigurationUnbootableRequestView request,
                                             SetConfigurationUnbootableCompleter::Sync& completer) {
  LOG("Setting configuration %d as unbootable\n", static_cast<uint32_t>(request->configuration));

  auto slot_index = ConfigurationToSlotIndex(request->configuration);
  auto status =
      slot_index ? abr_client_->MarkSlotUnbootable(*slot_index) : zx::error(ZX_ERR_INVALID_ARGS);
  if (status.is_error()) {
    ERROR("Failed to set configuration: %d unbootable\n",
          static_cast<uint32_t>(request->configuration));
    completer.Reply(status.error_value());
    return;
  }

  // This string is load-bearing in the firmware test suite, make sure they change together.
  LOG("Set %d configuration as unbootable\n", static_cast<uint32_t>(request->configuration));

  completer.Reply(ZX_OK);
}

void BootManager::SetConfigurationHealthy(SetConfigurationHealthyRequestView request,
                                          SetConfigurationHealthyCompleter::Sync& completer) {
  LOG("Setting configuration %d as healthy\n", static_cast<uint32_t>(request->configuration));

  auto slot_index = ConfigurationToSlotIndex(request->configuration);
  if (!slot_index) {
    ERROR("Invalid configuration %d\n", static_cast<uint32_t>(request->configuration));
    completer.Reply(ZX_ERR_INVALID_ARGS);
    return;
  }

  // Attempt to set it healthy normally first - this may fail if we're on the last boot attempt
  // and libabr thinks the slot is already unbootable, but in the common case this will work and
  // it's quick and harmless to attempt.
  auto status = abr_client_->MarkSlotSuccessful(*slot_index);
  if (status.is_error()) {
    // Failure is expected if we're on the final boot attempt; check if this was the case.
    auto slot_info = abr_client_->GetSlotInfo(*slot_index);
    if (slot_info.is_error()) {
      ERROR("Failed to query slot info (%d); assuming this is not the final boot attempt\n",
            slot_info.error_value());
    } else if (IsFinalBootAttempt(*slot_info, request->configuration)) {
      // Try again with |from_unbootable_ok| set. This must only be used when we are booting the
      // final attempt on a slot; it's unsafe otherwise and could lead to a bootloop.
      LOG("Last boot attempt; allowing unbootable -> success\n");
      status = abr_client_->MarkSlotSuccessful(*slot_index, true);
      if (status.is_error()) {
        ERROR("Failed to mark slot successful from last boot attempt (%d)\n", status.error_value());
      }
    }
  }

  if (status.is_ok()) {
    // This string is load-bearing in the firmware test suite, make sure they change together.
    LOG("Set configuration %d healthy\n", static_cast<uint32_t>(request->configuration));
    completer.Reply(ZX_OK);
  } else {
    ERROR("Failed to set configuration %d healthy (%d)\n",
          static_cast<uint32_t>(request->configuration), status.error_value());
    completer.Reply(status.error_value());
  }
}

void BootManager::SetOneShotRecovery(SetOneShotRecoveryCompleter::Sync& completer) {
  LOG("Setting one shot recovery\n");
  auto status = abr_client_->SetOneShotRecovery();
  if (status.is_error()) {
    ERROR("Failed to set one shot recovery: %s\n", status.status_string());
    completer.ReplyError(status.error_value());
    return;
  }
  LOG("Set one shot recovery succeed\n");
  completer.ReplySuccess();
}

void Paver::FindSysconfig(FindSysconfigRequestView request,
                          FindSysconfigCompleter::Sync& completer) {
  FindSysconfig(std::move(request->sysconfig));
}

void Paver::FindSysconfig(fidl::ServerEnd<fuchsia_paver::Sysconfig> sysconfig) {
  Sysconfig::Bind(dispatcher_, devices_, component::MaybeClone(svc_root_), config_, context_,
                  std::move(sysconfig));
}

void Paver::LifecycleStopCallback(fit::callback<void(zx_status_t status)> cb) {
  LOG("Lifecycle stop request received.");

  zx::result partitioner =
      DevicePartitionerFactory::Create(devices_, svc_root_.borrow(), config_, context_);
  if (partitioner.is_error()) {
    ERROR("Unable to initialize a partitioner: %s.\n", partitioner.status_string());
    cb(partitioner.status_value());
    return;
  }
  zx::result res = partitioner->OnStop();
  if (res.is_error()) {
    ERROR("Failed to process OnStop with partitioner.");
  }
  cb(ZX_OK);
}

}  // namespace paver
