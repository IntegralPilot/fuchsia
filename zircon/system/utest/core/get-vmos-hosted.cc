// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fdio/io.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <zircon/status.h>

#include <filesystem>
#include <string>
#include <unordered_map>

#include <fbl/unique_fd.h>

namespace standalone {
namespace {

std::unordered_map<std::string, zx::vmo> gVmos;

}  // namespace

zx::unowned_vmo GetVmo(std::string_view name) {
  auto [it, not_present] = gVmos.try_emplace(std::string{name});
  if (not_present) {
    std::filesystem::path path = "/boot/kernel";
    path /= name;
    fbl::unique_fd fd{open(path.c_str(), O_RDONLY)};
    if (fd) {
      zx_status_t status = fdio_get_vmo_exact(fd.get(), it->second.reset_and_get_address());
      if (status != ZX_OK) {
        fprintf(stderr, "fdio_get_vmo_exact: %s: %s\n", path.c_str(), zx_status_get_string(status));
      }
    }
  }
  return it->second.borrow();
}

}  // namespace standalone
