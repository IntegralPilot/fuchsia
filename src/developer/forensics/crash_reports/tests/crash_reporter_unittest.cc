// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/crash_reporter.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <fuchsia/mem/cpp/fidl.h>
#include <fuchsia/settings/cpp/fidl.h>
#include <fuchsia/time/cpp/fidl.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/types.h>
#include <zircon/utc.h>

#include <algorithm>
#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/crash_reports/constants.h"
#include "src/developer/forensics/crash_reports/info/info_context.h"
#include "src/developer/forensics/crash_reports/tests/scoped_test_report_store.h"
#include "src/developer/forensics/crash_reports/tests/stub_crash_server.h"
#include "src/developer/forensics/feedback/annotations/annotation_manager.h"
#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/testing/fakes/privacy_settings.h"
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/stubs/data_provider.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/cobalt/event.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/lib/files/path.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/fsl/vmo/strings.h"
#include "src/lib/fxl/strings/substitute.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics {
namespace crash_reports {
namespace {

using fuchsia::feedback::Annotation;
using fuchsia::feedback::Attachment;
using fuchsia::feedback::CrashReport;
using fuchsia::feedback::CrashReporter_FileReport_Result;
using fuchsia::feedback::FilingError;
using fuchsia::feedback::FilingSuccess;
using fuchsia::feedback::NativeCrashReport;
using fuchsia::feedback::RuntimeCrashReport;
using fuchsia::feedback::SpecificCrashReport;
using fuchsia::feedback::TextBacktraceCrashReport;
using fuchsia::settings::PrivacySettings;
using inspect::testing::ChildrenMatch;
using inspect::testing::NameMatches;
using inspect::testing::NodeMatches;
using inspect::testing::PropertyList;
using inspect::testing::StringIs;
using inspect::testing::UintIs;
using testing::ByRef;
using testing::Contains;
using testing::ElementsAre;
using testing::HasSubstr;
using testing::IsEmpty;
using testing::IsSupersetOf;
using testing::Not;
using testing::UnorderedElementsAre;
using testing::UnorderedElementsAreArray;

constexpr CrashServer::UploadStatus kUploadSuccessful = CrashServer::UploadStatus::kSuccess;
constexpr CrashServer::UploadStatus kUploadFailed = CrashServer::UploadStatus::kFailure;
constexpr CrashServer::UploadStatus kUploadThrottled = CrashServer::UploadStatus::kThrottled;
constexpr CrashServer::UploadStatus kUploadTimeout = CrashServer::UploadStatus::kTimedOut;

constexpr char kProgramName[] = "crashing_program";

constexpr char kBuildVersion[] = "some-version";
constexpr char kBuildPlatformVersion[] = "some-platform-version";
constexpr char kBuildProductVersion[] = "some-product-version";
const std::string kProductVersion =
    fxl::Substitute("$0--$1", kBuildProductVersion, kBuildPlatformVersion);
constexpr char kBuildBoard[] = "some-board";
constexpr char kBuildProduct[] = "some-product";
constexpr char kBuildLatestCommitDate[] = "some-date";
constexpr char kDefaultChannel[] = "some-channel";
constexpr char kDefaultDeviceId[] = "some-device-id";

constexpr char kSingleAttachmentKey[] = "attachment.key";
constexpr char kSingleAttachmentValue[] = "attachment.value";

constexpr bool kUserOptInDataSharing = true;
constexpr bool kUserOptOutDataSharing = false;

constexpr size_t kDailyPerProductQuota = 100u;
constexpr zx::duration kResetOffset = zx::min(20);

const std::map<std::string, std::string> kFeedbackAnnotations = {
    {feedback::kBuildVersionKey, kBuildVersion},
    {feedback::kBuildBoardKey, kBuildBoard},
    {feedback::kBuildPlatformVersionKey, kBuildPlatformVersion},
    {feedback::kBuildProductVersionKey, kBuildProductVersion},
    {feedback::kBuildProductKey, kBuildProduct},
    {feedback::kBuildLatestCommitDateKey, kBuildLatestCommitDate},
    {feedback::kDeviceFeedbackIdKey, kDefaultDeviceId},
    {feedback::kSystemUpdateChannelCurrentKey, kDefaultChannel},
};

constexpr char kDefaultAttachmentBundleKey[] = "feedback.attachment.bundle.key";
constexpr char kEmptyAttachmentBundleKey[] = "empty.attachment.key";

constexpr zx::duration kSnapshotSharedRequestWindow = zx::sec(5);

Attachment BuildAttachment(const std::string& key, const std::string& value) {
  Attachment attachment;
  attachment.key = key;
  FX_CHECK(fsl::VmoFromString(value, &attachment.value));
  return attachment;
}

PrivacySettings MakePrivacySettings(const std::optional<bool> user_data_sharing_consent) {
  PrivacySettings settings;
  if (user_data_sharing_consent.has_value()) {
    settings.set_user_data_sharing_consent(user_data_sharing_consent.value());
  }
  return settings;
}

template <typename K, typename V>
std::vector<testing::internal::PairMatcher<K, V>> Linearize(const std::map<K, V>& annotations) {
  std::vector<testing::internal::PairMatcher<K, V>> linearized;
  for (const auto& [k, v] : annotations) {
    linearized.push_back(testing::Pair(k, v));
  }
  return linearized;
}

// Unit-tests the implementation of the fuchsia.feedback.CrashReporter FIDL interface.
//
// This does not test the environment service. It directly instantiates the class, without
// connecting through FIDL.
class CrashReporterTest : public UnitTestFixture {
 public:
  void SetUp() override {
    clock_.SetMonotonic(zx::time_monotonic(0u));
    info_context_ =
        std::make_shared<InfoContext>(&InspectRoot(), &clock_, dispatcher(), services());
    crash_register_ = std::make_unique<CrashRegister>(info_context_, RegisterJsonPath());

    SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));
    RunLoopUntilIdle();
  }

