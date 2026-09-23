// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/queue.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <memory>
#include <string>
#include <variant>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/crash_reports/constants.h"
#include "src/developer/forensics/crash_reports/filing_result.h"
#include "src/developer/forensics/crash_reports/info/info_context.h"
#include "src/developer/forensics/crash_reports/reporting_policy_watcher.h"
#include "src/developer/forensics/crash_reports/tests/scoped_test_report_store.h"
#include "src/developer/forensics/crash_reports/tests/stub_crash_server.h"
#include "src/developer/forensics/feedback/annotations/annotation_manager.h"
#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/developer/forensics/utils/storage_size.h"
#include "src/lib/files/directory.h"
#include "src/lib/files/path.h"
#include "src/lib/fsl/vmo/strings.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics {
namespace crash_reports {
namespace {

using testing::Contains;
using testing::ElementsAre;
using testing::ElementsAreArray;
using testing::IsEmpty;
using testing::IsSupersetOf;
using testing::Not;
using testing::Pair;
using testing::UnorderedElementsAre;
using testing::UnorderedElementsAreArray;

constexpr CrashServer::UploadStatus kUploadSuccessful = CrashServer::UploadStatus::kSuccess;
constexpr CrashServer::UploadStatus kUploadFailed = CrashServer::UploadStatus::kFailure;
constexpr CrashServer::UploadStatus kUploadThrottled = CrashServer::UploadStatus::kThrottled;
constexpr CrashServer::UploadStatus kUploadTimedOut = CrashServer::UploadStatus::kTimedOut;

constexpr char kAttachmentKey[] = "attachment.key";
constexpr char kAttachmentValue[] = "attachment.value";
constexpr char kAnnotationKey[] = "annotation.key";
constexpr char kAnnotationValue[] = "annotation.value";
constexpr char kSnapshotUuidValue[] = "snapshot_uuid";
constexpr char kMinidumpKey[] = "uploadFileMinidump";
constexpr char kMinidumpValue[] = "minidump";

constexpr zx::duration kPeriodicUploadDuration = zx::min(15);
constexpr zx::duration kUploadResponseDelay = zx::sec(5);

fuchsia::mem::Buffer BuildAttachment(const std::string& value) {
  fuchsia::mem::Buffer attachment;
  FX_CHECK(fsl::VmoFromString(value, &attachment));
  return attachment;
}

std::map<std::string, fuchsia::mem::Buffer> MakeAttachments() {
  std::map<std::string, fuchsia::mem::Buffer> attachments;
  attachments[kAttachmentKey] = BuildAttachment(kAttachmentValue);
  return attachments;
}

AnnotationMap MakeAnnotations() { return {{kAnnotationKey, kAnnotationValue}}; }

Report MakeReport(const std::size_t report_id, const bool empty_annotations = false) {
  AnnotationMap annotations;
  if (!empty_annotations) {
    annotations = MakeAnnotations();
  }

  const std::optional<ProgramShortname> program_shortname =
      ProgramShortname::Create(fxl::StringPrintf("program_%ld", report_id));
  FX_CHECK(program_shortname.has_value());

  fpromise::result<Report> report =
      Report::MakeReport(report_id, *program_shortname, annotations, MakeAttachments(),
                         kSnapshotUuidValue, BuildAttachment(kMinidumpValue));
  FX_CHECK(report.is_ok());
  return std::move(report.value());
}

Report MakeHourlyReport(const std::size_t report_id, const bool empty_annotations = false) {
  AnnotationMap annotations;
  if (!empty_annotations) {
    annotations = MakeAnnotations();
  }
  fpromise::result<Report> report =
      Report::MakeReport(report_id, *ProgramShortname::Create(kHourlySnapshotProgramName),
                         annotations, MakeAttachments(), kSnapshotUuidValue,
                         BuildAttachment(kMinidumpValue), /*is_hourly_report=*/true);
  FX_CHECK(report.is_ok());
  return std::move(report.value());
}

class TestReportingPolicyWatcher : public ReportingPolicyWatcher {
 public:
  TestReportingPolicyWatcher() : ReportingPolicyWatcher(ReportingPolicy::kUndecided) {}

  void Set(const ReportingPolicy policy) { SetPolicy(policy); }
};

class QueueTest : public UnitTestFixture {
 public:
  QueueTest() : annotation_manager_(dispatcher(), {}) {}

  void SetUp() override {
    info_context_ =
        std::make_shared<InfoContext>(&InspectRoot(), &clock_, dispatcher(), services());
    report_store_ = std::make_unique<ScopedTestReportStore>(&annotation_manager_, info_context_);

    SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));
    RunLoopUntilIdle();
  }

 protected:
  void SetUpQueue(const std::vector<CrashServer::UploadStatus>& upload_attempt_results =
                      std::vector<CrashServer::UploadStatus>{}) {
    report_id_ = 1;
    crash_server_ =
        std::make_unique<StubCrashServer>(dispatcher(), services(), &annotation_manager_,
                                          upload_attempt_results, &clock_, kUploadResponseDelay);

    InitQueue();
  }

  void InitQueue() {
    queue_ = std::make_unique<Queue>(dispatcher(), services(), info_context_, &tags_,
                                     &report_store_->GetReportStore(), crash_server_.get());
    queue_->WatchReportingPolicy(&reporting_policy_watcher_);
  }

  SnapshotStore* GetSnapshotStore() { return report_store_->GetReportStore().GetSnapshotStore(); }

