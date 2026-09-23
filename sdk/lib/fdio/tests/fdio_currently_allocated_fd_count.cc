// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/fdio.h>
#include <lib/fdio/limits.h>

#include <zxtest/zxtest.h>

TEST(CurrentlyAllocatedFdCount, Basic) {
  size_t initial_count = fdio_currently_allocated_fd_count();
  ASSERT_LT(initial_count, FDIO_MAX_FD);
  int null_fd = fdio_fd_create_null();
  ASSERT_NE(null_fd, -1);
  size_t new_count = fdio_currently_allocated_fd_count();
  EXPECT_EQ(new_count, initial_count + 1);
  close(null_fd);
  size_t final_count = fdio_currently_allocated_fd_count();
  EXPECT_EQ(final_count, initial_count);
}
