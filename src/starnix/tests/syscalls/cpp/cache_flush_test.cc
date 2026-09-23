// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <stdint.h>
#include <sys/mman.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

// These are tests for an aarch32 (aka __arm__) specific syscall.
#if defined(__arm__)

#ifndef __ARM_NR_cacheflush
#define __ARM_NR_cacheflush 0x0f0002
#endif

namespace {

// We're issuing a raw syscall instead of using a libc wrapper so errors are
// returned as the negation of the error value in the return value instead of
// being in errno.
int cache_flush_syscall(char* start, char* end) {
  register int start_reg asm("r0") = (int)(intptr_t)start;
  const register int end_reg asm("r1") = (int)(intptr_t)end;
  const register int flags asm("r2") = 0;
  const register int syscall_nr asm("r7") = __ARM_NR_cacheflush;
  __asm __volatile("svc 0x0"
                   : "=r"(start_reg)
                   : "r"(syscall_nr), "r"(start_reg), "r"(end_reg), "r"(flags));
  return start_reg;
}

}  // namespace

TEST(CacheFlush, Simple) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping = mmap(nullptr, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED) << strerror(errno);
  char* start = static_cast<char*>(mapping);
  int rv = cache_flush_syscall(start, &start[page_size]);
  EXPECT_EQ(rv, 0);
  SAFE_SYSCALL(munmap(mapping, page_size));
}

TEST(CacheFlush, FlushNullPtr) {
  char* start = nullptr;
  char* end = start + 1;
  int rv = cache_flush_syscall(start, end);

  EXPECT_EQ(rv, -EFAULT);
}

TEST(CacheFlush, FlushUnmapped) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping = mmap(nullptr, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED) << strerror(errno);
  SAFE_SYSCALL(munmap(mapping, page_size));
  char* start = static_cast<char*>(mapping);
  int rv = cache_flush_syscall(start, &start[page_size]);
  EXPECT_EQ(rv, -EFAULT);
}

TEST(CacheFlush, InvalidRange) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping = mmap(nullptr, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED) << strerror(errno);
  char* start = static_cast<char*>(mapping);
  // Flip the start and end addresses
  int rv = cache_flush_syscall(&start[page_size], start);
  EXPECT_EQ(rv, -EINVAL);
  SAFE_SYSCALL(munmap(mapping, page_size));
}

TEST(CacheFlush, RangeWithGap) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping = mmap(nullptr, 3 * page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED) << strerror(errno);
  void* second_page = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapping) + page_size);
  SAFE_SYSCALL(munmap(second_page, page_size));
  char* start = static_cast<char*>(mapping);
  // Try to flush the full original mapping. This will span the first mapped page and
  // the second unmapped page.
  int rv = cache_flush_syscall(start, &start[page_size * 3]);
  EXPECT_EQ(rv, -EFAULT);
  SAFE_SYSCALL(munmap(mapping, 3 * page_size));
}

TEST(CacheFlush, ProtNone) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED) << strerror(errno);
  char* start = static_cast<char*>(mapping);
  int rv = cache_flush_syscall(start, &start[page_size * 3]);
  EXPECT_EQ(rv, -EFAULT);
  SAFE_SYSCALL(munmap(mapping, page_size));
}

TEST(CacheFlush, EmptyRange) {
  char* start = nullptr;
  int rv = cache_flush_syscall(start, start);
  EXPECT_EQ(rv, 0);
}

#endif  // !defined(__arm__)
