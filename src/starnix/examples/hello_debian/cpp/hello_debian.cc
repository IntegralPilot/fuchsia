// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unistd.h>

#include <iostream>

#ifdef __arm__
#define SIZE_STRING "32"
#else
#define SIZE_STRING ""
#endif

int main(int argc, char** argv) {
  std::cout << "hello debian" SIZE_STRING "\n";
  return 0;
}
