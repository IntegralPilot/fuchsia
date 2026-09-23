// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fidl/fuchsia.memory.heapdump.process/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>

#include "heapdump/bind.h"

static constexpr const char *kServicePath =
    fidl::DiscoverableProtocolDefaultPath<fuchsia_memory_heapdump_process::Registry>;
static constexpr const char *kServiceName =
    fidl::DiscoverableProtocolName<fuchsia_memory_heapdump_process::Registry>;

// Note: we cannot use neither stat() nor access() because they hang when instrumenting processes
// that run early at boot.
static bool check_capability_exists_in_svc_directory() {
  DIR *dir = opendir("/svc");
  if (!dir) {
    return false;
  }
  auto cleanup = fit::defer([&dir]() { closedir(dir); });

  while (dirent *entry = readdir(dir)) {
    if (strcmp(entry->d_name, kServiceName) == 0) {
      return true;
    }
  }

  return false;
}

__EXPORT void heapdump_bind_with_fdio(void) {
  zx::channel local, remote;
  auto status = zx::make_result(zx::channel::create(0, &local, &remote));
  ZX_ASSERT_MSG(status.is_ok(), "failed to create channel: %s", status.status_string());

  if (check_capability_exists_in_svc_directory() &&
      fdio_service_connect(kServicePath, remote.release()) == ZX_OK) {
    heapdump_bind_with_channel(local.release());
  } else {
    heapdump_bind_with_channel(ZX_HANDLE_INVALID);
  }
}
