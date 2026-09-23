// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/android/platform/bionic/libc/kernel/uapi/linux/ashmem.h"

#include <fcntl.h>
#include <lib/stdcompat/string_view.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include <fstream>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

struct ashmem_pin_op {
  ashmem_pin range;
  unsigned int op;
  unsigned int expected;
};

const unsigned PAGE_SIZE = getpagesize();
const unsigned long long LARGE_NUMBER_OF_PAGES = PAGE_SIZE * 1000 * 1000;

class AshmemTest : public ::testing::Test {
  void SetUp() override {
    // /dev/ashmem must exist
    if (access("/dev/ashmem", F_OK) != 0) {
      GTEST_SKIP() << "/dev/ashmem is not present.";
    }

    errno = 0;
  }

 protected:
  static fbl::unique_fd Open() {
    fbl::unique_fd fd(open("/dev/ashmem", O_RDWR));
    EXPECT_TRUE(fd.is_valid());
    return fd;
  }

  static fbl::unique_fd CreateRegion(char name[], size_t size) {
    fbl::unique_fd fd(open("/dev/ashmem", O_RDWR));
    EXPECT_TRUE(fd.is_valid());
    EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, size), SyscallSucceeds());
    if (name != nullptr) {
      EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, name), SyscallSucceeds());
    }
    return fd;
  }
};

// Can open "/dev/ashmem"
TEST_F(AshmemTest, OpenAndClose) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  EXPECT_THAT(close(fd.release()), SyscallSucceeds());
}

TEST_F(AshmemTest, DefaultSize) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_SIZE), SyscallSucceedsWithValue(0));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallSucceeds());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_SIZE), SyscallSucceedsWithValue((int)PAGE_SIZE));
}

TEST_F(AshmemTest, DefaultName) {
  char buf[256];
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_GET_NAME, buf), SyscallSucceeds());
  EXPECT_STREQ("dev/ashmem", buf);
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, "test"), SyscallSucceeds());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_GET_NAME, buf), SyscallSucceeds());
  EXPECT_STREQ("test", buf);
}

TEST_F(AshmemTest, DefaultProtections) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PROT_MASK),
              SyscallSucceedsWithValue(PROT_READ | PROT_WRITE | PROT_EXEC));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ), SyscallSucceeds());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PROT_MASK), SyscallSucceedsWithValue(PROT_READ));
}

// There must be a size associated with the region for mmap() to succeed
TEST_F(AshmemTest, NoAccessBeforeSetSize) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  const size_t kMapSize = PAGE_SIZE;

  // No size, fail
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr == MAP_FAILED && errno == EINVAL);

  // With size, succeed
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallSucceeds());
  addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Setting the size to zero does not permit mmap()
TEST_F(AshmemTest, SetSizeZero) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, 0), SyscallSucceeds());

  void *addr = mmap(nullptr, PAGE_SIZE, PROT_READ, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr == MAP_FAILED && errno == EINVAL);
}

// Size can be impossibly large
TEST_F(AshmemTest, SetSizeOverflow) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, LARGE_NUMBER_OF_PAGES), SyscallSucceeds());

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Size rounds up to page boundary, but this is hidden from us
TEST_F(AshmemTest, SetSizeMisaligned) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, 1), SyscallSucceeds());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_SIZE), SyscallSucceedsWithValue(1));

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE + 1), SyscallSucceeds());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_SIZE), SyscallSucceedsWithValue((int)PAGE_SIZE + 1));

  const size_t kMapSize = 2 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_WRITE, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);
  // This will not segfault because size rounds up to two pages despite reporting PAGE_SIZE + 1
  for (size_t i = 0; i < kMapSize; i++) {
    reinterpret_cast<volatile uint8_t *>(addr)[i] = 0;
  }

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Once mmap() succeeds on a region, the size is set in stone
TEST_F(AshmemTest, NoChangeSizeAfterMap) {
  auto fd = CreateRegion(nullptr, 2 * PAGE_SIZE);
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallSucceeds());

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, 3 * PAGE_SIZE), SyscallFailsWithErrno(EINVAL));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

TEST_F(AshmemTest, NoChangeSizeAfterMunmap) {
  auto fd = CreateRegion(nullptr, 2 * PAGE_SIZE);
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallSucceeds());

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, 3 * PAGE_SIZE), SyscallFailsWithErrno(EINVAL));
}

