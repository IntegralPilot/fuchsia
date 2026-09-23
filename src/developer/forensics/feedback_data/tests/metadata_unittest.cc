// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/metadata.h"

#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/feedback_data/metadata_schema.h"
#include "src/developer/forensics/testing/stubs/utc_clock_ready_watcher.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/timekeeper/test_clock.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/schema.h"

#define ANNOTATIONS_JSON_STATE_IS(json, state)                           \
  {                                                                      \
    ASSERT_TRUE(json.HasMember("files"));                                \
    auto& files = json["files"];                                         \
    ASSERT_TRUE(files.HasMember("annotations.json"));                    \
    ASSERT_TRUE(files["annotations.json"].HasMember("state"));           \
    EXPECT_STREQ(files["annotations.json"]["state"].GetString(), state); \
  }

#define HAS_PRESENT_ANNOTATION(json, name)                                                         \
  {                                                                                                \
    ASSERT_TRUE(json.HasMember("files"));                                                          \
    auto& files = json["files"];                                                                   \
    ASSERT_TRUE(files.HasMember("annotations.json"));                                              \
    ASSERT_TRUE(files["annotations.json"].HasMember("present annotations"));                       \
    {                                                                                              \
      bool has_annotation = false;                                                                 \
      for (const auto& annotation : files["annotations.json"]["present annotations"].GetArray()) { \
        if (strcmp(annotation.GetString(), name) == 0) {                                           \
          has_annotation = true;                                                                   \
          break;                                                                                   \
        }                                                                                          \
      }                                                                                            \
      EXPECT_TRUE(has_annotation && name);                                                         \
    }                                                                                              \
  }

#define HAS_MISSING_ANNOTATION(json, name, error)                                            \
  {                                                                                          \
    ASSERT_TRUE(json.HasMember("files"));                                                    \
    auto& files = json["files"];                                                             \
    ASSERT_TRUE(files.HasMember("annotations.json"));                                        \
    ASSERT_TRUE(files["annotations.json"].HasMember("missing annotations"));                 \
    ASSERT_TRUE(files["annotations.json"]["missing annotations"].HasMember(name));           \
    EXPECT_STREQ(files["annotations.json"]["missing annotations"][name].GetString(), error); \
  }

#define HAS_COMPLETE_ATTACHMENT(json, name)                     \
  {                                                             \
    ASSERT_TRUE(json.HasMember("files"));                       \
    auto& files = json["files"];                                \
    ASSERT_TRUE(files.HasMember(name));                         \
    ASSERT_TRUE(files[name].HasMember("state"));                \
    EXPECT_STREQ(files[name]["state"].GetString(), "complete"); \
  }

#define HAS_PARTIAL_ATTACHMENT(json, name, error)              \
  {                                                            \
    ASSERT_TRUE(json.HasMember("files"));                      \
    auto& files = json["files"];                               \
    ASSERT_TRUE(files.HasMember(name));                        \
    ASSERT_TRUE(files[name].HasMember("state"));               \
    EXPECT_STREQ(files[name]["state"].GetString(), "partial"); \
    ASSERT_TRUE(files[name].HasMember("error"));               \
    EXPECT_STREQ(files[name]["error"].GetString(), error);     \
  }

#define HAS_MISSING_ATTACHMENT(json, name, error)              \
  {                                                            \
    ASSERT_TRUE(json.HasMember("files"));                      \
    auto& files = json["files"];                               \
    ASSERT_TRUE(files.HasMember(name));                        \
    ASSERT_TRUE(files[name].HasMember("state"));               \
    EXPECT_STREQ(files[name]["state"].GetString(), "missing"); \
    ASSERT_TRUE(files[name].HasMember("error"));               \
    EXPECT_STREQ(files[name]["error"].GetString(), error);     \
  }

#define UTC_BOOT_DIFFERENCE_IS(json, name, utc_boot_difference)           \
  {                                                                       \
    ASSERT_TRUE(json.HasMember("files"));                                 \
    auto& files = json["files"];                                          \
    ASSERT_TRUE(files.HasMember(name));                                   \
    ASSERT_TRUE(files[name].HasMember("utc_monotonic_difference_nanos")); \
    ASSERT_TRUE(files[name]["utc_monotonic_difference_nanos"].IsInt64()); \
    EXPECT_EQ(files[name]["utc_monotonic_difference_nanos"].GetInt64(),   \
              utc_boot_difference.get());                                 \
  }

