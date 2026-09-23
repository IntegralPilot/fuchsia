// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
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

#include <new>

#include <fbl/unique_fd.h>
#include <fbl/vector.h>

#include "src/storage/lib/fs_management/cpp/component.h"
#include "src/storage/lib/fs_management/cpp/format.h"
#include "src/storage/lib/fs_management/cpp/options.h"

namespace fs_management {
namespace {

zx::result<> FsckComponentFs(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                             std::string_view device_path, const FsckOptions& options) {
  std::string device_path_str(device_path);
  auto device = component::Connect<fuchsia_storage_block::Block>(device_path);
  if (device.is_error())
    return device.take_error();

  auto startup_client_end = component::ConnectAt<fuchsia_fs_startup::Startup>(exposed_dir);
  if (startup_client_end.is_error())
    return startup_client_end.take_error();
  fidl::WireSyncClient startup_client{std::move(*startup_client_end)};

  auto res = startup_client->Check(std::move(*device), options.as_check_options());
  if (!res.ok())
    return zx::error(res.status());
  if (res->is_error())
    return zx::error(res->error_value());

  return zx::ok();
}

}  // namespace

__EXPORT
zx_status_t Fsck(std::string_view device_path, FsComponent& component, const FsckOptions& options) {
  auto exposed_dir = component.Connect();
  if (exposed_dir.is_error()) {
    return exposed_dir.status_value();
  }
  return FsckComponentFs(*exposed_dir, device_path, options).status_value();
}

}  // namespace fs_management
