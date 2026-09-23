// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"

#include <format>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace forensics {
namespace feedback {
namespace {

using ::fuchsia::hardware::power::statecontrol::ShutdownAction;
using ::fuchsia::hardware::power::statecontrol::ShutdownOptions;

constexpr char kFilename[] = "graceful_shutdown_info.json";

std::string BuildJsonWithEmptyReasons(const std::string_view action) {
  // Use {{ and }} to escape the curly braces for std::format.
  return std::format(
      R"({{
    "action": "{}",
    "reasons": []
}})",
      action);
}

TEST(GracefulShutdownInfoTest, VerifyLegacyContentConversion) {
  // ToLegacyFileContentForTesting() & FromLegacyTxtFile() for shutdown reasons from
  // |power::statecontrol::ShutdownReason| should be reversible.

  const std::vector<GracefulShutdownReason> reasons = {
      GracefulShutdownReason::kUserRequest,
      GracefulShutdownReason::kSystemUpdate,
      GracefulShutdownReason::kRetrySystemUpdate,
      GracefulShutdownReason::kHighTemperature,
      GracefulShutdownReason::kSessionFailure,
      GracefulShutdownReason::kSysmgrFailure,
      GracefulShutdownReason::kCriticalComponentFailure,
      GracefulShutdownReason::kFdr,
      GracefulShutdownReason::kZbiSwap,
      GracefulShutdownReason::kNotSupported,
      GracefulShutdownReason::kNetstackMigration,
  };

  for (const GracefulShutdownReason reason : reasons) {
    EXPECT_THAT(FromLegacyTxtFile(ToLegacyFileContentForTesting({reason})),
                testing::ElementsAre(reason));
  }
}

TEST(GracefulShutdownInfoTest, VerifyLegacyContentConversionWithMultipleReasons) {
  // ToLegacyFileContentForTesting() & FromLegacyTxtFile() for shutdown reasons from
  // |power::statecontrol::ShutdownReason| should be reversible when there are multiple reasons.

  const std::vector<GracefulShutdownReason> reasons = {
      GracefulShutdownReason::kUserRequest,
      GracefulShutdownReason::kSystemUpdate,
      GracefulShutdownReason::kRetrySystemUpdate,
      GracefulShutdownReason::kHighTemperature,
      GracefulShutdownReason::kSessionFailure,
      GracefulShutdownReason::kSysmgrFailure,
      GracefulShutdownReason::kCriticalComponentFailure,
      GracefulShutdownReason::kFdr,
      GracefulShutdownReason::kZbiSwap,
      GracefulShutdownReason::kNotSupported,
      GracefulShutdownReason::kNetstackMigration,
  };

  // Verify all reasons at once.
  EXPECT_THAT(FromLegacyTxtFile(ToLegacyFileContentForTesting(reasons)),
              testing::ElementsAreArray(reasons));
}

TEST(GracefulShutdownInfoTest, VerifyLegacyContentConversionWithNoReasons) {
  // ToLegacyFileContentForTesting() & FromLegacyTxtFile() for shutdown reasons from
  // |power::statecontrol::ShutdownReason| should be reversible when there are no reasons.
  EXPECT_TRUE(FromLegacyTxtFile(ToLegacyFileContentForTesting({})).empty());
}

TEST(GracefulShutdownInfoTest, VerifyContentConversion) {
  // ToJson() & FromJson() for shutdown reasons from
  // |power::statecontrol::ShutdownReason| should be reversible.
  const std::vector<GracefulShutdownReason> reasons = {
      GracefulShutdownReason::kUserRequest,
      GracefulShutdownReason::kSystemUpdate,
      GracefulShutdownReason::kRetrySystemUpdate,
      GracefulShutdownReason::kHighTemperature,
      GracefulShutdownReason::kSessionFailure,
      GracefulShutdownReason::kSysmgrFailure,
      GracefulShutdownReason::kCriticalComponentFailure,
      GracefulShutdownReason::kFdr,
      GracefulShutdownReason::kZbiSwap,
      GracefulShutdownReason::kNotSupported,
      GracefulShutdownReason::kNetstackMigration,
      GracefulShutdownReason::kAndroidUnexpectedReason,
      GracefulShutdownReason::kAndroidNoReason,
      GracefulShutdownReason::kAndroidRescueParty,
      GracefulShutdownReason::kAndroidCriticalProcessFailure,
      GracefulShutdownReason::kDeveloperRequest,
      GracefulShutdownReason::kUserRequestDeviceStuck,
      GracefulShutdownReason::kBatteryDrained,
      GracefulShutdownReason::kSuspensionFailure,
      GracefulShutdownReason::kCriticalDriverFailure,
  };

  for (const GracefulShutdownReason reason : reasons) {
    EXPECT_THAT(FromJson(ToJson(GracefulShutdownAction::kReboot, {reason})).reasons,
                testing::ElementsAre(reason));
  }
}

