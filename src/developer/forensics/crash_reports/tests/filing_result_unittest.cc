// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/filing_result.h"

#include <gtest/gtest.h>

#include "fuchsia/feedback/cpp/fidl.h"

namespace forensics::crash_reports {
namespace {

using fuchsia::feedback::FilingError;
using fuchsia::feedback::FilingSuccess;

TEST(FilingResult, ReportUploaded) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kReportUploaded);
  ASSERT_TRUE(fidl_result.is_ok());
  EXPECT_EQ(fidl_result.value(), FilingSuccess::REPORT_UPLOADED);
}

TEST(FilingResult, ReportOnDisk) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kReportOnDisk);
  ASSERT_TRUE(fidl_result.is_ok());
  EXPECT_EQ(fidl_result.value(), FilingSuccess::REPORT_ON_DISK);
}

TEST(FilingResult, ReportInMemory) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kReportInMemory);
  ASSERT_TRUE(fidl_result.is_ok());
  EXPECT_EQ(fidl_result.value(), FilingSuccess::REPORT_IN_MEMORY);
}

TEST(FilingResult, ReportNotFiledUserOptedOut) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kReportNotFiledUserOptedOut);
  ASSERT_TRUE(fidl_result.is_ok());
  EXPECT_EQ(fidl_result.value(), FilingSuccess::REPORT_NOT_FILED_USER_OPTED_OUT);
}

TEST(FilingResult, InvalidArgsError) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kInvalidArgsError);
  ASSERT_TRUE(fidl_result.is_error());
  EXPECT_EQ(fidl_result.error(), FilingError::INVALID_ARGS_ERROR);
}

TEST(FilingResult, ServerError) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kServerError);
  ASSERT_TRUE(fidl_result.is_error());
  EXPECT_EQ(fidl_result.error(), FilingError::SERVER_ERROR);
}

TEST(FilingResult, PersistenceError) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kPersistenceError);
  ASSERT_TRUE(fidl_result.is_error());
  EXPECT_EQ(fidl_result.error(), FilingError::PERSISTENCE_ERROR);
}

TEST(FilingResult, QuotaReachedError) {
  const auto fidl_result = ToFidlFilingResult(FilingResult::kQuotaReachedError);
  ASSERT_TRUE(fidl_result.is_error());
  EXPECT_EQ(fidl_result.error(), FilingError::QUOTA_REACHED_ERROR);
}

}  // namespace
}  // namespace forensics::crash_reports
