// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zygote.h"

#include <zircon/compiler.h>

#include "test-start.h"

// This is initialized data.  If this is shared across two calls to
// called_by_zygote_dep(), the second call will see different values.
static int counters[] = {6, 7, 8};
__EXPORT int* initialized_data[] = {&counters[2], &counters[1], &counters[0]};

// This is called by zygote_dep().  If its bss is shared across two calls, the
// second call will see a different value.
__EXPORT int called_by_zygote_dep() {
  static int bss;
  return *initialized_data[++bss]++;
}

// This is called by zygote-secondary.
__EXPORT int zygote_test_main() { return zygote_dep() + 1; }

extern "C" int64_t TestStart() { return zygote_test_main() + 1; }