// Mmap() arguments must not be out of bounds
TEST_F(AshmemTest, MapOutOfBounds) {
  auto fd = CreateRegion(nullptr, 3 * PAGE_SIZE);

  // Fill up data
  const size_t kMapSize = 3 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_WRITE, MAP_SHARED, fd.get(), 0);
  for (size_t i = 0; i < kMapSize / sizeof(int); i++) {
    ((int *)addr)[i] = (int)i;
  }
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());

  // Too big
  // Ashmem region:     [    3 pages    ]
  // Attempted to mmap: [       4 pages       ]
  addr = mmap(nullptr, 4 * PAGE_SIZE, PROT_READ, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr == MAP_FAILED && errno == EINVAL);

  // Completely outside bounds
  // TODO: Linux allows mmap() to succeed if the address is outside the region
  // but the size is within the region, which seems like a bug
  // Ashmem region:     [    3 pages    ]
  // Attempted to mmap:                 [  1 page  ]
  addr = mmap(nullptr, PAGE_SIZE, PROT_READ, MAP_SHARED, fd.get(), 4 * PAGE_SIZE);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, PAGE_SIZE), SyscallSucceeds());

  // Partially outside bounds
  // Ashmem region:     [    3 pages    ]
  // Attempted to mmap:            [  2 pages  ]
  addr = mmap(nullptr, 2 * PAGE_SIZE, PROT_READ, MAP_SHARED, fd.get(), 2 * PAGE_SIZE);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);
  // TODO: find a way to verify that accessing any page beyond the first segfaults
  for (size_t i = 0; i < PAGE_SIZE / sizeof(int); i++) {
    int val = (int)(i + 2 * PAGE_SIZE / sizeof(int));
    ASSERT_EQ(val, ((int *)addr)[i]);
  }

  ASSERT_THAT(munmap(addr, 2 * PAGE_SIZE), SyscallSucceeds());
}

// Once mmap() has been called the name is set in stone
TEST_F(AshmemTest, NoChangeNameAfterMap) {
  char name[] = "hello world!";
  char buf[256];
  auto fd = CreateRegion(name, PAGE_SIZE);

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, "goodbye"), SyscallFailsWithErrno(EINVAL));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_GET_NAME, buf), SyscallSucceeds());
  EXPECT_STREQ("hello world!", buf);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

TEST_F(AshmemTest, NoChangeNameAfterMunmap) {
  char name[] = "hello world!";
  char buf[256];
  auto fd = CreateRegion(name, PAGE_SIZE);

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, "goodbye"), SyscallFailsWithErrno(EINVAL));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_GET_NAME, buf), SyscallSucceeds());
  EXPECT_STREQ("hello world!", buf);
}

// Once mmap() has been called if there was no name before, there will never be a name
TEST_F(AshmemTest, NoSetNameAfterMap) {
  auto fd = CreateRegion(nullptr, PAGE_SIZE);

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, "test"), SyscallFailsWithErrno(EINVAL));
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// A bad pointer passed to set name ioctl will fail
TEST_F(AshmemTest, MalformedName) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, 0), SyscallFailsWithErrno(EFAULT));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, 1234), SyscallFailsWithErrno(EFAULT));
}

// Truncate names with length 256 or greater
TEST_F(AshmemTest, SetNameOverflow) {
  char name[300];
  char buf[300];
  memset(name, 'e', 300);  // Long string of 'e' s
  name[299] = '\0';        // Null terminator

  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_NAME, name), SyscallSucceeds());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_GET_NAME, buf), SyscallSucceeds());
  EXPECT_EQ(255, (int)strlen(buf));  // Truncated string
}

// You are not allowed to increase protections
TEST_F(AshmemTest, NoIncreaseProtections) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ), SyscallSucceeds());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_WRITE), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ | PROT_WRITE),
              SyscallFailsWithErrno(EINVAL));
}

