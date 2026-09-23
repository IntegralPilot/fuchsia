// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

class TouchStandbyTest : public ::testing::Test {
 public:
  void SetUp() override {
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (getuid() != 0) {
      GTEST_SKIP() << "Can only be run as root.";
    }

    touch_standby_fd_ = fbl::unique_fd(open("/dev/touch_standby", O_RDWR));
    ASSERT_TRUE(touch_standby_fd_.is_valid())
        << "open(\"/dev/touch_standby\") failed: " << strerror(errno) << "(" << errno << ")";
  }

 protected:
  fbl::unique_fd touch_standby_fd_;
};

TEST_F(TouchStandbyTest, WriteTouchStandbyState) {
  uint8_t write_standby = '1';
  auto res = write(touch_standby_fd_.get(), &write_standby, sizeof(write_standby));
  EXPECT_EQ(res, static_cast<ssize_t>(sizeof(write_standby)));

  uint8_t read_touch_on;
  res = read(touch_standby_fd_.get(), &read_touch_on, sizeof(read_touch_on));
  EXPECT_EQ(res, static_cast<ssize_t>(sizeof(read_touch_on)));
  EXPECT_EQ(read_touch_on, 1);

  write_standby = '0';
  res = write(touch_standby_fd_.get(), &write_standby, sizeof(write_standby));
  EXPECT_EQ(res, static_cast<ssize_t>(sizeof(write_standby)));

  uint8_t read_touch_off;
  res = read(touch_standby_fd_.get(), &read_touch_off, sizeof(read_touch_off));
  EXPECT_EQ(res, static_cast<ssize_t>(sizeof(read_touch_off)));
  EXPECT_EQ(read_touch_off, 0);

  uint8_t write_nl_standby[] = {'1', '\n'};
  res = write(touch_standby_fd_.get(), &write_nl_standby, sizeof(write_nl_standby));
  EXPECT_EQ(res, static_cast<ssize_t>(sizeof(write_nl_standby)));

  uint8_t read_nl_standby;
  res = read(touch_standby_fd_.get(), &read_nl_standby, sizeof(read_nl_standby));
  EXPECT_EQ(res, static_cast<ssize_t>(sizeof(read_nl_standby)));
  EXPECT_EQ(read_nl_standby, 1);

  // Expected failure: can't write multiple chars at a time
  const void* write_long_standby = "10011";
  res = write(touch_standby_fd_.get(), write_long_standby, sizeof(write_long_standby));
  EXPECT_EQ(res, -1);

  // Expected failure: can only write '0' or '1'
  write_standby = '2';
  res = write(touch_standby_fd_.get(), &write_standby, sizeof(write_standby));
  EXPECT_EQ(res, -1);
}

}  // namespace
