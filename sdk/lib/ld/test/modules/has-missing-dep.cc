// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/compiler.h>

#include "suffixed-symbol.h"

// An .ifs file is generated for the dependency that defines `missing_dep_sym`
// (see //sdk/lib/ld/test/modules:missing-dep-dep-ifs). This module doesn't
// exist so we expect a missing module error.
// This source file is similar to missing-dep.cc, except that it is used in a
// different test module dependency graph.

extern "C" int64_t SUFFIXED_SYMBOL(missing_dep_sym)();

extern "C" __EXPORT int64_t SUFFIXED_SYMBOL(has_missing_dep_sym)() {
  return SUFFIXED_SYMBOL(missing_dep_sym)();
}
