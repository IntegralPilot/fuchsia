// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/power_config/lib/cpp/power_config.h"

#include <lib/fdio/directory.h>

namespace power_config {
zx::result<fuchsia_hardware_power::ComponentPowerConfiguration> Load(const char* path) {
  auto [file_channel, server] = fidl::Endpoints<fuchsia_io::File>::Create();
  if (zx_status_t status = fdio_open3(path, static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                                      server.TakeChannel().release());
      status != ZX_OK) {
    return zx::error_result(status);
  }
  return Load(std::move(file_channel));
}

zx::result<fuchsia_hardware_power::ComponentPowerConfiguration> Load(
    fidl::ClientEnd<fuchsia_io::File> file_channel) {
  fidl::WireSyncClient file = fidl::WireSyncClient<fuchsia_io::File>(std::move(file_channel));
  fidl::WireResult seek_result = file->Seek(::fuchsia_io::wire::SeekOrigin::kEnd, 0);
  if (!seek_result.ok() || !seek_result->is_ok()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  uint64_t content_size = seek_result->value()->offset_from_start;
  fidl::WireResult seek_back_result = file->Seek(::fuchsia_io::wire::SeekOrigin::kStart, 0);
  if (!seek_back_result.ok() || !seek_back_result->is_ok()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  fidl::WireResult read_result = file->Read(content_size);
  if (!read_result.ok() || !read_result->is_ok()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  fit::result persisted_config =
      fidl::Unpersist<fuchsia_hardware_power::ComponentPowerConfiguration>(
          read_result->value()->data.get());
  if (!persisted_config.is_ok()) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok(std::move(persisted_config.value()));
}
}  // namespace power_config
