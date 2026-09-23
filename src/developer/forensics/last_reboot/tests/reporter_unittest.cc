// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/last_reboot/reporter.h"

#include <lib/fpromise/result.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"
#include "src/developer/forensics/feedback/reboot_log/reboot_log.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/stubs/crash_reporter.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/cobalt/event.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics {
namespace last_reboot {
namespace {

using ::forensics::feedback::GracefulShutdownAction;
using ::forensics::feedback::GracefulShutdownReason;
using ::forensics::feedback::SpontaneousRebootReason;
using ::forensics::feedback::ToJson;
using testing::IsEmpty;
using testing::UnorderedElementsAreArray;

constexpr char kHasReportedOnPath[] = "/tmp/has_reported_on_reboot_log.txt";
constexpr char kNoGracefulAction[] = "GRACEFUL SHUTDOWN ACTION: (NONE)";
constexpr char kNoGracefulReason[] = "GRACEFUL REBOOT REASONS: (NONE)";

struct UngracefulShutdownTestParam {
  std::string test_name;
  std::string zircon_reboot_log;
  std::string reboot_reason;

  std::string output_crash_signature;
  std::optional<zx::duration> output_uptime;
  std::optional<zx::duration> output_runtime;
  cobalt::LastRebootReason output_last_reboot_reason;
};

struct GracefulShutdownTestParam {
  std::string test_name;
  std::string graceful_shutdown_info;
  cobalt::LastRebootReason output_last_reboot_reason;
};

struct GracefulShutdownWithCrashTestParam {
  std::string test_name;
  std::string graceful_shutdown_info_contents;
  std::vector<GracefulShutdownReason> graceful_shutdown_reasons;
  std::string reboot_reason;

  std::string output_crash_signature;
  zx::duration output_uptime;
  zx::duration output_runtime;
  cobalt::LastRebootReason output_last_reboot_reason;
  std::string output_graceful_action;
};

template <typename TestParam>
class ReporterTest : public UnitTestFixture, public testing::WithParamInterface<TestParam> {
 public:
  ReporterTest()
      : cobalt_(dispatcher(), services(), &clock_),
        redactor_(new IdentityRedactor(inspect::BoolProperty())) {}

  void TearDown() override { files::DeletePath(kHasReportedOnPath, /*recursive=*/false); }

 protected:
  void SetUpCrashReporterServer(std::unique_ptr<stubs::CrashReporterBase> server) {
    crash_reporter_server_ = std::move(server);
  }

  void SetUpRedactor(std::unique_ptr<RedactorBase> redactor) { redactor_ = std::move(redactor); }

  void WriteZirconRebootLogContents(const std::string& contents) {
    FX_CHECK(tmp_dir_.NewTempFileWithData(contents, &zircon_reboot_log_path_));
  }

  void WriteGracefulShutdownInfoContents(const std::string& contents) {
    FX_CHECK(tmp_dir_.NewTempFileWithData(contents, &graceful_shutdown_info_path_));
  }

  void WriteLegacyGracefulRebootLogContents(const std::string& contents) {
    FX_CHECK(tmp_dir_.NewTempFileWithData(contents, &legacy_graceful_reboot_log_path_));
  }

  void SetAsFdr() { not_a_fdr_ = false; }

  void ReportOnRebootLog() {
    const feedback::RebootLog reboot_log = feedback::RebootLog::ParseRebootLog(
        zircon_reboot_log_path_, graceful_shutdown_info_path_, legacy_graceful_reboot_log_path_,
        /*previous_system_time_path=*/"", /*previous_boot_kernel_log_path=*/"",
        /*final_shutdown_info_path=*/"", not_a_fdr_,
        /*supports_user_initiated_poweroffs=*/false, /*first_component_instance=*/true,
        redactor_.get());
    ReportOn(reboot_log);
  }

  void ReportOn(const feedback::RebootLog& reboot_log) {
    Reporter reporter(dispatcher(), &cobalt_, redactor_.get(), crash_reporter_server_.get());
    reporter.ReportOn(reboot_log, /*delay=*/zx::sec(0), SpontaneousRebootReason::kSpontaneous);
    RunLoopUntilIdle();
  }

