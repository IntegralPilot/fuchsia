// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/utils.h"

#include <dirent.h>
#include <fidl/fuchsia.hardware.skipblock/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/watcher.h>

#include <string>
#include <string_view>

#include <fbl/algorithm.h>
#include <gpt/gpt.h>

#include "src/firmware/paver/pave-logging.h"
#include "src/lib/uuid/uuid.h"

namespace paver {

namespace {

using uuid::Uuid;

namespace partition = fuchsia_storage_block;
namespace skipblock = fuchsia_hardware_skipblock;

}  // namespace

// Not static so test can manipulate it.
zx_duration_t g_wipe_timeout = ZX_SEC(3);

zx::result<std::unique_ptr<VolumeConnector>> OpenBlockPartition(
    const paver::BlockDevices& devices, std::optional<Uuid> unique_guid,
    std::optional<Uuid> type_guid, std::optional<std::string_view> name, zx_duration_t timeout) {
  ZX_ASSERT(unique_guid || type_guid || name);

  auto cb = [&](const zx::channel& chan) {
    if (type_guid) {
      auto result =
          fidl::WireCall(fidl::UnownedClientEnd<partition::Block>(chan.borrow()))->GetTypeGuid();
      if (!result.ok()) {
        ERROR("Failed to GetTypeGuid: %s\n", result.status_string());
        return false;
      }
      auto& response = result.value();
      if (response.status != ZX_OK || type_guid != Uuid(response.guid->value.data())) {
        if (response.status != ZX_OK && response.status != ZX_ERR_NOT_SUPPORTED) {
          ERROR("Failed to GetTypeGuid: %s\n", zx_status_get_string(response.status));
        }
        return false;
      }
    }
    if (unique_guid) {
      auto result = fidl::WireCall(fidl::UnownedClientEnd<partition::Block>(chan.borrow()))
                        ->GetInstanceGuid();
      if (!result.ok()) {
        ERROR("Failed to GetInstanceGuid: %s\n", result.status_string());
        return false;
      }
      const auto& response = result.value();
      if (response.status != ZX_OK || unique_guid != Uuid(response.guid->value.data())) {
        if (response.status != ZX_OK) {
          ERROR("Failed to GetInstanceGuid: %s\n", zx_status_get_string(response.status));
        }
        return false;
      }
    }
    if (name) {
      auto result =
          fidl::WireCall(fidl::UnownedClientEnd<partition::Block>(chan.borrow()))->GetName();
      if (!result.ok()) {
        ERROR("Failed to GetName: %s\n", result.status_string());
        return false;
      }
      const auto& response = result.value();
      if (response.status != ZX_OK) {
        if (response.status != ZX_ERR_NOT_SUPPORTED) {
          ERROR("Failed to GetName: %s\n", zx_status_get_string(response.status));
        }
        return false;
      }
      std::string_view part_name(response.name.data(), response.name.size());
      return part_name == *name;
    }
    return true;
  };

  return devices.WaitForPartition(cb, timeout);
}

zx::result<std::unique_ptr<VolumeConnector>> OpenSkipBlockPartition(
    const paver::BlockDevices& devices, const Uuid& type_guid, zx_duration_t timeout) {
  auto cb = [&](const zx::channel& chan) {
    auto result = fidl::WireCall(fidl::UnownedClientEnd<skipblock::SkipBlock>(chan.borrow()))
                      ->GetPartitionInfo();
    if (!result.ok()) {
      return false;
    }
    const auto& response = result.value();
    return response.status == ZX_OK &&
           type_guid == Uuid(response.partition_info.partition_guid.data());
  };

  return devices.WaitForPartition(cb, timeout);
}

zx::result<std::string> GetBoardName(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root) {
  zx::result status =
      component::ConnectAt<fuchsia_sysinfo::SysInfo>(svc_root, "fuchsia.sysinfo.SysInfo");
  if (status.is_error()) {
    return status.take_error();
  }
  fidl::WireResult result = fidl::WireCall(status.value())->GetBoardName();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::string(response.name.data(), response.name.size()));
}

zx::result<> IsBoard(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                     std::string_view board_name) {
  zx::result<std::string> result = GetBoardName(svc_root);
  if (result.is_error()) {
    return result.take_error();
  }

  if (result.value() == board_name) {
    return zx::ok();
  }

  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

}  // namespace paver
