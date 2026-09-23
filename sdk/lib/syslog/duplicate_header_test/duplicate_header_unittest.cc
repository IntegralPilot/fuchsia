// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.diagnostics.types/cpp/fidl.h>

#include <gtest/gtest.h>
#include <sdk/lib/syslog/cpp/log_level.h>
#include <sdk/lib/syslog/structured_backend/fuchsia_syslog.h>

template <typename T, size_t Count>
static constexpr bool MultiEquals(const T values[Count]) {
  for (size_t i = 1; i < Count; i++) {
    if (values[i] != values[i - 1]) {
      return false;
    }
  }
  return true;
}

TEST(HeaderTest, CompileTimeAsserts) {
  // NOTE: Please ensure that all 3 headers above are updated
  // BEFORE approving a change to this file. All 3 files must be
  // manually kept in-sync, and this test needs to be kept up-to-date
  // to prevent future inadvertent breakages. Reviewers MUST make
  // sure that anything new added to those header files
  // are properly tested in this file prior to approval.
  constexpr int traces[] = {fuchsia_logging::LogSeverity::Trace, FUCHSIA_LOG_TRACE,
                            uint8_t{fuchsia_diagnostics_types::Severity::kTrace}};
  static_assert(MultiEquals<int, 3>(traces));
  constexpr int debugs[] = {fuchsia_logging::LogSeverity::Debug, FUCHSIA_LOG_DEBUG,
                            uint8_t{fuchsia_diagnostics_types::Severity::kDebug}};
  static_assert(MultiEquals<int, 3>(debugs));
  constexpr int infos[] = {fuchsia_logging::LogSeverity::Info, FUCHSIA_LOG_INFO,
                           uint8_t{fuchsia_diagnostics_types::Severity::kInfo}};
  static_assert(MultiEquals<int, 3>(infos));
  constexpr int errors[] = {fuchsia_logging::LogSeverity::Error, FUCHSIA_LOG_ERROR,
                            uint8_t{fuchsia_diagnostics_types::Severity::kError}};
  static_assert(MultiEquals<int, 3>(errors));
  constexpr int warns[] = {fuchsia_logging::LogSeverity::Warn, FUCHSIA_LOG_WARNING,
                           uint8_t{fuchsia_diagnostics_types::Severity::kWarn}};
  static_assert(MultiEquals<int, 3>(warns));
  constexpr int fatals[] = {fuchsia_logging::LogSeverity::Fatal, FUCHSIA_LOG_FATAL,
                            uint8_t{fuchsia_diagnostics_types::Severity::kFatal}};
  static_assert(MultiEquals<int, 3>(fatals));
}

int main(int argc, char **argv) {
  ::testing::InitGoogleTest(&argc, argv);
  return RUN_ALL_TESTS();
}
