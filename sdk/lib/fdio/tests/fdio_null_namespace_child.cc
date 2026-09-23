// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/namespace.h>
#include <lib/fit/defer.h>
#include <stdio.h>
#include <zircon/status.h>

// This is a test executable to examine what happens when a process is not
// given any namespace entries.
int main(int argc, char** argv) {
  fdio_flat_namespace_t* flat;
  if (zx_status_t status = fdio_ns_export_root(&flat); status != ZX_OK) {
    printf("fdio_ns_export_root returned: %d (%s)\n", status, zx_status_get_string(status));
    return 1;
  }
  auto cleanup = fit::defer([flat]() { fdio_ns_free_flat_ns(flat); });

  if (flat->count != 0) {
    printf("exported flat namespace was non-empty: %zu\n", flat->count);
    return 1;
  }

  return 0;
}
