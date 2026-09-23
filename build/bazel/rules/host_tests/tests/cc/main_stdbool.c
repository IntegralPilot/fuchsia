// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A small C file used to verify that stdbool.h can be properly
// included from C code. See https://fxbug.dev/440934285 for
// context.
#ifdef __cplusplus
#error "This file must not be compiled as C++, only as C"
#endif

#include <stdbool.h>
#include <stdio.h>

int main(void) {
  bool x = true;
  if (!x) {
    fprintf(stderr, "Error: 'true' does not match boolean truthiness!\n");
    return 1;
  }
  return 0;
}