// You are allowed to decrease protections, but changes are not retroactive
TEST_F(AshmemTest, DecreaseProtections) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallSucceeds());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ | PROT_WRITE), SyscallSucceeds());

  const size_t kMapSize = PAGE_SIZE;
  void *addr_rw = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr_rw != MAP_FAILED && addr_rw != nullptr);

  // Decrease protections
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ), SyscallSucceeds());
  void *addr_r = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr_r != MAP_FAILED && addr_r != nullptr);

  // Not retroactive; we can still write through addr_rw
  *(int *)addr_rw = 123;  // This would segfault if changes were retroactive
  EXPECT_EQ(123, *(int *)addr_rw);

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_GET_PROT_MASK), SyscallSucceedsWithValue(PROT_READ));

  ASSERT_THAT(munmap(addr_rw, kMapSize), SyscallSucceeds());
  ASSERT_THAT(munmap(addr_r, kMapSize), SyscallSucceeds());
}

// Mapping with protections not allowed by the region will fail
TEST_F(AshmemTest, MapProtectionsAgree) {
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;

  // PROT_READ | PROT_WRITE
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ | PROT_WRITE), SyscallSucceeds());
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());

  addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());

  addr = mmap(nullptr, kMapSize, PROT_WRITE, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());

  // PROT_READ
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ), SyscallSucceeds());

  addr = mmap(nullptr, kMapSize, PROT_WRITE, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr == MAP_FAILED && errno == EINVAL);
}

// The set protections ioctl will fail on malformed input
TEST_F(AshmemTest, MalformedProtections) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, 8), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, 27), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, 1002), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, "hello"), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, -1), SyscallFailsWithErrno(EINVAL));
}

TEST_F(AshmemTest, MapPrivate) {
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;

  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd.get(), 0);
  EXPECT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// No fileops are allowed until after the region has been mapped
