// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/vim3.h"

#include <lib/fzl/owned-vmo-mapper.h>

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

zx::result<std::unique_ptr<DevicePartitioner>> Vim3Partitioner::Initialize(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config) {
  auto status = IsBoard(svc_root, "vim3");
  if (status.is_error()) {
    return status.take_error();
  }

  auto status_or_gpt = GptDevicePartitioner::InitializeGpt(devices, svc_root, config);
  if (status_or_gpt.is_error()) {
    return status_or_gpt.take_error();
  }
  if (status_or_gpt->initialize_partition_tables) {
    LOG("Found GPT but it was missing expected partitions.  The device should be re-initialized "
        "via fastboot.\n");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  auto partitioner =
      WrapUnique(new Vim3Partitioner(std::move(status_or_gpt->gpt), devices.Duplicate()));

  LOG("Successfully initialized Vim3Partitioner Device Partitioner\n");
  return zx::ok(std::move(partitioner));
}

const paver::BlockDevices& Vim3Partitioner::Devices() const { return gpt_->devices(); }

fidl::UnownedClientEnd<fuchsia_io::Directory> Vim3Partitioner::SvcRoot() const {
  return gpt_->svc_root();
}

bool Vim3Partitioner::SupportsPartition(const PartitionSpec& spec) const {
  const PartitionSpec supported_specs[] = {
      PartitionSpec(paver::Partition::kBootloaderA),
      PartitionSpec(paver::Partition::kZirconA),
      PartitionSpec(paver::Partition::kZirconB),
      PartitionSpec(paver::Partition::kZirconR),
      PartitionSpec(paver::Partition::kVbMetaA),
      PartitionSpec(paver::Partition::kVbMetaB),
      PartitionSpec(paver::Partition::kVbMetaR),
      PartitionSpec(paver::Partition::kAbrMeta),
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager),
      PartitionSpec(paver::Partition::kFuchsiaVolumeManager, kOpaqueVolumeContentType),
  };
  return std::any_of(std::cbegin(supported_specs), std::cend(supported_specs),
                     [&](const PartitionSpec& supported) { return SpecMatches(spec, supported); });
}

zx::result<std::unique_ptr<PartitionClient>> Vim3Partitioner::GetEmmcBootPartitionClient() const {
  auto boot0_part = OpenBlockPartition(non_gpt_devices_, std::nullopt, Uuid(GUID_EMMC_BOOT1_VALUE),
                                       std::nullopt, ZX_SEC(5));
  if (boot0_part.is_error()) {
    return boot0_part.take_error();
  }
  zx::result boot0 = FixedOffsetBlockPartitionClient::Create(std::move(boot0_part.value()), 1, 0);
  if (boot0.is_error()) {
    return boot0.take_error();
  }

  auto boot1_part = OpenBlockPartition(non_gpt_devices_, std::nullopt, Uuid(GUID_EMMC_BOOT2_VALUE),
                                       std::nullopt, ZX_SEC(5));
  if (boot1_part.is_error()) {
    return boot1_part.take_error();
  }
  zx::result boot1 = FixedOffsetBlockPartitionClient::Create(std::move(boot1_part.value()), 2, 0);
  if (boot1.is_error()) {
    return boot1.take_error();
  }

  std::vector<std::unique_ptr<PartitionClient>> partitions;
  partitions.push_back(std::move(*boot0));
  partitions.push_back(std::move(*boot1));

  return zx::ok(std::make_unique<PartitionCopyClient>(std::move(partitions)));
}

zx::result<std::unique_ptr<PartitionClient>> Vim3Partitioner::FindPartition(
    const PartitionSpec& spec) const {
  if (!SupportsPartition(spec)) {
    ERROR("Unsupported partition %s\n", spec.ToString().c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  std::string_view part_name;

  switch (spec.partition) {
    case Partition::kBootloaderA:
      return GetEmmcBootPartitionClient();
    case Partition::kZirconA:
      part_name = GPT_ZIRCON_A_NAME;
      break;
    case Partition::kZirconB:
      part_name = GPT_ZIRCON_B_NAME;
      break;
    case Partition::kZirconR:
      part_name = GPT_ZIRCON_R_NAME;
      break;
    case Partition::kVbMetaA:
      part_name = GPT_VBMETA_A_NAME;
      break;
    case Partition::kVbMetaB:
      part_name = GPT_VBMETA_B_NAME;
      break;
    case Partition::kVbMetaR:
      part_name = GPT_VBMETA_R_NAME;
      break;
    case Partition::kAbrMeta:
      part_name = GPT_DURABLE_BOOT_NAME;
      break;
    case Partition::kFuchsiaVolumeManager:
      part_name = GPT_FVM_NAME;
      break;
    default:
      ERROR("Partition type is invalid\n");
      return zx::error(ZX_ERR_INVALID_ARGS);
  }

  LOG("Looking for part %s\n", std::string(part_name).c_str());

  auto status = gpt_->FindPartition(
      [part_name](const GptPartitionMetadata& part) { return FilterByName(part, part_name); });
  if (status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(*status));
}

zx::result<> Vim3Partitioner::ResetPartitionTables() const {
  ERROR("Initializing gpt partitions from paver is not supported on vim3\n");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> Vim3Partitioner::ValidatePayload(const PartitionSpec& spec,
                                              std::span<const uint8_t> data) const {
  if (!SupportsPartition(spec)) {
    ERROR("Unsupported partition %s\n", spec.ToString().c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok();
}

zx::result<std::unique_ptr<DevicePartitioner>> Vim3PartitionerFactory::New(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config, std::shared_ptr<Context> context) {
  return Vim3Partitioner::Initialize(devices, svc_root, config);
}

zx::result<std::unique_ptr<abr::Client>> Vim3Partitioner::CreateAbrClient() const {
  // ABR metadata has no need of a content type since it's always local rather
  // than provided in an update package, so just use the default content type.
  auto partition = FindPartition(paver::PartitionSpec(paver::Partition::kAbrMeta));
  if (partition.is_error()) {
    return partition.take_error();
  }

  return abr::AbrPartitionClient::Create(std::move(partition.value()));
}

}  // namespace paver