  void TearDown() override {
    ASSERT_TRUE(files::DeletePath(feedback::kProductQuotasPath, /*recursive=*/true));
  }

 protected:
  // Sets up the underlying crash reporter using the given |config| and |crash_server|.
  void SetUpCrashReporter(const feedback::CrashReportUploadPolicy crash_report_upload_policy,
                          const std::optional<uint64_t> daily_per_product_quota,
                          const bool enable_hourly_snapshots,
                          const std::vector<CrashServer::UploadStatus>& upload_attempt_results = {},
                          StorageSize max_reports_cache_size = StorageSize::Megabytes(1)) {
    FX_CHECK(data_provider_server_);
    annotation_manager_ = std::make_unique<feedback::AnnotationManager>(
        dispatcher(),
        std::set<std::string>{
            feedback::kBuildVersionKey,
            feedback::kBuildBoardKey,
            feedback::kBuildPlatformVersionKey,
            feedback::kBuildProductVersionKey,
            feedback::kBuildProductKey,
            feedback::kBuildLatestCommitDateKey,
            feedback::kDeviceFeedbackIdKey,
            feedback::kSystemUpdateChannelCurrentKey,
        },
        feedback::Annotations{
            {feedback::kBuildVersionKey, ErrorOrString(kBuildVersion)},
            {feedback::kBuildBoardKey, ErrorOrString(kBuildBoard)},
            {feedback::kBuildPlatformVersionKey, ErrorOrString(kBuildPlatformVersion)},
            {feedback::kBuildProductVersionKey, ErrorOrString(kBuildProductVersion)},
            {feedback::kBuildProductKey, ErrorOrString(kBuildProduct)},
            {feedback::kBuildLatestCommitDateKey, ErrorOrString(kBuildLatestCommitDate)},
            {feedback::kDeviceFeedbackIdKey, ErrorOrString(kDefaultDeviceId)},
            {feedback::kSystemUpdateChannelCurrentKey, ErrorOrString(kDefaultChannel)},
        });
    report_store_ = std::make_unique<ScopedTestReportStore>(
        annotation_manager_.get(), info_context_,
        /*max_reports_tmp_size=*/StorageSize::Megabytes(1), max_reports_cache_size);

    crash_server_ = std::make_unique<StubCrashServer>(
        dispatcher(), services(), annotation_manager_.get(), upload_attempt_results, &clock_);

    const feedback::BuildTypeConfig build_type_config{
        .crash_report_upload_policy = crash_report_upload_policy,
        .daily_per_product_crash_report_quota = daily_per_product_quota,
        .enable_hourly_snapshots = enable_hourly_snapshots,
    };

    zx_clock_create_args_v1_t clock_args{.backstop_time = 0};
    FX_CHECK(zx::clock::create(0u, &clock_args, &clock_handle_) == ZX_OK);

    if (const zx_status_t status =
            clock_handle_.signal(/*clear_mask=*/0,
                                 /*set_mask=*/fuchsia::time::SIGNAL_UTC_CLOCK_LOGGING_QUALITY);
        status != ZX_OK) {
      FX_PLOGS(FATAL, status) << "Failed to sync clock";
    }

    crash_reporter_ = std::make_unique<CrashReporter>(
        dispatcher(), services(), &clock_, zx::unowned_clock(clock_handle_.get_handle()),
        info_context_, build_type_config, crash_register_.get(), &tags_, crash_server_.get(),
        &report_store_->GetReportStore(), data_provider_server_.get(), kSnapshotSharedRequestWindow,
        kResetOffset);
    FX_CHECK(crash_reporter_);
  }