TEST_F(AshmemTest, NoFileOpBeforeMap) {
  char in[] = "hello world!";
  char out[256];
  auto fd = CreateRegion(nullptr, PAGE_SIZE);

  EXPECT_THAT(write(fd.get(), in, sizeof(in)), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(read(fd.get(), out, sizeof(in)), SyscallFailsWithErrno(EBADF));
  EXPECT_THAT(lseek(fd.get(), 0, SEEK_SET), SyscallFailsWithErrno(EBADF));
}

// Writing fails no matter what
TEST_F(AshmemTest, WriteFileOp) {
  char in[] = "hello world!";
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(write(fd.get(), in, sizeof(in)), SyscallFailsWithErrno(EINVAL));
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Read() is okay after mapping
TEST_F(AshmemTest, ReadFileOp) {
  char in[] = "hello world!";
  char out[256] = {0};
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;

  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  strcpy((char *)addr, in);

  EXPECT_THAT(read(fd.get(), out, sizeof(in)), SyscallSucceedsWithValue(sizeof(in)));
  EXPECT_STREQ(out, in);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Lseek()-- adapted from android KernelLibcutilsTest
// [   Zero   ][       Data        ][   Zero   ]
TEST_F(AshmemTest, LseekFileOp) {
  auto fd = CreateRegion(nullptr, 4 * PAGE_SIZE);

  const size_t kMapSize = 4 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  uint8_t *data = (uint8_t *)addr;

  // Initialize ashmem

  memset(data, 0, PAGE_SIZE);
  memset(data + PAGE_SIZE, 1, 2 * PAGE_SIZE);
  memset(data + 3 * PAGE_SIZE, 0, PAGE_SIZE);

  EXPECT_THAT(lseek(fd.get(), 99, SEEK_SET), SyscallSucceedsWithValue(99));
  EXPECT_THAT(lseek(fd.get(), PAGE_SIZE, SEEK_CUR), SyscallSucceedsWithValue(PAGE_SIZE + 99));
  // 0, SEEK_DATA
  EXPECT_THAT(lseek(fd.get(), -99, SEEK_CUR), SyscallSucceedsWithValue(PAGE_SIZE));
  // PAGE_SIZE, SEEK_HOLE
  EXPECT_THAT(lseek(fd.get(), 2 * PAGE_SIZE, SEEK_CUR), SyscallSucceedsWithValue(3 * PAGE_SIZE));
  EXPECT_THAT(lseek(fd.get(), -99, SEEK_END), SyscallSucceedsWithValue(4 * PAGE_SIZE - 99));
  EXPECT_THAT(lseek(fd.get(), static_cast<int>(-1 * PAGE_SIZE), SEEK_CUR),
              SyscallSucceedsWithValue(3 * PAGE_SIZE - 99));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Can read() without PROT_READ
TEST_F(AshmemTest, ReadFileOpProt) {
  char in[] = "hello world!";
  char out[256] = {0};
  auto fd = CreateRegion(nullptr, PAGE_SIZE);

  int status = ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_WRITE | PROT_EXEC);
  ASSERT_EQ(0, status);

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  strcpy((char *)addr, in);

  EXPECT_THAT(read(fd.get(), out, sizeof(in)), SyscallSucceedsWithValue(sizeof(in)));
  EXPECT_STREQ(out, in);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// File offset for read and lseek is local per ashmem region
TEST_F(AshmemTest, FileOffsetLocal) {
  char in[] = "hello world!";
  char out_1[256] = {0};
  char out_2[256] = {0};
  auto fd_1 = CreateRegion(nullptr, PAGE_SIZE);
  auto fd_2 = CreateRegion(nullptr, PAGE_SIZE);

  const size_t kMapSize = PAGE_SIZE;
  void *addr_1 = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd_1.get(), 0);
  ASSERT_TRUE(addr_1 != MAP_FAILED && addr_1 != nullptr);
  void *addr_2 = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd_2.get(), 0);
  ASSERT_TRUE(addr_2 != MAP_FAILED && addr_2 != nullptr);

  strcpy((char *)addr_1, in);
  strcpy((char *)addr_2, in);

  // 1) "hello world"
  //          ^
  // 2) "hello world"
  //     ^
  ASSERT_THAT(lseek(fd_1.get(), 6, SEEK_CUR), SyscallSucceedsWithValue(6));

  // Read and fill output buffers
  ASSERT_THAT(read(fd_2.get(), out_2, 5), SyscallSucceedsWithValue(5));
  ASSERT_THAT(read(fd_1.get(), out_1, 5), SyscallSucceedsWithValue(5));

  EXPECT_STREQ(out_1, "world");
  EXPECT_STREQ(out_2, "hello");

  ASSERT_THAT(munmap(addr_1, kMapSize), SyscallSucceeds());
  ASSERT_THAT(munmap(addr_2, kMapSize), SyscallSucceeds());
}

// fstat() reports zero size for an ashmem region no matter what
TEST_F(AshmemTest, StSizeAlwaysZero) {
  struct stat st;
  auto fd = CreateRegion(nullptr, PAGE_SIZE);

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(fstat(fd.get(), &st), SyscallSucceeds());
  EXPECT_EQ(0, st.st_size);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// No pinning permitted unless previously mapped
TEST_F(AshmemTest, NoPinBeforeMap) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};

  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin), SyscallFailsWithErrno(EINVAL));

  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Ashmem regions can still be pinned & unpinned even when not mapped
TEST_F(AshmemTest, PinAfterMunmap) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
}

// Ashmem regions are pinned by default
TEST_F(AshmemTest, DefaultPin) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};

  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin),
              SyscallSucceedsWithValue(ASHMEM_IS_PINNED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Create a region with two unpinned sub-regions and verify the state
//      [  Unpin   ][        Pin        ][  Unpin   ]
TEST_F(AshmemTest, BasicPinBehavior) {
  ashmem_pin pin_left = {.offset = 0, .len = PAGE_SIZE};
  ashmem_pin pin_right = {.offset = 3 * PAGE_SIZE, .len = PAGE_SIZE};
  ashmem_pin pin_middle = {.offset = PAGE_SIZE, .len = 2 * PAGE_SIZE};

  auto fd = CreateRegion(nullptr, 4 * PAGE_SIZE);
  const size_t kMapSize = 4 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin_left),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin_right),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin_left),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin_middle),
              SyscallSucceedsWithValue(ASHMEM_IS_PINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin_right),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Fail to pin, unpin, and get the state of a region out of bounds
