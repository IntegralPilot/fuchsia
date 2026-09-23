// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/fsl/io/fd.h"

#include <lib/fdio/fd.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

namespace fsl {

zx::channel CloneChannelFromFileDescriptor(int fd) {
  zx::handle handle;
  if (zx_status_t status = fdio_fd_clone(fd, handle.reset_and_get_address()); status != ZX_OK) {
    return {};
  }
  zx_info_handle_basic_t info;
  if (zx_status_t status =
          handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    return {};
  }
  if (info.type != ZX_OBJ_TYPE_CHANNEL) {
    return {};
  }
  return zx::channel(handle.release());
}

zx::channel TransferChannelFromFileDescriptor(fbl::unique_fd fd) {
  zx::handle handle;
  if (zx_status_t status = fdio_fd_transfer(fd.release(), handle.reset_and_get_address());
      status != ZX_OK) {
    return {};
  }
  zx_info_handle_basic_t info;
  if (zx_status_t status =
          handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    return {};
  }
  if (info.type != ZX_OBJ_TYPE_CHANNEL) {
    return {};
  }
  return zx::channel(handle.release());
}

fbl::unique_fd OpenChannelAsFileDescriptor(zx::channel channel) {
  fbl::unique_fd fd;
  if (zx_status_t status = fdio_fd_create(channel.release(), fd.reset_and_get_address());
      status != ZX_OK) {
    return {};
  }
  return fd;
}

}  // namespace fsl
