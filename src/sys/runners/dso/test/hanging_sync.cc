// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
#include <zircon/compiler.h>

#include <atomic>

#include "src/lib/dso/cpp/sync.h"

namespace {
// Use an atomic because we expect threads to run in parallel in this test.
std::atomic_uint32_t run_counter{0};
}  // namespace

__EXPORT
extern "C" uint32_t hanging_sync_read_run_counter() { return run_counter.load(); }

int dso_main(int argc, const char** argv, const char** envp) {
  run_counter.fetch_add(1);
  while (true) {
    sleep(1);
  }
  return 255;
}