  std::string zircon_reboot_log_path_;
  std::string graceful_shutdown_info_path_;
  std::string legacy_graceful_reboot_log_path_;
  std::unique_ptr<stubs::CrashReporterBase> crash_reporter_server_;

 private:
  timekeeper::TestClock clock_;
  cobalt::Logger cobalt_;
  std::unique_ptr<RedactorBase> redactor_;
  files::ScopedTempDir tmp_dir_;
  bool not_a_fdr_{true};
};

using GenericReporterTest = ReporterTest<UngracefulShutdownTestParam /*does not matter*/>;

TEST_F(GenericReporterTest, Succeed_WellFormedRebootLog) {
  const zx::duration uptime = zx::msec(74715002);
  const zx::duration runtime = zx::msec(73415072);
  const feedback::FinalShutdownInfo final_shutdown_info(feedback::FinalShutdownReason::kKernelPanic,
                                                        uptime, runtime,
                                                        /*critical_process=*/std::nullopt);
  const feedback::RebootLog reboot_log(
      final_shutdown_info,
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n74715002\nRUNTIME (ms)\n73415072");

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = "fuchsia-kernel-panic",
          .reboot_log = reboot_log.RebootLogStr(),
          .uptime = final_shutdown_info.Uptime(),
          .runtime = final_shutdown_info.Runtime(),
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::LastRebootReason::kKernelPanic, uptime.to_usecs()),
              }));
  EXPECT_TRUE(files::IsFile(kHasReportedOnPath));
}

TEST_F(GenericReporterTest, Succeed_RootJobTerminationRebootLog) {
  const zx::duration uptime = zx::msec(74715002);
  const zx::duration runtime = zx::msec(73415072);
  const feedback::FinalShutdownInfo final_shutdown_info(
      feedback::FinalShutdownReason::kRootJobTermination, uptime, runtime, "foo");
  const feedback::RebootLog reboot_log(
      final_shutdown_info,
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)\n\nUPTIME (ms)\n74715002\nRUNTIME (ms)\n73415072\n"
      "ROOT JOB TERMINATED BY CRITICAL PROCESS DEATH: foo (1)");

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = "fuchsia-reboot-foo-terminated",
          .reboot_log = reboot_log.RebootLogStr(),
          .uptime = final_shutdown_info.Uptime(),
          .runtime = final_shutdown_info.Runtime(),
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::LastRebootReason::kRootJobTermination, uptime.to_usecs()),
              }));
  EXPECT_TRUE(files::IsFile(kHasReportedOnPath));
}

TEST_F(GenericReporterTest, Succeed_NoUptime) {
  const feedback::FinalShutdownInfo final_shutdown_info(
      feedback::FinalShutdownReason::kKernelPanic);
  const feedback::RebootLog reboot_log(
      final_shutdown_info, "HW REBOOT REASON (WARM BOOT)\n\nZIRCON REBOOT REASON (KERNEL PANIC)\n");

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = "fuchsia-kernel-panic",
          .reboot_log = reboot_log.RebootLogStr(),
          .uptime = std::nullopt,
          .runtime = std::nullopt,
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::LastRebootReason::kKernelPanic, /*duration=*/0u),
              }));
}

TEST_F(GenericReporterTest, Succeed_NoRuntime) {
  const zx::duration uptime = zx::msec(74715002);
  const feedback::FinalShutdownInfo final_shutdown_info(feedback::FinalShutdownReason::kKernelPanic,
                                                        uptime, std::nullopt,
                                                        /*critical_process=*/std::nullopt);
  const feedback::RebootLog reboot_log(
      final_shutdown_info,
      "HW REBOOT REASON (WARM BOOT)\n\nZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n74715002");

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = "fuchsia-kernel-panic",
          .reboot_log = reboot_log.RebootLogStr(),
          .uptime = uptime,
          .runtime = std::nullopt,
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(
      ReceivedCobaltEvents(),
      UnorderedElementsAreArray({
          cobalt::Event(cobalt::LastRebootReason::kKernelPanic, /*duration=*/uptime.to_usecs()),
      }));
}

