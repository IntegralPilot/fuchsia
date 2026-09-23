// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_UTILS_H_
#define SRC_FIRMWARE_PAVER_UTILS_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.hardware.skipblock/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <memory>
#include <optional>
#include <string_view>

#include <fbl/unique_fd.h>

#include "src/firmware/paver/block-devices.h"
#include "src/lib/uuid/uuid.h"

namespace paver {

// Helper function to auto-deduce type.
template <typename T>
std::unique_ptr<T> WrapUnique(T* ptr) {
  return std::unique_ptr<T>(ptr);
}

zx::result<std::unique_ptr<VolumeConnector>> OpenBlockPartition(
    const paver::BlockDevices& devices, std::optional<uuid::Uuid> unique_guid,
    std::optional<uuid::Uuid> type_guid, std::optional<std::string_view> name,
    zx_duration_t timeout);

zx::result<std::unique_ptr<VolumeConnector>> OpenSkipBlockPartition(
    const paver::BlockDevices& devices, const uuid::Uuid& type_guid, zx_duration_t timeout);

zx::result<std::string> GetBoardName(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root);
zx::result<> IsBoard(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                     std::string_view board_name);

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_UTILS_H_
