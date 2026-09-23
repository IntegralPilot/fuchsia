// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "reverser.h"

#include <gtest/gtest.h>

TEST(Reverser, BasicTests) {
  EXPECT_EQ(reverser::reverse_string("hello"), "olleh");
  EXPECT_EQ(reverser::reverse_string("World"), "dlroW");
}
