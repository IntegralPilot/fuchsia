// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device.h"

#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <cstdio>
#include <filesystem>
#include <string>

namespace hwstress {

// Open the given path as a FIDL channel.
zx::result<zx::channel> OpenDeviceChannel(std::string_view folder_path) {
  zx::channel client, server;
  zx_status_t status = zx::channel::create(0, &client, &server);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  std::error_code error_code;  // Used such that we use the noexcept version of exists.
  if (!std::filesystem::exists(folder_path, error_code)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  for (const auto& entry : std::filesystem::directory_iterator(folder_path)) {
    return zx::make_result(fdio_service_connect(entry.path().c_str(), server.release()),
                           std::move(client));
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

}  // namespace hwstress