  // Sets up the underlying crash reporter using a default config.
  void SetUpCrashReporterDefaultConfig(
      const std::vector<CrashServer::UploadStatus>& upload_attempt_results = {}) {
    SetUpCrashReporter(
        /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
        /*daily_per_product_quota=*/kDailyPerProductQuota,
        /*enable_hourly_snapshots=*/true, upload_attempt_results);
  }

  void SetUpDataProviderServer(std::unique_ptr<stubs::DataProviderBase> server) {
    data_provider_server_ = std::move(server);
  }

  void SetUpPrivacySettingsServer(std::unique_ptr<fakes::PrivacySettings> server) {
    privacy_settings_server_ = std::move(server);
    if (privacy_settings_server_) {
      InjectServiceProvider(privacy_settings_server_.get());
    }
  }

  std::string RegisterJsonPath() { return files::JoinPath(tmp_dir_.path(), "register.json"); }

  // Checks that on the crash server the annotations received match the concatenation of:
  //   * |expected_extra_annotations|
  //   * default annotations
  //
  // In case of duplicate keys, the value from |expected_extra_annotations| is picked.
  void CheckAnnotationsOnServer(
      const std::map<std::string, std::string>& expected_extra_annotations = {}) {
    ASSERT_TRUE(crash_server_);

    std::map<std::string, testing::Matcher<std::string>> expected_annotations = {
        {"product", "Fuchsia"},
        {"version", kProductVersion},
        {"program", testing::StartsWith("crashing_program")},
        {"ptype", testing::StartsWith("crashing_program")},
        {feedback::kOSNameKey, "Fuchsia"},
        {feedback::kOSVersionKey, kBuildPlatformVersion},
        {feedback::kOSChannelKey, kDefaultChannel},
        {feedback::kBuildVersionKey, kBuildVersion},
        {feedback::kBuildBoardKey, kBuildBoard},
        {feedback::kBuildPlatformVersionKey, kBuildPlatformVersion},
        {feedback::kBuildProductVersionKey, kBuildProductVersion},
        {feedback::kBuildProductKey, kBuildProduct},
        {feedback::kBuildLatestCommitDateKey, kBuildLatestCommitDate},
        {feedback::kDeviceFeedbackIdKey, kDefaultDeviceId},
        {feedback::kSystemUpdateChannelCurrentKey, kDefaultChannel},
        {"reportTimeMillis", Not(IsEmpty())},
        {"guid", kDefaultDeviceId},
        {"channel", kDefaultChannel},
        {"debug.snapshot.shared-request.num-clients", Not(IsEmpty())},
        {feedback::kSnapshotUuid, Not(IsEmpty())},
        {feedback::kDebugReportUuid, Not(IsEmpty())},
    };
    for (const auto& [key, value] : expected_extra_annotations) {
      expected_annotations[key] = value;
    }

    EXPECT_THAT(crash_server_->latest_annotations().Raw(),
                testing::UnorderedElementsAreArray(Linearize(expected_annotations)));
  }

  // Checks that on the crash server the keys for the attachments received match the
  // concatenation of:
  //   * |expected_extra_attachment_keys|
  //   * data_provider_->attachment_bundle_key()
  void CheckAttachmentsOnServer(
      const std::vector<std::string>& expected_extra_attachment_keys = {}) {
    ASSERT_TRUE(crash_server_);

    std::vector<std::string> expected_attachment_keys = expected_extra_attachment_keys;

    EXPECT_THAT(crash_server_->latest_attachment_keys(),
                testing::UnorderedElementsAreArray(expected_attachment_keys));
  }

  // Checks that the crash server is still expecting at least one more request.
  //
  // This is useful to check that an upload request hasn't been made as we are using a strict stub.
  void CheckServerStillExpectRequests() {
    ASSERT_TRUE(crash_server_);
    EXPECT_TRUE(crash_server_->ExpectRequest());
  }

  // Files one crash report.
  CrashReporter_FileReport_Result FileOneCrashReport(CrashReport report) {
    // Run loop to start the clock.
    RunLoopUntilIdle();
    FX_CHECK(crash_reporter_ != nullptr)
        << "crash_reporter_ is nullptr. Call SetUpCrashReporter() or one of its variants "
           "at the beginning of a test case.";
    std::optional<CrashReporter_FileReport_Result> out_result{std::nullopt};
    crash_reporter_->FileReport(
        std::move(report),
        [&out_result](CrashReporter_FileReport_Result result) { out_result = std::move(result); });
    RunLoopFor(kSnapshotSharedRequestWindow);
    FX_CHECK(out_result.has_value());
    return std::move(*out_result);
  }

