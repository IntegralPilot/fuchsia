// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device-partitioner.h"

#include <lib/component/incoming/cpp/clone.h>
#include <lib/fdio/fd.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <algorithm>
#include <iterator>
#include <memory>
#include <optional>
#include <unordered_map>
#include <utility>

#include <fbl/string.h>
#include <fbl/string_printf.h>
#include <gpt/gpt.h>

#include "src/firmware/paver/config.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/firmware/paver/paver-context.h"
#include "src/firmware/paver/utils.h"
#include "src/lib/uuid/uuid.h"

namespace paver {

using uuid::Uuid;

namespace {

// Information for each Partition enum value.
struct PartitionInfo {
  // Default on-disk name and type GUID. Device aren't required to use these,
  // they may instead use the legacy values below or their own custom values.
  const char* name;
  Uuid type;

  // Legacy on-disk name and type GUID.
  const char* legacy_name;
  Uuid legacy_type;

  // Device-agnostic debug name. Something we can print to the logs that will
  // make sense for any device regardless of on-disk partition layout.
  const char* debug_name;
};

std::optional<PartitionInfo> GetPartitionInfo(Partition partition) {
  switch (partition) {
    case Partition::kBootloaderA:
      return PartitionInfo{.name = GPT_BOOTLOADER_A_NAME,
                           .type = GPT_BOOTLOADER_ABR_TYPE_GUID,
                           .legacy_name = GUID_EFI_NAME,
                           .legacy_type = GUID_BOOTLOADER_VALUE,
                           .debug_name = "Bootloader A"};
    case Partition::kBootloaderB:
      return PartitionInfo{.name = GPT_BOOTLOADER_B_NAME,
                           .type = GPT_BOOTLOADER_ABR_TYPE_GUID,
                           .legacy_name = GUID_EFI_NAME,
                           .legacy_type = GUID_BOOTLOADER_VALUE,
                           .debug_name = "Bootloader B"};
    case Partition::kBootloaderR:
      return PartitionInfo{.name = GPT_BOOTLOADER_R_NAME,
                           .type = GPT_BOOTLOADER_ABR_TYPE_GUID,
                           .legacy_name = GUID_EFI_NAME,
                           .legacy_type = GUID_BOOTLOADER_VALUE,
                           .debug_name = "Bootloader R"};
    case Partition::kZirconA:
      return PartitionInfo{.name = GPT_ZIRCON_A_NAME,
                           .type = GPT_ZIRCON_ABR_TYPE_GUID,
                           .legacy_name = GUID_ZIRCON_A_NAME,
                           .legacy_type = GUID_ZIRCON_A_VALUE,
                           .debug_name = "Zircon A"};
    case Partition::kZirconB:
      return PartitionInfo{.name = GPT_ZIRCON_B_NAME,
                           .type = GPT_ZIRCON_ABR_TYPE_GUID,
                           .legacy_name = GUID_ZIRCON_B_NAME,
                           .legacy_type = GUID_ZIRCON_B_VALUE,
                           .debug_name = "Zircon B"};
    case Partition::kZirconR:
      return PartitionInfo{.name = GPT_ZIRCON_R_NAME,
                           .type = GPT_ZIRCON_ABR_TYPE_GUID,
                           .legacy_name = GUID_ZIRCON_R_NAME,
                           .legacy_type = GUID_ZIRCON_R_VALUE,
                           .debug_name = "Zircon R"};
    case Partition::kVbMetaA:
      return PartitionInfo{.name = GPT_VBMETA_A_NAME,
                           .type = GPT_VBMETA_ABR_TYPE_GUID,
                           .legacy_name = GUID_VBMETA_A_NAME,
                           .legacy_type = GUID_VBMETA_A_VALUE,
                           .debug_name = "VBMeta A"};
    case Partition::kVbMetaB:
      return PartitionInfo{.name = GPT_VBMETA_B_NAME,
                           .type = GPT_VBMETA_ABR_TYPE_GUID,
                           .legacy_name = GUID_VBMETA_B_NAME,
                           .legacy_type = GUID_VBMETA_B_VALUE,
                           .debug_name = "VBMeta B"};
    case Partition::kVbMetaR:
      return PartitionInfo{.name = GPT_VBMETA_R_NAME,
                           .type = GPT_VBMETA_ABR_TYPE_GUID,
                           .legacy_name = GUID_VBMETA_R_NAME,
                           .legacy_type = GUID_VBMETA_R_VALUE,
                           .debug_name = "VBMeta R"};
    case Partition::kAbrMeta:
      return PartitionInfo{.name = GPT_DURABLE_BOOT_NAME,
                           .type = GPT_DURABLE_BOOT_TYPE_GUID,
                           .legacy_name = GUID_ABR_META_NAME,
                           .legacy_type = GUID_ABR_META_VALUE,
                           .debug_name = "A/B/R Metadata"};
    case Partition::kFuchsiaVolumeManager:
      return PartitionInfo{.name = GPT_FVM_NAME,
                           .type = GPT_FVM_TYPE_GUID,
                           .legacy_name = GUID_FVM_NAME,
                           .legacy_type = GUID_FVM_VALUE,
                           .debug_name = "FVM"};
    // sysconfig partition doesn't exist on any GPTs, so has no type GUID
    case Partition::kSysconfig:
    case Partition::kUnknown:
      return std::nullopt;
  }
}

}  // namespace

const char* PartitionName(Partition partition, PartitionScheme scheme) {
  std::optional<PartitionInfo> info = GetPartitionInfo(partition);
  if (info) {
    return scheme == PartitionScheme::kNew ? info->name : info->legacy_name;
  }
  return "Unknown";
}

std::optional<Uuid> PartitionTypeGuid(Partition partition, PartitionScheme scheme) {
  std::optional<PartitionInfo> info = GetPartitionInfo(partition);
  if (!info) {
    return std::nullopt;
  }
  return scheme == PartitionScheme::kNew ? info->type : info->legacy_type;
}

fbl::String PartitionSpec::ToString() const {
  const char* debug_name = "<Unknown Partition>";
  std::optional<PartitionInfo> info = GetPartitionInfo(partition);
  if (info) {
    debug_name = info->debug_name;
  }

  if (content_type.empty()) {
    return debug_name;
  }
  return fbl::StringPrintf("%s (%.*s)", debug_name, static_cast<int>(content_type.size()),
                           content_type.data());
}

std::vector<std::unique_ptr<DevicePartitionerFactory>>*
DevicePartitionerFactory::registered_factory_list() {
  static std::vector<std::unique_ptr<DevicePartitionerFactory>>* registered_factory_list = nullptr;
  if (registered_factory_list == nullptr) {
    registered_factory_list = new std::vector<std::unique_ptr<DevicePartitionerFactory>>();
  }
  return registered_factory_list;
}

zx::result<std::unique_ptr<DevicePartitioner>> DevicePartitionerFactory::Create(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config, std::shared_ptr<Context> context) {
  for (auto& factory : *registered_factory_list()) {
    if (auto status = factory->New(devices, svc_root, config, std::move(context)); status.is_ok()) {
      return zx::ok(std::move(status.value()));
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

void DevicePartitionerFactory::Register(std::unique_ptr<DevicePartitionerFactory> factory) {
  registered_factory_list()->push_back(std::move(factory));
}

/*====================================================*
 *               FIXED PARTITION MAP                  *
 *====================================================*/

constexpr PartitionScheme kFixedDevicePartitionScheme = PartitionScheme::kLegacy;

zx::result<std::unique_ptr<DevicePartitioner>> FixedDevicePartitioner::Initialize(
    const paver::BlockDevices& devices, fidl::ClientEnd<fuchsia_io::Directory> svc_root) {
  LOG("Successfully initialized FixedDevicePartitioner Device Partitioner\n");
  return zx::ok(new FixedDevicePartitioner(devices.Duplicate(), std::move(svc_root)));
}

bool FixedDevicePartitioner::SupportsPartition(const PartitionSpec& spec) const {
  const PartitionSpec supported_specs[] = {PartitionSpec(paver::Partition::kBootloaderA),
                                           PartitionSpec(paver::Partition::kZirconA),
                                           PartitionSpec(paver::Partition::kZirconB),
                                           PartitionSpec(paver::Partition::kZirconR),
                                           PartitionSpec(paver::Partition::kVbMetaA),
                                           PartitionSpec(paver::Partition::kVbMetaB),
                                           PartitionSpec(paver::Partition::kVbMetaR),
                                           PartitionSpec(paver::Partition::kAbrMeta),
                                           PartitionSpec(paver::Partition::kFuchsiaVolumeManager)};
  return std::any_of(std::cbegin(supported_specs), std::cend(supported_specs),
                     [&](const PartitionSpec& supported) { return SpecMatches(spec, supported); });
}

zx::result<std::unique_ptr<PartitionClient>> FixedDevicePartitioner::FindPartition(
    const PartitionSpec& spec) const {
  if (!SupportsPartition(spec)) {
    ERROR("Unsupported partition %s\n", spec.ToString().c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  std::optional<Uuid> type = PartitionTypeGuid(spec.partition, kFixedDevicePartitionScheme);
  // OK to assert; supported types all have a type GUID
  ZX_ASSERT(type);

  zx::result partition = OpenBlockPartition(devices_, std::nullopt, type, std::nullopt, ZX_SEC(5));
  if (partition.is_error()) {
    return partition.take_error();
  }

  return BlockPartitionClient::Create(std::move(partition.value()));
}

zx::result<> FixedDevicePartitioner::ResetPartitionTables() const {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> FixedDevicePartitioner::ValidatePayload(const PartitionSpec& spec,
                                                     std::span<const uint8_t> data) const {
  if (!SupportsPartition(spec)) {
    ERROR("Unsupported partition %s\n", spec.ToString().c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  return zx::ok();
}

zx::result<std::unique_ptr<DevicePartitioner>> DefaultPartitionerFactory::New(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config, std::shared_ptr<Context> context) {
  return FixedDevicePartitioner::Initialize(devices, component::MaybeClone(svc_root));
}

}  // namespace paver
