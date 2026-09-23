// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/data_provider.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/fpromise/result.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/stdcompat/string_view.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cstddef>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/annotation_manager.h"
#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/feedback_data/tests/stub_attachment_provider.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/archive.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/timekeeper/test_clock.h"
#include "src/lib/uuid/uuid.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/schema.h"

namespace fuchsia::feedback {

bool operator==(const Annotation& lhs, const Annotation& rhs) {
  return lhs.key == rhs.key && lhs.value == rhs.value;
}

}  // namespace fuchsia::feedback

namespace forensics {
namespace feedback_data {
namespace {

using fuchsia::feedback::Annotation;
using fuchsia::feedback::Snapshot;
using testing::UnorderedElementsAreArray;

const std::set<std::string> kDefaultAnnotations = {
    feedback::kBuildBoardKey, feedback::kBuildLatestCommitDateKey, feedback::kBuildProductKey,
    feedback::kBuildVersionKey, feedback::kDeviceBoardNameKey};

constexpr zx::duration kDefaultSnapshotFlowDuration = zx::usec(5);

// Timeout for a single asynchronous piece of data, e.g., syslog collection, if the client didn't
// specify one.
//
// 30s seems reasonable to collect everything.
constexpr zx::duration kDefaultDataTimeout = zx::sec(30);

// Unit-tests the implementation of the fuchsia.feedback.DataProvider FIDL interface.
//
// This does not test the environment service. It directly instantiates the class, without
// connecting through FIDL.
class DataProviderTest : public UnitTestFixture {
 public:
  void SetUp() override {
    cobalt_ = std::make_unique<cobalt::Logger>(dispatcher(), services(), &clock_);
    SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

    inspect_node_manager_ = std::make_unique<InspectNodeManager>(&InspectRoot());
    inspect_data_budget_ = std::make_unique<InspectDataBudget>(
        "non-existent_path", inspect_node_manager_.get(), cobalt_.get());
  }

 protected:
  void SetUpDataProvider(
      const std::set<std::string>& default_snapshot_annotations = kDefaultAnnotations,
      const feedback::AttachmentKeys& attachment_allowlist = {},
      const std::map<std::string, ErrorOrString>& startup_annotations = {},
      const std::map<std::string, feedback::AttachmentProvider*>& attachment_providers = {}) {
    std::set<std::string> allowlist;
    for (const auto& [k, v] : startup_annotations) {
      allowlist.insert(k);
    }
    annotation_manager_ =
        std::make_unique<feedback::AnnotationManager>(dispatcher(), allowlist, startup_annotations);
    attachment_manager_ = std::make_unique<feedback::AttachmentManager>(
        dispatcher(), &clock_, attachment_allowlist, attachment_providers);
    data_provider_ = std::make_unique<DataProvider>(
        dispatcher(), services(), &clock_, &redactor_, /*is_first_instance=*/true,
        default_snapshot_annotations, attachment_allowlist, cobalt_.get(),
        annotation_manager_.get(), attachment_manager_.get(), inspect_data_budget_.get());
  }

  Snapshot GetSnapshot(std::optional<zx::channel> channel = std::nullopt,
                       zx::duration snapshot_flow_duration = kDefaultSnapshotFlowDuration) {
    FX_CHECK(data_provider_);

    Snapshot snapshot;

    // We can set |clock_|'s start and end times because the call to start the timer happens
    // independently of the loop while the call to end it happens in a task that is posted on the
    // loop. So, as long the end time is set before the loop is run, a non-zero duration will be
    // recorded.
    clock_.SetMonotonic(zx::time_monotonic(0));
    fuchsia::feedback::GetSnapshotParameters params;
    if (channel) {
      params.set_response_channel(*std::move(channel));
    }
    data_provider_->GetSnapshot(std::move(params),
                                [&snapshot](Snapshot res) { snapshot = std::move(res); });
    clock_.SetMonotonic(zx::time_monotonic(0) + snapshot_flow_duration);
    RunLoopUntilIdle();
    return snapshot;
  }

