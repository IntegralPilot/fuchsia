// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/debug/debug.h"

template <typename T, int size>
void DefaultInitialize(T array[size]) {
  for (int i = 0; i < size; i++) {
    debug::BreakIntoDebuggerIfAttached();
    array[i] = T();
  }
}

int main() {
  int array[5];
  DefaultInitialize<int, 5>(array);
}