class SimpleRedactor : public RedactorBase {
 public:
  SimpleRedactor() : RedactorBase(inspect::BoolProperty()) {}

  std::string& Redact(std::string& text) override {
    text = "<REDACTED>";
    return text;
  }

  std::string& RedactJson(std::string& text) override { return Redact(text); }

  std::string UnredactedCanary() const override { return ""; }
  std::string RedactedCanary() const override { return ""; }
};

TEST_F(GenericReporterTest, Succeed_RedactsData) {
  const feedback::FinalShutdownInfo final_shutdown_info(
      feedback::FinalShutdownReason::kKernelPanic);
  const feedback::RebootLog reboot_log(
      final_shutdown_info, "HW REBOOT REASON (WARM BOOT)\n\nZIRCON REBOOT REASON (KERNEL PANIC)\n");

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = "fuchsia-kernel-panic",
          .reboot_log = "<REDACTED>",
          .uptime = std::nullopt,
          .runtime = std::nullopt,
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));
  SetUpRedactor(std::make_unique<SimpleRedactor>());

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::LastRebootReason::kKernelPanic, /*duration=*/0u),
              }));
}

TEST_F(GenericReporterTest, Succeed_NoCrashReportFiledCleanReboot) {
  const zx::duration uptime = zx::msec(74715002);
  const zx::duration runtime = zx::msec(73415072);
  const feedback::FinalShutdownInfo final_shutdown_info(
      feedback::FinalShutdownReason::kGenericGraceful, uptime, runtime,
      /*critical_process=*/std::nullopt);
  const feedback::RebootLog reboot_log(
      final_shutdown_info,
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n74715002\nRUNTIME (ms)\n73415072");

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = "fuchsia-shutdown-undetermined-userspace-reason",
          .reboot_log = reboot_log.RebootLogStr(),
          .uptime = uptime,
          .runtime = runtime,
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::LastRebootReason::kGenericGraceful, uptime.to_usecs()),
              }));
}

TEST_F(GenericReporterTest, Succeed_NoCrashReportFiledColdReboot) {
  const feedback::FinalShutdownInfo final_shutdown_info(feedback::FinalShutdownReason::kCold);
  const feedback::RebootLog reboot_log(final_shutdown_info, "");

  SetUpCrashReporterServer(std::make_unique<stubs::CrashReporterNoFileExpected>());
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::LastRebootReason::kCold, /*duration=*/0u),
              }));
}

TEST_F(GenericReporterTest, Fail_CrashReporterFailsToFile) {
  const zx::duration uptime = zx::msec(74715002);
  const zx::duration runtime = zx::msec(73415072);
  const feedback::FinalShutdownInfo final_shutdown_info(feedback::FinalShutdownReason::kKernelPanic,
                                                        uptime, runtime,
                                                        /*critical_process=*/std::nullopt);
  const feedback::RebootLog reboot_log(
      final_shutdown_info,
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n74715002\nRUNTIME (ms)\n73415072");
  SetUpCrashReporterServer(std::make_unique<stubs::CrashReporterAlwaysReturnsError>());
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::LastRebootReason::kKernelPanic, uptime.to_usecs()),
              }));
}

