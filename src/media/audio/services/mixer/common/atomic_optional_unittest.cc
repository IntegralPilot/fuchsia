// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/mixer/common/atomic_optional.h"

#include <gtest/gtest.h>

namespace media_audio {
namespace {

TEST(AtomicOptionalTest, PushPop) {
  AtomicOptional<int> v;
  EXPECT_EQ(v.pop(), std::nullopt);

  // Operate >2 times to make sure to test beyond the internal buffer size.
  for (int i = 1; i <= 3; ++i) {
    SCOPED_TRACE(i);

    // Only the first push will be successful.
    EXPECT_TRUE(v.push(i));
    EXPECT_FALSE(v.push(10 * i));
    EXPECT_FALSE(v.push(20 * i));
    EXPECT_FALSE(v.push(30 * i));

    // Only the first pop should have a value.
    EXPECT_EQ(v.pop(), i);
    EXPECT_EQ(v.pop(), std::nullopt);
    EXPECT_EQ(v.pop(), std::nullopt);
  }
}

}  // namespace
}  // namespace media_audio
