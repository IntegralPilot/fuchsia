// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <zircon/compiler.h>

#include <zxtest/zxtest.h>

// Provides a default entry point to run all registered tests. If the main program provides its own
// main, the library's main will be ignored.
__WEAK int main(int argc, char** argv) {
  // TODO(https://fxbug.dev/42073486): Remove this once the elf runner no longer
  // fools libc into block-buffering stdout.
  setlinebuf(stdout);
  return RUN_ALL_TESTS(argc, argv);
}
