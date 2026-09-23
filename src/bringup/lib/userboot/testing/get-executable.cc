// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/userboot/testing/launcher.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

namespace userboot::testing {

namespace fio = fuchsia_io;

zx::result<zx::vmo> GetExecutable(const char* file) {
  constexpr fio::wire::Flags kFlags =
      fio::wire::Flags::kProtocolFile | fio::wire::kPermReadable | fio::wire::kPermExecutable;
  fbl::unique_fd fd;
  zx_status_t status =
      fdio_open3_fd(file, static_cast<uint64_t>(kFlags), fd.reset_and_get_address());
  EXPECT_EQ(status, ZX_OK) << "Cannot open: " << file << ": " << zx_status_get_string(status);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  zx::vmo vmo;
  status = fdio_get_vmo_exec(fd.get(), vmo.reset_and_get_address());
  EXPECT_EQ(status, ZX_OK) << "fdio_get_vmo_exec: " << file << ": " << zx_status_get_string(status);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  return zx::ok(std::move(vmo));
}

}  // namespace userboot::testing
