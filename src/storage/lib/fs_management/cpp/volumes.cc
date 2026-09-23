// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/cpp/volumes.h"

#include <fidl/fuchsia.fxfs/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <fidl/fuchsia.unknown/cpp/markers.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fit/defer.h>
#include <lib/zx/result.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <zircon/errors.h>
#include <zircon/types.h>

namespace fs_management {

namespace {

zx::result<> CheckExists(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                         const std::string& path) {
  // Check if the volume exists.  This way, we can return an explicit NOT_FOUND if absent.
  // TODO(https://fxbug.dev/42174810): Check the epitaph of the call to Mount instead.
  auto [client, server] = fidl::Endpoints<fuchsia_io::Node>::Create();
  auto res = fidl::WireCall(exposed_dir)
                 ->Open(fidl::StringView::FromExternal(path),
                        fuchsia_io::wire::Flags::kProtocolNode, {}, server.TakeChannel());
  if (!res.ok()) {
    return zx::error(res.error().status());
  }
  auto query_res = fidl::WireCall(client)->Query();
  if (!query_res.ok()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return zx::ok();
}

}  // namespace

__EXPORT
zx::result<> CreateVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                          std::string_view name,
                          fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir,
                          fuchsia_fs_startup::wire::CreateOptions create_options,
                          fuchsia_fs_startup::wire::MountOptions options) {
  auto client = component::ConnectAt<fuchsia_fs_startup::Volumes>(exposed_dir);
  if (client.is_error())
    return client.take_error();

  auto result =
      fidl::WireCall(*client)->Create(fidl::StringView::FromExternal(name), std::move(outgoing_dir),
                                      create_options, std::move(options));
  if (!result.ok())
    return zx::error(result.error().status());
  if (result->is_error())
    return result->take_error();

  return zx::ok();
}

__EXPORT
zx::result<> OpenVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                        std::string_view name, fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir,
                        fuchsia_fs_startup::wire::MountOptions options) {
  std::string path = "volumes/" + std::string(name);
  if (auto status = CheckExists(exposed_dir, path); status.is_error()) {
    return status.take_error();
  }

  auto client = component::ConnectAt<fuchsia_fs_startup::Volume>(exposed_dir, path);
  if (client.is_error())
    return client.take_error();
  auto result = fidl::WireCall(*client)->Mount(std::move(outgoing_dir), options);
  if (!result.ok())
    return zx::error(result.error().status());
  if (result->is_error())
    return result->take_error();

  return zx::ok();
}

__EXPORT zx::result<> CheckVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                                  std::string_view name,
                                  fidl::ClientEnd<fuchsia_fxfs::Crypt> crypt_client) {
  std::string path = "volumes/" + std::string(name);
  if (auto status = CheckExists(exposed_dir, path); status.is_error()) {
    return status.take_error();
  }

  auto client = component::ConnectAt<fuchsia_fs_startup::Volume>(exposed_dir, path);
  if (client.is_error())
    return client.take_error();
  fidl::Arena arena;
  auto check_options =
      fuchsia_fs_startup::wire::CheckOptions::Builder(arena).crypt(std::move(crypt_client)).Build();
  auto result = fidl::WireCall(*client)->Check(check_options);
  if (!result.ok())
    return zx::error(result.error().status());
  if (result->is_error())
    return result->take_error();

  return zx::ok();
}

__EXPORT zx::result<> RemoveVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                                   std::string_view name) {
  auto client = component::ConnectAt<fuchsia_fs_startup::Volumes>(exposed_dir);
  if (client.is_error())
    return client.take_error();

  auto result = fidl::WireCall(*client)->Remove(fidl::StringView::FromExternal(name));
  if (!result.ok())
    return zx::error(result.error().status());
  if (result->is_error())
    return result->take_error();

  return zx::ok();
}

__EXPORT bool HasVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                        std::string_view name) {
  std::string path = "volumes/" + std::string(name);
  if (auto status = CheckExists(exposed_dir, path); status.is_error()) {
    return false;
  }
  return true;
}

}  // namespace fs_management
