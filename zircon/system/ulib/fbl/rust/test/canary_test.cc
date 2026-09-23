// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stddef.h>
#include <stdint.h>

#include <fbl/canary.h>

// Assume the Rust Canary has this layout
// Compile-time tests to ensure layout compatibility
static_assert(sizeof(fbl::Canary<0x12345678>) == 4, "Size mismatch");
static_assert(alignof(fbl::Canary<0x12345678>) == 4, "Align mismatch");

extern "C" {

// Runtime test helper to verify that a Rust Canary is recognized by C++
bool check_rust_canary(const void* ptr, uint32_t expected_magic) {
  if (expected_magic != 0x12345678) {
    return false;
  }
  const fbl::Canary<0x12345678>* cpp_ptr = static_cast<const fbl::Canary<0x12345678>*>(ptr);
  return cpp_ptr->Valid();
}
}
