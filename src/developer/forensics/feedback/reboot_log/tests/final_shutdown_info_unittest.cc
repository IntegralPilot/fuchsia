// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"

#include <fuchsia/feedback/cpp/fidl.h>

#include <limits>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/feedback/reboot_log/hw_shutdown_reason.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/utils/cobalt/metrics.h"

namespace forensics::feedback {
namespace {

TEST(FinalShutdownInfoTest, NotParseable) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kNotParseable);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(), cobalt::LastRebootReason::kUnknown);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-reboot-log-not-parseable");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "reboot-log");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(), std::nullopt);
}

TEST(FinalShutdownInfoTest, Cold) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info = std::make_unique<FinalShutdownInfo>(
      FinalShutdownReason::kCold, GracefulShutdownAction::kPoweroff);

  EXPECT_FALSE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(), cobalt::LastRebootReason::kCold);
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(), fuchsia::feedback::RebootReason::COLD);
  EXPECT_EQ(final_shutdown_info->ToGracefulShutdownAction(), GracefulShutdownAction::kPoweroff);
}

TEST(FinalShutdownInfoTest, Spontaneous) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kSpontaneousReboot);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kBriefPowerLoss);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-spontaneous-reboot");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "device");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::BRIEF_POWER_LOSS);
}

TEST(FinalShutdownInfoTest, BriefPowerLoss) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kSpontaneousReboot);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kBriefPowerLoss);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kBriefPowerLoss),
            "fuchsia-brief-power-loss");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "device");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::BRIEF_POWER_LOSS);
}

TEST(FinalShutdownInfoTest, HardReset) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kSpontaneousReboot);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kBriefPowerLoss);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kHardReset),
            "fuchsia-hard-reset");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "device");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::BRIEF_POWER_LOSS);
}

TEST(FinalShutdownInfoTest, KernelPanic) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kKernelPanic);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kKernelPanic);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-kernel-panic");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "kernel");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::KERNEL_PANIC);
}

TEST(FinalShutdownInfoTest, OOM) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kOom);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kSystemOutOfMemory);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-oom");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "system");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::SYSTEM_OUT_OF_MEMORY);
}

TEST(FinalShutdownInfoTest, HardwareWatchdogTimeout) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kHwWatchdog);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kHardwareWatchdogTimeout);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-hw-watchdog-timeout");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "device");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::HARDWARE_WATCHDOG_TIMEOUT);
}

TEST(FinalShutdownInfoTest, UserHardReset) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kUserHardReset);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-hard-reset-user-requested");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "device");
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kUserHardReset);
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::USER_HARD_RESET);
}

TEST(FinalShutdownInfoTest, SoftwareWatchdogTimeout) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kSwWatchdog);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kSoftwareWatchdogTimeout);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-sw-watchdog-timeout");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "system");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::SOFTWARE_WATCHDOG_TIMEOUT);
}

