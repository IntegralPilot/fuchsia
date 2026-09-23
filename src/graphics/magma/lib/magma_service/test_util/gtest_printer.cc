// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_logger.h>
#include <lib/magma_service/test_util/gtest_printer.h>

// The implementation here is a simplification of PrettyUnitTestResultPrinter
// taken from //third_party/googletest/src/googletest/src/gtest.cc

namespace magma {
namespace {

const char* TestPartResultTypeToString(testing::TestPartResult::Type type) {
  switch (type) {
    case testing::TestPartResult::kSkip:
      return "Skipped";
    case testing::TestPartResult::kSuccess:
      return "Success";

    case testing::TestPartResult::kNonFatalFailure:
    case testing::TestPartResult::kFatalFailure:
      return "Failure";
    default:
      return "Unknown result type";
  }
}

}  // namespace

void GtestPrinter::OnTestStart(const testing::TestInfo& test_info) {
  MAGMA_LOG(INFO, "[ RUN      ] %s.%s\n", test_info.test_suite_name(), test_info.name());
}

void GtestPrinter::OnTestDisabled(const testing::TestInfo& test_info) {
  MAGMA_LOG(INFO, "[ DISABLED ] %s.%s\n", test_info.test_suite_name(), test_info.name());
}

void GtestPrinter::OnTestPartResult(const testing::TestPartResult& result) {
  if (result.type() != testing::TestPartResult::kSuccess) {
    // Print failure message from the assertion
    // (e.g. expected this and got that).
    MAGMA_LOG(INFO, "%s:%d %s %s\n", result.file_name(), result.line_number(),
              TestPartResultTypeToString(result.type()), result.message());
  }
}

void GtestPrinter::OnTestEnd(const testing::TestInfo& test_info) {
  const char* kResult = "  FAILED  ";
  if (test_info.result()->Passed()) {
    kResult = "       OK ";
  } else if (test_info.result()->Skipped()) {
    kResult = "  SKIPPED ";
  };
  MAGMA_LOG(INFO, "[%s] %s.%s (%ld ms)\n", kResult, test_info.test_suite_name(), test_info.name(),
            test_info.result()->elapsed_time());
}

void GtestPrinter::OnTestIterationEnd(const testing::UnitTest& unit_test, int /*iteration*/) {
  MAGMA_LOG(INFO, "[==========] %d test(s) from %d suite(s) ran. (%ld ms total)\n",
            unit_test.test_to_run_count(), unit_test.test_suite_to_run_count(),
            unit_test.elapsed_time());

  MAGMA_LOG(INFO, "[  PASSED  ] %d test(s).\n", unit_test.successful_test_count());

  const int skipped_test_count = unit_test.skipped_test_count();
  if (skipped_test_count > 0) {
    MAGMA_LOG(INFO, "[  SKIPPED ] %d test(s).\n", skipped_test_count);
  }

  // Failed tests.
  const int failed_test_count = unit_test.failed_test_count();
  MAGMA_LOG(INFO, "[  FAILED  ] %d test(s), listed below:\n", failed_test_count);

  for (int i = 0; i < unit_test.total_test_suite_count(); ++i) {
    const testing::TestSuite& test_suite = *unit_test.GetTestSuite(i);
    if (!test_suite.should_run() || (test_suite.failed_test_count() == 0)) {
      continue;
    }
    for (int j = 0; j < test_suite.total_test_count(); ++j) {
      const testing::TestInfo& test_info = *test_suite.GetTestInfo(j);
      if (!test_info.should_run() || !test_info.result()->Failed()) {
        continue;
      }
      MAGMA_LOG(INFO, "[  FAILED  ] %s.%s\n", test_suite.name(), test_info.name());
    }
  }
  MAGMA_LOG(INFO, "\n%2d FAILED %s\n", failed_test_count,
            failed_test_count == 1 ? "TEST" : "TESTS");

  // Failed test suites.
  int suite_failure_count = 0;
  for (int i = 0; i < unit_test.total_test_suite_count(); ++i) {
    const testing::TestSuite& test_suite = *unit_test.GetTestSuite(i);
    if (!test_suite.should_run()) {
      continue;
    }
    if (test_suite.ad_hoc_test_result().Failed()) {
      MAGMA_LOG(INFO, "[  FAILED  ] %s: SetUpTestSuite or TearDownTestSuite\n", test_suite.name());
      ++suite_failure_count;
    }
  }
  if (suite_failure_count > 0) {
    MAGMA_LOG(INFO, "\n%2d FAILED TEST %s\n", suite_failure_count,
              suite_failure_count == 1 ? "SUITE" : "SUITES");
  }

  // Disabled tests.
  int num_disabled = unit_test.reportable_disabled_test_count();
  if (num_disabled) {
    MAGMA_LOG(INFO, "  YOU HAVE %d DISABLED %s\n\n", num_disabled,
              num_disabled == 1 ? "TEST" : "TESTS");
  }
}

}  // namespace magma
