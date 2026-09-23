// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

int main() {
  volatile int* p = new int;

  // force the compiler to not optimize away the new.
  __asm__("nop" : "=r"(p) : "0"(p));

  // Should be detected by GWP-ASan as an invalid free.
  delete (p + 1);
  return 0;
}
