// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A trivial program to verify that linking against the C++ runtime library works.
#include <iostream>

int main() {
  std::cout << "hello libc++" << std::endl;
  return 0;
}
