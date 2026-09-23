// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <inttypes.h>
#include <stdio.h>

extern "C" uint64_t ping(uint64_t *arg0, uint64_t *arg1, uint64_t *arg2, uint64_t *arg3);

extern "C" __attribute__((visibility("default"))) uint64_t call_ping(uint64_t *a) {
  return ping(a, a, a, a);
}

extern "C" __attribute__((visibility("default"))) uint64_t call_printf(uint64_t *a) {
  printf("A number: %u\n", static_cast<uint32_t>(*a));
  return 0;
}
