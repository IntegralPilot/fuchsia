// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-symbol.h"
#include "suffixed-test-start.h"

// `has_missing_dep_sym` is provided by a dependency (libhas-missing-dep.so)
// that in turn contains a symbol that needs to be resolved from its own
// dependency that's missing, so we expect a missing module error for that
// missing transitive dependency.

extern "C" int64_t SUFFIXED_SYMBOL(has_missing_dep_sym)();

extern "C" int64_t SUFFIXED_SYMBOL(TestStart)() { return SUFFIXED_SYMBOL(has_missing_dep_sym)(); }