  std::pair<feedback::Annotations, fuchsia::feedback::Attachment> GetSnapshotInternal(
      const std::string& uuid = uuid::Generate(),
      zx::duration snapshot_flow_duration = kDefaultSnapshotFlowDuration) {
    FX_CHECK(data_provider_);

    feedback::Annotations annotations;
    fuchsia::feedback::Attachment archive;

    // We can set |clock_|'s start and end times because the call to start the timer happens
    // independently of the loop while the call to end it happens in a task that is posted on the
    // loop. So, as long the end time is set before the loop is run, a non-zero duration will be
    // recorded.
    clock_.SetMonotonic(zx::time_monotonic(0));
    data_provider_->GetSnapshotInternal(
        kDefaultDataTimeout, uuid,
        [&annotations, &archive](feedback::Annotations resultAnnotations,
                                 fuchsia::feedback::Attachment resultArchive) {
          annotations = std::move(resultAnnotations);
          archive = std::move(resultArchive);
        });
    clock_.SetMonotonic(zx::time_monotonic(0) + snapshot_flow_duration);
    RunLoopUntilIdle();
    return {std::move(annotations), std::move(archive)};
  }

  size_t NumCurrentServedArchives() { return data_provider_->NumCurrentServedArchives(); }

  std::map<std::string, std::string> UnpackSnapshot(const Snapshot& snapshot) {
    FX_CHECK(snapshot.has_archive());
    FX_CHECK(snapshot.archive().key == kSnapshotFilename);
    std::map<std::string, std::string> unpacked_attachments;
    FX_CHECK(Unpack(snapshot.archive().value, &unpacked_attachments));
    return unpacked_attachments;
  }

 private:
  timekeeper::TestClock clock_;
  std::unique_ptr<feedback::AnnotationManager> annotation_manager_;
  std::unique_ptr<cobalt::Logger> cobalt_;
  IdentityRedactor redactor_{inspect::BoolProperty()};
  std::unique_ptr<feedback::AttachmentManager> attachment_manager_;

 protected:
  std::unique_ptr<DataProvider> data_provider_;

 private:
  std::unique_ptr<InspectNodeManager> inspect_node_manager_;
  std::unique_ptr<InspectDataBudget> inspect_data_budget_;
};

TEST_F(DataProviderTest, GetSnapshot_SmokeTest) {
  SetUpDataProvider();

  Snapshot snapshot = GetSnapshot();

  // There will always be a "manifest.json" so there will always be an archive.

  ASSERT_TRUE(snapshot.has_archive());

  const uint64_t archive_size = snapshot.archive().value.size;
  ASSERT_TRUE(archive_size > 0);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::SnapshotGenerationFlow::kSuccess,
                                kDefaultSnapshotFlowDuration.to_usecs()),
                  cobalt::Event(cobalt::SnapshotVersion::kV_01, archive_size),
              }));
}

TEST_F(DataProviderTest, GetSnapshotInvalidChannel) {
  SetUpDataProvider();

  zx::channel server_end;

  ASSERT_EQ(NumCurrentServedArchives(), 0u);
  GetSnapshot(std::optional<zx::channel>(std::move(server_end)));

  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 0u);
}

TEST_F(DataProviderTest, GetSnapshotViaChannel) {
  SetUpDataProvider();

  zx::channel server_end, client_end;
  ZX_ASSERT(zx::channel::create(0, &client_end, &server_end) == ZX_OK);

  ASSERT_EQ(NumCurrentServedArchives(), 0u);
  Snapshot snapshot = GetSnapshot(std::optional<zx::channel>(std::move(server_end)));

  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 1u);

  {
    // Archive sent through channel, so no archive here in snapshot.
    ASSERT_FALSE(snapshot.has_archive());

    fuchsia::io::FilePtr archive;
    archive.Bind(std::move(client_end));
    ASSERT_TRUE(archive.is_bound());

    // Get archive attributes.
    uint64_t archive_size{};
    archive->GetAttributes(fuchsia::io::NodeAttributesQuery::CONTENT_SIZE,
                           [&archive_size](fuchsia::io::Node_GetAttributes_Result result) {
                             ASSERT_TRUE(result.is_response());
                             if (result.response().immutable_attributes.has_content_size()) {
                               archive_size = result.response().immutable_attributes.content_size();
                             }
                           });

    RunLoopUntilIdle();
    ASSERT_TRUE(archive_size > 0);

    uint64_t read_count = 0;
    uint64_t increment = 0;
    do {
      archive->Read(fuchsia::io::MAX_BUF, [&increment](fuchsia::io::Readable_Read_Result result) {
        EXPECT_TRUE(result.is_response()) << zx_status_get_string(result.err());
        increment = result.response().data.size();
      });
      RunLoopUntilIdle();
      read_count += increment;
    } while (increment);

    ASSERT_EQ(archive_size, read_count);

    EXPECT_THAT(ReceivedCobaltEvents(),
                UnorderedElementsAreArray({
                    cobalt::Event(cobalt::SnapshotGenerationFlow::kSuccess,
                                  kDefaultSnapshotFlowDuration.to_usecs()),
                    cobalt::Event(cobalt::SnapshotVersion::kV_01, archive_size),
                }));
  }

  // The channel went out of scope
  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 0u);
}

