// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "lib.h"

TEST(RestrictedModeUnifiedStress, SharedRegionTestShort) {
  Orchestrator(zx::sec(10), zx::sec(1), TestMode::Shared);
}

TEST(RestrictedModeUnifiedStress, RestrictedRegionTestShort) {
  Orchestrator(zx::sec(10), zx::sec(1), TestMode::Restricted);
}
