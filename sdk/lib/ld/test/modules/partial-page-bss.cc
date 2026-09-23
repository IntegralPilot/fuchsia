// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stddef.h>

#include <array>

#include "test-start.h"

namespace {

constexpr size_t kPageSize = 4096;
constexpr size_t kPartialPageSize = 1024;

// This ensures there will be a data segment with p_filesz of an odd size.
alignas(kPageSize) std::array<char, kPageSize - kPartialPageSize> gData = {1, 2, 3};

// This will be at the end of that segment between p_filesz and p_memsz.
// So some partial-page zeroing will be required to load the module.
std::array<char, kPartialPageSize> gBss;

}  // namespace

extern "C" int64_t TestStart() {
  if ((reinterpret_cast<uintptr_t>(&gData + 1) & -kPageSize) !=
      (reinterpret_cast<uintptr_t>(&gBss) & -kPageSize)) {
    // This isn't testing the right thing if the bss does not share a partial
    // page with the initialized data.
    return 2;
  }

  // These prevent the compiler from discarding either variable or presuming
  // what their values are.
  __asm__ volatile("" : "=m"(gData) : "m"(gBss));
  __asm__ volatile("" : "=m"(gBss) : "m"(gData));

  // Make sure the partial page of .bss got zeroed.
  for (size_t i = 0; i < kPartialPageSize; ++i) {
    if (gBss[i] != 0) {
      return 3;
    }
  }

  return 17;
}