TEST(FinalShutdownInfoTest, Brownout) {
  std::unique_ptr<FinalShutdownInfo> final_shutdown_info =
      std::make_unique<FinalShutdownInfo>(FinalShutdownReason::kBrownout);

  EXPECT_TRUE(final_shutdown_info->IsCrash());
  EXPECT_EQ(final_shutdown_info->ToCobaltLastRebootReason(), cobalt::LastRebootReason::kBrownout);
  EXPECT_EQ(final_shutdown_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-brownout");
  EXPECT_EQ(final_shutdown_info->ToCrashProgramName(), "device");
  EXPECT_EQ(final_shutdown_info->ToFidlRebootReason(), fuchsia::feedback::RebootReason::BROWNOUT);
}

TEST(FinalShutdownInfoTest, RootJobTerminationNoCriticalProcess) {
  FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kRootJobTermination);

  EXPECT_TRUE(final_shutdown_info.IsCrash());
  EXPECT_EQ(final_shutdown_info.ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kRootJobTermination);
  EXPECT_EQ(final_shutdown_info.ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-root-job-termination");
  EXPECT_EQ(final_shutdown_info.ToCrashProgramName(), "system");
  EXPECT_EQ(final_shutdown_info.ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::ROOT_JOB_TERMINATION);
}

TEST(FinalShutdownInfoTest, RootJobTerminationWithCriticalProcess) {
  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kRootJobTermination,
                                              /*uptime=*/std::nullopt, /*runtime=*/std::nullopt,
                                              "critical_process");

  EXPECT_TRUE(final_shutdown_info.IsCrash());
  EXPECT_EQ(final_shutdown_info.ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kRootJobTermination);
  EXPECT_EQ(final_shutdown_info.ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-reboot-critical_process-terminated");
  EXPECT_EQ(final_shutdown_info.ToCrashProgramName(), "system");
  EXPECT_EQ(final_shutdown_info.ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::ROOT_JOB_TERMINATION);
}

struct GracefulNoReportTestParams {
  std::string test_name;
  std::vector<GracefulShutdownReason> reasons;
  cobalt::LastRebootReason expected_cobalt_reason;
  fuchsia::feedback::RebootReason expected_fidl_reboot_reason;
};

class FinalShutdownInfoGracefulNoReportTest
    : public testing::TestWithParam<GracefulNoReportTestParams> {};

INSTANTIATE_TEST_SUITE_P(WithVariousReasons, FinalShutdownInfoGracefulNoReportTest,
                         ::testing::ValuesIn(std::vector<GracefulNoReportTestParams>({
                             {
                                 "SystemUpdateAndNetstackMigration",
                                 {
                                     GracefulShutdownReason::kSystemUpdate,
                                     GracefulShutdownReason::kNetstackMigration,
                                 },
                                 cobalt::LastRebootReason::kSystemUpdate,
                                 fuchsia::feedback::RebootReason::SYSTEM_UPDATE,
                             },
                             {
                                 "UserRequest",
                                 {GracefulShutdownReason::kUserRequest},
                                 cobalt::LastRebootReason::kUserRequest,
                                 fuchsia::feedback::RebootReason::USER_REQUEST,
                             },
                             {
                                 "SystemUpdate",
                                 {GracefulShutdownReason::kSystemUpdate},
                                 cobalt::LastRebootReason::kSystemUpdate,
                                 fuchsia::feedback::RebootReason::SYSTEM_UPDATE,
                             },
                             {
                                 "ZbiSwap",
                                 {GracefulShutdownReason::kZbiSwap},
                                 cobalt::LastRebootReason::kZbiSwap,
                                 fuchsia::feedback::RebootReason::ZBI_SWAP,
                             },
                             {
                                 "GracefulFdr",
                                 {GracefulShutdownReason::kFdr},
                                 cobalt::LastRebootReason::kFactoryDataReset,
                                 fuchsia::feedback::RebootReason::FACTORY_DATA_RESET,
                             },
                             {
                                 "NetstackMigration",
                                 {GracefulShutdownReason::kNetstackMigration},
                                 cobalt::LastRebootReason::kNetstackMigration,
                                 fuchsia::feedback::RebootReason::NETSTACK_MIGRATION,
                             },
                             {
                                 "AndroidNoReason",
                                 {GracefulShutdownReason::kAndroidNoReason},
                                 cobalt::LastRebootReason::kAndroidNoReason,
                                 fuchsia::feedback::RebootReason::ANDROID_NO_REASON,
                             },
                             {
                                 "BatteryDrained",
                                 {GracefulShutdownReason::kBatteryDrained},
                                 cobalt::LastRebootReason::kBatteryDrained,
                                 fuchsia::feedback::RebootReason::BATTERY_DRAINED,
                             },
                         })),
                         [](const testing::TestParamInfo<GracefulNoReportTestParams>& info) {
                           return info.param.test_name;
                         });

TEST_P(FinalShutdownInfoGracefulNoReportTest, CheckProperties) {
  const GracefulNoReportTestParams& params = GetParam();
  const FinalShutdownInfo final_shutdown_info = FinalShutdownInfo::MakeFinalShutdownInfo(
      HwShutdownReason::kWarm, ZirconShutdownReason::kNoCrash,
      GracefulShutdownInfo{GracefulShutdownAction::kNotParseable, params.reasons},
      /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false,
      /*uptime=*/std::nullopt, /*runtime=*/std::nullopt, /*critical_process=*/std::nullopt);

  EXPECT_FALSE(final_shutdown_info.IsCrash());
  EXPECT_EQ(final_shutdown_info.ToCobaltLastRebootReason(), params.expected_cobalt_reason);
  EXPECT_EQ(final_shutdown_info.ToFidlRebootReason(), params.expected_fidl_reboot_reason);
}

struct GracefulTestParams {
  std::string test_name;
  std::vector<GracefulShutdownReason> reasons;
  cobalt::LastRebootReason expected_cobalt_reason;
  std::optional<fuchsia::feedback::RebootReason> expected_fidl_reboot_reason;
  std::string expected_crash_signature;
  std::string expected_crash_program_name;
};

class FinalShutdownInfoGracefulTest : public testing::TestWithParam<GracefulTestParams> {};

INSTANTIATE_TEST_SUITE_P(WithVariousReasons, FinalShutdownInfoGracefulTest,
                         ::testing::ValuesIn(std::vector<GracefulTestParams>(
                             {{
                                  "GenericGraceful",
                                  /*reasons=*/{},
                                  cobalt::LastRebootReason::kGenericGraceful,
                                  /*expected_fidl_reboot_reason=*/std::nullopt,
                                  "fuchsia-shutdown-undetermined-userspace-reason",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "UnexpectedMultipleGraceful",
                                  {
                                      GracefulShutdownReason::kAndroidCriticalProcessFailure,
                                      GracefulShutdownReason::kSessionFailure,
                                  },
                                  cobalt::LastRebootReason::kUnexpectedReasonGraceful,
                                  /*expected_fidl_reboot_reason=*/std::nullopt,
                                  "fuchsia-shutdown-unexpected-userspace-reason",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "HighTemperature",
                                  {GracefulShutdownReason::kHighTemperature},
                                  cobalt::LastRebootReason::kHighTemperature,
                                  fuchsia::feedback::RebootReason::HIGH_TEMPERATURE,
                                  "fuchsia-shutdown-high-temperature",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "SessionFailure",
                                  {GracefulShutdownReason::kSessionFailure},
                                  cobalt::LastRebootReason::kSessionFailure,
                                  fuchsia::feedback::RebootReason::SESSION_FAILURE,
                                  "fuchsia-session-failure",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "SysmgrFailure",
                                  {GracefulShutdownReason::kSysmgrFailure},
                                  cobalt::LastRebootReason::kSysmgrFailure,
                                  fuchsia::feedback::RebootReason::SYSMGR_FAILURE,
                                  "fuchsia-sysmgr-failure",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "CriticalComponentFailure",
                                  {GracefulShutdownReason::kCriticalComponentFailure},
                                  cobalt::LastRebootReason::kCriticalComponentFailure,
                                  fuchsia::feedback::RebootReason::CRITICAL_COMPONENT_FAILURE,
                                  "fuchsia-critical-component-failure",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "CriticalDriverFailure",
                                  {GracefulShutdownReason::kCriticalDriverFailure},
                                  cobalt::LastRebootReason::kCriticalDriverFailure,
                                  fuchsia::feedback::RebootReason::CRITICAL_DRIVER_FAILURE,
                                  "fuchsia-critical-driver-failure",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "RetrySystemUpdate",
                                  {GracefulShutdownReason::kRetrySystemUpdate},
                                  cobalt::LastRebootReason::kRetrySystemUpdate,
                                  fuchsia::feedback::RebootReason::RETRY_SYSTEM_UPDATE,
                                  "fuchsia-retry-system-update",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "OOM",
                                  {GracefulShutdownReason::kOutOfMemory},
                                  cobalt::LastRebootReason::kSystemOutOfMemory,
                                  fuchsia::feedback::RebootReason::SYSTEM_OUT_OF_MEMORY,
                                  "fuchsia-oom",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "AndroidUnexpectedReason",
                                  {GracefulShutdownReason::kAndroidUnexpectedReason},
                                  cobalt::LastRebootReason::kAndroidUnexpectedReason,
                                  fuchsia::feedback::RebootReason::ANDROID_UNEXPECTED_REASON,
                                  "fuchsia-shutdown-android-unexpected-reason",
                                  /*expected_crash_program_name=*/"android",
                              },
                              {
                                  "AndroidRescueParty",
                                  {GracefulShutdownReason::kAndroidRescueParty},
                                  cobalt::LastRebootReason::kAndroidRescueParty,
                                  fuchsia::feedback::RebootReason::ANDROID_RESCUE_PARTY,
                                  "fuchsia-shutdown-android-rescue-party",
                                  /*expected_crash_program_name=*/"android",
                              },
                              {
                                  "AndroidCriticalProcessFailure",
                                  {GracefulShutdownReason::kAndroidCriticalProcessFailure},
                                  cobalt::LastRebootReason::kAndroidCriticalProcessFailure,
                                  fuchsia::feedback::RebootReason::ANDROID_CRITICAL_PROCESS_FAILURE,
                                  "fuchsia-shutdown-android-critical-process-failure",
                                  /*expected_crash_program_name=*/"android",
                              },
                              {
                                  "UserRequestDeviceStuck",
                                  {GracefulShutdownReason::kUserRequestDeviceStuck},
                                  cobalt::LastRebootReason::kUserRequestDeviceStuck,
                                  fuchsia::feedback::RebootReason::USER_REQUEST_DEVICE_STUCK,
                                  "fuchsia-shutdown-user-request-device-stuck",
                                  /*expected_crash_program_name=*/"system",
                              },
                              {
                                  "SuspensionFailure",
                                  {GracefulShutdownReason::kSuspensionFailure},
                                  cobalt::LastRebootReason::kSuspensionFailure,
                                  fuchsia::feedback::RebootReason::SUSPENSION_FAILURE,
                                  "fuchsia-shutdown-suspension-failure",
                                  /*expected_crash_program_name=*/"system",
                              }})),
                         [](const testing::TestParamInfo<GracefulTestParams>& info) {
                           return info.param.test_name;
                         });

TEST_P(FinalShutdownInfoGracefulTest, CheckProperties) {
  const GracefulTestParams& params = GetParam();
  const FinalShutdownInfo final_shutdown_info = FinalShutdownInfo::MakeFinalShutdownInfo(
      HwShutdownReason::kWarm, ZirconShutdownReason::kNoCrash,
      GracefulShutdownInfo{GracefulShutdownAction::kReboot, params.reasons},
      /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false,
      /*uptime=*/std::nullopt, /*runtime=*/std::nullopt, /*critical_process=*/std::nullopt);

  EXPECT_TRUE(final_shutdown_info.IsCrash());
  EXPECT_EQ(final_shutdown_info.ToCobaltLastRebootReason(), params.expected_cobalt_reason);
  EXPECT_EQ(final_shutdown_info.ToFidlRebootReason(), params.expected_fidl_reboot_reason);

  EXPECT_EQ(final_shutdown_info.ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            params.expected_crash_signature);

  EXPECT_EQ(final_shutdown_info.ToCrashProgramName(), params.expected_crash_program_name);
}

TEST(FinalShutdownInfoGracefulTest, InferredFdr) {
  const FinalShutdownInfo final_shutdown_info = FinalShutdownInfo::MakeFinalShutdownInfo(
      HwShutdownReason::kWarm, ZirconShutdownReason::kNoCrash,
      /*graceful_shutdown_info=*/std::nullopt,
      /*not_a_fdr=*/false, /*supports_user_initiated_poweroffs=*/false,
      /*uptime=*/std::nullopt, /*runtime=*/std::nullopt, /*critical_process=*/std::nullopt);

  EXPECT_FALSE(final_shutdown_info.IsCrash());
  EXPECT_EQ(final_shutdown_info.ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kFactoryDataReset);
  EXPECT_EQ(final_shutdown_info.ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::FACTORY_DATA_RESET);
}

TEST(FinalShutdownInfoTest, InferredFdr_Cold) {
  const FinalShutdownInfo final_shutdown_info = FinalShutdownInfo::MakeFinalShutdownInfo(
      HwShutdownReason::kCold, ZirconShutdownReason::kNotSet,
      /*graceful_shutdown_info=*/std::nullopt,
      /*not_a_fdr=*/false, /*supports_user_initiated_poweroffs=*/false,
      /*uptime=*/std::nullopt, /*runtime=*/std::nullopt, /*critical_process=*/std::nullopt);

  EXPECT_FALSE(final_shutdown_info.IsCrash());
  EXPECT_EQ(final_shutdown_info.ToCobaltLastRebootReason(),
            cobalt::LastRebootReason::kFactoryDataReset);
  EXPECT_EQ(final_shutdown_info.ToFidlRebootReason(),
            fuchsia::feedback::RebootReason::FACTORY_DATA_RESET);
}

TEST(FinalShutdownInfoTest, LastRebootReasonAnnotationSpontaneous) {
  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kSpontaneousReboot);
  EXPECT_EQ(final_shutdown_info.ToSnapshotAnnotationReason(SpontaneousRebootReason::kSpontaneous),
            "spontaneous");
}

