// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_runtime.h>

#include <gtest/gtest.h>

class FixtureTest : public ::testing::Test {
 private:
  fdf_testing::DriverRuntime runtime;
  fdf::UnownedSynchronizedDispatcher dispatcher_ = runtime.StartBackgroundDispatcher();
};

TEST_F(FixtureTest, BuildTest) {}
