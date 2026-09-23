// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ldsvc/cpp/wire.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/fdio/fd.h>
#include <lib/zx/channel.h>
#include <zircon/dlfcn.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cerrno>

#include <gtest/gtest.h>

namespace elfldltl::testing {

// Fuchsia-specific implementation of TryGetTestLib (see get-test-data-path.cc);
// a custom implementation is needed because fdio open does not support
// returning an executable fd.
fbl::unique_fd TryGetTestLib(std::string_view libname) {
  fbl::unique_fd fd;
  zx::vmo lib_vmo = TryGetTestLibVmo(libname);
  // If the VMO is not found, return the default-constructed invalid fd to the
  // caller.
  if (!lib_vmo) {
    return fd;
  }
  // Create and return a valid FD for the VMO handle.
  int fd_out;
  zx_status_t status = fdio_fd_create(lib_vmo.release(), &fd_out);
  EXPECT_EQ(status, ZX_OK) << libname << ": fdio_fd_create: " << zx_status_get_string(status);
  return fbl::unique_fd{fd_out};
}

fbl::unique_fd GetTestLib(std::string_view libname) {
  auto fd = TryGetTestLib(libname);
  EXPECT_TRUE(fd) << "elfldltl::GetTestLib(\"" << libname << "\"): VMO not found";
  return fd;
}

}  // namespace elfldltl::testing