TEST_F(GenericReporterTest, Succeed_DoesNothingIfAlreadyReportedOn) {
  ASSERT_TRUE(files::WriteFile(kHasReportedOnPath, /*data=*/"", /*size=*/0));

  const feedback::FinalShutdownInfo final_shutdown_info(feedback::FinalShutdownReason::kKernelPanic,
                                                        /*uptime=*/zx::msec(74715002),
                                                        /*runtime=*/zx::msec(73415072),
                                                        /*critical_process=*/std::nullopt);
  const feedback::RebootLog reboot_log(
      final_shutdown_info,
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n74715002\nRUNTIME (ms)\n73415072");

  SetUpCrashReporterServer(std::make_unique<stubs::CrashReporterNoFileExpected>());
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOn(reboot_log);

  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

using UngracefulReporterTest = ReporterTest<UngracefulShutdownTestParam>;

INSTANTIATE_TEST_SUITE_P(
    WithVariousRebootLogs, UngracefulReporterTest,
    ::testing::ValuesIn(std::vector<UngracefulShutdownTestParam>({
        {
            "KernelPanic",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920",
            "FINAL REBOOT REASON (KERNEL PANIC)",
            "fuchsia-kernel-panic",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kKernelPanic,
        },
        {
            "OOM",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (OOM)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920",
            "FINAL REBOOT REASON (OOM)",
            "fuchsia-oom",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kSystemOutOfMemory,
        },
        {
            "Spontaneous",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (UNKNOWN)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920",
            "FINAL REBOOT REASON (SPONTANEOUS)",
            "fuchsia-spontaneous-reboot",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kBriefPowerLoss,
        },
        {
            "SoftwareWatchdogTimeout",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (SW WATCHDOG)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920",
            "FINAL REBOOT REASON (SOFTWARE WATCHDOG TIMEOUT)",
            "fuchsia-sw-watchdog-timeout",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kSoftwareWatchdogTimeout,
        },
        {
            "HardwareWatchdogTimeout",
            "HW REBOOT REASON (HW WATCHDOG)\n\nZIRCON REBOOT REASON (UNKNOWN)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920",
            "FINAL REBOOT REASON (HARDWARE WATCHDOG TIMEOUT)",
            "fuchsia-hw-watchdog-timeout",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kHardwareWatchdogTimeout,
        },
        {
            "BrownoutPower",
            "HW REBOOT REASON (BROWNOUT)\n\nZIRCON REBOOT REASON (UNKNOWN)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920",
            "FINAL REBOOT REASON (BROWNOUT)",
            "fuchsia-brownout",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kBrownout,
        },
        {
            "NotParseable",
            "NOT PARSEABLE",
            "FINAL REBOOT REASON (NOT PARSEABLE)",
            "fuchsia-reboot-log-not-parseable",
            std::nullopt,
            std::nullopt,
            cobalt::LastRebootReason::kUnknown,
        },
    })),
    [](const testing::TestParamInfo<UngracefulShutdownTestParam>& info) {
      return info.param.test_name;
    });

TEST_P(UngracefulReporterTest, Succeed) {
  const UngracefulShutdownTestParam& param = GetParam();

  WriteZirconRebootLogContents(param.zircon_reboot_log);
  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = param.output_crash_signature,
          .reboot_log =
              fxl::StringPrintf("%s\n%s\n%s\n\n%s", param.zircon_reboot_log.c_str(),
                                kNoGracefulAction, kNoGracefulReason, param.reboot_reason.c_str()),
          .uptime = param.output_uptime,
          .runtime = param.output_runtime,
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOnRebootLog();

  const zx::duration expected_uptime =
      (param.output_uptime.has_value()) ? param.output_uptime.value() : zx::usec(0);
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(param.output_last_reboot_reason, expected_uptime.to_usecs()),
              }));
}

using GracefulReporterTest = ReporterTest<GracefulShutdownTestParam>;

INSTANTIATE_TEST_SUITE_P(
    WithVariousRebootLogs, GracefulReporterTest,
    ::testing::ValuesIn(std::vector<GracefulShutdownTestParam>({
        {
            "UserRequest",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kUserRequest}),
            cobalt::LastRebootReason::kUserRequest,
        },
        {
            "SystemUpdate",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kSystemUpdate}),
            cobalt::LastRebootReason::kSystemUpdate,
        },
        {
            "ZbiSwap",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kZbiSwap}),
            cobalt::LastRebootReason::kZbiSwap,
        },
    })),
    [](const testing::TestParamInfo<GracefulShutdownTestParam>& info) {
      return info.param.test_name;
    });