TEST_F(DataProviderTest, GetMultipleSnapshotViaChannel) {
  SetUpDataProvider();

  zx::channel server_end_1, client_end_1;
  zx::channel server_end_2, client_end_2;
  zx::channel server_end_3, client_end_3;
  ZX_ASSERT(zx::channel::create(0, &client_end_1, &server_end_1) == ZX_OK);
  ZX_ASSERT(zx::channel::create(0, &client_end_2, &server_end_2) == ZX_OK);
  ZX_ASSERT(zx::channel::create(0, &client_end_3, &server_end_3) == ZX_OK);

  ASSERT_EQ(NumCurrentServedArchives(), 0u);

  // Serve clients.
  GetSnapshot(std::optional<zx::channel>(std::move(server_end_1)));
  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 1u);

  GetSnapshot(std::optional<zx::channel>(std::move(server_end_2)));
  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 2u);

  GetSnapshot(std::optional<zx::channel>(std::move(server_end_3)));
  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 3u);

  // Close clients.
  client_end_2.reset();
  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 2u);

  client_end_1.reset();
  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 1u);

  client_end_3.reset();
  RunLoopUntilIdle();
  ASSERT_EQ(NumCurrentServedArchives(), 0u);
}

TEST_F(DataProviderTest, GetSnapshot_AnnotationsAsAttachment) {
  SetUpDataProvider();

  Snapshot snapshot = GetSnapshot();
  std::map<std::string, std::string> unpacked_attachments = UnpackSnapshot(snapshot);

  // There should be an "annotations.json" attachment present in the snapshot.
  ASSERT_NE(unpacked_attachments.find(kAttachmentAnnotations), unpacked_attachments.end());
  const std::string annotations_json = unpacked_attachments[kAttachmentAnnotations];
  ASSERT_FALSE(annotations_json.empty());

  // JSON verification.
  // We check that the output is a valid JSON and that it matches the schema.
  rapidjson::Document json;
  ASSERT_FALSE(json.Parse(annotations_json.c_str()).HasParseError());
  rapidjson::Document schema_json;
  ASSERT_FALSE(schema_json
                   .Parse(fxl::StringPrintf(
                       R"({
  "type": "object",
 "properties": {
    "%s": {
      "type": "string"
    },
    "%s": {
      "type": "string"
    },
    "%s": {
      "type": "string"
    },
    "%s": {
      "type": "string"
    },
    "%s": {
      "type": "string"
    }
  },
  "additionalProperties": false
})",
                       feedback::kBuildBoardKey, feedback::kBuildLatestCommitDateKey,
                       feedback::kBuildProductKey, feedback::kBuildVersionKey,
                       feedback::kDeviceBoardNameKey))
                   .HasParseError());
  rapidjson::SchemaDocument schema(schema_json);
  rapidjson::SchemaValidator validator(schema);
  EXPECT_TRUE(json.Accept(validator));
}

TEST_F(DataProviderTest, GetSnapshot_ManifestAsAttachment) {
  SetUpDataProvider();

  Snapshot snapshot = GetSnapshot();
  const std::map<std::string, std::string> unpacked_attachments = UnpackSnapshot(snapshot);

  // There should be a "metadata.json" attachment present in the snapshot.
  ASSERT_NE(unpacked_attachments.find(kAttachmentMetadata), unpacked_attachments.end());
}

TEST_F(DataProviderTest, GetSnapshot_SingleAttachmentOnEmptyAttachmentAllowlist) {
  SetUpDataProvider(kDefaultAnnotations, /*attachment_allowlist=*/{});

  Snapshot snapshot = GetSnapshot();
  const std::map<std::string, std::string> unpacked_attachments = UnpackSnapshot(snapshot);
  EXPECT_EQ(unpacked_attachments.count(kAttachmentAnnotations), 1u);
}

