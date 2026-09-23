// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "resources.h"

#include <errno.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

zx_status_t get_info_resource(zx_handle_t* info_resource) {
  auto client_end = component::Connect<fuchsia_kernel::InfoResource>();
  if (client_end.is_error()) {
    fprintf(stderr, "ERROR: Cannot open fuchsia.kernel.InfoResource: %s (%d)\n",
            client_end.status_string(), client_end.status_value());
    return ZX_ERR_NOT_FOUND;
  }
  auto result = fidl::WireSyncClient(std::move(*client_end))->Get();
  if (result.status() != ZX_OK) {
    fprintf(stderr, "ERROR: Cannot obtain info resource: %s (%d)\n",
            zx_status_get_string(result.status()), result.status());
    return ZX_ERR_NOT_FOUND;
  }

  *info_resource = result->resource.release();

  return ZX_OK;
}