TEST(GracefulShutdownInfoTest, VerifyContentConversionWithMultipleReasons) {
  // ToJson() & FromJson() for shutdown reasons from
  // |power::statecontrol::ShutdownReason| should be reversible when there are multiple reasons.
  const std::vector<GracefulShutdownReason> reasons = {
      GracefulShutdownReason::kUserRequest,
      GracefulShutdownReason::kSystemUpdate,
      GracefulShutdownReason::kRetrySystemUpdate,
      GracefulShutdownReason::kHighTemperature,
      GracefulShutdownReason::kSessionFailure,
      GracefulShutdownReason::kSysmgrFailure,
      GracefulShutdownReason::kCriticalComponentFailure,
      GracefulShutdownReason::kFdr,
      GracefulShutdownReason::kZbiSwap,
      GracefulShutdownReason::kNotSupported,
      GracefulShutdownReason::kNetstackMigration,
      GracefulShutdownReason::kAndroidUnexpectedReason,
      GracefulShutdownReason::kAndroidNoReason,
      GracefulShutdownReason::kAndroidRescueParty,
      GracefulShutdownReason::kAndroidCriticalProcessFailure,
      GracefulShutdownReason::kDeveloperRequest,
      GracefulShutdownReason::kUserRequestDeviceStuck,
      GracefulShutdownReason::kBatteryDrained,
      GracefulShutdownReason::kSuspensionFailure,
      GracefulShutdownReason::kCriticalDriverFailure,
  };

  // Verify all reasons at once.
  EXPECT_THAT(FromJson(ToJson(GracefulShutdownAction::kReboot, reasons)).reasons,
              testing::ElementsAreArray(reasons));
}

struct ReadActionTestParam {
  std::string test_name;
  std::string input_action;
  GracefulShutdownAction expected_action;
};

