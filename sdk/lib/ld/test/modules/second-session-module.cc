// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "indirect-deps.h"
#include "second-session.h"
#include "test-start.h"

// The first function called is defined in the main executable (from
// second-session.cc), which is visible in this dynamic linking domain.  The
// second function called is defined in this module's own dependency graph,
// though a symbol of that name exists in the main executable's domain too.
// The two a() functions return different values, ensuring it's the right one.
int64_t TestStart() { return defined_in_main_executable() + a(); }