  std::optional<ReportId> AddNewReport(const bool is_hourly_report,
                                       const bool empty_annotations = false) {
    return AddNewReportWithStatus(
        is_hourly_report, [](const FilingResult& result, const std::string& report_id) {},
        empty_annotations);
  }

  std::optional<ReportId> AddNewReportWithStatus(const bool is_hourly_report,
                                                 FilingResultFn callback,
                                                 const bool empty_annotations = false) {
    ++report_id_;
    queue_->AddReportUsingSnapshot(kSnapshotUuidValue, report_id_);
    Report report = (is_hourly_report) ? MakeHourlyReport(report_id_, empty_annotations)
                                       : MakeReport(report_id_, empty_annotations);

    if (queue_->Add(std::move(report), std::move(callback))) {
      return report_id_;
    }
    return std::nullopt;
  }

  void CheckAnnotationsOnServer() {
    FX_CHECK(crash_server_);

    // Expect annotations that |snapshot_collector_| will for using |kSnapshotUuidValue| as the
    // snapshot uuid.
    EXPECT_THAT(crash_server_->latest_annotations().Raw(),
                UnorderedElementsAreArray({
                    Pair(kAnnotationKey, kAnnotationValue),
                    Pair(feedback::kDebugSnapshotErrorKey, "not persisted"),
                    Pair(feedback::kDebugSnapshotPresentKey, "false"),
                }));
  }

  void CheckAttachmentKeysOnServer() {
    FX_CHECK(crash_server_);
    EXPECT_THAT(crash_server_->latest_attachment_keys(),
                UnorderedElementsAre(kAttachmentKey, kMinidumpKey));
  }

  std::optional<std::string> DeleteReportFromStore() {
    auto RemoveCurDir = [](std::vector<std::string>* contents) {
      contents->erase(std::remove(contents->begin(), contents->end(), "."), contents->end());
    };

    std::vector<std::string> program_shortnames;
    files::ReadDirContents(report_store_->GetCacheReportsPath(), &program_shortnames);
    RemoveCurDir(&program_shortnames);
    for (const std::string& program_shortname : program_shortnames) {
      const std::string path =
          files::JoinPath(report_store_->GetCacheReportsPath(), program_shortname);

      std::vector<std::string> report_ids;
      files::ReadDirContents(path, &report_ids);
      RemoveCurDir(&report_ids);

      if (!report_ids.empty()) {
        files::DeletePath(files::JoinPath(path, report_ids.back()), /*recursive=*/true);
        return report_ids.back();
      }
    }
    return std::nullopt;
  }

  LogTags tags_;
  std::unique_ptr<Queue> queue_;
  TestReportingPolicyWatcher reporting_policy_watcher_;

  size_t report_id_ = 1;

  timekeeper::TestClock clock_;
  feedback::AnnotationManager annotation_manager_;
  std::unique_ptr<StubCrashServer> crash_server_;
  std::unique_ptr<ScopedTestReportStore> report_store_;
  std::shared_ptr<InfoContext> info_context_;
  std::shared_ptr<cobalt::Logger> cobalt_;
};

TEST_F(QueueTest, Add_ReportingPolicyUndecided) {
  SetUpQueue();

  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);
  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);

  ASSERT_TRUE(*report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));
}

TEST_F(QueueTest, Add_ReportingPolicyUndecided_HourlyReports) {
  SetUpQueue();

  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);
  const std::optional<ReportId> report_id_1 = AddNewReport(/*is_hourly_report=*/true);

  ASSERT_TRUE(*report_id_1);
  EXPECT_TRUE(queue_->Contains(*report_id_1));
}

TEST_F(QueueTest, AddTwoHourlyReports) {
  SetUpQueue();
  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);

  const std::optional<ReportId> report_id_1 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_1.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));

  const std::optional<ReportId> report_id_2 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_2.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_TRUE(queue_->Contains(*report_id_2));
}

TEST_F(QueueTest, AddThreeHourlyReportsDeletesMiddleReport) {
  SetUpQueue();
  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);

  const std::optional<ReportId> report_id_1 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_1.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));

  const std::optional<ReportId> report_id_2 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_2.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_TRUE(queue_->Contains(*report_id_2));

  const std::optional<ReportId> report_id_3 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_3.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_FALSE(queue_->Contains(*report_id_2));
  EXPECT_TRUE(queue_->Contains(*report_id_3));
}

TEST_F(QueueTest, KeepsHourlyReportOnFailureIfOldestReport) {
  SetUpQueue({kUploadFailed, kUploadSuccessful});
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  const std::optional<ReportId> report_id_1 = AddNewReport(/*is_hourly_report=*/true);
  const std::optional<ReportId> report_id_2 = AddNewReport(/*is_hourly_report=*/true);

  ASSERT_TRUE(report_id_1.has_value());
  ASSERT_TRUE(report_id_2.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_TRUE(queue_->Contains(*report_id_2));

  RunLoopFor(kUploadResponseDelay);
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_TRUE(queue_->Contains(*report_id_2));
}

TEST_F(QueueTest, KeepsHourlyReportOnFailureIfNewestReport) {
  SetUpQueue({kUploadFailed, kUploadFailed});
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  const std::optional<ReportId> report_id_1 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_1.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));

  RunLoopFor(kUploadResponseDelay);

  const std::optional<ReportId> report_id_2 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_2.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_2));

  RunLoopFor(kUploadResponseDelay);
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_TRUE(queue_->Contains(*report_id_2));
}

