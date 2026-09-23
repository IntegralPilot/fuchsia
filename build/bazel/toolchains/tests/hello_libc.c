// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A trivial program used to verify that linking against the C library works.
#include <stdio.h>

int main(int argc, char** argv) {
  printf("hello libc\n");
  return 0;
}
