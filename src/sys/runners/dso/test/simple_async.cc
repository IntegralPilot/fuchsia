// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include "src/lib/dso/cpp/async.h"

namespace {
// This is global storage, not thread local so all instances of this component in the DSO runner
// share it.
uint32_t run_counter = 0;
}  // namespace

__EXPORT
extern "C" uint32_t simple_async_read_run_counter() { return run_counter; }

int dso_main_async(int argc, const char** argv, const char** envp, zx_handle_t _svc,
                   zx_handle_t _pkg, zx_handle_t _directory_request, zx_handle_t lifecycle,
                   zx_handle_t _config, fdf_dispatcher_t* dispatcher) {
  FX_CHECK(lifecycle != ZX_HANDLE_INVALID);
  ++run_counter;
  // Close lifecycle channel immediately to exit the component.
  zx_status_t s = zx_handle_close(lifecycle);
  FX_CHECK(s == ZX_OK) << zx_status_get_string(s);
  return 0;
}
