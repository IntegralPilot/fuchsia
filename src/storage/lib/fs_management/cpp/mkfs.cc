// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/limits.h>
#include <lib/zx/channel.h>
#include <unistd.h>
#include <zircon/compiler.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <iterator>
#include <new>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/string_buffer.h>
#include <fbl/unique_fd.h>
#include <fbl/vector.h>

#include "src/storage/lib/fs_management/cpp/component.h"
#include "src/storage/lib/fs_management/cpp/format.h"
#include "src/storage/lib/fs_management/cpp/mount.h"
#include "src/storage/lib/fs_management/cpp/volumes.h"

namespace fs_management {
namespace {

zx::result<> MkfsComponentFs(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                             fidl::ClientEnd<fuchsia_storage_block::Block> device,
                             const MkfsOptions& options) {
  auto startup_client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(exposed_dir);
  if (startup_client_end.is_error())
    return startup_client_end.take_error();
  fidl::WireSyncClient startup_client{std::move(*startup_client_end)};

  fidl::Arena arena;
  auto res = startup_client->Format(std::move(device), options.as_format_options(arena));
  if (!res.ok()) {
    return zx::error(res.status());
  }
  if (res->is_error()) {
    return zx::error(res->error_value());
  }

  return zx::ok();
}

}  // namespace

__EXPORT
zx_status_t Mkfs(fidl::ClientEnd<fuchsia_storage_block::Block> device, FsComponent& component,
                 const MkfsOptions& options) {
  auto exposed_dir = component.Connect();
  if (exposed_dir.is_error()) {
    return exposed_dir.status_value();
  }
  return MkfsComponentFs(*exposed_dir, std::move(device), options).status_value();
}

__EXPORT
zx_status_t Mkfs(const char* device_path, FsComponent& component, const MkfsOptions& options) {
  zx::result device = component::Connect<fuchsia_storage_block::Block>(device_path);
  if (device.is_error()) {
    return device.status_value();
  }
  return Mkfs(std::move(*device), component, options);
}

}  // namespace fs_management