TEST_F(AshmemTest, NoPinOutOfBounds) {
  ashmem_pin pin_out_of_bounds = {.offset = 2 * PAGE_SIZE, .len = PAGE_SIZE};
  ashmem_pin pin_overflow = {.offset = 0, .len = 2 * PAGE_SIZE};

  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin_out_of_bounds), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin_out_of_bounds), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin_out_of_bounds),
              SyscallFailsWithErrno(EINVAL));

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin_overflow), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin_overflow), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin_overflow), SyscallFailsWithErrno(EINVAL));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Fail to pin, unpin, and get the state of a misaligned region
TEST_F(AshmemTest, NoPinMisaligned) {
  ashmem_pin pin = {.offset = 1, .len = PAGE_SIZE};

  auto fd = CreateRegion(nullptr, 2 * PAGE_SIZE);
  const size_t kMapSize = 2 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallFailsWithErrno(EINVAL));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin), SyscallFailsWithErrno(EINVAL));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Overlap a bunch of pins and unpins
//
//  (3)   [   U   ]                  [   U   ]                           [U]
//  (2)         [             ]         [                   ]
//  (1)   [ ][ ][ ][ ][U][U][U][U][U][U][U][U][U][ ][ ][ ][U][ ][ ][ ][ ][ ][ ][ ][ ]
//         0           4              9             14              19             24
//
//  --->  [   U   ][          ][       U     ][                         ][U][       ]
//
TEST_F(AshmemTest, MessyPinning) {
  // Data corresponding to ioctl command arguments of each round
  ashmem_pin_op rounds[] = {
      // Round #1
      {.range = {.offset = 0, .len = 4 * PAGE_SIZE},
       .op = ASHMEM_PIN,
       .expected = ASHMEM_NOT_PURGED},
      {.range = {.offset = 4 * PAGE_SIZE, .len = 9 * PAGE_SIZE},
       .op = ASHMEM_UNPIN,
       .expected = ASHMEM_IS_UNPINNED},
      {.range = {.offset = 13 * PAGE_SIZE, .len = 3 * PAGE_SIZE},
       .op = ASHMEM_PIN,
       .expected = ASHMEM_NOT_PURGED},
      {.range = {.offset = 16 * PAGE_SIZE, .len = PAGE_SIZE},
       .op = ASHMEM_UNPIN,
       .expected = ASHMEM_IS_UNPINNED},
      {.range = {.offset = 17 * PAGE_SIZE, .len = 8 * PAGE_SIZE},
       .op = ASHMEM_PIN,
       .expected = ASHMEM_NOT_PURGED},

      // Round #2
      {.range = {.offset = 2 * PAGE_SIZE, .len = 5 * PAGE_SIZE},
       .op = ASHMEM_PIN,
       .expected = ASHMEM_NOT_PURGED},
      {.range = {.offset = 10 * PAGE_SIZE, .len = 7 * PAGE_SIZE},
       .op = ASHMEM_PIN,
       .expected = ASHMEM_NOT_PURGED},

      // Round #3
      {.range = {.offset = 0, .len = 3 * PAGE_SIZE},
       .op = ASHMEM_UNPIN,
       .expected = ASHMEM_IS_UNPINNED},
      {.range = {.offset = 9 * PAGE_SIZE, .len = 3 * PAGE_SIZE},
       .op = ASHMEM_UNPIN,
       .expected = ASHMEM_IS_UNPINNED},
      {.range = {.offset = 21 * PAGE_SIZE, .len = PAGE_SIZE},
       .op = ASHMEM_UNPIN,
       .expected = ASHMEM_IS_UNPINNED},
  };

  ashmem_pin_op verify[] = {
      {.range = {.offset = 0, .len = 3 * PAGE_SIZE},
       .op = ASHMEM_GET_PIN_STATUS,
       .expected = ASHMEM_IS_UNPINNED},
      {.range = {.offset = 3 * PAGE_SIZE, .len = 4 * PAGE_SIZE},
       .op = ASHMEM_GET_PIN_STATUS,
       .expected = ASHMEM_IS_PINNED},
      {.range = {.offset = 7 * PAGE_SIZE, .len = 5 * PAGE_SIZE},
       .op = ASHMEM_GET_PIN_STATUS,
       .expected = ASHMEM_IS_UNPINNED},
      {.range = {.offset = 12 * PAGE_SIZE, .len = 9 * PAGE_SIZE},
       .op = ASHMEM_GET_PIN_STATUS,
       .expected = ASHMEM_IS_PINNED},
      {.range = {.offset = 21 * PAGE_SIZE, .len = PAGE_SIZE},
       .op = ASHMEM_GET_PIN_STATUS,
       .expected = ASHMEM_IS_UNPINNED},
      {.range = {.offset = 22 * PAGE_SIZE, .len = 3 * PAGE_SIZE},
       .op = ASHMEM_GET_PIN_STATUS,
       .expected = ASHMEM_IS_PINNED},
  };

  auto fd = CreateRegion(nullptr, 25 * PAGE_SIZE);
  const size_t kMapSize = 25 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  // Apply pins and unpins
  for (const auto &operation : rounds) {
    ASSERT_THAT(ioctl(fd.get(), operation.op, &operation.range),
                SyscallSucceedsWithValue(operation.expected));
  }

  // Verify all ranges
  for (const auto &operation : verify) {
    EXPECT_THAT(ioctl(fd.get(), operation.op, &operation.range),
                SyscallSucceedsWithValue(operation.expected));
  }

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// ASHMEM_GET_PIN_STATUS is sensitive to overlap
TEST_F(AshmemTest, PinStatusOverlap) {
  ashmem_pin pin_left = {.offset = 0, .len = PAGE_SIZE};
  ashmem_pin pin_total = {.offset = 0, .len = 2 * PAGE_SIZE};

  auto fd = CreateRegion(nullptr, 2 * PAGE_SIZE);
  const size_t kMapSize = 2 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin_left),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, &pin_total),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Unsigned integer overflow with pin logic
