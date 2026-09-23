// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/purge_memory.h"

#include <gtest/gtest.h>

#include "src/developer/forensics/testing/unit_test_fixture.h"

namespace forensics {
namespace {

using PurgeMemoryTest = UnitTestFixture;

TEST_F(PurgeMemoryTest, PurgesAfterDelay) {
  // We've had issues in the past where purging memory caused a process crash. See
  // https://fxbug.dev/398058583.
  PurgeAllMemoryAfter(dispatcher(), zx::sec(1));
  RunLoopFor(zx::sec(2));
}

}  // namespace
}  // namespace forensics
