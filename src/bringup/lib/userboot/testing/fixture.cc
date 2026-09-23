// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/userboot/testing/fixture.h"

namespace userboot::testing {

Launcher Fixture::launcher_;

void Fixture::SetUpTestSuite() {
  auto launcher = Launcher::Create();
  ASSERT_TRUE(launcher.is_ok());
  launcher_ = *std::move(launcher);
}

void Fixture::TearDownTestSuite() { launcher_ = {}; }

void Fixture::SetUp() {
  ASSERT_FALSE(job_);
  ASSERT_NO_FATAL_FAILURE(job_.Init());
  ASSERT_FALSE(log_fd_);
  ASSERT_NO_FATAL_FAILURE(log_.Init(log_fd_));
}

fbl::unique_fd Fixture::TakeLogFd() {
  EXPECT_TRUE(log_fd_);
  return std::exchange(log_fd_, {});
}

std::string Fixture::FinishLog() { return std::move(log_).Finish(); }

void Fixture::Launch(zx::vmo vmo, std::vector<zx::handle> handles) {
  auto result = launcher().Launch(job_.Get(), std::move(vmo), TakeLogFd(), std::move(handles));
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  process_ = *std::move(result);
}

void Fixture::Launch(const char* file, std::vector<zx::handle> handles) {
  auto vmo = userboot::testing::GetExecutable(file);
  ASSERT_TRUE(vmo.is_ok()) << vmo.status_string();
  Launch(*std::move(vmo), std::move(handles));
}

zx::result<int64_t> Fixture::Wait() {
  EXPECT_TRUE(process_);
  return WaitForTermination(std::exchange(process_, {}).borrow());
}

}  // namespace userboot::testing