TEST_F(AshmemTest, PinUnsignedOverflow) {
  ashmem_pin pin = {.offset = 2 * PAGE_SIZE, .len = (uint32_t)1048575 * PAGE_SIZE};

  auto fd = CreateRegion(nullptr, 4 * PAGE_SIZE);
  const size_t kMapSize = 4 * PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  int status = ioctl(fd.get(), ASHMEM_PIN, pin);
  EXPECT_EQ(-1, status);
  EXPECT_EQ(EFAULT, errno);
  status = ioctl(fd.get(), ASHMEM_UNPIN, pin);
  EXPECT_EQ(-1, status);
  EXPECT_EQ(EFAULT, errno);
  status = ioctl(fd.get(), ASHMEM_GET_PIN_STATUS, pin);
  EXPECT_EQ(-1, status);
  EXPECT_EQ(EFAULT, errno);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// No purging permitted unless previously mapped
TEST_F(AshmemTest, NoPurgeBeforeMap) {
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr), SyscallFailsWithErrno(EINVAL));
}

// Basic memory purge functionality
TEST_F(AshmemTest, Purge) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceedsWithValue(ASHMEM_WAS_PURGED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Purge when no intervals are unpinned
TEST_F(AshmemTest, PinAndPurge) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};
  const size_t kMapSize = PAGE_SIZE;
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
              SyscallSucceedsWithValue(ASHMEM_IS_PINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Unpinning a purged region
TEST_F(AshmemTest, PugeAndUnpin) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Purge a region, pin it, then try to purge it again
TEST_F(AshmemTest, PurgeTwice) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceedsWithValue(ASHMEM_WAS_PURGED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
              SyscallSucceedsWithValue(ASHMEM_IS_PINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Purge a region which is half unpinned
TEST_F(AshmemTest, PurgeOverlap) {
  ashmem_pin pin_left = {.offset = 0, .len = PAGE_SIZE};
  ashmem_pin pin_right = {.offset = PAGE_SIZE, .len = PAGE_SIZE};
  ashmem_pin pin_total = {.offset = 0, .len = 2 * PAGE_SIZE};

  auto fd = CreateRegion(nullptr, 2 * PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin_left),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin_right), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));

  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin_total), SyscallSucceedsWithValue(ASHMEM_WAS_PURGED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin_left), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Purged regions are zeroed out