TEST_F(DataProviderTest, GetSnapshotAnnotationsInFidl) {
  SetUpDataProvider(kDefaultAnnotations, /*attachment_allowlist=*/{},
                    {{"annotation1", ErrorOrString("value1")}});

  Snapshot snapshot = GetSnapshot();

  const std::vector<fuchsia::feedback::Annotation> expected_annotations = {
      {"annotation1", "value1"}};

  ASSERT_TRUE(snapshot.has_annotations2());
  EXPECT_EQ(snapshot.annotations2(), expected_annotations);
}

TEST_F(DataProviderTest, GetSnapshotUnfilteredAnnotations_DoesNotFilterMissingAnnotations) {
  SetUpDataProvider(kDefaultAnnotations, /*attachment_allowlist=*/{},
                    {{"annotation1", ErrorOrString(Error::kMissingValue)}});

  auto [annotations, archive] = GetSnapshotInternal();
  EXPECT_EQ(annotations.size(), 1u);
  EXPECT_TRUE(annotations.contains("annotation1"));
}

TEST_F(DataProviderTest, GetSnapshotUnfilteredAnnotations_ReturnsFilledArchive) {
  SetUpDataProvider(kDefaultAnnotations, /*attachment_allowlist=*/{});

  auto [annotations, archive] = GetSnapshotInternal();
  EXPECT_TRUE(archive.value.size > 0u);
}

TEST_F(DataProviderTest, GetSnapshot_Timeout) {
  const std::string kSuccessFile = "success.txt";
  const std::string kTimeoutFile = "timeout.txt";
  const std::string kSuccessValue = "success value";
  const std::string kTimeoutValue = "timeout value";

  StubAttachmentProvider provider_successful(kTimeoutValue);
  StubAttachmentProvider provider_timeout(kTimeoutValue);

  SetUpDataProvider(kDefaultAnnotations, /*attachment_allowlist=*/
                    {
                        kSuccessFile,
                        kTimeoutFile,
                    },
                    /*startup_annotations=*/{},
                    /*attachment_providers=*/
                    {
                        {kSuccessFile, &provider_successful},
                        {kTimeoutFile, &provider_timeout},
                    });

  feedback::Annotations annotations;
  fuchsia::feedback::Attachment archive;

  data_provider_->GetSnapshotInternal(
      zx::sec(1), uuid::Generate(),
      [&annotations, &archive](feedback::Annotations result_annotations,
                               fuchsia::feedback::Attachment result_archive) {
        annotations = std::move(result_annotations);
        archive = std::move(result_archive);
      });

  provider_successful.CompleteSuccessfully(kSuccessValue);
  RunLoopFor(zx::sec(5));

  ASSERT_TRUE(archive.value.size > 0u);

  std::map<std::string, std::string> unpacked_attachments;
  FX_CHECK(Unpack(archive.value, &unpacked_attachments));

  // There should be |kAttachmentMetadata|, |kSuccessFile| and |kTimeoutFile| attachments present in
  // the snapshot.
  EXPECT_NE(unpacked_attachments.find(kAttachmentMetadata), unpacked_attachments.end());
  ASSERT_NE(unpacked_attachments.find(kSuccessFile), unpacked_attachments.end());
  ASSERT_NE(unpacked_attachments.find(kTimeoutFile), unpacked_attachments.end());

  EXPECT_EQ(unpacked_attachments[kSuccessFile], kSuccessValue);
  EXPECT_EQ(unpacked_attachments[kTimeoutFile], kTimeoutValue);
}

TEST_F(DataProviderTest, GetSnapshotInternalUsesUuid) {
  SetUpDataProvider(kDefaultAnnotations, /*attachment_allowlist=*/{});
  const auto [annotations, archive] = GetSnapshotInternal("test-uuid");

  ASSERT_TRUE(archive.value.size > 0u);

  std::map<std::string, std::string> unpacked_attachments;
  FX_CHECK(Unpack(archive.value, &unpacked_attachments));

  EXPECT_NE(unpacked_attachments.find(kAttachmentMetadata), unpacked_attachments.end());
  EXPECT_TRUE(cpp23::contains(unpacked_attachments[kAttachmentMetadata],
                              R"("snapshot_uuid": "test-uuid")"));
}

}  // namespace
}  // namespace feedback_data
}  // namespace forensics