#define LOG_SOURCE_IS(json, name, expected_source)                                        \
  {                                                                                       \
    ASSERT_TRUE(json.HasMember("files"));                                                 \
    auto& files = json["files"];                                                          \
    ASSERT_TRUE(files.HasMember(name));                                                   \
    ASSERT_TRUE(files[name].HasMember(kAttachmentMetadataSourceKey));                     \
    ASSERT_TRUE(files[name][kAttachmentMetadataSourceKey].IsString());                    \
    EXPECT_STREQ(files[name][kAttachmentMetadataSourceKey].GetString(), expected_source); \
  }

#define COLLECTION_DURATION_IS(json, name, collection_duration)                \
  {                                                                            \
    ASSERT_TRUE(json.HasMember("files"));                                      \
    auto& files = json["files"];                                               \
    ASSERT_TRUE(files.HasMember(name));                                        \
    ASSERT_TRUE(files[name].HasMember("collection_duration_monotonic_nanos")); \
    ASSERT_TRUE(files[name]["collection_duration_monotonic_nanos"].IsInt64()); \
    EXPECT_EQ(files[name]["collection_duration_monotonic_nanos"].GetInt64(),   \
              collection_duration.get());                                      \
  }

namespace forensics {
namespace feedback_data {
namespace {

constexpr zx::duration kPreviousBootUtcBootDifference = zx::sec(100);
constexpr const char* kSnapshotUuid = "snapshot_uuid";

template <typename C>
std::vector<std::string> ToVector(const C& json_array) {
  FX_CHECK(json_array.IsArray());
  std::vector<std::string> v;
  for (const auto& e : json_array.GetArray()) {
    FX_CHECK(e.IsString());
    v.push_back(e.GetString());
  }

  return v;
}

static const std::string kSuccessfullyRedactedCanary = "SUCCESSFULLY REDACTED CANARY";
static const std::string kUnsuccessfullyRedactedCanary = "UNSUCCESSFULLY REDACTED CANARY";

class RedactorForTest : public RedactorBase {
 public:
  RedactorForTest() : RedactorBase(inspect::BoolProperty{}) {}

  std::string& Redact(std::string& text) override {
    if (text == UnredactedCanary()) {
      text = std::string(kSuccessfullyRedactedCanary);
    } else {
      text = std::string(kUnsuccessfullyRedactedCanary);
    }
    return text;
  }

  std::string& RedactJson(std::string& text) override { return Redact(text); }

  std::string UnredactedCanary() const override { return "UNREDACTED CANARY MESSAGE"; }
  std::string RedactedCanary() const override { return ""; }
};

class MetadataTest : public UnitTestFixture {
 protected:
  void SetUp() override {
    FX_CHECK(files::WriteFile(files::JoinPath("/cache", kUtcBootDifferenceFile),
                              std::to_string(kPreviousBootUtcBootDifference.get())));
  }

  void TearDown() override {
    files::DeletePath(files::JoinPath("/tmp", kUtcBootDifferenceFile), /*recursive=*/false);
    files::DeletePath(files::JoinPath("/cache", kUtcBootDifferenceFile), /*recursive=*/false);
  }

  void SetUpMetadata(const std::set<std::string>& annotation_allowlist,
                     const feedback::AttachmentKeys& attachment_allowlist) {
    metadata_ = std::make_unique<Metadata>(
        dispatcher(), &clock_, &utc_clock_ready_watcher_, &redactor_,
        /*is_first_instance=*/true, annotation_allowlist, attachment_allowlist);

    utc_clock_ready_watcher_.StartClock();
  }

