// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/elfldltl/testing/test-pipe-reader.h"

#include <fcntl.h>
#include <unistd.h>

#include <gtest/gtest.h>

namespace elfldltl::testing {

constexpr char kFillByte = 0x55;

void TestPipeReader::Init(fbl::unique_fd& write_pipe) {
  int fds[2];
  ASSERT_EQ(0, pipe(fds)) << "pipe: " << strerror(errno);
  read_pipe_.reset(fds[0]);
  write_pipe.reset(fds[1]);
  ASSERT_EQ(fcntl(read_pipe_.get(), F_SETFD, FD_CLOEXEC), 0) << strerror(errno);
  ASSERT_EQ(fcntl(write_pipe.get(), F_SETFD, FD_CLOEXEC), 0) << strerror(errno);
  pipe_buf_size_ = fpathconf(read_pipe_.get(), _PC_PIPE_BUF);
  ASSERT_GT(pipe_buf_size_, 0u);
  thread_ = std::thread(&TestPipeReader::ReaderThread, this);
}

TestPipeReader::~TestPipeReader() {
  if (thread_.joinable()) {
    ADD_FAILURE() << "elfldltl::testing::TestPipeReader::Finish() not called";
    // Join the thread so its destructor doesn't panic.
    thread_.join();
  }
}

// The reader thread will append everything read from the pipe to the string.
void TestPipeReader::ReaderThread() {
  ssize_t n;
  do {
    size_t contents_size = contents_.size();
    contents_.append(pipe_buf_size_, kFillByte);
    n = read(read_pipe_.get(), &contents_[contents_size], pipe_buf_size_);
    contents_.resize(contents_size + std::max<ssize_t>(n, 0));
    ASSERT_GE(n, 0) << "read: " << strerror(errno);
  } while (n > 0);
}

}  // namespace elfldltl::testing
