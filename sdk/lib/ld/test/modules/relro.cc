// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/compiler.h>

#include <cstddef>

#include "suffixed-test-start.h"

namespace {

// This pagesize is larger than any reasonable size we expect this test to run
// with.
constexpr size_t kBiggerThanPageSize = 0x10000;
struct RelroData {
  std::byte before_padding[kBiggerThanPageSize];
  int* relocated;
  std::byte after_padding[kBiggerThanPageSize - sizeof(int*)];
};

int sym;

__CONSTINIT const RelroData relro_data{.relocated = &sym};

}  // namespace

extern "C" int64_t SUFFIXED_SYMBOL(TestStart)() {
  // We need the compiler to lose provenence information on this address so it
  // can't optimize away these reads and writes knowing that this object is
  // const.
  int** p = const_cast<int**>(&relro_data.relocated);
  __asm__("" : "=r"(p) : "0"(p));

  if (*p != &sym) {
    return 1;
  }
  // This should crash after relro has been protected.
  *p = nullptr;
  return 2;
}