TEST_F(AshmemTest, PurgeIsZeroed) {
  char in[] = "hello world!";
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};

  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
  char *out = (char *)addr;

  // Fill with data
  strcpy(out, in);
  // Verify
  EXPECT_STREQ(in, out);

  // Purge region
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin), SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));

  // Verify data has been zeroed out
  for (size_t i = 0; i < sizeof(in); i++) {
    EXPECT_EQ('\0', out[i]);
  }

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Input to a getter command is ignored
TEST_F(AshmemTest, IgnoreGetterInput) {
  ashmem_pin pin = {.offset = 0, .len = PAGE_SIZE};

  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_WRITE), SyscallSucceeds());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin), SyscallSucceeds());

  // Protections
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PROT_MASK, 10), SyscallSucceedsWithValue(PROT_WRITE));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_PROT_MASK, "hello"), SyscallSucceedsWithValue(PROT_WRITE));

  // Size
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_SIZE, 10), SyscallSucceedsWithValue(PAGE_SIZE));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_SIZE, "hello"), SyscallSucceedsWithValue(PAGE_SIZE));

  // Purge
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, 10),
              SyscallSucceedsWithValue(ASHMEM_IS_PINNED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, "hello"),
              SyscallSucceedsWithValue(ASHMEM_IS_PINNED));

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Fork, child writes data, parent reads
TEST_F(AshmemTest, Fork) {
  char input[] = "hello world!";
  auto fd = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *parent_map = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(parent_map != MAP_FAILED && parent_map != nullptr);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    void *child_map = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
    ASSERT_TRUE(child_map != MAP_FAILED && child_map != nullptr);
    char *shared_msg = (char *)child_map;
    strcpy(shared_msg, input);
    shared_msg[sizeof(input)] = '\0';
    ASSERT_THAT(munmap(child_map, kMapSize), SyscallSucceeds());
  });

  ASSERT_TRUE(helper.WaitForChildren());
  EXPECT_STREQ((char *)parent_map, input);
  ASSERT_THAT(munmap(parent_map, kMapSize), SyscallSucceeds());
}

// Fork, fail to set size in parent after child maps the region
TEST_F(AshmemTest, ForkSetSize) {
  auto fd = Open();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, PAGE_SIZE), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, 2 * PAGE_SIZE), SyscallSucceeds());
    const size_t kMapSize = 2 * PAGE_SIZE;
    void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
    ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
    ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
  });

  ASSERT_TRUE(helper.WaitForChildren());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_SET_SIZE, 3 * PAGE_SIZE), SyscallFailsWithErrno(EINVAL));
}

// Fork, child purges memory, parent can see it has been purged
TEST_F(AshmemTest, ForkPurge) {
  ashmem_pin pin_left = {.offset = 0, .len = PAGE_SIZE};
  ashmem_pin pin_right = {.offset = PAGE_SIZE, .len = PAGE_SIZE};

  auto fd = CreateRegion(nullptr, 2 * PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *parent_map = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(parent_map != MAP_FAILED && parent_map != nullptr);
  ASSERT_THAT(ioctl(fd.get(), ASHMEM_UNPIN, &pin_left),
              SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(ioctl(fd.get(), ASHMEM_PURGE_ALL_CACHES, nullptr),
                SyscallSucceedsWithValue(ASHMEM_IS_UNPINNED));
  });

  ASSERT_TRUE(helper.WaitForChildren());
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin_left), SyscallSucceedsWithValue(ASHMEM_WAS_PURGED));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_PIN, &pin_right), SyscallSucceedsWithValue(ASHMEM_NOT_PURGED));
  ASSERT_THAT(munmap(parent_map, kMapSize), SyscallSucceeds());
}

// Fork, child reduces permissions, parent is affected
TEST_F(AshmemTest, ForkProt) {
  auto fd = CreateRegion(0, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr_rw = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr_rw != MAP_FAILED && addr_rw != nullptr);
  ASSERT_THAT(munmap(addr_rw, kMapSize), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess(
      [&] { ASSERT_THAT(ioctl(fd.get(), ASHMEM_SET_PROT_MASK, PROT_READ), SyscallSucceeds()); });

  ASSERT_TRUE(helper.WaitForChildren());

  // Another map fails
  addr_rw = mmap(nullptr, 2 * PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr_rw == MAP_FAILED);

  // Reduced prot map succeeds
  void *addr_r = mmap(nullptr, kMapSize, PROT_READ, MAP_SHARED, fd.get(), 0);
  EXPECT_TRUE(addr_r != MAP_FAILED && addr_r != nullptr);
  ASSERT_THAT(munmap(addr_r, kMapSize), SyscallSucceeds());
}

// Fork, child lseeks, parent is affected
TEST_F(AshmemTest, ForkLseek) {
  char in[] = "hello world";
  char out[256] = {0};
  auto fd = CreateRegion(0, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  strcpy((char *)addr, in);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] { ASSERT_THAT(lseek(fd.get(), 6, SEEK_CUR), SyscallSucceeds()); });

  ASSERT_TRUE(helper.WaitForChildren());
  ASSERT_THAT(read(fd.get(), out, 5), SyscallSucceedsWithValue(5));

  EXPECT_STREQ("world", out);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Fork, child writes, parent calls read()
TEST_F(AshmemTest, ForkRead) {
  char in[] = "hello world";
  char out[256] = {0};
  auto fd = CreateRegion(0, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] { strcpy((char *)addr, in); });

  ASSERT_TRUE(helper.WaitForChildren());
  ASSERT_THAT(read(fd.get(), out, 5), SyscallSucceedsWithValue(5));

  EXPECT_STREQ("hello", out);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
}

