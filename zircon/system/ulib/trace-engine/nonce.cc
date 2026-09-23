// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/trace-engine/instrumentation.h>
#include <lib/zx/clock.h>
#include <zircon/compiler.h>
#include <zircon/syscalls.h>

__EXPORT uint64_t trace_generate_nonce() {
  uint64_t time = zx::clock::get_boot().get();
  uint64_t high_order = time << 16;
  uint16_t random_val;
  zx_cprng_draw(&random_val, sizeof(random_val));
  uint64_t nonce = high_order | random_val;
  if (unlikely(nonce == 0)) {
    return 1;
  }
  return nonce;
}

#if FUCHSIA_API_LEVEL_AT_LEAST(28)
__EXPORT uint64_t trace_time_based_id(zx_koid_t thread_id) {
  return (trace_generate_nonce() & ~0xFF00) | ((thread_id & 0xFF) << 8);
}
#endif
