// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-symbol.h"

// Defines the symbol foo() that returns a constant value, 17.

extern "C" [[gnu::visibility("default")]] int64_t SUFFIXED_SYMBOL(foo)();

extern "C" int64_t SUFFIXED_SYMBOL(foo)() { return 17; }