  // Get the integrity metadata for the provided annotations and attachments, check that it adheres
  // to the schema, and turn it into a json document
  rapidjson::Document MakeJsonReport(const feedback::Annotations& annotations,
                                     const feedback::Attachments& attachments,
                                     const bool missing_non_platform_annotations = false) {
    FX_CHECK(metadata_);
    const std::string metadata_str = metadata_->MakeMetadata(
        annotations, attachments, kSnapshotUuid, missing_non_platform_annotations);

    rapidjson::Document json;
    FX_CHECK(!json.Parse(metadata_str.c_str()).HasParseError());

    rapidjson::Document schema_json;
    FX_CHECK(!schema_json.Parse(kMetadataSchema).HasParseError());
    rapidjson::SchemaDocument schema(schema_json);
    rapidjson::SchemaValidator validator(schema);
    FX_CHECK(json.Accept(validator));

    // Convert to std::string to use its '==' operator.
    FX_CHECK(json["snapshot_version"].GetString() == std::string(SnapshotVersion::kString));
    FX_CHECK(json["metadata_version"].GetString() == std::string(Metadata::kVersion));
    FX_CHECK(json["snapshot_uuid"].GetString() == std::string(kSnapshotUuid));
    FX_CHECK(ToVector(json["log_redaction_canary"]) ==
             std::vector<std::string>({kSuccessfullyRedactedCanary}));

    return json;
  }

  timekeeper::TestClock clock_;
  RedactorForTest redactor_;
  std::unique_ptr<Metadata> metadata_;

