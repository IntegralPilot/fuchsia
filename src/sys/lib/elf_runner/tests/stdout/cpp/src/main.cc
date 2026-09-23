// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <chrono>
#include <iostream>
#include <thread>

int main() {
  std::cout << "Hello Stdout!" << std::endl;
  std::cerr << "Hello Stderr!" << std::endl;
}
