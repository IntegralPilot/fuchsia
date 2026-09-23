// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <stdio.h>
#include <zircon/compiler.h>

#include "src/lib/dso/cpp/sync.h"

namespace {
// This is global storage, not thread local so all instances of this component in the DSO runner
// share it.
uint32_t run_counter = 0;
}  // namespace

__EXPORT
extern "C" uint32_t simple_sync_read_run_counter() { return run_counter; }

int dso_main(int argc, const char** argv, const char** envp) {
  ++run_counter;
  return 0;
}