TEST_F(QueueTest, DeletesHourlyReportOnFailureIfMiddleReport) {
  SetUpQueue({kUploadFailed, kUploadFailed, kUploadSuccessful});
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  const std::optional<ReportId> report_id_1 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_1.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));

  RunLoopFor(kUploadResponseDelay);

  const std::optional<ReportId> report_id_2 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_2.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_TRUE(queue_->Contains(*report_id_2));

  const std::optional<ReportId> report_id_3 = AddNewReport(/*is_hourly_report=*/true);
  ASSERT_TRUE(report_id_3.has_value());
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_TRUE(queue_->Contains(*report_id_2));
  EXPECT_TRUE(queue_->Contains(*report_id_3));

  RunLoopFor(kUploadResponseDelay);
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_FALSE(queue_->Contains(*report_id_2));
  EXPECT_TRUE(queue_->Contains(*report_id_3));
}

TEST_F(QueueTest, Add_ReportingPolicyDoNotFileAndDelete) {
  SetUpQueue();

  reporting_policy_watcher_.Set(ReportingPolicy::kDoNotFileAndDelete);
  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);

  ASSERT_TRUE(*report_id);
  EXPECT_FALSE(queue_->Contains(*report_id));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::CrashState::kDeleted),
                                      }));
}

TEST_F(QueueTest, Add_ReportingPolicyArchive) {
  SetUpQueue();

  reporting_policy_watcher_.Set(ReportingPolicy::kArchive);
  std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);

  ASSERT_TRUE(*report_id);
  EXPECT_FALSE(queue_->Contains(*report_id));

  report_id = AddNewReport(/*is_hourly_report=*/true);

  ASSERT_TRUE(*report_id);
  EXPECT_FALSE(queue_->Contains(*report_id));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::CrashState::kArchived),
                                          cobalt::Event(cobalt::CrashState::kArchived),
                                      }));
}

TEST_F(QueueTest, Add_ReportingPolicyUpload) {
  SetUpQueue({kUploadSuccessful});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);

  ASSERT_TRUE(*report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  report_id = AddNewReport(/*is_hourly_report=*/false);

  ASSERT_TRUE(*report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  report_id = AddNewReport(/*is_hourly_report=*/true);

  ASSERT_TRUE(*report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  EXPECT_EQ(queue_->Size(), 3u);
}

TEST_F(QueueTest, ReportingPolicyChangedToDoNotFileAndDelete_DeletesSnapshots) {
  SetUpQueue();

  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));

  GetSnapshotStore()->AddSnapshot(kSnapshotUuidValue, std::move(snapshot));
  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  ASSERT_TRUE(*report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  reporting_policy_watcher_.Set(ReportingPolicy::kDoNotFileAndDelete);

  EXPECT_FALSE(queue_->Contains(*report_id));
  EXPECT_FALSE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::CrashState::kDeleted),
                                      }));
}

TEST_F(QueueTest, ReportingPolicyChangedToDoNotFileAndDelete_DuringUploadFromStore) {
  SetUpQueue({
      kUploadFailed,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);

  std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);

  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));
  ASSERT_TRUE(report_store_->GetReportStore().Contains(*report_id));

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  queue_->SetNetworkIsReachable(true);

  reporting_policy_watcher_.Set(ReportingPolicy::kDoNotFileAndDelete);

  RunLoopFor(kUploadResponseDelay);
  EXPECT_FALSE(queue_->Contains(*report_id));
}

TEST_F(QueueTest, ReportingPolicyChangedToArchive_ArchivesReports) {
  SetUpQueue({kUploadSuccessful});
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  // Add a second report while |report_id| is being uploaded. This will keep |report_id2| in the
  // Queue, letting us test that a change to |kArchive| results in |report_id2| being moved to the
  // report store.
  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  const std::optional<ReportId> report_id2 = AddNewReport(/*is_hourly_report=*/false);

  ASSERT_TRUE(report_id);
  ASSERT_TRUE(report_id2);
  EXPECT_TRUE(queue_->Contains(*report_id));
  EXPECT_TRUE(queue_->Contains(*report_id2));
  EXPECT_FALSE(report_store_->GetReportStore().Contains(*report_id2));

  reporting_policy_watcher_.Set(ReportingPolicy::kArchive);
  EXPECT_FALSE(queue_->Contains(*report_id2));
  EXPECT_TRUE(report_store_->GetReportStore().Contains(*report_id2));
}

TEST_F(QueueTest, Upload) {
  SetUpQueue({kUploadSuccessful, kUploadFailed, kUploadSuccessful, kUploadFailed});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::vector<ReportId> report_ids;
  for (size_t i = 0; i < 4; ++i) {
    const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
    ASSERT_TRUE(report_id);
    ASSERT_TRUE(queue_->Contains(*report_id));
    report_ids.push_back(*report_id);
  }

  RunLoopFor(kUploadResponseDelay * report_ids.size());

  EXPECT_FALSE(queue_->Contains(report_ids[0]));
  EXPECT_TRUE(queue_->Contains(report_ids[1]));
  EXPECT_FALSE(queue_->Contains(report_ids[2]));
  EXPECT_TRUE(queue_->Contains(report_ids[3]));

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 1u),
              }));
}

