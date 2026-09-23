// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "second-session.h"

#include <lib/zx/channel.h>
#include <stdint.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include "indirect-deps.h"
#include "test-start.h"

extern "C" [[noreturn]] void _start(zx_handle_t bootstrap, void* vdso) {
  // The bootstrap handle is a channel where the test that launched this
  // process will write the address of a function pointer after it's finished
  // the second dynamic linking session.  This process starts before that
  // second session is performed, so block until the channel is written to.
  zx::channel channel{bootstrap};
  zx_signals_t pending;
  zx_status_t status = channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &pending);
  if (status != ZX_OK) {
    zx_process_exit(status);
  }
  if (!(pending & ZX_CHANNEL_READABLE)) {
    zx_process_exit(1);
  }

  // The fact that the channel became readable means that the controlling test
  // has loaded additional modules in a second dynamic linking session.  It's
  // written the address of a TestStart() function in the secondary dynamic
  // linking domain's root module.
  uintptr_t ptr;
  uint32_t actual_bytes, actual_handles;
  status = channel.read(0, &ptr, nullptr, sizeof(ptr), 0, &actual_bytes, &actual_handles);
  if (status != ZX_OK) {
    zx_process_exit(status);
  }
  if (actual_bytes != sizeof(ptr)) {
    zx_process_exit(2);
  }
  if (actual_handles != 0) {
    zx_process_exit(3);
  }

  // This should get second-session-module.cc's TestStart().
  const auto func = reinterpret_cast<decltype(TestStart)*>(ptr);

  // In this main executable's domain a() -> (b() -> 6) + (c() -> 7) -> 13.
  if (a() != 13) {
    zx_process_exit(4000 + a());
  }

  if (defined_in_main_executable() != 14) {
    zx_process_exit(5000 + defined_in_main_executable());
  }

  // In the secondary dynamic linking domain, TestStart() returns
  // (defined_in_main_executable() -> 14) + (a() -> 3) -> 17.
  zx_process_exit(func());
}

// This function is exported from this main executable, which is preserved as a
// preloaded implicit module for the secondary dynamic linking domain.  It
// references symbols exported by the indirect-deps modules, which are
// dependencies of this main executable not visible to the secondary domain.
__EXPORT int64_t defined_in_main_executable() { return a() + 1; }
