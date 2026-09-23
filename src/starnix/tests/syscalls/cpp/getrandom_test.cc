// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "test_helper.h"

namespace {

// Define our own wrapper so that libc doesn't do anything surprising.
ssize_t sys_getrandom(void* buffer, size_t buffer_size, unsigned int flags) {
  return syscall(__NR_getrandom, buffer, buffer_size, flags);
}

TEST(GetRandomTest, BasicSuccess) {
  ssize_t buffer_len = 0x1000000ul;
  auto buffer = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, buffer_len, PROT_WRITE | PROT_READ, MAP_ANONYMOUS | MAP_PRIVATE, -1, 0));
  EXPECT_EQ(sys_getrandom(buffer.mapping(), buffer_len, 0), buffer_len);
}

TEST(GetRandomTest, OnlyWritesValidMappedBuffer) {
  ssize_t page_size = sysconf(_SC_PAGESIZE);
  ssize_t buffer_len = 0x1000000ul;

  // First, map the full buffer we expect plus an additional page.
  auto buffer = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, buffer_len + page_size, PROT_WRITE | PROT_READ, MAP_ANONYMOUS | MAP_PRIVATE, -1, 0));

  // Then unmap the last page.
  ASSERT_FALSE(munmap((void*)(static_cast<std::byte*>(buffer.mapping()) + buffer_len), page_size))
      << strerror(errno);

  // We should only see random bytes up to the end of the mapped region.
  EXPECT_EQ(sys_getrandom(buffer.mapping(), buffer_len * 2, 0), buffer_len);
}

}  // anonymous namespace
