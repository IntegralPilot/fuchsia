// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <iostream>

#include "foo.h"

int main() {
  int expected = 42;
  int actual = foo();
  if (actual != expected) {
    std::cerr << "ERROR: Got " << actual << " but expected " << expected << std::endl;
    return 1;
  }
  std::cout << "OK" << std::endl;
  return 0;
}
