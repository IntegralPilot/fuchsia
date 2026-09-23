// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/self.h>

#include "test-start.h"

extern "C" int64_t TestStart() { return static_cast<int64_t>(elfldltl::Self<>::LoadBias()); }
