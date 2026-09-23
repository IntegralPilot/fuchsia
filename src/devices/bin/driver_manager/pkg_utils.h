// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_PKG_UTILS_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_PKG_UTILS_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/zx/result.h>

namespace pkg_utils {

inline zx::result<zx::vmo> OpenPkgFile(fidl::UnownedClientEnd<fuchsia_io::Directory> pkg_dir,
                                       std::string_view relative_binary_path) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::File>::Create();
  zx_status_t status = fdio_open3_at(
      pkg_dir.channel()->get(), relative_binary_path.data(),
      static_cast<uint64_t>(fuchsia_io::wire::kPermReadable | fuchsia_io::wire::kPermExecutable),
      server_end.TakeChannel().release());
  if (status != ZX_OK) {
    return zx::error(status);
  }

  fidl::SyncClient file(std::move(client_end));
  auto result =
      file->GetBackingMemory(fuchsia_io::VmoFlags::kRead | fuchsia_io::VmoFlags::kExecute |
                             fuchsia_io::VmoFlags::kPrivateClone);
  if (result.is_error()) {
    zx_status_t status = result.error_value().is_domain_error()
                             ? result.error_value().domain_error()
                             : ZX_ERR_BAD_HANDLE;
    return zx::error(status);
  }
  return zx::ok(std::move(result->vmo()));
}

inline zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenLibDir(
    fidl::UnownedClientEnd<fuchsia_io::Directory> pkg_dir) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  zx_status_t status = fdio_open3_at(
      pkg_dir.channel()->get(), "lib",
      static_cast<uint64_t>(fuchsia_io::wire::Flags::kProtocolDirectory |
                            fuchsia_io::wire::kPermReadable | fuchsia_io::wire::kPermExecutable),
      server_end.TakeChannel().release());
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(client_end));
}

}  // namespace pkg_utils

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_PKG_UTILS_H_
