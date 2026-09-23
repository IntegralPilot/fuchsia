// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/utc.h>

#include <thread>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

class FakeClockNodeathTest : public gtest::RealLoopFixture {};

TEST_F(FakeClockNodeathTest, NoCrash) { ASSERT_NE(ZX_HANDLE_INVALID, zx_utc_reference_get()); }
