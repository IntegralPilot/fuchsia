// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>
#include <zircon/dlfcn.h>
#include <zircon/errors.h>

__EXPORT zx_status_t dl_clone_loader_service(zx_handle_t* out) {
  // TODO(https://fxbug.dev/338239708): Figure out what to do here in new
  // world.  Without libdl, there need be no known service still live in libc.
  // However, fdio_spawn will want one.
  return ZX_ERR_NOT_SUPPORTED;
}