class ReadActionTest : public UnitTestFixture,
                       public testing::WithParamInterface<ReadActionTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownActions, ReadActionTest,
                         ::testing::ValuesIn(std::vector<ReadActionTestParam>({
                             {
                                 "Poweroff",
                                 "POWEROFF",
                                 GracefulShutdownAction::kPoweroff,
                             },
                             {
                                 "Reboot",
                                 "REBOOT",
                                 GracefulShutdownAction::kReboot,
                             },
                             {
                                 "RebootToRecovery",
                                 "REBOOT_TO_RECOVERY",
                                 GracefulShutdownAction::kRebootToRecovery,
                             },
                             {
                                 "RebootToBootloader",
                                 "REBOOT_TO_BOOTLOADER",
                                 GracefulShutdownAction::kRebootToBootloader,
                             },
                             {
                                 "NotParseable",
                                 "SOMETHING_INVALID",
                                 GracefulShutdownAction::kNotParseable,
                             },
                         })),
                         [](const testing::TestParamInfo<ReadActionTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(ReadActionTest, FromJson) {
  const ReadActionTestParam& param = GetParam();
  EXPECT_THAT(FromJson(BuildJsonWithEmptyReasons(param.input_action)).action,
              param.expected_action);
}

TEST(GracefulShutdownInfoTest, VerifyContentConversionWithNoReasons) {
  // ToJson() & FromJson() for shutdown reasons from
  // |power::statecontrol::ShutdownReason| should be reversible when there are no reasons.
  EXPECT_TRUE(FromJson(ToJson(GracefulShutdownAction::kReboot, {})).reasons.empty());
}

TEST(GracefulShutdownInfoTest, MissingActionImpliesReboot) {
  // Devices updating from an older version of Fuchsia may not have the action included in the json.
  const GracefulShutdownInfo shutdown_info = FromJson(R"({ "reasons": [ "SYSTEM UPDATE" ] })");
  EXPECT_THAT(shutdown_info.reasons,
              testing::ElementsAreArray({GracefulShutdownReason::kSystemUpdate}));
  EXPECT_EQ(shutdown_info.action, GracefulShutdownAction::kReboot);
}

TEST(GracefulShutdownInfoTest, ActionIsNotAString) {
  EXPECT_EQ(FromJson(R"({ "action": [], "reasons" : [] })"),
            (GracefulShutdownInfo{
                .action = GracefulShutdownAction::kNotParseable,
                .reasons = {GracefulShutdownReason::kNotParseable},
            }));
}

TEST(GracefulShutdownInfoTest, ReasonsIsNotAnArray) {
  EXPECT_EQ(FromJson(R"({ "reasons" : "not-an-array" })"),
            (GracefulShutdownInfo{
                .action = GracefulShutdownAction::kNotParseable,
                .reasons = {GracefulShutdownReason::kNotParseable},
            }));
}

TEST(GracefulShutdownInfoTest, SpuriousField) {
  EXPECT_EQ(FromJson(R"({ "reasons" : [], "spurious_field": "spurious-value" })"),
            (GracefulShutdownInfo{
                .action = GracefulShutdownAction::kNotParseable,
                .reasons = {GracefulShutdownReason::kNotParseable},
            }));
}

struct ToGracefulShutdownActionTestParam {
  std::string test_name;
  ShutdownAction input_shutdown_action;
  GracefulShutdownAction output_reason;
};

class ToGracefulShutdownActionTest
    : public UnitTestFixture,
      public testing::WithParamInterface<ToGracefulShutdownActionTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownActions, ToGracefulShutdownActionTest,
                         ::testing::ValuesIn(std::vector<ToGracefulShutdownActionTestParam>({
                             {
                                 "Poweroff",
                                 ShutdownAction::POWEROFF,
                                 GracefulShutdownAction::kPoweroff,
                             },
                             {
                                 "Reboot",
                                 ShutdownAction::REBOOT,
                                 GracefulShutdownAction::kReboot,
                             },
                             {
                                 "RebootToRecovery",
                                 ShutdownAction::REBOOT_TO_RECOVERY,
                                 GracefulShutdownAction::kRebootToRecovery,
                             },
                             {
                                 "RebootToBootloader",
                                 ShutdownAction::REBOOT_TO_BOOTLOADER,
                                 GracefulShutdownAction::kRebootToBootloader,
                             },
                             {
                                 "NotSupported",
                                 static_cast<ShutdownAction>(100u),
                                 GracefulShutdownAction::kNotSupported,
                             },
                         })),
                         [](const testing::TestParamInfo<ToGracefulShutdownActionTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(ToGracefulShutdownActionTest, ToGracefulShutdownAction) {
  const ToGracefulShutdownActionTestParam& param = GetParam();

  ShutdownOptions options;
  options.set_action(param.input_shutdown_action);
  EXPECT_EQ(ToGracefulShutdownAction(options), param.output_reason);
}

TEST(GracefulShutdownInfoDeathTest, ToGracefulShutdownActionRequiresAction) {
  ASSERT_DEATH(ToGracefulShutdownAction(ShutdownOptions()), "");
}

struct ReasonTestParam {
  std::string test_name;
  GracefulShutdownReason input_shutdown_reason;
  std::string output_reason;
};

class WriteGracefulShutdownInfoTest : public UnitTestFixture {
 protected:
  std::string Path() { return files::JoinPath(tmp_dir_.path(), kFilename); }

  static std::string BuildJson(const std::string_view action, const std::string& reason) {
    // Use {{ and }} to escape the curly braces for std::format.
    return std::format(
        R"({{
    "action": "{}",
    "reasons": [
        "{}"
    ]
}})",
        action, reason);
  }

 private:
  files::ScopedTempDir tmp_dir_;
};