TEST_F(QueueTest, FilingStatusUpload) {
  SetUpQueue({kUploadSuccessful, kUploadSuccessful});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::vector<FilingResult> results;
  std::vector<std::string> report_ids;
  for (size_t i = 0; i < 2; ++i) {
    AddNewReportWithStatus(
        /*is_hourly_report=*/false,
        [&results, &report_ids](const FilingResult& new_result, const std::string& new_report_id) {
          results.push_back(new_result);
          report_ids.push_back(new_report_id);
        });
  }

  RunLoopFor(kUploadResponseDelay * 2);

  EXPECT_THAT(results, ElementsAreArray({
                           FilingResult::kReportUploaded,
                           FilingResult::kReportUploaded,
                       }));

  EXPECT_THAT(report_ids, ElementsAreArray({
                              "1",
                              "2",
                          }));
}

TEST_F(QueueTest, FilingStatusReportOnDisk) {
  SetUpQueue({kUploadFailed});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::optional<FilingResult> result;
  std::optional<std::string> report_id;
  AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result, &report_id](const FilingResult& new_result, const std::string& new_report_id) {
        result = new_result;
        report_id = new_report_id;
      });

  RunLoopFor(kUploadResponseDelay);

  EXPECT_EQ(result, FilingResult::kReportOnDisk);
  EXPECT_EQ(report_id, "");
}

TEST_F(QueueTest, FilingStatusReportInMemory) {
  report_store_ =
      std::make_unique<ScopedTestReportStore>(&annotation_manager_, info_context_,
                                              /*max_reports_tmp_size=*/StorageSize::Megabytes(5),
                                              /*max_reports_cache_size=*/StorageSize::Bytes(0));

  SetUpQueue({kUploadFailed});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::optional<FilingResult> result;
  std::optional<std::string> report_id;
  AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result, &report_id](const FilingResult& new_result, const std::string& new_report_id) {
        result = new_result;
        report_id = new_report_id;
      });

  RunLoopFor(kUploadResponseDelay);

  EXPECT_EQ(result, FilingResult::kReportInMemory);
  EXPECT_EQ(report_id, "");
}

TEST_F(QueueTest, FilingStatusDelete) {
  SetUpQueue();

  reporting_policy_watcher_.Set(ReportingPolicy::kDoNotFileAndDelete);
  std::optional<FilingResult> result;
  std::optional<std::string> report_id;
  AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result, &report_id](const FilingResult& new_result, const std::string& new_report_id) {
        result = new_result;
        report_id = new_report_id;
      });

  EXPECT_EQ(result, FilingResult::kReportNotFiledUserOptedOut);
  EXPECT_EQ(report_id, "");
}

TEST_F(QueueTest, FilingStatusDeleteDuringUpload) {
  SetUpQueue({kUploadFailed});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::vector<FilingResult> results;
  std::vector<std::string> report_ids;
  for (size_t i = 0; i < 2; ++i) {
    AddNewReportWithStatus(
        /*is_hourly_report=*/false,
        [&results, &report_ids](const FilingResult& new_result, const std::string& new_report_id) {
          results.push_back(new_result);
          report_ids.push_back(new_report_id);
        });
  }

  reporting_policy_watcher_.Set(ReportingPolicy::kDoNotFileAndDelete);
  RunLoopFor(kUploadResponseDelay);

  EXPECT_THAT(results, ElementsAreArray({
                           FilingResult::kReportNotFiledUserOptedOut,
                           FilingResult::kReportNotFiledUserOptedOut,
                       }));

  EXPECT_THAT(report_ids, ElementsAreArray({
                              "",
                              "",
                          }));
}

TEST_F(QueueTest, FilingStatusServerTimedOut) {
  SetUpQueue({kUploadTimedOut});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::optional<FilingResult> result;
  std::optional<std::string> report_id;
  AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result, &report_id](const FilingResult& new_result, const std::string& new_report_id) {
        result = new_result;
        report_id = new_report_id;
      });

  RunLoopFor(kUploadResponseDelay);

  EXPECT_EQ(result, FilingResult::kServerError);
  EXPECT_EQ(report_id, "");
}

TEST_F(QueueTest, FilingStatusServerThrottled) {
  SetUpQueue({kUploadThrottled});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::optional<FilingResult> result;
  std::optional<std::string> report_id;
  AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result, &report_id](const FilingResult& new_result, const std::string& new_report_id) {
        result = new_result;
        report_id = new_report_id;
      });

  RunLoopFor(kUploadResponseDelay);

  EXPECT_EQ(result, FilingResult::kServerError);
  EXPECT_EQ(report_id, "");
}

TEST_F(QueueTest, FilingStatusPersistenceError) {
  report_store_ =
      std::make_unique<ScopedTestReportStore>(&annotation_manager_, info_context_,
                                              /*max_reports_tmp_size=*/StorageSize::Bytes(0),
                                              /*max_reports_cache_size=*/StorageSize::Bytes(0));

  SetUpQueue({kUploadFailed});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::optional<FilingResult> result;
  std::optional<std::string> report_id;
  AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result, &report_id](const FilingResult& new_result, const std::string& new_report_id) {
        result = new_result;
        report_id = new_report_id;
      });

  RunLoopFor(kUploadResponseDelay);

  EXPECT_EQ(result, FilingResult::kPersistenceError);
  EXPECT_EQ(report_id, "");
}

