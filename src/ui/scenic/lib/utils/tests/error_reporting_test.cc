// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/tests/error_reporting_test.h"

namespace utils {
namespace test {

namespace {
const char* kSetUpTearDownErrorMsg =
    "subclasses of ErrorReportingTest must call SetUp() and TearDown()";
}

void TestErrorReporter::ReportError(fuchsia_logging::LogSeverity severity,
                                    std::string error_string) {
  // Typically, we don't want to log expected errors when running the tests.
  // However, it is useful to print these errors while writing the tests.
#if ENABLE_DLOG
  // Allow force printing of errors via --verbose=3 as a parameter.
  switch (severity) {
    case ::fuchsia_logging::LOG_INFO:
      FX_LOGS(INFO) << error_string;
      break;
    case ::fuchsia_logging::LOG_WARNING:
      FX_LOGS(WARNING) << error_string;
      break;
    case ::fuchsia_logging::LOG_ERROR:
      FX_LOGS(ERROR) << error_string;
      break;
    case ::fuchsia_logging::LOG_FATAL:
      FX_LOGS(FATAL) << error_string;
      break;
  }
#endif

  reported_errors_.push_back(error_string);
}

ErrorReportingTest::ErrorReportingTest() = default;

ErrorReportingTest::~ErrorReportingTest() {
  FX_CHECK(setup_called_) << kSetUpTearDownErrorMsg;
  FX_CHECK(teardown_called_) << kSetUpTearDownErrorMsg;
}

ErrorReporter* ErrorReportingTest::error_reporter() const { return shared_error_reporter().get(); }

std::shared_ptr<ErrorReporter> ErrorReportingTest::shared_error_reporter() const {
  FX_CHECK(setup_called_) << kSetUpTearDownErrorMsg;
  return error_reporter_;
}

void ErrorReportingTest::ExpectErrorAt(size_t pos, const char* expected_error_string) {
  if (expected_error_string) {
    // Ensure pos is inside the array and references the error string.
    EXPECT_LT(pos, error_reporter_->errors().size());
    EXPECT_STREQ(error_reporter_->errors()[pos].c_str(), expected_error_string);
  } else {
    // Error string was not present, so ensure pos is outside of the errors
    // array.
    EXPECT_TRUE(pos >= error_reporter_->errors().size());
  }
}

void ErrorReportingTest::ExpectLastReportedError(const char* expected_error_string) {
  if (error_reporter_->errors().empty()) {
    EXPECT_EQ(nullptr, expected_error_string);
  } else {
    ExpectErrorAt(error_reporter_->errors().size() - 1, expected_error_string);
  }
}

void ErrorReportingTest::SetUp() {
  setup_called_ = true;

  error_reporter_ = std::make_shared<TestErrorReporter>();
}

void ErrorReportingTest::TearDown() {
  teardown_called_ = true;

  error_reporter_.reset();
}

}  // namespace test
}  // namespace utils
