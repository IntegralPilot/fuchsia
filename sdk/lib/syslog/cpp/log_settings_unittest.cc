// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace fuchsia_logging {
namespace {

class LogSettingsFixture : public ::testing::Test {
 public:
  LogSettingsFixture() : old_severity_(GetMinLogSeverity()), old_stderr_(dup(STDERR_FILENO)) {}
  ~LogSettingsFixture() {
    LogSettingsBuilder builder;
    builder.WithMinLogSeverity(old_severity_);
    builder.BuildAndInitialize();
    dup2(old_stderr_.get(), STDERR_FILENO);
  }

 private:
  fuchsia_logging::RawLogSeverity old_severity_;
  fbl::unique_fd old_stderr_;
};
#ifndef __Fuchsia__
TEST_F(LogSettingsFixture, SetAndGet) {
  LogSettingsBuilder builder;
  builder.WithMinLogSeverity(5);
  builder.BuildAndInitialize();
  EXPECT_EQ(5, GetMinLogSeverity());
}

TEST_F(LogSettingsFixture, SetValidLogFile) {
  const char kTestMessage[] = "TEST MESSAGE";

  files::ScopedTempDir temp_dir;
  std::string log_file;
  ASSERT_TRUE(temp_dir.NewTempFile(&log_file));
  LogSettingsBuilder builder;
  builder.WithLogFile(log_file);
  builder.BuildAndInitialize();
  FX_LOGS(INFO) << kTestMessage;

  ASSERT_EQ(0, access(log_file.c_str(), R_OK));
  std::string log;
  ASSERT_TRUE(files::ReadFileToString(log_file, &log));
  EXPECT_TRUE(log.find(kTestMessage) != std::string::npos);
}

TEST_F(LogSettingsFixture, SetInvalidLogFile) {
  std::string invalid_path = "\\\\//invalid-path";
  LogSettingsBuilder builder;
  builder.WithLogFile(invalid_path);
  builder.BuildAndInitialize();

  EXPECT_NE(0, access(invalid_path.c_str(), R_OK));
}
#endif

}  // namespace
}  // namespace fuchsia_logging