TEST_P(GracefulReporterTest, Succeed) {
  const GracefulShutdownTestParam& param = GetParam();

  WriteZirconRebootLogContents(
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920");
  WriteGracefulShutdownInfoContents(param.graceful_shutdown_info);

  SetUpCrashReporterServer(std::make_unique<stubs::CrashReporterNoFileExpected>());
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOnRebootLog();

  const zx::duration expected_uptime = zx::msec(65487494);
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(param.output_last_reboot_reason, expected_uptime.to_usecs()),
              }));
}

TEST_P(GracefulReporterTest, Succeed_FDR) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n65487494\nRUNTIME (ms)\n64208920");
  SetAsFdr();

  SetUpCrashReporterServer(std::make_unique<stubs::CrashReporterNoFileExpected>());
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOnRebootLog();

  const zx::duration expected_uptime = zx::msec(65487494);
  EXPECT_THAT(
      ReceivedCobaltEvents(),
      UnorderedElementsAreArray({
          cobalt::Event(cobalt::LastRebootReason::kFactoryDataReset, expected_uptime.to_usecs()),
      }));
}

using GracefulWithCrashReporterTest = ReporterTest<GracefulShutdownWithCrashTestParam>;

INSTANTIATE_TEST_SUITE_P(
    WithVariousRebootLogs, GracefulWithCrashReporterTest,
    ::testing::ValuesIn(std::vector<GracefulShutdownWithCrashTestParam>({
        {
            "SessionFailure",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kSessionFailure}),
            {GracefulShutdownReason::kSessionFailure},
            "FINAL REBOOT REASON (SESSION FAILURE)",
            "fuchsia-session-failure",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kSessionFailure,
            "REBOOT",
        },
        {
            "OOM",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kOutOfMemory}),
            {GracefulShutdownReason::kOutOfMemory},
            "FINAL REBOOT REASON (OOM)",
            "fuchsia-oom",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kSystemOutOfMemory,
            "REBOOT",
        },
        {
            "SysmgrFailure",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kSysmgrFailure}),
            {GracefulShutdownReason::kSysmgrFailure},
            "FINAL REBOOT REASON (SYSMGR FAILURE)",
            "fuchsia-sysmgr-failure",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kSysmgrFailure,
            "REBOOT",
        },
        {
            "CriticalComponentFailure",
            ToJson(GracefulShutdownAction::kReboot,
                   {GracefulShutdownReason::kCriticalComponentFailure}),
            {GracefulShutdownReason::kCriticalComponentFailure},
            "FINAL REBOOT REASON (CRITICAL COMPONENT FAILURE)",
            "fuchsia-critical-component-failure",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kCriticalComponentFailure,
            "REBOOT",
        },
        {
            "CriticalDriverFailure",
            ToJson(GracefulShutdownAction::kReboot,
                   {GracefulShutdownReason::kCriticalDriverFailure}),
            {GracefulShutdownReason::kCriticalDriverFailure},
            "FINAL REBOOT REASON (CRITICAL DRIVER FAILURE)",
            "fuchsia-critical-driver-failure",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kCriticalDriverFailure,
            "REBOOT",
        },
        {
            "RetrySystemUpdate",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kRetrySystemUpdate}),
            {GracefulShutdownReason::kRetrySystemUpdate},
            "FINAL REBOOT REASON (RETRY SYSTEM UPDATE)",
            "fuchsia-retry-system-update",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kRetrySystemUpdate,
            "REBOOT",
        },
        {
            "HighTemperature",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kHighTemperature}),
            {GracefulShutdownReason::kHighTemperature},
            "FINAL REBOOT REASON (HIGH TEMPERATURE)",
            "fuchsia-shutdown-high-temperature",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kHighTemperature,
            "REBOOT",
        },
        {
            "NotSupported",
            ToJson(GracefulShutdownAction::kReboot, {GracefulShutdownReason::kNotSupported}),
            {GracefulShutdownReason::kNotSupported},
            "FINAL REBOOT REASON (GENERIC GRACEFUL)",
            "fuchsia-shutdown-undetermined-userspace-reason",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kGenericGraceful,
            "REBOOT",
        },
        {
            "NotParseable",
            R"({"reasons": [ "NOT PARSEABLE REASON" ] })",
            {GracefulShutdownReason::kNotParseable},
            "FINAL REBOOT REASON (GENERIC GRACEFUL)",
            "fuchsia-shutdown-undetermined-userspace-reason",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kGenericGraceful,
            "REBOOT",
        },
        {
            "None",
            ToJson(GracefulShutdownAction::kReboot, {}),
            {},
            "FINAL REBOOT REASON (GENERIC GRACEFUL)",
            "fuchsia-shutdown-undetermined-userspace-reason",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kGenericGraceful,
            "REBOOT",
        },
        {
            "InvalidSchema",
            R"({ "reasons": "not-an-array" })",
            {GracefulShutdownReason::kNotParseable},
            "FINAL REBOOT REASON (GENERIC GRACEFUL)",
            "fuchsia-shutdown-undetermined-userspace-reason",
            zx::msec(65487494),
            zx::msec(64208920),
            cobalt::LastRebootReason::kGenericGraceful,
            "NOT PARSEABLE",
        },
    })),
    [](const testing::TestParamInfo<GracefulShutdownWithCrashTestParam>& info) {
      return info.param.test_name;
    });