  // Files one crash report.
  CrashReporter_FileReport_Result FileOneCrashReport(
      const std::vector<Annotation>& annotations = {}, std::vector<Attachment> attachments = {}) {
    CrashReport report;
    report.set_program_name(kProgramName);
    if (!annotations.empty()) {
      report.set_annotations(annotations);
    }
    if (!attachments.empty()) {
      report.set_attachments(std::move(attachments));
    }
    return FileOneCrashReport(std::move(report));
  }

  // Files one crash report.
  //
  // |attachment| is useful to control the lower bound of the size of the report by controlling the
  // size of some of the attachment(s).
  CrashReporter_FileReport_Result FileOneCrashReportWithSingleAttachment(
      const std::string& attachment = kSingleAttachmentValue) {
    std::vector<Attachment> attachments;
    attachments.emplace_back(BuildAttachment(kSingleAttachmentKey, attachment));
    return FileOneCrashReport(/*annotations=*/{},
                              /*attachments=*/std::move(attachments));
  }

  // Files one native crash report.
  CrashReporter_FileReport_Result FileOneNativeCrashReport(
      std::optional<fuchsia::mem::Buffer> minidump,
      const std::optional<std::string>& crash_signature) {
    NativeCrashReport native_report;
    if (minidump.has_value()) {
      native_report.set_minidump(std::move(minidump.value()));
    }
    native_report.set_process_name("crashing_process");
    native_report.set_process_koid(123u);
    native_report.set_thread_name("crashing_thread");
    native_report.set_thread_koid(1234u);

    SpecificCrashReport specific_report;
    specific_report.set_native(std::move(native_report));

    CrashReport report;
    report.set_program_name("crashing_program_native");
    report.set_specific_report(std::move(specific_report));

    if (crash_signature.has_value()) {
      report.set_crash_signature(crash_signature.value());
    }

    return FileOneCrashReport(std::move(report));
  }

  // Files one Dart crash report.
  CrashReporter_FileReport_Result FileOneDartCrashReport(
      const std::optional<std::string>& exception_type,
      const std::optional<std::string>& exception_message,
      std::optional<fuchsia::mem::Buffer> exception_stack_trace) {
    RuntimeCrashReport dart_report;
    if (exception_type.has_value()) {
      dart_report.set_exception_type(exception_type.value());
    }
    if (exception_message.has_value()) {
      dart_report.set_exception_message(exception_message.value());
    }
    if (exception_stack_trace.has_value()) {
      dart_report.set_exception_stack_trace(std::move(exception_stack_trace.value()));
    }

    SpecificCrashReport specific_report;
    specific_report.set_dart(std::move(dart_report));

    CrashReport report;
    report.set_program_name("crashing_program_dart");
    report.set_specific_report(std::move(specific_report));

    return FileOneCrashReport(std::move(report));
  }

  // Files one Fuchsia text backtrace crash report.
  CrashReporter_FileReport_Result FileOneFuchsiaTextBacktraceCrashReport(
      std::optional<fuchsia::mem::Buffer> backtrace,
      const std::optional<std::string>& crash_signature) {
    TextBacktraceCrashReport text_backtrace_report;
    if (backtrace.has_value()) {
      text_backtrace_report.set_fuchsia_backtrace(std::move(backtrace.value()));
    }
    text_backtrace_report.set_process_name("crashing_process");
    text_backtrace_report.set_process_koid(123u);
    text_backtrace_report.set_thread_name("crashing_thread");
    text_backtrace_report.set_thread_koid(1234u);

    SpecificCrashReport specific_report;
    specific_report.set_text_backtrace(std::move(text_backtrace_report));

    CrashReport report;
    report.set_program_name("crashing_program_text_backtrace");
    report.set_specific_report(std::move(specific_report));

    if (crash_signature.has_value()) {
      report.set_crash_signature(crash_signature.value());
    }

    return FileOneCrashReport(std::move(report));
  }

  // Files one empty crash report.
  CrashReporter_FileReport_Result FileOneEmptyCrashReport() {
    CrashReport report;
    return FileOneCrashReport(std::move(report));
  }

  // Files one crash report with the provided crash signature.
  CrashReporter_FileReport_Result FileOneCrashReportWithSignature(const std::string& signature) {
    CrashReport report;
    report.set_program_name("crashing_program_generic");
    report.set_crash_signature(signature);

    return FileOneCrashReport(std::move(report));
  }

  // Files one crash report with the provided is fatal value.
  CrashReporter_FileReport_Result FileOneCrashReportWithIsFatal(const bool is_fatal) {
    CrashReport report;
    report.set_program_name("crashing_program_generic");
    report.set_is_fatal(is_fatal);

    return FileOneCrashReport(std::move(report));
  }