 private:
  stubs::UtcClockReadyWatcher utc_clock_ready_watcher_;
};

TEST_F(MetadataTest, Check_AddsMissingAnnotationsOnNoAnnotations) {
  const std::set<std::string> annotation_allowlist = {
      "annotation 1",
  };

  SetUpMetadata(annotation_allowlist, /*attachment_allowlist=*/{});

  const rapidjson::Document metadata_json = MakeJsonReport({}, {});
  HAS_MISSING_ANNOTATION(metadata_json, "annotation 1", "feedback logic error");
}

TEST_F(MetadataTest, Check_AddsMissingAnnotationsOnEmptyAnnotations) {
  const std::set<std::string> annotation_allowlist = {
      "annotation 1",
  };

  SetUpMetadata(annotation_allowlist, /*attachment_allowlist=*/{});

  const rapidjson::Document metadata_json = MakeJsonReport({}, {});
  HAS_MISSING_ANNOTATION(metadata_json, "annotation 1", "feedback logic error");
}

TEST_F(MetadataTest, Check_AddsMissingAttachmentsOnNoAttachments) {
  const feedback::AttachmentKeys attachment_allowlist = {
      "attachment 1",
  };

  SetUpMetadata(/*annotation_allowlist=*/{}, attachment_allowlist);

  const rapidjson::Document metadata_json = MakeJsonReport({}, {});
  HAS_MISSING_ATTACHMENT(metadata_json, "attachment 1", "feedback logic error");
}

TEST_F(MetadataTest, Check_AddsMissingAttachmentsOnEmptyAttachments) {
  const feedback::AttachmentKeys attachment_allowlist = {
      "attachment 1",
  };

  SetUpMetadata(/*annotation_allowlist=*/{}, attachment_allowlist);

  const rapidjson::Document metadata_json = MakeJsonReport({}, {});
  HAS_MISSING_ATTACHMENT(metadata_json, "attachment 1", "feedback logic error");
}

TEST_F(MetadataTest, Check_FormatAnnotationsProperly) {
  const std::set<std::string> annotation_allowlist = {
      "present annotation 1",
      "present annotation 2",
      "missing annotation 1",
      "missing annotation 2",
  };

  const feedback::Annotations annotations = {
      {"present annotation 1", ErrorOrString("")},
      {"present annotation 2", ErrorOrString("")},
      {"missing annotation 1", ErrorOrString(Error::kConnectionError)},
      {"missing annotation 2", ErrorOrString(Error::kFileWriteFailure)},
  };

  SetUpMetadata(annotation_allowlist, /*attachment_allowlist=*/{});

  const rapidjson::Document metadata_json = MakeJsonReport(std::move(annotations), {});

  ANNOTATIONS_JSON_STATE_IS(metadata_json, "partial");

  HAS_PRESENT_ANNOTATION(metadata_json, "present annotation 1");
  HAS_PRESENT_ANNOTATION(metadata_json, "present annotation 2");

  HAS_MISSING_ANNOTATION(metadata_json, "missing annotation 1", "FIDL connection error");
  HAS_MISSING_ANNOTATION(metadata_json, "missing annotation 2", "file write failure");
}

TEST_F(MetadataTest, Check_FormatAttachmentsProperly) {
  const feedback::AttachmentKeys attachment_allowlist = {
      "complete attachment 1", "complete attachment 2", "partial attachment 1",
      "partial attachment 2",  "missing attachment 1",  "missing attachment 2",
  };

  feedback::Attachments attachments;
  attachments.insert({"complete attachment 1", feedback::AttachmentValue("", zx::msec(10))});
  attachments.insert({"complete attachment 2", feedback::AttachmentValue("", zx::msec(20))});
  attachments.insert(
      {"partial attachment 1", feedback::AttachmentValue("", Error::kTimeout, zx::msec(30))});
  attachments.insert({"partial attachment 2",
                      feedback::AttachmentValue("", Error::kAsyncTaskPostFailure, zx::msec(40))});
  attachments.insert(
      {"missing attachment 1", feedback::AttachmentValue(Error::kBadValue, zx::msec(50))});
  attachments.insert(
      {"missing attachment 2", feedback::AttachmentValue(Error::kFileReadFailure, zx::msec(60))});

  SetUpMetadata(/*annotation_allowlist=*/{}, attachment_allowlist);

  const rapidjson::Document metadata_json = MakeJsonReport({}, std::move(attachments));

  HAS_COMPLETE_ATTACHMENT(metadata_json, "complete attachment 1");
  HAS_COMPLETE_ATTACHMENT(metadata_json, "complete attachment 2");

  HAS_PARTIAL_ATTACHMENT(metadata_json, "partial attachment 1", "data collection timeout");
  HAS_PARTIAL_ATTACHMENT(metadata_json, "partial attachment 2", "async post task failure");

  HAS_MISSING_ATTACHMENT(metadata_json, "missing attachment 1", "bad data returned");
  HAS_MISSING_ATTACHMENT(metadata_json, "missing attachment 2", "file read failure");

  COLLECTION_DURATION_IS(metadata_json, "complete attachment 1", zx::msec(10));
  COLLECTION_DURATION_IS(metadata_json, "complete attachment 2", zx::msec(20));
  COLLECTION_DURATION_IS(metadata_json, "partial attachment 1", zx::msec(30));
  COLLECTION_DURATION_IS(metadata_json, "partial attachment 2", zx::msec(40));
  COLLECTION_DURATION_IS(metadata_json, "missing attachment 1", zx::msec(50));
  COLLECTION_DURATION_IS(metadata_json, "missing attachment 2", zx::msec(60));
}

TEST_F(MetadataTest, FormatAttachmentWithSourceDisk) {
  const feedback::AttachmentKeys attachment_allowlist = {
      "log.system.txt",
  };

  feedback::AttachmentData attachment_data(
      "log content", {{kAttachmentMetadataSourceKey, kAttachmentMetadataSourceDisk}});
  feedback::Attachments attachments;
  attachments.insert(
      {"log.system.txt", feedback::AttachmentValue(std::move(attachment_data), zx::msec(10))});

  SetUpMetadata(/*annotation_allowlist=*/{}, attachment_allowlist);

  const rapidjson::Document metadata_json = MakeJsonReport({}, attachments);

  HAS_COMPLETE_ATTACHMENT(metadata_json, "log.system.txt");
  LOG_SOURCE_IS(metadata_json, "log.system.txt", kAttachmentMetadataSourceDisk);
  COLLECTION_DURATION_IS(metadata_json, "log.system.txt", zx::msec(10));
}

TEST_F(MetadataTest, FormatAttachmentWithSourceStream) {
  const feedback::AttachmentKeys attachment_allowlist = {
      "log.system.txt",
  };

  feedback::AttachmentData attachment_data(
      "log content", {{kAttachmentMetadataSourceKey, kAttachmentMetadataSourceStream}});
  feedback::Attachments attachments;
  attachments.insert(
      {"log.system.txt", feedback::AttachmentValue(std::move(attachment_data), zx::msec(10))});

  SetUpMetadata(/*annotation_allowlist=*/{}, attachment_allowlist);

  const rapidjson::Document metadata_json = MakeJsonReport({}, attachments);

  HAS_COMPLETE_ATTACHMENT(metadata_json, "log.system.txt");
  LOG_SOURCE_IS(metadata_json, "log.system.txt", kAttachmentMetadataSourceStream);
  COLLECTION_DURATION_IS(metadata_json, "log.system.txt", zx::msec(10));
}

TEST_F(MetadataTest, FormatPartialAttachmentWithSource) {
  const feedback::AttachmentKeys attachment_allowlist = {
      "log.system.txt",
  };

  feedback::AttachmentData attachment_data(
      "partial log content", Error::kTimeout,
      {{kAttachmentMetadataSourceKey, kAttachmentMetadataSourceDisk}});
  feedback::Attachments attachments;
  attachments.insert(
      {"log.system.txt", feedback::AttachmentValue(std::move(attachment_data), zx::msec(10))});

  SetUpMetadata(/*annotation_allowlist=*/{}, attachment_allowlist);

  const rapidjson::Document metadata_json = MakeJsonReport({}, attachments);

  HAS_PARTIAL_ATTACHMENT(metadata_json, "log.system.txt", "data collection timeout");
  LOG_SOURCE_IS(metadata_json, "log.system.txt", kAttachmentMetadataSourceDisk);
  COLLECTION_DURATION_IS(metadata_json, "log.system.txt", zx::msec(10));
}

TEST_F(MetadataTest, FormatMissingAttachmentWithSource) {
  const feedback::AttachmentKeys attachment_allowlist = {
      "log.system.txt",
  };

  feedback::AttachmentData attachment_data(
      Error::kFileReadFailure, {{kAttachmentMetadataSourceKey, kAttachmentMetadataSourceStream}});
  feedback::Attachments attachments;
  attachments.insert(
      {"log.system.txt", feedback::AttachmentValue(std::move(attachment_data), zx::msec(10))});

  SetUpMetadata(/*annotation_allowlist=*/{}, attachment_allowlist);

  const rapidjson::Document metadata_json = MakeJsonReport({}, attachments);

  HAS_MISSING_ATTACHMENT(metadata_json, "log.system.txt", "file read failure");
  LOG_SOURCE_IS(metadata_json, "log.system.txt", kAttachmentMetadataSourceStream);
  COLLECTION_DURATION_IS(metadata_json, "log.system.txt", zx::msec(10));
}

TEST_F(MetadataTest, Check_NonPlatformAnnotationsComplete) {
  const feedback::Annotations annotations = {
      {"non-platform annotation", ErrorOrString("")},
  };

  SetUpMetadata(/*annotation_allowlist=*/{}, /*attachment_allowlist=*/{});

  const rapidjson::Document metadata_json = MakeJsonReport(std::move(annotations), {});

  HAS_PRESENT_ANNOTATION(metadata_json, "non-platform annotations");
}

TEST_F(MetadataTest, Check_NonPlatformAnnotationsPartial) {
  const feedback::Annotations annotations = {
      {"non-platform annotation", ErrorOrString("")},
  };

  SetUpMetadata(/*annotation_allowlist=*/{}, /*attachment_allowlist=*/{});

  const rapidjson::Document metadata_json =
      MakeJsonReport(std::move(annotations), {},
                     /*missing_non_platform_annotations=*/true);

  HAS_MISSING_ANNOTATION(metadata_json, "non-platform annotations",
                         "too many non-platfrom annotations added");
}

TEST_F(MetadataTest, Check_NonPlatformAnnotationsMissing) {
  SetUpMetadata(/*annotation_allowlist=*/{}, /*attachment_allowlist=*/{});

  const rapidjson::Document metadata_json =
      MakeJsonReport({}, {},
                     /*missing_non_platform_annotations=*/true);

  HAS_MISSING_ANNOTATION(metadata_json, "non-platform annotations",
                         "too many non-platfrom annotations added");
}

TEST_F(MetadataTest, Check_SmokeTest) {
  const std::set<std::string> annotation_allowlist = {
      "present annotation 1", "present annotation 2", "missing annotation 1",
      "missing annotation 2", "missing annotation 3",
  };

  const feedback::Annotations annotations = {
      {"present annotation 1", ErrorOrString("")},
      {"present annotation 2", ErrorOrString("")},
      {"missing annotation 1", ErrorOrString(Error::kConnectionError)},
      {"missing annotation 2", ErrorOrString(Error::kFileWriteFailure)},
      {"non-platform annotation 1", ErrorOrString("")},
  };

  const feedback::AttachmentKeys attachment_allowlist = {
      "complete attachment 1", "complete attachment 2", "partial attachment 1",
      "partial attachment 2",  "missing attachment 1",  "missing attachment 2",
      "missing attachment 3",
  };
  feedback::Attachments attachments;
  attachments.insert({"complete attachment 1", feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert({"complete attachment 2", feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert(
      {"partial attachment 1", feedback::AttachmentValue("", Error::kTimeout, zx::duration(0))});
  attachments.insert(
      {"partial attachment 2",
       feedback::AttachmentValue("", Error::kAsyncTaskPostFailure, zx::duration(0))});
  attachments.insert(
      {"missing attachment 1", feedback::AttachmentValue(Error::kBadValue, zx::duration(0))});
  attachments.insert({"missing attachment 2",
                      feedback::AttachmentValue(Error::kFileReadFailure, zx::duration(0))});

  SetUpMetadata(annotation_allowlist, attachment_allowlist);

  const rapidjson::Document metadata_json =
      MakeJsonReport(std::move(annotations), std::move(attachments),
                     /*missing_non_platform_annotations=*/true);

  HAS_COMPLETE_ATTACHMENT(metadata_json, "complete attachment 1");
  HAS_COMPLETE_ATTACHMENT(metadata_json, "complete attachment 2");

  HAS_PARTIAL_ATTACHMENT(metadata_json, "partial attachment 1", "data collection timeout");
  HAS_PARTIAL_ATTACHMENT(metadata_json, "partial attachment 2", "async post task failure");

  HAS_MISSING_ATTACHMENT(metadata_json, "missing attachment 1", "bad data returned");
  HAS_MISSING_ATTACHMENT(metadata_json, "missing attachment 2", "file read failure");
  HAS_MISSING_ATTACHMENT(metadata_json, "missing attachment 3", "feedback logic error");

  ANNOTATIONS_JSON_STATE_IS(metadata_json, "partial");

  HAS_PRESENT_ANNOTATION(metadata_json, "present annotation 1");
  HAS_PRESENT_ANNOTATION(metadata_json, "present annotation 2");

  HAS_MISSING_ANNOTATION(metadata_json, "missing annotation 1", "FIDL connection error");
  HAS_MISSING_ANNOTATION(metadata_json, "missing annotation 2", "file write failure");
  HAS_MISSING_ANNOTATION(metadata_json, "missing annotation 3", "feedback logic error");

  HAS_MISSING_ANNOTATION(metadata_json, "non-platform annotations",
                         "too many non-platfrom annotations added");
}

TEST_F(MetadataTest, Check_EmptySnapshot) {
  SetUpMetadata(/*annotation_allowlist=*/{}, /*attachment_allowlist=*/{});

  std::string metadata_str = metadata_->MakeMetadata({}, {}, kSnapshotUuid,
                                                     /*missing_non_platform_annotations=*/false);

  rapidjson::Document json;
  ASSERT_TRUE(!json.Parse(metadata_str.c_str()).HasParseError());

  rapidjson::Document schema_json;
  ASSERT_TRUE(!schema_json.Parse(kMetadataSchema).HasParseError());
  rapidjson::SchemaDocument schema(schema_json);
  rapidjson::SchemaValidator validator(schema);
  ASSERT_TRUE(json.Accept(validator));

  // Convert to std::string to use its '==' operator.
  EXPECT_STREQ(json["snapshot_version"].GetString(), SnapshotVersion::kString);
  EXPECT_STREQ(json["metadata_version"].GetString(), Metadata::kVersion);
  EXPECT_STREQ(json["snapshot_uuid"].GetString(), kSnapshotUuid);
  EXPECT_EQ(ToVector(json["log_redaction_canary"]),
            std::vector<std::string>({kSuccessfullyRedactedCanary}));

  EXPECT_TRUE(json.HasMember("files"));
  EXPECT_TRUE(json["files"].IsObject());
  EXPECT_TRUE(json["files"].GetObject().ObjectEmpty());
}

TEST_F(MetadataTest, Check_UtcBootDifference) {
  const std::set<std::string> annotation_allowlist = {
      "annotation 1",
  };

  const feedback::AttachmentKeys attachment_allowlist = {
      kAttachmentInspect,           kAttachmentInspectPreviousBoot, kAttachmentLogKernel,
      kAttachmentLogKernelPrevious, kAttachmentLogSystem,           feedback::kPreviousLogsFilePath,
  };

  const feedback::Annotations annotations = {
      {"annotation 1", ErrorOrString("annotation")},
  };

  feedback::Attachments attachments;
  attachments.insert({kAttachmentInspect, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert(
      {kAttachmentInspectPreviousBoot, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert({kAttachmentLogKernel, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert(
      {kAttachmentLogKernelPrevious, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert({kAttachmentLogSystem, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert(
      {kAttachmentLogSystemPrevious, feedback::AttachmentValue("", zx::duration(0))});

  SetUpMetadata(annotation_allowlist, attachment_allowlist);
  RunLoopUntilIdle();

  zx::time_boot boot;
  timekeeper::time_utc utc;

  clock_.SetUtc(timekeeper::time_utc(0));
  clock_.SetBoot(zx::time_boot(0));

  const zx::duration utc_boot_difference(utc.get() - boot.get());

  boot = clock_.BootNow();
  ASSERT_EQ(clock_.UtcNow(&utc), ZX_OK);

  const rapidjson::Document metadata_json =
      MakeJsonReport(std::move(annotations), std::move(attachments));

  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentInspect, utc_boot_difference);
  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentInspectPreviousBoot,
                         kPreviousBootUtcBootDifference);
  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentLogKernel, utc_boot_difference);
  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentLogKernelPrevious,
                         kPreviousBootUtcBootDifference);
  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentLogSystem, utc_boot_difference);
  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentLogSystemPrevious,
                         kPreviousBootUtcBootDifference);

  ASSERT_TRUE(metadata_json["files"].HasMember(kAttachmentAnnotations));
  ASSERT_FALSE(
      metadata_json["files"][kAttachmentAnnotations].HasMember("utc_monotonic_difference"));
}

TEST_F(MetadataTest, Check_NoUtcMontonicDifferenceAvailable) {
  const std::set<std::string> annotation_allowlist = {
      "annotation 1",
  };

  const feedback::AttachmentKeys attachment_allowlist = {
      "attachment 1",
  };

  const feedback::Annotations annotations = {
      {"annotation 1", ErrorOrString("")},
  };

  feedback::Attachments attachments;
  attachments.insert({"attachment 1", feedback::AttachmentValue("", zx::duration(0))});

  SetUpMetadata(annotation_allowlist, attachment_allowlist);

  const rapidjson::Document metadata_json =
      MakeJsonReport(std::move(annotations), std::move(attachments));

  ASSERT_TRUE(metadata_json["files"].HasMember(kAttachmentAnnotations));
  ASSERT_FALSE(
      metadata_json["files"][kAttachmentAnnotations].HasMember("utc_monotonic_difference"));

  ASSERT_TRUE(metadata_json["files"].HasMember("attachment 1"));
  ASSERT_FALSE(metadata_json["files"]["attachment 1"].HasMember("utc_monotonic_difference"));
}

TEST_F(MetadataTest, Check_NoUtcBootDifferenceMissingFile) {
  const std::set<std::string> annotation_allowlist = {
      "annotation 1",
  };

  const feedback::AttachmentKeys attachment_allowlist = {
      kAttachmentInspect,
      kAttachmentLogKernel,
      kAttachmentLogSystem,
      feedback::kPreviousLogsFilePath,
  };

  const feedback::Annotations annotations = {
      {"annotation 1", ErrorOrString("annotation")},
  };

  feedback::Attachments attachments;
  attachments.insert({kAttachmentInspect, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert({kAttachmentLogKernel, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert({kAttachmentLogSystem, feedback::AttachmentValue("", zx::duration(0))});
  attachments.insert(
      {kAttachmentLogSystemPrevious, feedback::AttachmentValue(Error::kCustom, zx::duration(0))});

  SetUpMetadata(annotation_allowlist, attachment_allowlist);
  RunLoopUntilIdle();

  zx::time_boot boot;
  timekeeper::time_utc utc;

  clock_.SetUtc(timekeeper::time_utc(0));
  clock_.SetBoot(zx::time_boot(0));

  const zx::duration utc_boot_difference(utc.get() - boot.get());

  boot = clock_.BootNow();
  ASSERT_EQ(clock_.UtcNow(&utc), ZX_OK);

  const rapidjson::Document metadata_json =
      MakeJsonReport(std::move(annotations), std::move(attachments));

  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentInspect, utc_boot_difference);
  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentLogKernel, utc_boot_difference);
  UTC_BOOT_DIFFERENCE_IS(metadata_json, kAttachmentLogSystem, utc_boot_difference);

  ASSERT_TRUE(metadata_json["files"].HasMember(kAttachmentLogSystemPrevious));
  ASSERT_FALSE(
      metadata_json["files"][kAttachmentLogSystemPrevious].HasMember("utc_monotonic_difference"));

  ASSERT_TRUE(metadata_json["files"].HasMember(kAttachmentAnnotations));
  ASSERT_FALSE(
      metadata_json["files"][kAttachmentAnnotations].HasMember("utc_monotonic_difference"));
}

struct TestParam {
  std::string test_name;
  std::set<std::string> annotation_allowlist;
  feedback::Annotations annotations;
  bool missing_non_platform_annotations;
  std::string state;
};

class AnnotationsJsonStateTest : public MetadataTest,
                                 public testing::WithParamInterface<TestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousAnnotations, AnnotationsJsonStateTest,
                         ::testing::ValuesIn(
                             std::vector<TestParam>(
                                 {
                                     TestParam{
                                         .test_name = "CompletePlatform_CompleteNonPlatform",
                                         .annotation_allowlist = {"platform"},
                                         .annotations =
                                             {
                                                 {"platform", ErrorOrString("")},
                                                 {"non-platform", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = false,
                                         .state = "complete",

                                     },
                                     TestParam{
                                         .test_name = "CompletePlatform_PartialNonPlatform",
                                         .annotation_allowlist = {"platform"},
                                         .annotations =
                                             {
                                                 {"platform", ErrorOrString("")},
                                                 {"non-platform", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = true,
                                         .state = "partial",

                                     },
                                     TestParam{
                                         .test_name = "CompletePlatform_MissingNonPlatform",
                                         .annotation_allowlist = {"platform"},
                                         .annotations =
                                             {
                                                 {"platform", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = true,
                                         .state = "partial",

                                     },
                                     TestParam{
                                         .test_name = "PartialPlatform_CompleteNonPlatform",
                                         .annotation_allowlist = {"platform 1", "platform 2"},
                                         .annotations =
                                             {
                                                 {"platform 1", ErrorOrString("")},
                                                 {"non-platform", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = false,
                                         .state = "partial",

                                     },
                                     TestParam{
                                         .test_name = "PartialPlatform_PartialNonPlatform",
                                         .annotation_allowlist = {"platform 1", "platform 2"},
                                         .annotations =
                                             {
                                                 {"platform 1", ErrorOrString("")},
                                                 {"non-platform", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = true,
                                         .state = "partial",

                                     },
                                     TestParam{
                                         .test_name = "PartialPlatform_MissingNonPlatform",
                                         .annotation_allowlist = {"platform 1", "platform 2"},
                                         .annotations =
                                             {
                                                 {"platform 1", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = true,
                                         .state = "partial",

                                     },
                                     TestParam{
                                         .test_name = "MissingPlatform_CompleteNonPlatform",
                                         .annotation_allowlist = {"platform"},
                                         .annotations =
                                             {
                                                 {"non-platform", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = false,
                                         .state = "partial",

                                     },
                                     TestParam{
                                         .test_name = "MissingPlatform_PartialNonPlatform",
                                         .annotation_allowlist = {"platform"},
                                         .annotations =
                                             {
                                                 {"non-platform", ErrorOrString("")},
                                             },
                                         .missing_non_platform_annotations = true,
                                         .state = "partial",

                                     },
                                     TestParam{
                                         .test_name = "MissingPlatform_MissingNonPlatform",
                                         .annotation_allowlist = {"platform"},
                                         .annotations = {},
                                         .missing_non_platform_annotations = true,
                                         .state = "missing",

                                     },
                                 })),
                         [](const testing::TestParamInfo<TestParam>& info) {
                           return info.param.test_name;
                         });
TEST_P(AnnotationsJsonStateTest, Succeed) {
  const TestParam& param = GetParam();
  SetUpMetadata(param.annotation_allowlist, /*attachment_allowlist=*/{});

  const rapidjson::Document metadata_json =
      MakeJsonReport(param.annotations, {}, param.missing_non_platform_annotations);
  ANNOTATIONS_JSON_STATE_IS(metadata_json, param.state.c_str());
}

}  // namespace
}  // namespace feedback_data
}  // namespace forensics
