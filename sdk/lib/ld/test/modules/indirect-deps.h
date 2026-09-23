// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_INDIRECT_DEPS_H_
#define LIB_LD_TEST_MODULES_INDIRECT_DEPS_H_

#include <stdint.h>

#include "suffixed-symbol.h"

extern "C" int64_t SUFFIXED_SYMBOL(a)(), SUFFIXED_SYMBOL(b)(), SUFFIXED_SYMBOL(c)();

#endif  // LIB_LD_TEST_MODULES_INDIRECT_DEPS_H_