TEST_F(QueueTest, SkipEmptyAnnotationUpload) {
  SetUpQueue({});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  std::vector<ReportId> report_ids;
  for (size_t i = 0; i < 4; ++i) {
    const std::optional<ReportId> report_id =
        AddNewReport(/*is_hourly_report=*/false, /*empty_annotations=*/true);
    ASSERT_TRUE(report_id);
    ASSERT_FALSE(queue_->Contains(*report_id));
    report_ids.push_back(*report_id);
  }

  RunLoopFor(kUploadResponseDelay * report_ids.size());

  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::CrashState::kGarbageCollected),
                                          cobalt::Event(cobalt::CrashState::kGarbageCollected),
                                          cobalt::Event(cobalt::CrashState::kGarbageCollected),
                                          cobalt::Event(cobalt::CrashState::kGarbageCollected),
                                      }));
}

TEST_F(QueueTest, StopUploading) {
  SetUpQueue({kUploadFailed});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  std::vector<ReportId> report_ids;
  for (size_t i = 0; i < 3; ++i) {
    const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
    ASSERT_TRUE(*report_id);
    EXPECT_TRUE(queue_->Contains(*report_id));
    report_ids.push_back(*report_id);
  }

  queue_->StopUploading();
  RunLoopFor(kUploadResponseDelay);

  EXPECT_FALSE(queue_->IsPeriodicUploadScheduled());
  for (const ReportId& report_id : report_ids) {
    EXPECT_FALSE(queue_->Contains(report_id));
  }
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
              }));

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  EXPECT_FALSE(queue_->IsPeriodicUploadScheduled());
  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(*report_id);
  EXPECT_FALSE(queue_->Contains(*report_id));
  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
              }));
}

TEST_F(QueueTest, PeriodicUpload) {
  SetUpQueue({
      kUploadFailed,
      kUploadFailed,
      kUploadFailed,
      kUploadSuccessful,
      kUploadSuccessful,
      kUploadSuccessful,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);

  std::vector<ReportId> report_ids;
  for (size_t i = 0; i < 3; ++i) {
    std::optional<ReportId> report_id = AddNewReport(/*is_hourly_upload=*/false);

    ASSERT_TRUE(report_id);
    EXPECT_TRUE(queue_->Contains(*report_id));
    report_ids.push_back(*report_id);
  }

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  ASSERT_TRUE(queue_->IsPeriodicUploadScheduled());
  RunLoopFor(kUploadResponseDelay * report_ids.size());

  RunLoopFor(kPeriodicUploadDuration);
  for (const ReportId& report_id : report_ids) {
    EXPECT_FALSE(queue_->Contains(report_id));
  }

  CheckAnnotationsOnServer();
  CheckAttachmentKeysOnServer();
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 2u),
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 2u),
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 2u),
              }));
}

TEST_F(QueueTest, PeriodicUpload_ReportingPolicyChanges) {
  SetUpQueue();

  EXPECT_FALSE(queue_->IsPeriodicUploadScheduled());

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  EXPECT_TRUE(queue_->IsPeriodicUploadScheduled());

  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);
  EXPECT_FALSE(queue_->IsPeriodicUploadScheduled());

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  EXPECT_TRUE(queue_->IsPeriodicUploadScheduled());

  reporting_policy_watcher_.Set(ReportingPolicy::kDoNotFileAndDelete);
  EXPECT_FALSE(queue_->IsPeriodicUploadScheduled());

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  EXPECT_TRUE(queue_->IsPeriodicUploadScheduled());

  reporting_policy_watcher_.Set(ReportingPolicy::kArchive);
  EXPECT_FALSE(queue_->IsPeriodicUploadScheduled());
}

TEST_F(QueueTest, PeriodicUpload_AfterStopUploading) {
  SetUpQueue();

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  ASSERT_TRUE(queue_->IsPeriodicUploadScheduled());

  queue_->StopUploading();
  EXPECT_FALSE(queue_->IsPeriodicUploadScheduled());
}

TEST_F(QueueTest, UploadOnNetworkReachable) {
  SetUpQueue({
      kUploadFailed,
      kUploadFailed,
      kUploadFailed,
      kUploadSuccessful,
      kUploadSuccessful,
      kUploadSuccessful,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);

  std::vector<ReportId> report_ids;
  for (size_t i = 0; i < 3; ++i) {
    std::optional<ReportId> report_id = AddNewReport(/*is_hourly_upload=*/false);

    ASSERT_TRUE(report_id);
    EXPECT_TRUE(queue_->Contains(*report_id));
    report_ids.push_back(*report_id);
  }

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  ASSERT_TRUE(queue_->IsPeriodicUploadScheduled());
  RunLoopFor(kUploadResponseDelay * report_ids.size());

  queue_->SetNetworkIsReachable(true);
  RunLoopFor(kUploadResponseDelay * report_ids.size());
  for (const ReportId& report_id : report_ids) {
    EXPECT_FALSE(queue_->Contains(report_id));
  }

  CheckAnnotationsOnServer();
  CheckAttachmentKeysOnServer();
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 2u),
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 2u),
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 2u),
              }));
}

TEST_F(QueueTest, UnblockAllAfterSuccessfulEagerUpload) {
  SetUpQueue({
      kUploadFailed,
      kUploadSuccessful,
      kUploadSuccessful,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  std::optional<FilingResult> result_1;
  const std::optional<ReportId> report_id_1 = AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result_1](const FilingResult& new_result, const std::string& server_report_id) {
        result_1 = new_result;
      });

  RunLoopFor(kUploadResponseDelay);
  ASSERT_TRUE(report_id_1);
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_EQ(result_1, FilingResult::kReportOnDisk);

  std::optional<FilingResult> result_2;
  const std::optional<ReportId> report_id_2 = AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result_2](const FilingResult& new_result, const std::string& server_report_id) {
        result_2 = new_result;
      });

  RunLoopFor(kUploadResponseDelay);
  ASSERT_TRUE(report_id_2);
  EXPECT_FALSE(queue_->Contains(*report_id_2));
  EXPECT_EQ(result_2, FilingResult::kReportUploaded);

  RunLoopFor(kUploadResponseDelay);
  EXPECT_FALSE(queue_->Contains(*report_id_1));
}

