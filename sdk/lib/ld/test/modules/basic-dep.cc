// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-symbol.h"
#include "suffixed-test-start.h"

extern "C" int64_t SUFFIXED_SYMBOL(a)();

extern "C" int64_t SUFFIXED_SYMBOL(TestStart)() { return SUFFIXED_SYMBOL(a)() + 4; }
