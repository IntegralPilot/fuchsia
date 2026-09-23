// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/compiler.h>

#include "suffixed-test-start.h"

// These use C linkage to make debugging easier.
// Only TestStart _needs_ to be extern "C".
extern "C" {

__EXPORT int Seven = 7;

__EXPORT int BasicSymbol() { return 36; }

// This will call BasicSymbol via a PLT entry whose GOT slot has to be relocated,
// so whether BasicSymbol is resolved to the version here or a different version
// that returns a different value should matter.
__EXPORT int NeedsPlt() { return BasicSymbol() - 26; }

// This will load Seven via GOT indirection, so whether the GOT slot is resolved
// to the definition above or to a different definition with a different value
// matters.
__EXPORT int NeedsGot() { return Seven; }

// The value returned depends on how all the symbols above got resolved.
// The versions defined above add up to (36 - 26) + 7 = 17.
int64_t SUFFIXED_SYMBOL(TestStart)() { return NeedsPlt() + NeedsGot(); }
}
