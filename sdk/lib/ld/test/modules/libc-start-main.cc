// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

namespace {

using InitFn = void();

enum State {
  kInitial,

  // The Check() calls should be made in this exact order.
  kPreinitArray,
  kCtor,
  kMain,
};

void Check(State new_state) {
  static State gState = kInitial;
  ZX_ASSERT(gState == new_state - 1);
  gState = new_state;
}

// The executable's DT_PREINIT_ARRAY is called before anything else after
// libc's own internal setup.
void PreinitArrayFn() { Check(kPreinitArray); }

[[gnu::section(".preinit_array"), gnu::used,
  gnu::retain]] alignas(InitFn*) InitFn* const kCallPreinitArrayFn = PreinitArrayFn;

// Normal constructors (via DT_INIT_ARRAY) are called next.
[[gnu::constructor]] void Ctor() { Check(kCtor); }

}  // namespace

int main() {
  // Constructors precede the call to main.
  Check(kMain);
  return 0;
}
