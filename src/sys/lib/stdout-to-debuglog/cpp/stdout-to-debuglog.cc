// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/fdio.h>
#include <lib/zx/debuglog.h>

namespace StdoutToDebuglog {

zx_status_t Init() {
  zx::result client_end = component::Connect<fuchsia_boot::WriteOnlyLog>();
  if (client_end.is_error()) {
    return client_end.status_value();
  }
  const fidl::WireSyncClient write_only_log(std::move(client_end.value()));
  const fidl::WireResult result = write_only_log->Get();
  if (!result.ok()) {
    return result.status();
  }
  const auto& response = result.value();
  const zx::debuglog& log = response.log;
  for (int fd = 1; fd <= 2; ++fd) {
    zx::debuglog dup;
    if (zx_status_t status = log.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); status != ZX_OK) {
      return status;
    }
    fdio_t* logger = nullptr;
    if (zx_status_t status = fdio_create(dup.release(), &logger); status != ZX_OK) {
      return status;
    }
    const int out_fd = fdio_bind_to_fd(logger, fd, 0);
    if (out_fd != fd) {
      return ZX_ERR_BAD_STATE;
    }
  }
  return ZX_OK;
}

}  // namespace StdoutToDebuglog
