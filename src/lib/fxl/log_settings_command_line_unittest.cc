// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/fxl/log_settings_command_line.h"

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/fxl/command_line.h"

namespace fxl {
namespace {

class LogSettingsFixture : public ::testing::Test {
 public:
  LogSettingsFixture()
      : old_severity_(fuchsia_logging::GetMinLogSeverity()), old_stderr_(dup(STDERR_FILENO)) {}
  ~LogSettingsFixture() {
    fuchsia_logging::LogSettingsBuilder builder;
    builder.WithMinLogSeverity(old_severity_).BuildAndInitialize();
    dup2(old_stderr_.get(), STDERR_FILENO);
  }

 private:
  fuchsia_logging::RawLogSeverity old_severity_;
  fbl::unique_fd old_stderr_;
};

TEST(LogSettings, ParseValidOptions) {
  fxl::LogSettings settings;
  settings.min_log_level = FUCHSIA_LOG_FATAL;

  EXPECT_TRUE(ParseLogSettings(CommandLineFromInitializerList({"argv0"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);

  EXPECT_TRUE(ParseLogSettings(CommandLineFromInitializerList({"argv0", "--quiet=0"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_INFO, settings.min_log_level);

  EXPECT_TRUE(ParseLogSettings(CommandLineFromInitializerList({"argv0", "--quiet"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_WARNING, settings.min_log_level);

  EXPECT_TRUE(ParseLogSettings(CommandLineFromInitializerList({"argv0", "--quiet=3"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=TRACE"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_TRACE, settings.min_log_level);

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=DEBUG"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_DEBUG, settings.min_log_level);

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=INFO"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_INFO, settings.min_log_level);

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=WARNING"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_WARNING, settings.min_log_level);

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=ERROR"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_ERROR, settings.min_log_level);

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=FATAL"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);
#ifndef __Fuchsia__
  EXPECT_TRUE(ParseLogSettings(
      CommandLineFromInitializerList({"argv0", "--log-file=/tmp/custom.log"}), &settings));
  EXPECT_EQ("/tmp/custom.log", settings.log_file);
#endif
}

TEST(LogSettings, ParseInvalidOptions) {
  fxl::LogSettings settings;
  settings.min_log_level = FUCHSIA_LOG_FATAL;

  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);

  EXPECT_FALSE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--quiet=-1"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);

  EXPECT_FALSE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--quiet=123garbage"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);

  EXPECT_FALSE(ParseLogSettings(
      CommandLineFromInitializerList({"argv0", "--severity=TRACEgarbage"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);

  EXPECT_FALSE(ParseLogSettings(
      CommandLineFromInitializerList({"argv0", "--severity=TRACE --quiet=1"}), &settings));
  EXPECT_EQ(FUCHSIA_LOG_FATAL, settings.min_log_level);
}

TEST_F(LogSettingsFixture, ToArgv) {
  fxl::LogSettings settings;
  EXPECT_TRUE(LogSettingsToArgv(settings).empty());

  EXPECT_TRUE(ParseLogSettings(CommandLineFromInitializerList({"argv0", "--quiet"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--severity=WARNING"});

  EXPECT_TRUE(ParseLogSettings(CommandLineFromInitializerList({"argv0", "--quiet=3"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--severity=FATAL"});

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=TRACE"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--severity=TRACE"});

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=DEBUG"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--severity=DEBUG"});

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=WARNING"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--severity=WARNING"});

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=ERROR"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--severity=ERROR"});

  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--severity=FATAL"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--severity=FATAL"});

  // Reset |settings| back to defaults so we don't pick up previous tests.
  settings = fxl::LogSettings{};
#ifndef __Fuchsia__
  EXPECT_TRUE(
      ParseLogSettings(CommandLineFromInitializerList({"argv0", "--log-file=/foo"}), &settings));
  EXPECT_TRUE(LogSettingsToArgv(settings) == std::vector<std::string>{"--log-file=/foo"})
      << LogSettingsToArgv(settings)[0];
#endif
}

}  // namespace
}  // namespace fxl