  void SetPrivacySettings(std::optional<bool> user_data_sharing_consent) {
    FX_CHECK(privacy_settings_server_);

    ::fpromise::result<void, fuchsia::settings::Error> set_result;
    privacy_settings_server_->Set(
        MakePrivacySettings(user_data_sharing_consent),
        [&set_result](::fpromise::result<void, fuchsia::settings::Error> result) {
          set_result = std::move(result);
        });
    EXPECT_TRUE(set_result.is_ok());
  }

 private:
  files::ScopedTempDir tmp_dir_;

  LogTags tags_;

  // Stubs and fake servers.
  std::unique_ptr<stubs::DataProviderBase> data_provider_server_;
  std::unique_ptr<fakes::PrivacySettings> privacy_settings_server_;

 protected:
  std::unique_ptr<StubCrashServer> crash_server_;
  timekeeper::TestClock clock_;

 private:
  std::shared_ptr<InfoContext> info_context_;
  std::unique_ptr<ScopedTestReportStore> report_store_;
  zx::clock clock_handle_;

 protected:
  std::unique_ptr<feedback::AnnotationManager> annotation_manager_;
  std::unique_ptr<CrashRegister> crash_register_;
  std::unique_ptr<CrashReporter> crash_reporter_;
};

TEST_F(CrashReporterTest, Succeed_OnInputCrashReport) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_response());
  EXPECT_EQ(result.response().results.result(), FilingSuccess::REPORT_UPLOADED);

  CheckAnnotationsOnServer();
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, EnforcesQuota) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/false,
      std::vector<CrashServer::UploadStatus>(kDailyPerProductQuota, kUploadSuccessful));

  for (size_t i = 0; i < kDailyPerProductQuota; ++i) {
    ASSERT_TRUE(FileOneCrashReport().is_response());
  }

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::QUOTA_REACHED_ERROR);
}

TEST_F(CrashReporterTest, ResetsQuota) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/false,
      std::vector<CrashServer::UploadStatus>(kDailyPerProductQuota * 2, kUploadSuccessful));

  for (size_t i = 0; i < kDailyPerProductQuota; ++i) {
    ASSERT_TRUE(FileOneCrashReport().is_response());
  }

  timekeeper::time_utc utc_time;
  ASSERT_EQ(clock_.UtcNow(&utc_time), ZX_OK);
  clock_.SetUtc(utc_time + zx::hour(24) + kResetOffset);

  for (size_t i = 0; i < kDailyPerProductQuota; ++i) {
    ASSERT_TRUE(FileOneCrashReport().is_response());
  }
}

TEST_F(CrashReporterTest, NoQuota) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/std::nullopt,
      /*enable_hourly_snapshots=*/true,
      std::vector<CrashServer::UploadStatus>(kDailyPerProductQuota + 1 /*first hourly snapshot*/,
                                             kUploadSuccessful));

  for (size_t i = 0; i < kDailyPerProductQuota; ++i) {
    ASSERT_TRUE(FileOneCrashReport().is_response());
  }
}

TEST_F(CrashReporterTest, Check_RegisteredProduct) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  fuchsia::feedback::CrashReportingProduct product;
  product.set_name("some name");
  product.set_version("some version");
  product.set_channel("some channel");
  crash_register_->Upsert(kProgramName, std::move(product));

  ASSERT_TRUE(FileOneCrashReport().is_response());

  ASSERT_TRUE(crash_server_->latest_annotations().Contains("product"));
  EXPECT_EQ(crash_server_->latest_annotations().Get("product"), "some name");
  ASSERT_TRUE(crash_server_->latest_annotations().Contains("version"));
  EXPECT_EQ(crash_server_->latest_annotations().Get("version"), "some version");
  ASSERT_TRUE(crash_server_->latest_annotations().Contains("channel"));
  EXPECT_EQ(crash_server_->latest_annotations().Get("channel"), "some channel");
}