class WriteGracefulShutdownReasonsTest : public WriteGracefulShutdownInfoTest,
                                         public testing::WithParamInterface<ReasonTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownReasons, WriteGracefulShutdownReasonsTest,
                         ::testing::ValuesIn(std::vector<ReasonTestParam>({
                             {
                                 "UserRequest",
                                 GracefulShutdownReason::kUserRequest,
                                 "USER REQUEST",
                             },
                             {
                                 "SystemUpdate",
                                 GracefulShutdownReason::kSystemUpdate,
                                 "SYSTEM UPDATE",
                             },
                             {
                                 "RetrySystemUpdate",
                                 GracefulShutdownReason::kRetrySystemUpdate,
                                 "RETRY SYSTEM UPDATE",
                             },
                             {
                                 "HighTemperature",
                                 GracefulShutdownReason::kHighTemperature,
                                 "HIGH TEMPERATURE",
                             },
                             {
                                 "SessionFailure",
                                 GracefulShutdownReason::kSessionFailure,
                                 "SESSION FAILURE",
                             },
                             {
                                 "SystemFailure",
                                 GracefulShutdownReason::kSysmgrFailure,
                                 "SYSMGR FAILURE",
                             },
                             {
                                 "CriticalComponentFailure",
                                 GracefulShutdownReason::kCriticalComponentFailure,
                                 "CRITICAL COMPONENT FAILURE",
                             },
                             {
                                 "CriticalDriverFailure",
                                 GracefulShutdownReason::kCriticalDriverFailure,
                                 "CRITICAL DRIVER FAILURE",
                             },
                             {
                                 "FactoryDataReset",
                                 GracefulShutdownReason::kFdr,
                                 "FACTORY DATA RESET",
                             },
                             {
                                 "ZbiSwap",
                                 GracefulShutdownReason::kZbiSwap,
                                 "ZBI SWAP",
                             },
                             {
                                 "OutOfMemory",
                                 GracefulShutdownReason::kOutOfMemory,
                                 "OUT OF MEMORY",
                             },
                             {
                                 "NetstackMigration",
                                 GracefulShutdownReason::kNetstackMigration,
                                 "NETSTACK MIGRATION",
                             },
                             {
                                 "NotSupported",
                                 static_cast<GracefulShutdownReason>(100u),
                                 "NOT SUPPORTED",
                             },
                             {
                                 "AndroidUnexpectedReason",
                                 GracefulShutdownReason::kAndroidUnexpectedReason,
                                 "ANDROID UNEXPECTED REASON",
                             },
                             {
                                 "AndroidNoReason",
                                 GracefulShutdownReason::kAndroidNoReason,
                                 "ANDROID NO REASON",
                             },
                             {
                                 "AndroidRescueParty",
                                 GracefulShutdownReason::kAndroidRescueParty,
                                 "ANDROID RESCUE PARTY",
                             },
                             {
                                 "AndroidCriticalProcessFailure",
                                 GracefulShutdownReason::kAndroidCriticalProcessFailure,
                                 "ANDROID CRITICAL PROCESS FAILURE",
                             },
                             {
                                 "DeveloperRequest",
                                 GracefulShutdownReason::kDeveloperRequest,
                                 "DEVELOPER REQUEST",
                             },
                             {
                                 "UserRequestDeviceStuck",
                                 GracefulShutdownReason::kUserRequestDeviceStuck,
                                 "USER REQUEST DEVICE STUCK",
                             },
                             {
                                 "BatteryDrained",
                                 GracefulShutdownReason::kBatteryDrained,
                                 "BATTERY DRAINED",
                             },
                             {
                                 "SuspensionFailure",
                                 GracefulShutdownReason::kSuspensionFailure,
                                 "SUSPENSION FAILURE",
                             },
                         })),
                         [](const testing::TestParamInfo<ReasonTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(WriteGracefulShutdownReasonsTest, WritesReasons) {
  const ReasonTestParam& param = GetParam();

  WriteGracefulShutdownInfo(GracefulShutdownAction::kReboot, {param.input_shutdown_reason}, Path());

  std::string contents;
  ASSERT_TRUE(files::ReadFileToString(Path(), &contents));
  EXPECT_EQ(contents, BuildJson(ToString(GracefulShutdownAction::kReboot), param.output_reason));

  RunLoopUntilIdle();
}

struct ActionTestParam {
  std::string test_name;
  GracefulShutdownAction input_shutdown_action;
  std::string output_action;
};

class WriteGracefulShutdownActionTest : public WriteGracefulShutdownInfoTest,
                                        public testing::WithParamInterface<ActionTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownActions, WriteGracefulShutdownActionTest,
                         ::testing::ValuesIn(std::vector<ActionTestParam>({
                             {
                                 "Poweroff",
                                 GracefulShutdownAction::kPoweroff,
                                 "POWEROFF",
                             },
                             {
                                 "Reboot",
                                 GracefulShutdownAction::kReboot,
                                 "REBOOT",
                             },
                             {
                                 "RebootToRecovery",
                                 GracefulShutdownAction::kRebootToRecovery,
                                 "REBOOT_TO_RECOVERY",
                             },
                             {
                                 "RebootToBootloader",
                                 GracefulShutdownAction::kRebootToBootloader,
                                 "REBOOT_TO_BOOTLOADER",
                             },
                             {
                                 "NotSupported",
                                 static_cast<GracefulShutdownAction>(100u),
                                 "NOT SUPPORTED",
                             },
                         })),
                         [](const testing::TestParamInfo<ActionTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(WriteGracefulShutdownActionTest, WritesAction) {
  const ActionTestParam& param = GetParam();

  WriteGracefulShutdownInfo(param.input_shutdown_action, /*reasons=*/{}, Path());

  std::string contents;
  ASSERT_TRUE(files::ReadFileToString(Path(), &contents));
  EXPECT_EQ(contents, BuildJsonWithEmptyReasons(param.output_action));

  RunLoopUntilIdle();
}

}  // namespace
}  // namespace feedback
}  // namespace forensics
