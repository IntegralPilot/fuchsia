// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTING_UTIL_ZXTEST_HELPERS_H_
#define SRC_UI_TESTING_UTIL_ZXTEST_HELPERS_H_

#include <zxtest/zxtest.h>

// Add EXPECT_NEAR() for zxtest
#define EXPECT_NEAR(val1, val2, eps)                                         \
  EXPECT_LE(std::abs(static_cast<double>(val1) - static_cast<double>(val2)), \
            static_cast<double>(eps))

#endif  // SRC_UI_TESTING_UTIL_ZXTEST_HELPERS_H_