TEST_F(CrashReporterTest, Succeed_OnInputCrashReportWithAdditionalData) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment(kSingleAttachmentKey, kSingleAttachmentValue));

  ASSERT_TRUE(FileOneCrashReport(
                  /*annotations=*/
                  {
                      {"annotation.key", "annotation.value"},
                  },
                  /*attachments=*/std::move(attachments))
                  .is_response());
  CheckAnnotationsOnServer({
      {"annotation.key", "annotation.value"},
  });
  CheckAttachmentsOnServer({kSingleAttachmentKey, kEmptyAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnInputCrashReportWithEventId) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  CrashReport report;
  report.set_program_name(kProgramName);
  report.set_event_id("some-event-id");

  ASSERT_TRUE(FileOneCrashReport(std::move(report)).is_response());
  CheckAnnotationsOnServer({
      {"comments", "some-event-id"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnInputCrashReportWithProgramUptime) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  CrashReport report;
  report.set_program_name(kProgramName);
  const zx::duration uptime =
      zx::hour(3) * 24 + zx::hour(15) + zx::min(33) + zx::sec(17) + zx::msec(54);
  report.set_program_uptime(uptime.get());

  ASSERT_TRUE(FileOneCrashReport(std::move(report)).is_response());
  CheckAnnotationsOnServer({
      {"ptime", std::to_string(uptime.to_msecs())},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnNativeInputCrashReport) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  fuchsia::mem::Buffer minidump;
  fsl::VmoFromString("minidump", &minidump);

  ASSERT_TRUE(FileOneNativeCrashReport(std::move(minidump), std::nullopt).is_response());
  CheckAnnotationsOnServer({
      {"crash.process.name", "crashing_process"},
      {"crash.process.koid", "123"},
      {"crash.thread.name", "crashing_thread"},
      {"crash.thread.koid", "1234"},
  });
  CheckAttachmentsOnServer({"uploadFileMinidump", kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnNativeInputCrashReportWithoutMinidump) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  ASSERT_TRUE(FileOneNativeCrashReport(std::nullopt, std::nullopt).is_response());
  CheckAnnotationsOnServer({
      {"crash.process.name", "crashing_process"},
      {"crash.process.koid", "123"},
      {"crash.thread.name", "crashing_thread"},
      {"crash.thread.koid", "1234"},
      {"signature", "fuchsia-no-minidump"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnNativeInputCrashReportWithoutMinidumpButCrashSignature) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  ASSERT_TRUE(FileOneNativeCrashReport(std::nullopt, "some-signature").is_response());
  CheckAnnotationsOnServer({
      {"crash.process.name", "crashing_process"},
      {"crash.process.koid", "123"},
      {"crash.thread.name", "crashing_thread"},
      {"crash.thread.koid", "1234"},
      {"signature", "some-signature"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnDartInputCrashReport) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  fuchsia::mem::Buffer stack_trace;
  fsl::VmoFromString("#0", &stack_trace);

  ASSERT_TRUE(
      FileOneDartCrashReport("FileSystemException", "cannot open file", std::move(stack_trace))
          .is_response());
  CheckAnnotationsOnServer({
      {"error_runtime_type", "FileSystemException"},
      {"error_message", "cannot open file"},
      {"type", "DartError"},
  });
  CheckAttachmentsOnServer({"DartError", kEmptyAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnDartInputCrashReportWithoutExceptionData) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  ASSERT_TRUE(FileOneDartCrashReport(std::nullopt, std::nullopt, std::nullopt).is_response());
  CheckAnnotationsOnServer({
      {"type", "DartError"},
      {"signature", "fuchsia-no-dart-stack-trace"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnFuchsiaTextBacktraceInputCrashReport) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  fuchsia::mem::Buffer backtrace;
  fsl::VmoFromString("#0", &backtrace);

  ASSERT_TRUE(
      FileOneFuchsiaTextBacktraceCrashReport(std::move(backtrace), std::nullopt).is_response());
  CheckAnnotationsOnServer({
      {"crash.process.name", "crashing_process"},
      {"crash.process.koid", "123"},
      {"crash.thread.name", "crashing_thread"},
      {"crash.thread.koid", "1234"},
  });
  CheckAttachmentsOnServer({"backtrace.txt", kEmptyAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnFuchsiaTextBacktraceInputCrashReportWithoutBacktrace) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  ASSERT_TRUE(FileOneFuchsiaTextBacktraceCrashReport(std::nullopt, std::nullopt).is_response());
  CheckAnnotationsOnServer({
      {"crash.process.name", "crashing_process"},
      {"crash.process.koid", "123"},
      {"crash.thread.name", "crashing_thread"},
      {"crash.thread.koid", "1234"},
      {"signature", "fuchsia-no-fuchsia-text-backtrace"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnInputCrashReportWithSignature) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  ASSERT_TRUE(FileOneCrashReportWithSignature("some-signature").is_response());
  CheckAnnotationsOnServer({
      {"signature", "some-signature"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, FailOnInvalidInputNoProgramName) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  const CrashReporter_FileReport_Result result = FileOneEmptyCrashReport();
  EXPECT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputEmptyShortenedProgramName) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("fuchsia-pkg://");
  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));

  EXPECT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputZeroWeight) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  report.set_weight(0);
  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));

  EXPECT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputAttachmentKeyIsEmpty) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment("", "content"));
  report.set_attachments(std::move(attachments));

  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputAttachmentKeyContainsSlash) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment("my/attachment.key", "content"));
  report.set_attachments(std::move(attachments));

  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputAttachmentKeyContainsSpace) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment("my attachment.key", "content"));
  report.set_attachments(std::move(attachments));

  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputAttachmentKeyContainsNonPrintableAscii) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment(std::string("key\0with_null", 13), "content"));
  report.set_attachments(std::move(attachments));

  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputAttachmentKeyIsDot) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment(".", "content"));
  report.set_attachments(std::move(attachments));

  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, FailOnInvalidInputAttachmentKeyIsDotDot) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment("..", "content"));
  report.set_attachments(std::move(attachments));

  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

