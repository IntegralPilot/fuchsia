// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/test_lib/test_header.h>

int main() {
  // Ensure the function and all its dependencies are in the dependency tree.
  return SomeFunction();
}
