// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/shutdown_message.h"

#include <lib/zx/time.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace forensics::feedback_data::system_log_recorder {
namespace {

TEST(ShutdownMessageTest, TimestampsFormattedAsSecondsAndMilliseconds) {
  const zx::duration secs = zx::sec(100);
  const zx::duration msecs = zx::msec(123);
  const zx::duration usecs = zx::usec(456);

  const zx::time_boot now((secs + msecs + usecs).get());
  EXPECT_EQ(
      ShutdownMessage(now),
      "!!! [00100.123] SYSTEM SHUTDOWN SIGNAL RECEIVED FURTHER LOGS ARE NOT GUARANTEED !!!\n");
}

}  // namespace
}  // namespace forensics::feedback_data::system_log_recorder
