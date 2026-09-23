// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/sherlock.h"

#include <algorithm>
#include <iterator>

#include <gpt/gpt.h>
#include <soc/aml-common/aml-guid.h>

#include "src/firmware/paver/pave-logging.h"
#include "src/firmware/paver/utils.h"
#include "src/lib/uuid/uuid.h"

namespace paver {
namespace {

using uuid::Uuid;

}  // namespace

zx::result<std::unique_ptr<DevicePartitioner>> SherlockPartitioner::Initialize(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config) {
  auto status = IsBoard(svc_root, "sherlock");
  if (status.is_error()) {
    return status.take_error();
  }

  auto status_or_gpt = GptDevicePartitioner::InitializeGpt(devices, svc_root, config);
  if (status_or_gpt.is_error()) {
    ERROR("Failed to initialize GPT partitioner: %s\n", status_or_gpt.status_string());
    return status_or_gpt.take_error();
  }
  // NOTE: We explicitly ignore status_or_gpt->initialize_partition_tables here. The GPT component
  // might start successfully but in a state that requires reformatting if the partition table is
  // missing or malformed. In this case, there is no error at this point, but subsequent attempts
  // to modify the partition table will fail with ZX_ERR_BAD_STATE.

  auto partitioner =
      WrapUnique(new SherlockPartitioner(std::move(status_or_gpt->gpt), devices.Duplicate()));

  LOG("Successfully initialized SherlockPartitioner Device Partitioner\n");
  return zx::ok(std::move(partitioner));
}

const paver::BlockDevices& SherlockPartitioner::Devices() const { return gpt_->devices(); }

fidl::UnownedClientEnd<fuchsia_io::Directory> SherlockPartitioner::SvcRoot() const {
  return gpt_->svc_root();
}

// Sherlock bootloader types:
//
// -- default [deprecated] --
// The combined BL2 + TPL image.
//
// This was never actually added to any update packages, because older
// SherlockBootloaderPartitionClient implementations had a bug where they would
// write this image to the wrong place in flash which would overwrite critical
// metadata and brick the device on reboot.
//
// In order to prevent this from happening when updating older devices, never
// use this bootloader type on Sherlock.
//
// -- "skip_metadata" --
// The combined BL2 + TPL image.
//
// The image itself is identical to the default, but adding the "skip_metadata"
// type ensures that older pavers will ignore this image, and only newer
// implementations which properly skip the metadata section will write it.
bool SherlockPartitioner::SupportsPartition(const PartitionSpec& spec) const {
  const PartitionSpec supported_specs[] = {
      PartitionSpec(paver::Partition::kBootloaderA, "skip_metadata"),
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

zx::result<std::unique_ptr<PartitionClient>> SherlockPartitioner::FindPartition(
    const PartitionSpec& spec) const {
  if (!SupportsPartition(spec)) {
    ERROR("Unsupported partition %s\n", spec.ToString().c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // TODO(b/173125535): Remove legacy GPT support
  Uuid legacy_type;
  std::string_view part_name;
  std::string_view secondary_part_name;

  switch (spec.partition) {
    case Partition::kBootloaderA: {
      auto boot0_part = OpenBlockPartition(non_gpt_devices_, std::nullopt,
                                           Uuid(GUID_EMMC_BOOT1_VALUE), std::nullopt, ZX_SEC(5));
      if (boot0_part.is_error()) {
        return boot0_part.take_error();
      }
      zx::result boot0 =
          FixedOffsetBlockPartitionClient::Create(std::move(boot0_part.value()), 1, 0);
      if (boot0.is_error()) {
        return boot0.take_error();
      }

      auto boot1_part = OpenBlockPartition(non_gpt_devices_, std::nullopt,
                                           Uuid(GUID_EMMC_BOOT2_VALUE), std::nullopt, ZX_SEC(5));
      if (boot1_part.is_error()) {
        return boot1_part.take_error();
      }
      zx::result boot1 =
          FixedOffsetBlockPartitionClient::Create(std::move(boot1_part.value()), 2, 0);
      if (boot1.is_error()) {
        return boot1.take_error();
      }

      std::vector<std::unique_ptr<PartitionClient>> partitions;
      partitions.push_back(std::move(*boot0));
      partitions.push_back(std::move(*boot1));

      return zx::ok(std::make_unique<PartitionCopyClient>(std::move(partitions)));
    }
    case Partition::kZirconA:
      legacy_type = GUID_ZIRCON_A_VALUE;
      part_name = GPT_ZIRCON_A_NAME;
      secondary_part_name = "boot";
      break;
    case Partition::kZirconB:
      legacy_type = GUID_ZIRCON_B_VALUE;
      part_name = GPT_ZIRCON_B_NAME;
      secondary_part_name = "system";
      break;
    case Partition::kZirconR:
      legacy_type = GUID_ZIRCON_R_VALUE;
      part_name = GPT_ZIRCON_R_NAME;
      secondary_part_name = "recovery";
      break;
    case Partition::kVbMetaA:
      legacy_type = GUID_VBMETA_A_VALUE;
      part_name = GPT_VBMETA_A_NAME;
      break;
    case Partition::kVbMetaB:
      legacy_type = GUID_VBMETA_B_VALUE;
      part_name = GPT_VBMETA_B_NAME;
      break;
    case Partition::kVbMetaR:
      legacy_type = GUID_VBMETA_R_VALUE;
      part_name = GPT_VBMETA_R_NAME;
      break;
    case Partition::kAbrMeta:
      legacy_type = GUID_ABR_META_VALUE;
      part_name = GPT_DURABLE_BOOT_NAME;
      break;
    case Partition::kFuchsiaVolumeManager:
      legacy_type = GUID_FVM_VALUE;
      part_name = GPT_FVM_NAME;
      break;
    default:
      ERROR("Partition type is invalid\n");
      return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const auto filter = [&legacy_type, &part_name,
                       &secondary_part_name](const GptPartitionMetadata& part) {
    // Only filter by partition name instead of name + type due to bootloader bug (b/173801312)
    return FilterByType(part, legacy_type) || FilterByName(part, part_name) ||
           FilterByName(part, secondary_part_name);
  };
  auto status = gpt_->FindPartition(std::move(filter));
  if (status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(*status));
}

zx::result<> SherlockPartitioner::ResetPartitionTables() const {
  ERROR("Initializing gpt partitions from paver is not supported on sherlock\n");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> SherlockPartitioner::ValidatePayload(const PartitionSpec& spec,
                                                  std::span<const uint8_t> data) const {
  if (!SupportsPartition(spec)) {
    ERROR("Unsupported partition %s\n", spec.ToString().c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  return zx::ok();
}

zx::result<std::unique_ptr<DevicePartitioner>> SherlockPartitionerFactory::New(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config, std::shared_ptr<Context> context) {
  return SherlockPartitioner::Initialize(devices, svc_root, config);
}

zx::result<std::unique_ptr<abr::Client>> SherlockPartitioner::CreateAbrClient() const {
  // ABR metadata has no need of a content type since it's always local rather
  // than provided in an update package, so just use the default content type.
  auto partition = FindPartition(paver::PartitionSpec(paver::Partition::kAbrMeta));
  if (partition.is_error()) {
    return partition.take_error();
  }

  return abr::AbrPartitionClient::Create(std::move(partition.value()));
}

}  // namespace paver