TEST_F(QueueTest, DoNotUnblockAllAfterSuccessfulUneagerUpload) {
  // Add 2 blocked reports to the queue, then trigger the 15 minute periodic upload. The 2nd report
  // being successfully uploaded should not result in the 1st retrying again.
  SetUpQueue({
      kUploadFailed,
      kUploadFailed,
      kUploadFailed,
      kUploadSuccessful,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  std::optional<FilingResult> result_1;
  const std::optional<ReportId> report_id_1 = AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result_1](const FilingResult& new_result, const std::string& server_report_id) {
        result_1 = new_result;
      });

  RunLoopFor(kUploadResponseDelay);
  ASSERT_TRUE(report_id_1);
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_EQ(result_1, FilingResult::kReportOnDisk);

  std::optional<FilingResult> result_2;
  const std::optional<ReportId> report_id_2 = AddNewReportWithStatus(
      /*is_hourly_report=*/false,
      [&result_2](const FilingResult& new_result, const std::string& server_report_id) {
        result_2 = new_result;
      });

  RunLoopFor(kUploadResponseDelay);
  ASSERT_TRUE(report_id_2);
  EXPECT_TRUE(queue_->Contains(*report_id_2));
  EXPECT_EQ(result_2, FilingResult::kReportOnDisk);

  RunLoopFor(kPeriodicUploadDuration);
  RunLoopFor(2 * kUploadResponseDelay);
  EXPECT_TRUE(queue_->Contains(*report_id_1));
  EXPECT_FALSE(queue_->Contains(*report_id_2));
}

TEST_F(QueueTest, UploadThrottled) {
  SetUpQueue({kUploadThrottled, kUploadFailed, kUploadThrottled});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  RunLoopFor(kUploadResponseDelay * 2);
  EXPECT_TRUE(queue_->Contains(*report_id));

  RunLoopFor(kPeriodicUploadDuration);
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::CrashState::kUploadThrottled),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadThrottled, 1u),
                  cobalt::Event(cobalt::CrashState::kUploadThrottled),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadThrottled, 2u),
              }));
}

TEST_F(QueueTest, kUploadTimedOut) {
  SetUpQueue({kUploadTimedOut, kUploadFailed, kUploadTimedOut});

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  RunLoopFor(kUploadResponseDelay * 2);
  EXPECT_TRUE(queue_->Contains(*report_id));

  RunLoopFor(kPeriodicUploadDuration);
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::CrashState::kUploadTimedOut),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadTimedOut, 1u),
                  cobalt::Event(cobalt::CrashState::kUploadTimedOut),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 2u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadTimedOut, 2u),
              }));
}

TEST_F(QueueTest, InitializeFromStore) {
  // This test cannot call RunLoopUntilIdle in any capacity once InitQueue has been called been
  // called for the second time. The watchers still hold callbacks tied to the old, deleted queue
  // and will crash if they attempt to execute the callbacks.
  SetUpQueue();
  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);

  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  SetUpQueue();
  EXPECT_TRUE(queue_->Contains(*report_id));
}

TEST_F(QueueTest, ReportDeletedByStore) {
  SetUpQueue();
  reporting_policy_watcher_.Set(ReportingPolicy::kUndecided);

  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  ASSERT_TRUE(DeleteReportFromStore().has_value());
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  RunLoopFor(kPeriodicUploadDuration);

  EXPECT_FALSE(queue_->Contains(*report_id));
}

TEST_F(QueueTest, SnapshotKeptAllReportsAdded) {
  SetUpQueue({kUploadSuccessful, kUploadSuccessful});
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  // report_id_ will get incremented when we call AddNewReport. Add all clients before any get added
  // to Queue.
  queue_->AddReportUsingSnapshot(kSnapshotUuidValue, report_id_ + 1);
  queue_->AddReportUsingSnapshot(kSnapshotUuidValue, report_id_ + 2);

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));

  GetSnapshotStore()->AddSnapshot(kSnapshotUuidValue, std::move(snapshot));
  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  const std::optional<ReportId> report_id2 = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id2);
  EXPECT_TRUE(queue_->Contains(*report_id2));

  RunLoopFor(kUploadResponseDelay);

  // Queue shouldn't delete the snapshot if there's still an internal client.
  ASSERT_FALSE(queue_->Contains(*report_id));
  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  RunLoopFor(kUploadResponseDelay);

  EXPECT_FALSE(queue_->Contains(*report_id2));
  EXPECT_FALSE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));
}