TEST(FinalShutdownInfoTest, LastRebootReasonAnnotationBriefPowerLoss) {
  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kSpontaneousReboot);
  EXPECT_EQ(
      final_shutdown_info.ToSnapshotAnnotationReason(SpontaneousRebootReason::kBriefPowerLoss),
      "brief loss of power");
}

TEST(FinalShutdownInfoTest, LastRebootReasonAnnotationHardReset) {
  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kSpontaneousReboot);
  EXPECT_EQ(final_shutdown_info.ToSnapshotAnnotationReason(SpontaneousRebootReason::kHardReset),
            "hard reset");
}

TEST(FinalShutdownInfoTest, JsonSerializationWithCriticalProcess) {
  FinalShutdownInfo info(FinalShutdownReason::kRootJobTermination, zx::msec(1000), zx::msec(500),
                         "test-critical-process");
  const std::string json = info.ToJson();

  const std::optional<FinalShutdownInfo> parsed_info = FinalShutdownInfo::FromJson(json);
  ASSERT_TRUE(parsed_info.has_value());

  EXPECT_EQ(parsed_info->ToRebootReasonString(), info.ToRebootReasonString());
  EXPECT_EQ(parsed_info->Uptime(), info.Uptime());
  EXPECT_EQ(parsed_info->Runtime(), info.Runtime());
  EXPECT_EQ(parsed_info->ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            info.ToCrashSignature(SpontaneousRebootReason::kSpontaneous));
}

