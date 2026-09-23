// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <stdlib.h>

#include "zircon/types.h"

__attribute__((noinline)) uint64_t count(int n) {
  if (n == 0) {
    return 1;
  }
  return 1 + count(n - 1);
}
__attribute__((noinline)) void add(uint64_t* addr) { *addr = *addr + 1; }
__attribute__((noinline)) void sub(uint64_t* addr) { *addr = *addr - 1; }
__attribute__((noinline)) void collatz(uint64_t* addr) {
  switch (*addr % 2) {
    case 0:
      *addr /= 2;
      break;
    case 1:
      *addr *= 3;
      *addr += 1;
      break;
  }
}

// A sample app that jumps between different function call chains to check sampling profiler results
int main() {
  // Map a vmo and write random data to it
  zx::vmo vmo;
  zx_status_t result = zx::vmo::create(zx_system_get_page_size(), 0, &vmo);
  if (result != ZX_OK) {
    return 1;
  }
  zx_vaddr_t out;
  result = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                      zx_system_get_page_size(), &out);
  if (result != ZX_OK) {
    return 1;
  }

  // Now we write randomish data to the vmo
  uint64_t* ptr = reinterpret_cast<uint64_t*>(out);
  *ptr = 0;
  for (;;) {
    switch (rand() % 4) {
      case 0:
        add(ptr);
        break;
      case 1:
        sub(ptr);
        break;
      case 2:
        collatz(ptr);
        break;
      case 3:
        *ptr = count(100);
        break;
    }
  }
}
