// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-symbol.h"

// This template is useful for providing a function that calls a symbol from a
// a dependency.

extern "C" [[gnu::visibility("default")]] int64_t SUFFIXED_SYMBOL(call_foo)();

extern "C" int64_t SUFFIXED_SYMBOL(foo)();

extern "C" int64_t SUFFIXED_SYMBOL(call_foo)() { return SUFFIXED_SYMBOL(foo)(); }

extern "C" int64_t SUFFIXED_SYMBOL(TestStart)() { return SUFFIXED_SYMBOL(call_foo)(); }
