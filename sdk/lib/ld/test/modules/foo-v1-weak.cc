// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-symbol.h"

extern "C" [[gnu::visibility("default")]] int64_t SUFFIXED_SYMBOL(foo)();

extern "C" [[gnu::weak]] int64_t SUFFIXED_SYMBOL(foo)() { return 2; }
extern "C" int64_t SUFFIXED_SYMBOL(foo_v1)() { return 2; }
