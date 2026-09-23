// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/driver_host_loader_service.h"

#include <zircon/errors.h>

#include "src/devices/lib/log/log.h"
#include "src/lib/files/path.h"

namespace driver_manager {
namespace {

// TODO(https://fxbug.dev/42155836): Read this list from a config file instead of having an array.
constexpr std::array kDriverAllowlist{
    "libasync-default.so",
    "libclang_rt.asan.so",
    "libclang_rt.hwasan.so",
    "libcrypto.so",
    "libc.so",
    "libdriver_runtime.so",
    "libfdio.so",
    "libssl.so",
    "libsyslog.so",
    "libtrace-engine.so",
    "libbackend_fuchsia_globals.so",
    "libzircon.so",
    "libtee-client-api.so",
    "ld.so.1",
    "libc++.so.2",
    "libc++abi.so.1",
    "libunwind.so.1",
    "libsvc.so",
    "libvfs_internal.so",
    "libvfs_rust.so",
};

// Do NOT add to this list when predictable library names can be added to
// kDriverAllowlist.
constexpr std::array kDriverPrefixAllowlist{
    "libstd-",
};

// Check if the driver is in the allowlist.
bool InAllowlist(std::string path) {
  // path may have multiple path components, e.g. if loading the asan variant of a library, and
  // these should be allowed as long as the library name is in the allowlist.
  std::string base = files::GetBaseName(path);
  for (const char* entry : kDriverAllowlist) {
    if (base == entry) {
      return true;
    }
  }

  for (const char* prefix : kDriverPrefixAllowlist) {
    if (base.starts_with(prefix)) {
      return true;
    }
  }

  fdf_log::error("Driver-Loader: {}: Not in allowlist", path);
  return false;
}

}  // namespace

// static
std::shared_ptr<DriverHostLoaderService> DriverHostLoaderService::Create(
    async_dispatcher_t* dispatcher, fbl::unique_fd lib_fd, std::string name) {
  // Can't use make_shared because constructor is private
  return std::shared_ptr<DriverHostLoaderService>(
      new DriverHostLoaderService(dispatcher, std::move(lib_fd), std::move(name)));
}

zx::result<zx::vmo> DriverHostLoaderService::LoadObjectImpl(std::string path) {
  if (!InAllowlist(path)) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }
  return LoaderService::LoadObjectImpl(path);
}

}  // namespace driver_manager
