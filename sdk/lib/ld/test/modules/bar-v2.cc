// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-symbol.h"

// Similar to foo.cc, except exports a different function name that calls foo().

extern "C" [[gnu::visibility("default")]] int64_t SUFFIXED_SYMBOL(bar_v2)();

extern "C" int64_t SUFFIXED_SYMBOL(foo)();

extern "C" int64_t SUFFIXED_SYMBOL(bar_v2)() { return SUFFIXED_SYMBOL(foo)(); }
