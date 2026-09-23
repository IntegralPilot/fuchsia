// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <atomic>

#include "src/lib/dso/cpp/async.h"

namespace {
// Use an atomic because we expect threads to run in parallel in this test.
std::atomic_uint32_t run_counter{0};
}  // namespace

__EXPORT
extern "C" uint32_t hanging_async_read_run_counter() { return run_counter.load(); }

int dso_main_async(int argc, const char** argv, const char** envp, zx_handle_t _svc,
                   zx_handle_t _pkg, zx_handle_t _directory_request, zx_handle_t _lifecycle,
                   zx_handle_t _config, fdf_dispatcher_t* dispatcher) {
  run_counter.fetch_add(1);
  // We don't need to hang here to "hang" the program. Because this is an async component it
  // continues running until its dispatcher is shutdown, which we simply don't do.
  return 0;
}