TEST_F(QueueTest, SnapshotKeptNotAllReportsAdded) {
  SetUpQueue({kUploadSuccessful, kUploadSuccessful});
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  // report_id_ will get incremented when we call AddNewReport. Add all clients before any get added
  // to Queue.
  queue_->AddReportUsingSnapshot(kSnapshotUuidValue, report_id_ + 1);
  queue_->AddReportUsingSnapshot(kSnapshotUuidValue, report_id_ + 2);

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));

  GetSnapshotStore()->AddSnapshot(kSnapshotUuidValue, std::move(snapshot));
  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  RunLoopFor(kUploadResponseDelay);

  // Queue shouldn't delete the snapshot if there's still a client in SnapshotCollector.
  ASSERT_FALSE(queue_->Contains(*report_id));
  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  const std::optional<ReportId> report_id2 = AddNewReport(/*is_hourly_report=*/false);
  ASSERT_TRUE(report_id2);
  EXPECT_TRUE(queue_->Contains(*report_id2));

  RunLoopFor(kUploadResponseDelay);

  EXPECT_FALSE(queue_->Contains(*report_id2));
  EXPECT_FALSE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));
}

TEST_F(QueueTest, Check_SpecialCaseClientsRemoved) {
  SetUpQueue({
      kUploadSuccessful,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));

  GetSnapshotStore()->AddSnapshot(kSnapshotUuidValue, std::move(snapshot));
  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  ++report_id_;
  queue_->AddReportUsingSnapshot(kSnapshotUuidValue, report_id_);
  Report report = MakeReport(report_id_, {});

  // Modify report to have special case uuid.
  report.SnapshotUuid() = kShutdownSnapshotUuid;

  queue_->Add(std::move(report),
              [](const FilingResult& new_result, const std::string& new_report_id) {});

  EXPECT_TRUE(queue_->Contains(report_id_));

  // Queue should delete snapshot despite the report ending up with a special case snapshot uuid.
  EXPECT_FALSE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));
}

TEST_F(QueueTest, PreventStrandedSnapshot) {
  report_store_ = std::make_unique<ScopedTestReportStore>(
      &annotation_manager_, info_context_,
      /*max_reports_tmp_size=*/StorageSize::Megabytes(5),
      /*max_reports_cache_size=*/StorageSize::Bytes(0),
      /*max_snapshots_tmp_size=*/StorageSize::Megabytes(1),
      /*max_snapshots_cache_size=*/StorageSize::Megabytes(1));

  // Fail the first 2 uploads so the reports get moved to /tmp. Fail the last upload so we
  // can verify the snapshot was moved from /cache to /tmp.
  SetUpQueue({
      kUploadFailed,
      kUploadFailed,
      kUploadSuccessful,
      kUploadFailed,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/
                                                         false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  const std::optional<ReportId> report_id2 = AddNewReport(/*is_hourly_report=*/
                                                          false);
  ASSERT_TRUE(report_id2);
  EXPECT_TRUE(queue_->Contains(*report_id2));

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));
  GetSnapshotStore()->AddSnapshot(kSnapshotUuidValue, std::move(snapshot));

  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  ASSERT_EQ(GetSnapshotStore()->MoveToPersistence(kSnapshotUuidValue, /*only_consider_tmp=*/false),
            ItemLocation::kCache);
  ASSERT_EQ(GetSnapshotStore()->SnapshotLocation(kSnapshotUuidValue), ItemLocation::kCache);

  // Initial upload attempts + 15 minute retry.
  RunLoopFor(2 * kUploadResponseDelay + zx::min(15));

  EXPECT_EQ(GetSnapshotStore()->SnapshotLocation(kSnapshotUuidValue), ItemLocation::kTmp);
}

TEST_F(QueueTest, PreventStrandedSnapshot_FailedMove) {
  report_store_ = std::make_unique<ScopedTestReportStore>(
      &annotation_manager_, info_context_,
      /*max_reports_tmp_size=*/StorageSize::Megabytes(5),
      /*max_reports_cache_size=*/StorageSize::Bytes(0),
      /*max_snapshots_tmp_size=*/StorageSize::Bytes(0),
      /*max_snapshots_cache_size=*/StorageSize::Megabytes(1));

  // Fail the first 2 uploads so the reports get moved to /tmp. Fail the 4th upload so we
  // can verify the snapshot was deleted after failing to move to /tmp. Succeed on the last upload
  // so we can verify the debug.snapshot annotations added to the 2nd report.
  SetUpQueue({
      kUploadFailed,
      kUploadFailed,
      kUploadSuccessful,
      kUploadFailed,
      kUploadSuccessful,
  });
  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);

  const std::optional<ReportId> report_id = AddNewReport(/*is_hourly_report=*/
                                                         false);
  ASSERT_TRUE(report_id);
  EXPECT_TRUE(queue_->Contains(*report_id));

  const std::optional<ReportId> report_id2 = AddNewReport(/*is_hourly_report=*/
                                                          false);
  ASSERT_TRUE(report_id2);
  EXPECT_TRUE(queue_->Contains(*report_id2));

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));
  GetSnapshotStore()->AddSnapshot(kSnapshotUuidValue, std::move(snapshot));

  ASSERT_TRUE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  ASSERT_EQ(GetSnapshotStore()->MoveToPersistence(kSnapshotUuidValue, /*only_consider_tmp=*/false),
            ItemLocation::kCache);
  ASSERT_EQ(GetSnapshotStore()->SnapshotLocation(kSnapshotUuidValue), ItemLocation::kCache);

  // Initial upload attempts + 15 minute retry.
  RunLoopFor(2 * kUploadResponseDelay + zx::min(15));

  EXPECT_FALSE(GetSnapshotStore()->SnapshotExists(kSnapshotUuidValue));

  RunLoopFor(zx::min(15));

  FX_CHECK(crash_server_);

  EXPECT_THAT(crash_server_->latest_annotations().Raw(),
              UnorderedElementsAreArray({
                  Pair(kAnnotationKey, kAnnotationValue),
                  Pair(feedback::kDebugSnapshotErrorKey, "failed move to tmp"),
                  Pair(feedback::kDebugSnapshotPresentKey, "false"),
              }));
}

