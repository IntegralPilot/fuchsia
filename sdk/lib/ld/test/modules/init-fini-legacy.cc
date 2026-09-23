// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "init-fini-test.h"
#include "startup-symbols.h"

// This module defines initializers and finalizer functions that are combined
// into a single DT_INIT/DT_FINI entry point. They expect a global variable to
// be in a certain state before it is updated.

extern "C" [[gnu::retain]] void _init() { Callback(101); }

extern "C" [[gnu::retain]] void _fini() { Callback(102); }
