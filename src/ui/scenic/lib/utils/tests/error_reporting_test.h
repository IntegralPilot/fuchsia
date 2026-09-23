// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_UTILS_TESTS_ERROR_REPORTING_TEST_H_
#define SRC_UI_SCENIC_LIB_UTILS_TESTS_ERROR_REPORTING_TEST_H_

#include <lib/syslog/cpp/macros.h>

#include <string>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/scenic/util/error_reporter.h"

namespace utils {
namespace test {

using scenic_impl::ErrorReporter;

// Use of this macro allows us to remain consistent with gtest syntax, aiding
// readability.
#define EXPECT_SCENIC_SESSION_ERROR_COUNT(n) ExpectErrorCount((n))

class TestErrorReporter : public ErrorReporter {
 public:
  const std::vector<std::string>& errors() const { return reported_errors_; }

 private:
  // |ErrorReporter|
  void ReportError(fuchsia_logging::LogSeverity severity, std::string error_string) override;

  std::vector<std::string> reported_errors_;
};

class ErrorReportingTest : public ::gtest::TestLoopFixture {
 protected:
  ErrorReportingTest();
  virtual ~ErrorReportingTest();

  ErrorReporter* error_reporter() const;
  std::shared_ptr<ErrorReporter> shared_error_reporter() const;

  // Verify that the expected number of errors were reported.
  void ExpectErrorCount(size_t errors_expected) {
    EXPECT_EQ(errors_expected, error_reporter_->errors().size());
  }

  // Verify that the error at position |pos| in the list is as expected.  If
  // |pos| is >= the number of errors, then the verification will fail.  If no
  // error is expected, use nullptr as |expected_error_string|.
  void ExpectErrorAt(size_t pos, const char* expected_error_string);

  // Verify that the last reported error is as expected.  If no error is
  // expected, use nullptr as |expected_error_string|.
  void ExpectLastReportedError(const char* expected_error_string);

  // | ::testing::Test |
  void SetUp() override;
  // | ::testing::Test |
  void TearDown() override;

 private:
  std::shared_ptr<TestErrorReporter> error_reporter_;

  // Help subclasses remember to call SetUp() and TearDown() on superclass.
  bool setup_called_ = false;
  bool teardown_called_ = false;
};

}  // namespace test
}  // namespace utils

#endif  // SRC_UI_SCENIC_LIB_UTILS_TESTS_ERROR_REPORTING_TEST_H_