TEST_P(GracefulWithCrashReporterTest, Succeed) {
  const GracefulShutdownWithCrashTestParam& param = GetParam();

  const std::string zircon_reboot_log = fxl::StringPrintf(
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n%lu\nRUNTIME (ms)\n%lu",
      param.output_uptime.to_msecs(), param.output_runtime.to_secs());
  WriteZirconRebootLogContents(zircon_reboot_log);

  if (!param.graceful_shutdown_info_contents.empty()) {
    WriteGracefulShutdownInfoContents(param.graceful_shutdown_info_contents);
  }

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = param.output_crash_signature,
          .reboot_log = fxl::StringPrintf(
              "%s\nGRACEFUL SHUTDOWN ACTION: (%s)\nGRACEFUL REBOOT REASONS: (%s)\n\n%s",
              zircon_reboot_log.c_str(), param.output_graceful_action.c_str(),
              feedback::ToRawStrings(param.graceful_shutdown_reasons).c_str(),
              param.reboot_reason.c_str()),
          .uptime = param.output_uptime,
          .runtime = param.output_runtime,
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOnRebootLog();

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(param.output_last_reboot_reason, param.output_uptime.to_usecs()),
              }));
}

TEST_P(GracefulWithCrashReporterTest, SucceedForLegacyFile) {
  const GracefulShutdownWithCrashTestParam& param = GetParam();

  const std::string zircon_reboot_log = fxl::StringPrintf(
      "HW REBOOT REASON (WARM BOOT)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n%lu\nRUNTIME (ms)\n%lu",
      param.output_uptime.to_msecs(), param.output_runtime.to_secs());
  WriteZirconRebootLogContents(zircon_reboot_log);

  if (!param.graceful_shutdown_reasons.empty()) {
    WriteLegacyGracefulRebootLogContents(ToRawStrings(param.graceful_shutdown_reasons));
  }

  SetUpCrashReporterServer(
      std::make_unique<stubs::CrashReporter>(stubs::CrashReporter::Expectations{
          .crash_signature = param.output_crash_signature,
          .reboot_log = fxl::StringPrintf(
              "%s\nGRACEFUL SHUTDOWN ACTION: (%s)\nGRACEFUL REBOOT REASONS: (%s)\n\n%s",
              zircon_reboot_log.c_str(),
              // The legacy file didn't store the action, so it's inferred from the reasons both
              // here and in the production code.
              param.graceful_shutdown_reasons.empty() ? "NONE" : "REBOOT",
              feedback::ToRawStrings(param.graceful_shutdown_reasons).c_str(),
              param.reboot_reason.c_str()),
          .uptime = param.output_uptime,
          .runtime = param.output_runtime,
          .is_fatal = true,
      }));
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  ReportOnRebootLog();

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(param.output_last_reboot_reason, param.output_uptime.to_usecs()),
              }));
}

}  // namespace
}  // namespace last_reboot
}  // namespace forensics
