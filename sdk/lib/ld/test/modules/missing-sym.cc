// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-test-start.h"

// This file depends on a generated .ifs file (i.e. a stub shared object) that
// says it defines `missing_sym` so that at link the time the linker is
// satisfied that `missing_sym` exists.
// That .ifs file specifies that it has the soname libld-dep-missing-sym-dep.so,
// so the linker adds a DT_NEEDED on libld-dep-missing-sym-dep.so. The actual
// libld-dep-missing-sym-dep doesn't define `missing_sym`, so at runtime there
// will be a missing symbol error.

extern "C" int64_t SUFFIXED_SYMBOL(missing_sym)();

extern "C" int64_t SUFFIXED_SYMBOL(TestStart)() { return SUFFIXED_SYMBOL(missing_sym)() + 4; }
