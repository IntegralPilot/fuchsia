// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "startup-symbols.h"
#include "suffixed-symbol.h"

// This file provides a `call_foo` symbol that in turn calls foo(), which is
// provided by a dependency during symbol resolution at runtime.
extern "C" [[gnu::visibility("default")]] int64_t SUFFIXED_SYMBOL(call_foo)();

extern "C" int64_t SUFFIXED_SYMBOL(foo)();
extern "C" int64_t SUFFIXED_SYMBOL(call_foo)() { return SUFFIXED_SYMBOL(foo)(); }

// This file is linked with foo-v1. Call a symbol provided explicitly by foo-v1
extern "C" int64_t SUFFIXED_SYMBOL(foo_v1)();
// Provide a `call_foo_v1` function that returns the value provided by its
// direct dependency, foo-v1.
extern "C" int64_t SUFFIXED_SYMBOL(call_foo_v1)() { return SUFFIXED_SYMBOL(foo_v1)(); }
