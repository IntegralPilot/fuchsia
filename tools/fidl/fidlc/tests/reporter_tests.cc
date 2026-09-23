// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostic_types.h"
#include "tools/fidl/fidlc/src/reporter.h"
#include "tools/fidl/fidlc/src/source_span.h"
#include "tools/fidl/fidlc/src/virtual_source_file.h"

namespace fidlc {
namespace {

using ::testing::HasSubstr;
using ::testing::Not;

const ErrorId kTestErrorId = 123;
const std::string kTestErrorIdStr = "fi-0123";
const ErrorId kTestWarningId = 124;
const std::string kTestWarningIdStr = "fi-0124";

constexpr ErrorDef<kTestErrorId, std::string_view, std::string_view> ErrTest(
    "This test error has one string param '{0}' and another '{1}'.");
constexpr WarningDef<kTestWarningId, std::string_view, std::string_view> WarnTest(
    "This test warning has one string param '{0}' and another '{1}'.");

constexpr ErrorDef<kTestErrorId, std::string_view, std::string_view> ReuseParamsErrTest(
    "This test error has one string param '{0}' and another '{1}'. "
    "Backwards, that's '{1}' and '{0}'.");

TEST(ReporterTests, ReportErrorFormatParams) {
  Reporter reporter;
  VirtualSourceFile file("fake");
  SourceSpan span("span text", file);
  reporter.Fail(ErrTest, span, "param1", "param2");

  const auto& errors = reporter.errors();
  ASSERT_EQ(errors.size(), 1u);
  ASSERT_EQ(errors[0]->span, span);
  EXPECT_EQ(errors[0]->def.FormatId(), kTestErrorIdStr);
  EXPECT_THAT(errors[0]->Format(), HasSubstr(kTestErrorIdStr));
  EXPECT_THAT(errors[0]->msg, Not(HasSubstr(kTestErrorIdStr)));
  EXPECT_EQ(errors[0]->msg, "This test error has one string param 'param1' and another 'param2'.");
}

TEST(ReporterTests, MakeErrorThenReportIt) {
  Reporter reporter;
  VirtualSourceFile file("fake");
  SourceSpan span("span text", file);
  std::unique_ptr<Diagnostic> diag = Diagnostic::MakeError(ErrTest, span, "param1", "param2");
  reporter.Report(std::move(diag));

  const auto& errors = reporter.errors();
  ASSERT_EQ(errors.size(), 1u);
  ASSERT_EQ(errors[0]->span, span);
  EXPECT_EQ(errors[0]->def.FormatId(), kTestErrorIdStr);
  EXPECT_THAT(errors[0]->Format(), HasSubstr(kTestErrorIdStr));
  EXPECT_THAT(errors[0]->msg, Not(HasSubstr(kTestErrorIdStr)));
  EXPECT_EQ(errors[0]->msg, "This test error has one string param 'param1' and another 'param2'.");
}

TEST(ReporterTests, ReportWarningFormatParams) {
  Reporter reporter;
  VirtualSourceFile file("fake");
  SourceSpan span("span text", file);
  reporter.Warn(WarnTest, span, "param1", "param2");

  const auto& warnings = reporter.warnings();
  ASSERT_EQ(warnings.size(), 1u);
  ASSERT_EQ(warnings[0]->span, span);
  EXPECT_EQ(warnings[0]->def.FormatId(), kTestWarningIdStr);
  EXPECT_THAT(warnings[0]->Format(), HasSubstr(kTestWarningIdStr));
  EXPECT_THAT(warnings[0]->msg, Not(HasSubstr(kTestWarningIdStr)));
  EXPECT_EQ(warnings[0]->msg,
            "This test warning has one string param 'param1' and another 'param2'.");
}

TEST(ReporterTests, MakeWarningThenReportIt) {
  Reporter reporter;
  VirtualSourceFile file("fake");
  SourceSpan span("span text", file);
  std::unique_ptr<Diagnostic> diag = Diagnostic::MakeWarning(WarnTest, span, "param1", "param2");
  reporter.Report(std::move(diag));

  const auto& warnings = reporter.warnings();
  ASSERT_EQ(warnings.size(), 1u);
  ASSERT_EQ(warnings[0]->span, span);
  EXPECT_EQ(warnings[0]->def.FormatId(), kTestWarningIdStr);
  EXPECT_THAT(warnings[0]->Format(), HasSubstr(kTestWarningIdStr));
  EXPECT_THAT(warnings[0]->msg, Not(HasSubstr(kTestWarningIdStr)));
  EXPECT_EQ(warnings[0]->msg,
            "This test warning has one string param 'param1' and another 'param2'.");
}

TEST(ReporterTests, ReportErrorWithReusedFormatParams) {
  Reporter reporter;
  VirtualSourceFile file("fake");
  SourceSpan span("span text", file);
  reporter.Fail(ReuseParamsErrTest, span, "param1", "param2");

  const auto& errors = reporter.errors();
  ASSERT_EQ(errors.size(), 1u);
  EXPECT_EQ(errors[0]->msg,
            "This test error has one string param 'param1' and another 'param2'. "
            "Backwards, that's 'param2' and 'param1'.");
}

}  // namespace
}  // namespace fidlc