TEST(FinalShutdownInfoTest, JsonSerializationWithAction) {
  FinalShutdownInfo info(FinalShutdownReason::kUserRequest, GracefulShutdownAction::kReboot,
                         zx::msec(1000), zx::msec(500));
  const std::string json = info.ToJson();

  const std::optional<FinalShutdownInfo> parsed_info = FinalShutdownInfo::FromJson(json);
  ASSERT_TRUE(parsed_info.has_value());

  EXPECT_EQ(parsed_info->ToRebootReasonString(), info.ToRebootReasonString());
  EXPECT_EQ(parsed_info->Uptime(), info.Uptime());
  EXPECT_EQ(parsed_info->Runtime(), info.Runtime());
  EXPECT_EQ(parsed_info->ToGracefulShutdownAction(), info.ToGracefulShutdownAction());
}

TEST(FinalShutdownInfoTest, FromJsonFailsIfMissingReason) {
  const std::string json = R"({"uptime_ms": 1000})";
  EXPECT_EQ(FinalShutdownInfo::FromJson(json).ToRebootReasonString(), "NOT PARSEABLE");
}

TEST(FinalShutdownInfoTest, FromJsonFailsIfActionAndCriticalProcessPresent) {
  const std::string json = R"({
    "reason": "ROOT JOB TERMINATION",
    "graceful_action": "REBOOT",
    "critical_process": "test-critical-process"
  })";
  EXPECT_EQ(FinalShutdownInfo::FromJson(json).ToRebootReasonString(), "NOT PARSEABLE");
}

