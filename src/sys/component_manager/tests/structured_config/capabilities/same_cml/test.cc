// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/sys/component_manager/tests/structured_config/capabilities/same_cml/config.h"

namespace {

// This test is checking that the CML configuration capability overrides the values set in the
// CVF.
TEST(ScTest, CheckValues) {
  config::Config c = config::Config::TakeFromStartupHandle();
  // CVF sets this to false, CML sets this to true.
  ASSERT_EQ(c.my_flag(), true);

  // CVF sets these integers to 0, CML sets them to incrementing numbers.
  ASSERT_EQ(c.my_uint8(), 8);
  ASSERT_EQ(c.my_uint16(), 16);
  ASSERT_EQ(c.my_uint32(), 32);
  ASSERT_EQ(c.my_uint64(), 64);

  // CVF sets these integers to 0, CML sets them to decrementing numbers.
  ASSERT_EQ(c.my_int8(), -8);
  ASSERT_EQ(c.my_int16(), -16);
  ASSERT_EQ(c.my_int32(), -32);
  ASSERT_EQ(c.my_int64(), -64);

  // CVF sets this to "default", CML sets this to "test".
  ASSERT_EQ(c.my_string(), "test");

  ASSERT_EQ(c.my_uint8_vector().size(), 3);
  ASSERT_EQ(c.my_uint8_vector()[0], 1);
  ASSERT_EQ(c.my_uint8_vector()[1], 2);
  ASSERT_EQ(c.my_uint8_vector()[2], 3);

  ASSERT_EQ(c.my_string_vector().size(), 2);
  ASSERT_EQ(c.my_string_vector()[0], "Hello");
  ASSERT_EQ(c.my_string_vector()[1], "World!");

  // Check that our transitional value with a default got the default value.
  ASSERT_EQ(c.my_uint16_default(), 15);
}
}  // namespace
