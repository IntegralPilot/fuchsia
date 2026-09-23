// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/report_util.h"

#include <map>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/crash_reports/annotation_map.h"
#include "src/developer/forensics/crash_reports/snapshot.h"
#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics {
namespace crash_reports {
namespace {

using ::testing::IsEmpty;
using ::testing::IsSupersetOf;
using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

TEST(MakeReport, AddsSnapshotAnnotations) {
  const feedback::Annotations annotations = {
      {"snapshot_annotation_key", ErrorOrString("snapshot_annotation_value")},
  };

  fuchsia::feedback::CrashReport crash_report;
  crash_report.set_program_name("program_name");

  Product product{
      .name = "product_name",
      .version = ErrorOrString("product_version"),
      .channel = ErrorOrString("product_channel"),
  };

  const ::fpromise::result<Report> report =
      MakeReport(std::move(crash_report), *ProgramShortname::Create("program_name"),
                 /*report_id=*/0, "snapshot_uuid", annotations,
                 /*current_time=*/std::nullopt, std::move(product),
                 /*is_hourly_report=*/false);
  ASSERT_TRUE(report.is_ok());
  EXPECT_EQ(report.value().Annotations().Get("snapshot_annotation_key"),
            "snapshot_annotation_value");
}

TEST(MakeReport, AddsCrashServerAnnotationsWithoutReportTime) {
  const feedback::Annotations annotations = {
      {feedback::kDeviceFeedbackIdKey, ErrorOrString("device_id")},
  };

  fuchsia::feedback::CrashReport crash_report;
  crash_report.set_program_name("program_name");

  Product product{
      .name = "product_name",
      .version = ErrorOrString("product_version"),
      .channel = ErrorOrString("product_channel"),
  };

  const fpromise::result<Report> report = MakeReport(
      std::move(crash_report), *ProgramShortname::Create("program_name"), /*report_id=*/0,
      "snapshot_uuid", annotations, /*current_time=*/std::nullopt, std::move(product),
      /*is_hourly_report=*/false);
  ASSERT_TRUE(report.is_ok());
  EXPECT_THAT(report.value().Annotations().Raw(), IsSupersetOf({
                                                      Pair("ptype", "program_name"),
                                                      Pair("program", "program_name"),
                                                      Pair("debug.report-time.set", "false"),
                                                      Pair("guid", "device_id"),
                                                  }));
}

TEST(MakeReport, AddsCrashServerAnnotationsWithReportTime) {
  const feedback::Annotations annotations = {
      {feedback::kDeviceFeedbackIdKey, ErrorOrString("device_id")},
  };

  fuchsia::feedback::CrashReport crash_report;
  crash_report.set_program_name("program_name");

  Product product{
      .name = "product_name",
      .version = ErrorOrString("product_version"),
      .channel = ErrorOrString("product_channel"),
  };

  const fpromise::result<Report> report = MakeReport(
      std::move(crash_report), *ProgramShortname::Create("program_name"), /*report_id=*/0,
      "snapshot_uuid", annotations, /*current_time=*/timekeeper::time_utc(zx::sec(55).get()),
      std::move(product), /*is_hourly_report=*/false);
  ASSERT_TRUE(report.is_ok());
  EXPECT_THAT(report.value().Annotations().Raw(), IsSupersetOf({
                                                      Pair("ptype", "program_name"),
                                                      Pair("program", "program_name"),
                                                      Pair("reportTimeMillis", "55000"),
                                                      Pair("guid", "device_id"),
                                                  }));
}

TEST(MakeReport, AddsRequiredAnnotations) {
  fuchsia::feedback::CrashReport crash_report;
  crash_report.set_program_name("program_name");

  Product product{
      .name = "product_name",
      .version = ErrorOrString("product_version"),
      .channel = ErrorOrString("product_channel"),
  };

  const ::fpromise::result<Report> report =
      MakeReport(std::move(crash_report), *ProgramShortname::Create("program_name"),
                 /*report_id=*/0, "snapshot_uuid", {},
                 /*current_time=*/std::nullopt, std::move(product),
                 /*is_hourly_report=*/false);

  ASSERT_TRUE(report.is_ok());
  EXPECT_EQ(report.value().Annotations().Get(feedback::kOSNameKey), "Fuchsia");
  EXPECT_THAT(report.value().Annotations().Get(feedback::kDebugReportUuid), Not(IsEmpty()));
}

TEST(SnapshotAnnotationsTest, GetReportAnnotations_EmptySnapshotAnnotations) {
  const AnnotationMap annotations = GetReportAnnotations({});

  EXPECT_THAT(annotations.Raw(), UnorderedElementsAreArray({
                                     Pair(feedback::kOSVersionKey, "unknown"),
                                     Pair("debug.osVersion.error", "missing"),
                                     Pair(feedback::kOSChannelKey, "unknown"),
                                     Pair("debug.osChannel.error", "missing"),
                                 }));
}

TEST(SnapshotAnnotationsTest, GetReportAnnotations_Snapshot) {
  const feedback::Annotations startup_annotations = {
      {feedback::kBuildVersionKey, ErrorOrString("version")},
      {feedback::kBuildPlatformVersionKey, ErrorOrString("platform-version")},
      {feedback::kSystemUpdateChannelCurrentKey, ErrorOrString("channel")},
      {feedback::kBuildBoardKey, ErrorOrString("board")},
      {feedback::kBuildProductKey, ErrorOrString(Error::kTimeout)},
      {feedback::kBuildLatestCommitDateKey, ErrorOrString(Error::kFileReadFailure)},
  };

  const AnnotationMap annotations = GetReportAnnotations(startup_annotations);

  EXPECT_THAT(annotations.Raw(),
              UnorderedElementsAreArray({
                  Pair(feedback::kOSVersionKey, "platform-version"),
                  Pair(feedback::kOSChannelKey, "channel"),
                  Pair(feedback::kBuildVersionKey, "version"),
                  Pair(feedback::kBuildPlatformVersionKey, "platform-version"),
                  Pair(feedback::kSystemUpdateChannelCurrentKey, "channel"),
                  Pair(feedback::kBuildBoardKey, "board"),
                  Pair(feedback::kBuildProductKey, "unknown"),
                  Pair("debug.build.product.error", "timeout"),
                  Pair(feedback::kBuildLatestCommitDateKey, "unknown"),
                  Pair("debug.build.latest-commit-date.error", "file read failure"),
              }));
}

TEST(SnapshotAnnotationsTest, GetReportAnnotations_Product) {
  AnnotationMap annotations = {
      {feedback::kBuildPlatformVersionKey, "platform-version"},
      {feedback::kBuildProductVersionKey, "product-version"},
      {feedback::kSystemUpdateChannelCurrentKey, "channel"},
  };
  Product product = Product::DefaultPlatformProduct();

  AnnotationMap added_annotations = GetReportAnnotations(product, annotations);

  EXPECT_THAT(added_annotations.Raw(), UnorderedElementsAreArray({
                                           Pair("product", "Fuchsia"),
                                           Pair("version", "product-version--platform-version"),
                                           Pair("channel", "channel"),
                                       }));
}

TEST(MakeReport, AddsWeight) {
  fuchsia::feedback::CrashReport crash_report;
  crash_report.set_program_name("program_name");
  crash_report.set_weight(5);

  Product product{
      .name = "product_name",
      .version = ErrorOrString("product_version"),
      .channel = ErrorOrString("product_channel"),
  };

  const ::fpromise::result<Report> report =
      MakeReport(std::move(crash_report), *ProgramShortname::Create("program_name"),
                 /*report_id=*/0, "snapshot_uuid", /*snapshot_annotations=*/{},
                 /*current_time=*/std::nullopt, std::move(product),
                 /*is_hourly_report=*/false);
  ASSERT_TRUE(report.is_ok());
  EXPECT_EQ(report.value().Annotations().Get("weight"), "5");
}

}  // namespace
}  // namespace crash_reports
}  // namespace forensics
