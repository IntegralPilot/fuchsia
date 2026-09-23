// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

#include <lib/async/cpp/task.h>
#include <lib/async/cpp/time.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace gtest {
namespace {

// This file contains some simple smoke tests.
// Tests concerning |::loop_fixture::RealLoop| are located in //sdk/lib/async-loop-testing/cpp.

using RealLoopFixtureTest = RealLoopFixture;

TEST_F(RealLoopFixtureTest, SmokeTest) {
  static_assert(std::is_base_of_v<loop_fixture::RealLoop, RealLoopFixture>);
  RunLoopUntilIdle();
}

}  // namespace
}  // namespace gtest
