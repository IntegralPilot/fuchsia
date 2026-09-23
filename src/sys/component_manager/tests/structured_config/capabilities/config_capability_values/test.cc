// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/sys/component_manager/tests/structured_config/capabilities/config_capability_values/config.h"

namespace {
TEST(ScTest, CheckValues) {
  config::Config c = config::Config::TakeFromStartupHandle();
  ASSERT_EQ(c.my_flag(), true);
}
}  // namespace
