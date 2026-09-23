// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/abr-client.h"

#include <dirent.h>
#include <endian.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/abr/abr.h>
#include <lib/cksum.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/wire_messaging.h>
#include <lib/fit/defer.h>
#include <stdio.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/hw/gpt.h>
#include <zircon/status.h>

#include <string_view>

#include <fbl/algorithm.h>

#include "lib/fidl/cpp/wire/internal/transport_channel.h"
#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/lib/uuid/uuid.h"
#include "zircon/time.h"

namespace abr {

namespace partition = fuchsia_storage_block;
using fuchsia_paver::wire::Asset;
using fuchsia_paver::wire::Configuration;

zx::result<Configuration> CurrentSlotToConfiguration(std::string_view slot) {
  // Some bootloaders prefix slot with dash or underscore. We strip them for consistency.
  slot.remove_prefix(std::min(slot.find_first_not_of("_-"), slot.size()));
  if (slot.compare("a") == 0) {
    return zx::ok(Configuration::kA);
  }
  if (slot.compare("b") == 0) {
    return zx::ok(Configuration::kB);
  }
  if (slot.compare("r") == 0) {
    return zx::ok(Configuration::kRecovery);
  }
  ERROR("Invalid value `%.*s` found in zvb.current_slot!\n", static_cast<int>(slot.size()),
        slot.data());
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

bool FindPartitionLabelByGuid(const paver::BlockDevices& devices, const uint8_t* guid,
                              std::string& out) {
  auto partition = devices.OpenPartition([&](const zx::channel& channel) {
    auto client = fidl::UnownedClientEnd<fuchsia_storage_block::Block>((channel.borrow()));
    auto result = fidl::WireCall(client)->GetInstanceGuid();
    if (!result.ok()) {
      return false;
    }
    const auto& response = result.value();
    if (response.status != ZX_OK) {
      return false;
    }
    return memcmp(response.guid->value.data_, guid, GPT_GUID_LEN) == 0;
  });
  if (partition.is_error()) {
    if (partition.status_value() != ZX_ERR_NOT_FOUND) {
      ERROR("Failed to open partitions: %s\n", partition.status_string());
    }
    return false;
  }

  zx::result client = partition->Connect();
  if (client.is_error()) {
    ERROR("Failed to connect to partition: %s\n", zx_status_get_string(client.status_value()));
    return false;
  }
  auto result = fidl::WireCall(*client)->GetName();
  if (!result.ok()) {
    ERROR("Failed to call GetName on partition: %s\n", result.status_string());
    return false;
  }
  const auto& response = result.value();
  if (response.status != ZX_OK) {
    ERROR("Failed to call GetName on partition: %s\n", zx_status_get_string(response.status));
    return false;
  }
  std::string_view name(response.name.data(), response.name.size());
  out = name;
  return true;
}

zx::result<Configuration> PartitionUuidToConfiguration(const paver::BlockDevices& devices,
                                                       uuid::Uuid uuid) {
  std::string name;
  auto result = FindPartitionLabelByGuid(devices, uuid.bytes(), name);
  if (!result) {
    ERROR("Failed to get partition label by GUID\n");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // Partition label should be zircon-<slot> or zircon_<slot>. This is case insensitive.
  static const size_t kZirconLength = sizeof("zircon") - 1;  // no null terminator.
  // Partition must start with "zircon".
  if (strncasecmp(name.data(), "zircon", kZirconLength) != 0) {
    ERROR("Incorrect partition name: %s \n", name.c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  name.erase(0, kZirconLength);

  if (name[0] != '-' && name[0] != '_') {
    ERROR("Incorrect partition name: %s \n", name.c_str());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  switch (name[1]) {
    case 'a':
    case 'A':
      return zx::ok(Configuration::kA);
    case 'b':
    case 'B':
      return zx::ok(Configuration::kB);
    case 'r':
    case 'R':
      return zx::ok(Configuration::kRecovery);
  }

  ERROR("Incorrect partition name: %s \n", name.c_str());
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<Configuration> QueryBootConfig(const paver::BlockDevices& devices,
                                          const paver::PaverConfig& config) {
  if (!config.zvb_current_slot.empty()) {
    return CurrentSlotToConfiguration(config.zvb_current_slot);
  }
  if (!config.zvb_boot_partition_uuid.empty()) {
    std::string_view uuid_str = config.zvb_boot_partition_uuid;
    auto uuid = uuid::Uuid::FromString(uuid_str);
    if (uuid == std::nullopt) {
      ERROR("Invalid UUID in zvb.boot-partition-uuid: %.*s \n", (int)uuid_str.size(),
            uuid_str.data());
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }

    return PartitionUuidToConfiguration(devices, uuid.value());
  }
  if (!config.android_boot_slot_suffix.empty()) {
    std::string_view prefix_str = config.android_boot_slot_suffix;
    if (prefix_str.length() == 1) {
      return CurrentSlotToConfiguration(prefix_str.substr(0, 1));
    } else if (prefix_str.length() == 2 && prefix_str[0] == '_') {
      return CurrentSlotToConfiguration(prefix_str.substr(1));
    }

    ERROR("Invalid prefix string: %.*s \n", (int)prefix_str.length(), prefix_str.data());
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  ERROR(
      "Kernel cmdline param zvb.current_slot, zvb.boot-partition-uuid "
      "or androidboot.slot_suffix not found!\n");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<bool> SupportsVerifiedBoot(const paver::BlockDevices& devices,
                                      const paver::PaverConfig& config) {
  if (zx::result result = QueryBootConfig(devices, config); result.is_error()) {
    if (result.error_value() == ZX_ERR_NOT_SUPPORTED) {
      return zx::ok(false);
    } else {
      return result.take_error();
    }
  }
  return zx::ok(true);
}

zx::result<std::unique_ptr<abr::Client>> AbrPartitionClient::Create(
    std::unique_ptr<paver::PartitionClient> partition) {
  auto status = partition->GetBlockSize();
  if (status.is_error()) {
    ERROR("Unabled to get block size\n");
    return status.take_error();
  }
  size_t block_size = status.value();

  zx::vmo vmo;
  if (auto status = zx::make_result(
          zx::vmo::create(fbl::round_up(block_size, zx_system_get_page_size()), 0, &vmo));
      status.is_error()) {
    ERROR("Failed to create vmo\n");
    return status.take_error();
  }

  if (auto status = partition->Read(vmo, block_size); status.is_error()) {
    ERROR("Failed to read from partition\n");
    return status.take_error();
  }

  return zx::ok(new AbrPartitionClient(std::move(partition), std::move(vmo), block_size));
}

zx::result<> AbrPartitionClient::Read(uint8_t* buffer, size_t size) {
  if (auto status = partition_->Read(vmo_, block_size_); status.is_error()) {
    return status.take_error();
  }
  if (auto status = zx::make_result(vmo_.read(buffer, 0, size)); status.is_error()) {
    return status.take_error();
  }
  return zx::ok();
}

zx::result<> AbrPartitionClient::Write(const uint8_t* buffer, size_t size) {
  if (auto status = zx::make_result(vmo_.write(buffer, 0, size)); status.is_error()) {
    return status.take_error();
  }
  if (auto status = partition_->Write(vmo_, block_size_); status.is_error()) {
    return status.take_error();
  }
  return zx::ok();
}

bool Client::ReadAbrMetaData(void* context, size_t size, uint8_t* buffer) {
  if (auto res = static_cast<Client*>(context)->Read(buffer, size); res.is_error()) {
    ERROR("Failed to read abr data from storage. %s\n", res.status_string());
    return false;
  }
  return true;
}

bool Client::WriteAbrMetaData(void* context, const uint8_t* buffer, size_t size) {
  if (auto res = static_cast<Client*>(context)->Write(buffer, size); res.is_error()) {
    ERROR("Failed to write abr data to storage. %s\n", res.status_string());
    return false;
  }
  return true;
}

bool Client::ReadAbrMetadataCustom(void* context, AbrSlotData* a, AbrSlotData* b,
                                   uint8_t* one_shot_recovery) {
  if (auto res = static_cast<Client*>(context)->ReadCustom(a, b, one_shot_recovery);
      res.is_error()) {
    ERROR("Failed to read abr data from storage. %s\n", res.status_string());
    return false;
  }
  return true;
}

bool Client::WriteAbrMetadataCustom(void* context, const AbrSlotData* a, const AbrSlotData* b,
                                    uint8_t one_shot_recovery) {
  if (auto res = static_cast<Client*>(context)->WriteCustom(a, b, one_shot_recovery);
      res.is_error()) {
    ERROR("Failed to write abr data to storage. %s\n", res.status_string());
    return false;
  }
  return true;
}

zx::result<> Client::AbrResultToZxStatus(AbrResult status) {
  switch (status) {
    case kAbrResultOk:
      return zx::ok();
    case kAbrResultErrorIo:
      return zx::error(ZX_ERR_IO);
    case kAbrResultErrorInvalidData:
      return zx::error(ZX_ERR_INVALID_ARGS);
    case kAbrResultErrorUnsupportedVersion:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  ERROR("Unknown Abr result code %d!\n", status);
  return zx::error(ZX_ERR_INTERNAL);
}

extern "C" uint32_t AbrCrc32(const void* buf, size_t buf_size) {
  return crc32(0UL, reinterpret_cast<const uint8_t*>(buf), buf_size);
}

}  // namespace abr