// Ashmem regions are backed by independent VMOs
TEST_F(AshmemTest, DistinctAshmemVMO) {
  auto fd_1 = CreateRegion(nullptr, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr_1 = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd_1.get(), 0);
  ASSERT_TRUE(addr_1 != MAP_FAILED && addr_1 != nullptr);
  int *data_1 = (int *)addr_1;

  auto fd_2 = CreateRegion(nullptr, PAGE_SIZE);
  void *addr_2 = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd_2.get(), 0);
  ASSERT_TRUE(addr_2 != MAP_FAILED && addr_1 != nullptr);
  int *data_2 = (int *)addr_2;

  *data_1 = 1;
  *data_2 = 2;

  EXPECT_EQ(1, *data_1);
  EXPECT_EQ(2, *data_2);

  ASSERT_THAT(munmap(addr_1, kMapSize), SyscallSucceeds());
  ASSERT_THAT(munmap(addr_2, kMapSize), SyscallSucceeds());
}

// Ashmem inode number is hidden from fstat()
TEST_F(AshmemTest, HiddenInos) {
  auto fd_1 = Open();
  auto fd_2 = Open();

  struct stat st_1;
  struct stat st_2;

  ASSERT_THAT(fstat(fd_1.get(), &st_1), SyscallSucceeds());
  ASSERT_THAT(fstat(fd_2.get(), &st_2), SyscallSucceeds());

  EXPECT_EQ(st_1.st_ino, st_2.st_ino);
}

TEST_F(AshmemTest, DistinctFileIDs) {
  auto fd_1 = Open();
  auto fd_2 = Open();

  long ino_1;
  long ino_2;

  ASSERT_THAT(ioctl(fd_1.get(), ASHMEM_GET_FILE_ID, &ino_1), SyscallSucceeds());
  ASSERT_THAT(ioctl(fd_2.get(), ASHMEM_GET_FILE_ID, &ino_2), SyscallSucceeds());

  EXPECT_NE(ino_1, ino_2);
}

TEST_F(AshmemTest, MalformedFileIDs) {
  auto fd = Open();
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_FILE_ID, 0), SyscallFailsWithErrno(EFAULT));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_FILE_ID, 10), SyscallFailsWithErrno(EFAULT));
  EXPECT_THAT(ioctl(fd.get(), ASHMEM_GET_FILE_ID, "hello"), SyscallFailsWithErrno(EFAULT));
}

// Ashmem region name written as entry to /proc/<pid>/maps
TEST_F(AshmemTest, ProcMaps) {
  char name[] = "hello";
  auto fd = CreateRegion(name, PAGE_SIZE);
  const size_t kMapSize = PAGE_SIZE;
  void *addr = mmap(nullptr, kMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_TRUE(addr != MAP_FAILED && addr != nullptr);
  std::ifstream proc_maps("/proc/self/maps", std::ios::in);
  ASSERT_TRUE(proc_maps.good());

  std::string line;
  bool has_ashmap = false;

  while (getline(proc_maps, line)) {
    if (cpp23::contains(line, "/dev/ashmem/hello")) {
      has_ashmap = true;
      break;
    }
  }

  EXPECT_TRUE(has_ashmap);

  ASSERT_THAT(munmap(addr, kMapSize), SyscallSucceeds());
  proc_maps.close();
}

}  // namespace
