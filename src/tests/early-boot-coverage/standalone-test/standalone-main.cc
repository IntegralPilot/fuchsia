// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>

#include "src/lib/llvm-profdata/coverage-example.h"

[[clang::optnone]] int main(int argc, const char**) {
  MaybeCallRunTimeDeadFunction(false);
  RunTimeCoveredFunction();
  RunTimeCoveredFunction();
  standalone::LogWrite("standalone-test completed.\n");
  return 0;
}
