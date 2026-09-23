// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <stdio.h>

// Must not receive SHOULD_NOT_BE_DEFINED
#if defined(SHOULD_NOT_BE_DEFINED)
#error "Unexpected SHOULD_NOT_BE_DEFINED definition"
#endif

// Should have received CFLAGS
#if !defined(CFLAGS)
#error "Missing CFLAGS definition"
#elif CFLAGS != 1
#error "Invalid CFLAGS definition"
#endif

extern int fx_cc_library_for_test();

int main() {
  int ret = fx_cc_library_for_test();
  int expected = 42;
  if (ret != expected) {
    fprintf(stderr, "Unexpected value from fx_cc_library_for_test(): %d, expected %d\n", ret,
            expected);
    return 1;
  }
  return 0;
}
