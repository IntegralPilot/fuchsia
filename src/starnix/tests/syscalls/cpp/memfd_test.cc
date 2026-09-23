// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/uio.h>
#include <unistd.h>

#include <linux/memfd.h>

// Must be included after <linux/*.h> to avoid conflicts between Bionic UAPI and glibc headers.
// TODO(b/307959737): Build these tests without glibc.
#include <sys/mman.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "fault_test.h"
#include "fault_test_suite.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

#if !defined(__NR_memfd_create)
#if defined(__x86_64__)
#define __NR_memfd_create 319
#elif defined(__aarch64__) || defined(__riscv)
#define __NR_memfd_create 279
#elif defined(__arm__)
#define __NR_memfd_create 385
#else
#error Add support for this architecture
#endif
#endif  // !defined(__NR_memfd_create)

#if !defined(MFD_ALLOW_SEALING)
#define MFD_ALLOW_SEALING 0x0002U
#endif

#if !defined(MFD_NOEXEC_SEAL)
#define MFD_NOEXEC_SEAL 0x0008U
#endif

#if !defined(MFD_EXEC)
#define MFD_EXEC 0x0010U
#endif

#if !defined(F_SEAL_FUTURE_WRITE)
#define F_SEAL_FUTURE_WRITE 0x0010
#endif

#if !defined(F_ADD_SEALS)
#define F_ADD_SEALS 1033
#endif

#if !defined(F_SEAL_EXEC)
#define F_SEAL_EXEC 0x0020
#endif

namespace {

int CreateMemFd() {
  return static_cast<int>(syscall(__NR_memfd_create, "test_memfd", MFD_ALLOW_SEALING));
}

const int kPageSize = 4096;

class MemfdTest : public ::testing::Test {
  void SetUp() override { ASSERT_TRUE(fd_ = fbl::unique_fd(CreateMemFd())) << strerror(errno); }

  void TearDown() override {
    if (addr_ != MAP_FAILED) {
      ASSERT_EQ(munmap(addr_, kPageSize), 0);
    }
    fd_.reset();
  }

 protected:
  fbl::unique_fd fd_;
  void* addr_ = MAP_FAILED;
};

// These tests currently test only `F_SEAL_FUTURE_WRITE`. `F_SEAL_WRITE` is covered
// by GVisor tests.
TEST_F(MemfdTest, SealFutureWriteThenMmap) {
  ASSERT_EQ(fcntl(fd_.get(), F_ADD_SEALS, F_SEAL_FUTURE_WRITE), 0);

  // Readable mapping succeeds even when created from writable FD.
  addr_ = mmap(0, kPageSize, PROT_READ, MAP_SHARED, fd_.get(), 0);
  ASSERT_NE(addr_, MAP_FAILED);

  // `mprotect()` should fail since the file was sealed for write.
  ASSERT_EQ(mprotect(addr_, kPageSize, PROT_READ | PROT_WRITE), -1);
}

TEST_F(MemfdTest, MmapThenSealFutureWrite) {
  addr_ = mmap(0, kPageSize, PROT_READ, MAP_SHARED, fd_.get(), 0);
  ASSERT_NE(addr_, MAP_FAILED);

  ASSERT_EQ(fcntl(fd_.get(), F_ADD_SEALS, F_SEAL_FUTURE_WRITE), 0);

  // `mprotect()` still succeed since the mapping was created before the seal.
  ASSERT_EQ(mprotect(addr_, kPageSize, PROT_READ | PROT_WRITE), 0);
}

// These tests currently test `F_SEAL_FUTURE_WRITE`. `F_SEAL_WRITE` is covered
// by GVisor tests.
TEST_F(MemfdTest, MapWritableThenSealFutureWrite) {
  addr_ = mmap(0, kPageSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd_.get(), 0);
  ASSERT_NE(addr_, MAP_FAILED);

  // `F_SEAL_FUTURE_WRITE` should still succeed when there are writable mappings.
  ASSERT_EQ(fcntl(fd_.get(), F_ADD_SEALS, F_SEAL_FUTURE_WRITE), 0);
}

TEST_F(MemfdTest, ChmodWithNOEXEC) {
  auto fd = fbl::unique_fd(
      static_cast<int>(syscall(__NR_memfd_create, "memfd_no_exec", MFD_NOEXEC_SEAL)));

  // Setting the memfd that has the exec seal to executable should fail.
  ASSERT_EQ(fchmod(fd.get(), S_IRWXU | S_IRWXG | S_IRWXO), -1);

  // Setting the memfd that does not have the exec seal to executable should succeed.
  ASSERT_EQ(fchmod(fd_.get(), S_IRWXU | S_IRWXG | S_IRWXO), 0);
}

TEST_F(MemfdTest, MapExecutableWithNOEXEC) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on host in CQ";
  }

  auto fd = fbl::unique_fd(
      static_cast<int>(syscall(__NR_memfd_create, "memfd_no_exec", MFD_NOEXEC_SEAL)));

  // Map the fd with the noexec seal.
  addr_ = mmap(0, kPageSize, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_SHARED, fd.get(), 0);
  ASSERT_EQ(addr_, MAP_FAILED);

  // Map the default memfd that does not have the exec seal.
  addr_ = mmap(0, kPageSize, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_SHARED, fd_.get(), 0);
  ASSERT_NE(addr_, MAP_FAILED);
}

TEST_F(MemfdTest, MapNonExecutableWithNOEXEC) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on host in CQ";
  }
  auto fd = fbl::unique_fd(
      static_cast<int>(syscall(__NR_memfd_create, "memfd_no_exec", MFD_NOEXEC_SEAL)));
  addr_ = mmap(0, kPageSize, PROT_READ | PROT_WRITE, MAP_SHARED, fd.get(), 0);
  ASSERT_NE(addr_, MAP_FAILED);
}

TEST_F(MemfdTest, CreateExecutableThenSealThenMap) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on host in CQ";
  }
  auto fd = fbl::unique_fd(
      static_cast<int>(syscall(__NR_memfd_create, "memfd_no_exec", MFD_EXEC | MFD_ALLOW_SEALING)));
  ASSERT_EQ(fcntl(fd.get(), F_ADD_SEALS, F_SEAL_EXEC), 0);

  addr_ = mmap(0, kPageSize, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_SHARED, fd.get(), 0);
  ASSERT_EQ(addr_, MAP_FAILED);

  ASSERT_EQ(fchmod(fd.get(), S_IRWXU | S_IRWXG | S_IRWXO), -1);
}

INSTANTIATE_TEST_SUITE_P(MemfdFaultTest, FaultFileTest, ::testing::Values(CreateMemFd));

}  // namespace
