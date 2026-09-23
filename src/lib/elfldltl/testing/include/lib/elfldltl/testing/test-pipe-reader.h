// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_TEST_PIPE_READER_H_
#define SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_TEST_PIPE_READER_H_

#include <concepts>
#include <string>
#include <thread>

#include <fbl/unique_fd.h>

namespace elfldltl::testing {

class TestPipeReader {
 public:
  TestPipeReader() = default;

  // The object cannot be safely moved after Init() since the reader thread
  // will use its this pointer to the members.
  TestPipeReader(TestPipeReader&&) = delete;
  TestPipeReader& operator=(TestPipeReader&&) = delete;

  // This creates a pipe and yields the write half.
  void Init(fbl::unique_fd& write_pipe);

  // This must be called before destruction and nothing else after it.
  std::string Finish() && {
    thread_.join();
    return std::move(contents_);
  }

  ~TestPipeReader();

 private:
  void ReaderThread();

  std::string contents_;
  fbl::unique_fd read_pipe_;
  size_t pipe_buf_size_;
  std::thread thread_;
};
static_assert(!std::copyable<TestPipeReader>);
static_assert(!std::movable<TestPipeReader>);

}  // namespace elfldltl::testing

#endif  // SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_TEST_PIPE_READER_H_