struct ReservedAttachmentKeyTestParam {
  std::string test_name;
  std::string key;
};

class ReservedAttachmentKeyTest
    : public CrashReporterTest,
      public testing::WithParamInterface<ReservedAttachmentKeyTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownActions, ReservedAttachmentKeyTest,
                         ::testing::ValuesIn(std::vector<ReservedAttachmentKeyTestParam>({
                             {
                                 "AnnotationsJson",
                                 feedback_data::kAttachmentAnnotations,
                             },
                             {
                                 "MinidumpDmp",
                                 feedback_data::kMinidumpFilename,
                             },
                             {
                                 "SnapshotUuidTxt",
                                 feedback_data::kSnapshotUuidFilename,
                             },
                             {
                                 "SnapshotZip",
                                 feedback_data::kSnapshotFilename,
                             },
                             {
                                 "UploadFileMinidump",
                                 feedback_data::kMinidumpUploadKey,
                             },
                         })),
                         [](const testing::TestParamInfo<ReservedAttachmentKeyTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(ReservedAttachmentKeyTest, FailOnInvalidInputAttachmentKeyIsReserved) {
  const ReservedAttachmentKeyTestParam& param = GetParam();

  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  std::vector<Attachment> attachments;
  attachments.emplace_back(BuildAttachment(param.key, "content"));
  report.set_attachments(std::move(attachments));

  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::INVALID_ARGS_ERROR);
}

TEST_F(CrashReporterTest, Succeed_OnInputCrashReportWithIsFatalTrue) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  ASSERT_TRUE(FileOneCrashReportWithIsFatal(true).is_response());
  CheckAnnotationsOnServer({
      {"isFatal", "true"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnInputCrashReportWithIsFatalFalse) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  ASSERT_TRUE(FileOneCrashReportWithIsFatal(false).is_response());
  CheckAnnotationsOnServer({
      {"isFatal", "false"},
  });
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Upload_OnUserAlreadyOptedInDataSharing) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kReadFromPrivacySettings,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true, std::vector({kUploadSuccessful}));
  SetUpPrivacySettingsServer(std::make_unique<fakes::PrivacySettings>());
  SetPrivacySettings(kUserOptInDataSharing);

  ASSERT_TRUE(FileOneCrashReport().is_response());
  CheckAnnotationsOnServer();
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Archive_OnUserAlreadyOptedOutDataSharing) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderTracksNumCalls>(0u));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kReadFromPrivacySettings,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true);
  SetUpPrivacySettingsServer(std::make_unique<fakes::PrivacySettings>());
  SetPrivacySettings(kUserOptOutDataSharing);
  RunLoopUntilIdle();

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_response());
  EXPECT_EQ(result.response().results.result(), FilingSuccess::REPORT_NOT_FILED_USER_OPTED_OUT);
}

TEST_F(CrashReporterTest, Upload_OnceUserOptInDataSharing) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kReadFromPrivacySettings,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true, std::vector({kUploadSuccessful}));
  SetUpPrivacySettingsServer(std::make_unique<fakes::PrivacySettings>());

  ASSERT_TRUE(FileOneCrashReport().is_response());
  CheckServerStillExpectRequests();

  SetPrivacySettings(kUserOptInDataSharing);
  ASSERT_TRUE(RunLoopUntilIdle());

  CheckAnnotationsOnServer();
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Succeed_OnFailedUploadPutIntoDisk) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true, std::vector({kUploadFailed}));

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_response());
  EXPECT_EQ(result.response().results.result(), FilingSuccess::REPORT_ON_DISK);
}

TEST_F(CrashReporterTest, Succeed_OnFailedUploadPutIntoMemory) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true, std::vector({kUploadFailed}),
      /*max_reports_cache_size=*/StorageSize::Bytes(0));

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_response());
  EXPECT_EQ(result.response().results.result(), FilingSuccess::REPORT_IN_MEMORY);
}

TEST_F(CrashReporterTest, Fail_OnThrottledUpload) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true, std::vector({kUploadThrottled}));

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::SERVER_ERROR);
}

TEST_F(CrashReporterTest, Fail_OnUploadTimeout) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true, std::vector({kUploadTimeout}));

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_err());
  EXPECT_EQ(result.err(), FilingError::SERVER_ERROR);
}