TEST(FinalShutdownInfoTest, AllReasonsParseable) {
  // Looping to max() over a switch statement without a default case will ensure that a developer
  // can't forget to test a new enum value. Values outside the range of the enum will safely fall
  // out of the switch statement.
  for (int i = 0; i <= std::numeric_limits<std::uint8_t>::max(); ++i) {
    const FinalShutdownReason reason = static_cast<FinalShutdownReason>(i);

    switch (reason) {
      case FinalShutdownReason::kNotParseable:
      case FinalShutdownReason::kCold:
      case FinalShutdownReason::kBrownout:
      case FinalShutdownReason::kHwWatchdog:
      case FinalShutdownReason::kUserHardReset:
      case FinalShutdownReason::kSpontaneousReboot:
      case FinalShutdownReason::kKernelPanic:
      case FinalShutdownReason::kOom:
      case FinalShutdownReason::kSwWatchdog:
      case FinalShutdownReason::kRootJobTermination:
      case FinalShutdownReason::kGenericGraceful:
      case FinalShutdownReason::kUnexpectedReasonGraceful:
      case FinalShutdownReason::kUserRequest:
      case FinalShutdownReason::kSystemUpdate:
      case FinalShutdownReason::kRetrySystemUpdate:
      case FinalShutdownReason::kHighTemperature:
      case FinalShutdownReason::kSessionFailure:
      case FinalShutdownReason::kSysmgrFailure:
      case FinalShutdownReason::kCriticalComponentFailure:
      case FinalShutdownReason::kFdr:
      case FinalShutdownReason::kZbiSwap:
      case FinalShutdownReason::kNetstackMigration:
      case FinalShutdownReason::kAndroidUnexpectedReason:
      case FinalShutdownReason::kAndroidNoReason:
      case FinalShutdownReason::kAndroidRescueParty:
      case FinalShutdownReason::kAndroidCriticalProcessFailure:
      case FinalShutdownReason::kDeveloperRequest:
      case FinalShutdownReason::kUserRequestDeviceStuck:
      case FinalShutdownReason::kSuspensionFailure:
      case FinalShutdownReason::kBatteryDrained:
      case FinalShutdownReason::kCriticalDriverFailure:
        const FinalShutdownInfo info(reason);
        const std::string json = info.ToJson();
        EXPECT_EQ(info.ToRebootReasonString(),
                  FinalShutdownInfo::FromJson(json).ToRebootReasonString());
        break;
    }
  }
}

}  // namespace
}  // namespace forensics::feedback