TEST_F(QueueTest, Check_SnapshotClientsReloaded) {
  report_store_ = std::make_unique<ScopedTestReportStore>(
      &annotation_manager_, info_context_,
      /*max_reports_tmp_size=*/StorageSize::Megabytes(5),
      /*max_reports_cache_size=*/StorageSize::Kilobytes(500),
      /*max_snapshots_tmp_size=*/StorageSize::Bytes(0),
      /*max_snapshots_cache_size=*/StorageSize::Megabytes(1));

  Report report = MakeReport(report_id_, /*empty_annotations=*/false);

  ++report_id_;
  Report report2 = MakeReport(report_id_, /*empty_annotations=*/false);

  const ReportId report_id = report.Id();
  const ReportId report2_id = report2.Id();

  std::vector<ReportId> garbage_collected_reports;
  report_store_->GetReportStore().Add(std::move(report), &garbage_collected_reports);
  report_store_->GetReportStore().Add(std::move(report2), &garbage_collected_reports);

  ASSERT_TRUE(garbage_collected_reports.empty());

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));

  GetSnapshotStore()->AddSnapshot(kSnapshotUuidValue, std::move(snapshot));
  ASSERT_EQ(GetSnapshotStore()->SnapshotLocation(kSnapshotUuidValue), ItemLocation::kMemory);

  ASSERT_EQ(GetSnapshotStore()->MoveToPersistence(kSnapshotUuidValue, /*only_consider_tmp=*/false),
            ItemLocation::kCache);
  ASSERT_EQ(GetSnapshotStore()->SnapshotLocation(kSnapshotUuidValue), ItemLocation::kCache);

  // Verify report clients are reloaded by checking if the snapshot gets deleted prematurely.
  SetUpQueue({
      kUploadSuccessful,
      kUploadSuccessful,
  });

  ASSERT_TRUE(queue_->Contains(report_id));
  ASSERT_TRUE(queue_->Contains(report2_id));

  reporting_policy_watcher_.Set(ReportingPolicy::kUpload);
  queue_->SetNetworkIsReachable(true);

  RunLoopFor(kUploadResponseDelay);
  EXPECT_FALSE(queue_->Contains(report_id));
  EXPECT_TRUE(queue_->Contains(report2_id));

  ASSERT_EQ(GetSnapshotStore()->SnapshotLocation(kSnapshotUuidValue), ItemLocation::kCache);

  RunLoopFor(kUploadResponseDelay);
  EXPECT_FALSE(queue_->Contains(report_id));
  EXPECT_FALSE(queue_->Contains(report2_id));
  EXPECT_FALSE(GetSnapshotStore()->SnapshotLocation(kSnapshotUuidValue).has_value());
}

TEST_F(QueueTest, Check_CleansUpStrandedSnapshotsInCache) {
  report_store_ = std::make_unique<ScopedTestReportStore>(
      &annotation_manager_, info_context_,
      /*max_reports_tmp_size=*/StorageSize::Megabytes(5),
      /*max_reports_cache_size=*/StorageSize::Kilobytes(500),
      /*max_snapshots_tmp_size=*/StorageSize::Bytes(0),
      /*max_snapshots_cache_size=*/StorageSize::Megabytes(1));

  SetUpQueue();

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));

  const std::string kTestUuid = "test uuid";
  GetSnapshotStore()->AddSnapshot(kTestUuid, std::move(snapshot));
  ASSERT_EQ(GetSnapshotStore()->MoveToPersistence(kTestUuid, /*only_consider_tmp=*/false),
            ItemLocation::kCache);
  ASSERT_EQ(GetSnapshotStore()->SnapshotLocation(kTestUuid), ItemLocation::kCache);

  // Queue should clean up stranded snapshots on construction.
  SetUpQueue();

  EXPECT_FALSE(GetSnapshotStore()->SnapshotExists(kTestUuid));
}

TEST_F(QueueTest, Check_CleansUpStrandedSnapshotsInTmp) {
  report_store_ = std::make_unique<ScopedTestReportStore>(
      &annotation_manager_, info_context_,
      /*max_reports_tmp_size=*/StorageSize::Megabytes(5),
      /*max_reports_cache_size=*/StorageSize::Kilobytes(500),
      /*max_snapshots_tmp_size=*/StorageSize::Megabytes(1),
      /*max_snapshots_cache_size=*/StorageSize::Bytes(0));

  SetUpQueue();

  fuchsia::feedback::Attachment snapshot;
  snapshot.key = kAttachmentKey;
  FX_CHECK(fsl::VmoFromString("", &snapshot.value));

  const std::string kTestUuid = "test uuid";
  GetSnapshotStore()->AddSnapshot(kTestUuid, std::move(snapshot));
  ASSERT_EQ(GetSnapshotStore()->MoveToPersistence(kTestUuid, /*only_consider_tmp=*/true),
            ItemLocation::kTmp);
  ASSERT_EQ(GetSnapshotStore()->SnapshotLocation(kTestUuid), ItemLocation::kTmp);

  // Queue should clean up stranded snapshots on construction.
  SetUpQueue();

  EXPECT_FALSE(GetSnapshotStore()->SnapshotExists(kTestUuid));
}

}  // namespace
}  // namespace crash_reports
}  // namespace forensics