TEST_F(CrashReporterTest, Succeed_OnDisabledUpload) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kDisabled,
      /*daily_per_product_quota=*/std::nullopt,
      /*enable_hourly_snapshots=*/true);

  const CrashReporter_FileReport_Result result = FileOneCrashReport();
  ASSERT_TRUE(result.is_response());
  EXPECT_EQ(result.response().results.result(), FilingSuccess::REPORT_ON_DISK);
}

TEST_F(CrashReporterTest, Succeed_OnNoFeedbackAttachments) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProviderReturnsNoAttachment>(kFeedbackAnnotations));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  EXPECT_TRUE(FileOneCrashReportWithSingleAttachment().is_response());
  CheckAnnotationsOnServer({{feedback::kDebugSnapshotPresentKey, "false"}});
  CheckAttachmentsOnServer({kSingleAttachmentKey});
}

TEST_F(CrashReporterTest, Upload_HourlySnapshot) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful, kUploadSuccessful});

  RunLoopFor(zx::min(5));
  EXPECT_THAT(crash_server_->latest_annotations().Raw(),
              IsSupersetOf(Linearize(std::map<std::string, testing::Matcher<std::string>>({
                  {"ptime", Not(IsEmpty())},
                  {"signature", kHourlySnapshotSignature},
              }))));
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});

  RunLoopFor(zx::hour(1));
  EXPECT_THAT(crash_server_->latest_annotations().Raw(),
              IsSupersetOf(Linearize(std::map<std::string, testing::Matcher<std::string>>({
                  {"ptime", Not(IsEmpty())},
                  {"signature", kHourlySnapshotSignature},
              }))));
  CheckAttachmentsOnServer({kDefaultAttachmentBundleKey});
}

TEST_F(CrashReporterTest, Skip_HourlySnapshotIfNegativeConsent) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kDefaultAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kReadFromPrivacySettings,
      /*daily_per_product_quota=*/kDailyPerProductQuota,
      /*enable_hourly_snapshots=*/true, std::vector<CrashServer::UploadStatus>({}));
  SetUpPrivacySettingsServer(std::make_unique<fakes::PrivacySettings>());
  SetPrivacySettings(kUserOptOutDataSharing);

  RunLoopFor(zx::min(5));
}

TEST_F(CrashReporterTest, Check_CobaltAfterSuccessfulUpload) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporterDefaultConfig({kUploadSuccessful});

  EXPECT_TRUE(FileOneCrashReport().is_response());

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::CrashState::kFiled),
                  cobalt::Event(cobalt::CrashState::kUploaded),
                  cobalt::Event(cobalt::UploadAttemptState::kUploadAttempt, 1u),
                  cobalt::Event(cobalt::UploadAttemptState::kUploaded, 1u),
              }));
}

TEST_F(CrashReporterTest, Check_CobaltAfterQuotaReached) {
  SetUpDataProviderServer(
      std::make_unique<stubs::DataProvider>(kFeedbackAnnotations, kEmptyAttachmentBundleKey));
  SetUpCrashReporter(
      /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kEnabled,
      /*daily_per_product_quota=*/0u,
      /*enable_hourly_snapshots=*/true);

  EXPECT_TRUE(FileOneCrashReport().is_err());
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::CrashState::kOnDeviceQuotaReached),
                                      }));
}

TEST_F(CrashReporterTest, CheckCobaltAfterInvalidInputNoProgramName) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  EXPECT_TRUE(FileOneEmptyCrashReport().is_err());
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::CrashState::kDropped),
                                      }));
}

TEST_F(CrashReporterTest, CheckCobaltAfterInvalidInputZeroWeight) {
  SetUpDataProviderServer(std::make_unique<stubs::DataProviderReturnsEmptySnapshot>());
  SetUpCrashReporterDefaultConfig();

  CrashReport report;
  report.set_program_name("crashing_program_generic");
  report.set_weight(0);
  const CrashReporter_FileReport_Result result = FileOneCrashReport(std::move(report));

  EXPECT_TRUE(result.is_err());
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::CrashState::kDropped),
                                      }));
}

TEST_F(CrashReporterTest, Crash_UploadDisabledWithQuota) {
  ASSERT_DEATH(
      {
        SetUpDataProviderServer(std::make_unique<stubs::DataProvider>(kFeedbackAnnotations,
                                                                      kDefaultAttachmentBundleKey));
        SetUpCrashReporter(
            /*crash_report_upload_policy=*/feedback::CrashReportUploadPolicy::kDisabled,
            /*daily_per_product_quota=*/kDailyPerProductQuota,
            /*enable_hourly_snapshots=*/true);
      },
      HasSubstr("quota is 100"));
}

}  // namespace
}  // namespace crash_reports
}  // namespace forensics
