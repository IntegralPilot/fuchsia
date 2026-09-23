// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_FAULT_TEST_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_FAULT_TEST_H_

#include <fcntl.h>
#include <sys/mman.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

class FaultTest : public testing::Test {
 protected:
  static void SetUpTestSuite();
  static void TearDownTestSuite();

  static constexpr size_t kFaultingSize_ = 987;
  static void* faulting_ptr_;
};

class FaultFileTest : public FaultTest, public testing::WithParamInterface<int (*)()> {
 protected:
  void SetUp() override;

  void TearDown() override;

  void SetFdNonBlocking();

  const fbl::unique_fd& fd() { return fd_; }

 private:
  fbl::unique_fd fd_;
};

void* FaultTest::faulting_ptr_ = nullptr;

void FaultTest::SetUpTestSuite() {
  faulting_ptr_ = mmap(nullptr, kFaultingSize_, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(faulting_ptr_, MAP_FAILED);
}

void FaultTest::TearDownTestSuite() {
  faulting_ptr_ = mmap(nullptr, kFaultingSize_, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(faulting_ptr_, MAP_FAILED);
}

void FaultFileTest::SetUp() {
  auto f = GetParam();
  ASSERT_TRUE(fd_ = fbl::unique_fd(f())) << strerror(errno);
}

void FaultFileTest::TearDown() { fd_.reset(); }

void FaultFileTest::SetFdNonBlocking() {
  int flags = fcntl(fd().get(), F_GETFL, 0);
  ASSERT_GE(flags, 0) << strerror(errno);
  ASSERT_EQ(fcntl(fd().get(), F_SETFL, flags | O_NONBLOCK), 0) << strerror(errno);
}

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_FAULT_TEST_H_
