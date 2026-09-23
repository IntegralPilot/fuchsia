// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/reboot_log/reboot_log.h"

#include <fuchsia/hardware/power/statecontrol/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics {
namespace feedback {
namespace {

using fuchsia::hardware::power::statecontrol::ShutdownAction;
using fuchsia::hardware::power::statecontrol::ShutdownReason;

using ::testing::HasSubstr;

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

struct RebootReasonTestParam {
  std::string test_name;
  std::optional<std::string> zircon_reboot_log;
  std::optional<ShutdownReason> shutdown_reason;
  std::string output_reboot_reason;
};

struct ShutdownActionTestParam {
  std::string test_name;
  std::optional<ShutdownAction> shutdown_action;
  std::optional<GracefulShutdownAction> output_shutdown_action;
};

struct RebootMultiReasonTestParam {
  std::string test_name;
  std::vector<ShutdownReason> reasons;
  std::string output_reboot_reason;
};

struct TimeTestParam {
  std::string test_name;
  std::optional<std::string> zircon_reboot_log;
  std::optional<zx::duration> output_uptime;
  std::optional<zx::duration> output_runtime;
};

struct RebootLogStrTestParam {
  std::string test_name;
  std::optional<std::string> zircon_reboot_log;
  std::vector<ShutdownReason> shutdown_reasons;
  std::optional<std::string> output_reboot_log_str;
};

class RebootLogTest : public UnitTestFixture {
 public:
  RebootLogTest() {
    redactor_ = std::make_unique<forensics::IdentityRedactor>(
        InspectRoot().CreateBool("redaction_enabled", true));
    previous_boot_kernel_log_path_ =
        files::JoinPath(tmp_dir_.path(), "log.kernel.previous_boot.txt");
    final_shutdown_info_path_ = files::JoinPath(tmp_dir_.path(), "final_shutdown_info.json");
  }

 protected:
  void WriteZirconRebootLogContents(const std::string& contents) {
    FX_CHECK(tmp_dir_.NewTempFileWithData(contents, &zircon_reboot_log_path_))
        << "Failed to create temporary Zircon reboot log";
  }

  void WriteGracefulShutdownInfoContents(const std::string& contents) {
    FX_CHECK(tmp_dir_.NewTempFileWithData(contents, &graceful_shutdown_info_path_))
        << "Failed to create temporary graceful shutdown info";
  }

  void WriteGracefulShutdownInfoContents(
      fuchsia::hardware::power::statecontrol::ShutdownOptions options) {
    FX_CHECK(tmp_dir_.NewTempFileWithData("", &graceful_shutdown_info_path_))
        << "Failed to create temporary graceful shutdown info";

    FX_CHECK(files::WriteFile(
        graceful_shutdown_info_path_,
        ToJson(ToGracefulShutdownAction(options), ToGracefulShutdownReasons(options))));
  }

  void WritePreviousSystemTimeContents(const std::string& contents) {
    FX_CHECK(tmp_dir_.NewTempFileWithData(contents, &previous_system_time_path_))
        << "Failed to create temporary previous system time file";
  }

  std::string WriteLegacyGracefulRebootLogContents(
      fuchsia::hardware::power::statecontrol::ShutdownOptions options) {
    std::string path;
    FX_CHECK(tmp_dir_.NewTempFileWithData("", &path))
        << "Failed to create temporary graceful reboot log";

    FX_CHECK(
        files::WriteFile(path, ToLegacyFileContentForTesting(ToGracefulShutdownReasons(options))));
    return path;
  }

  RebootLog ParseRebootLog(const std::string& zircon_reboot_log_path,
                           const std::string& graceful_shutdown_info_path,
                           const std::string& legacy_graceful_reboot_log_path,
                           const std::string& previous_system_time_path, bool not_a_fdr,
                           bool supports_user_initiated_poweroffs) {
    return RebootLog::ParseRebootLog(zircon_reboot_log_path, graceful_shutdown_info_path,
                                     legacy_graceful_reboot_log_path, previous_system_time_path,
                                     previous_boot_kernel_log_path_, final_shutdown_info_path_,
                                     not_a_fdr, supports_user_initiated_poweroffs,
                                     /*first_component_instance=*/true, redactor_.get());
  }

  std::string zircon_reboot_log_path_;
  std::string graceful_shutdown_info_path_;
  std::string previous_system_time_path_;
  std::string previous_boot_kernel_log_path_;
  std::string final_shutdown_info_path_;
  std::unique_ptr<RedactorBase> redactor_;

 private:
  timekeeper::TestClock clock_;
  files::ScopedTempDir tmp_dir_;
};

class RebootLogReasonTest : public RebootLogTest,
                            public testing::WithParamInterface<RebootReasonTestParam> {};

fuchsia::hardware::power::statecontrol::ShutdownOptions NewShutdownOptions(
    ShutdownAction action, std::vector<ShutdownReason> reasons) {
  fuchsia::hardware::power::statecontrol::ShutdownOptions options;
  options.set_action(action);
  options.set_reasons(std::move(reasons));
  return options;
}

INSTANTIATE_TEST_SUITE_P(
    WithVariousRebootLogs, RebootLogReasonTest,
    ::testing::ValuesIn(std::vector<RebootReasonTestParam>({
        {
            "ZirconCleanNoGraceful",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            std::nullopt,
            "GENERIC GRACEFUL",
        },
        {
            "ZirconCleanGracefulUserRequest",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::USER_REQUEST,
            "USER REQUEST",
        },
        {
            "ZirconCleanGracefulSystemUpdate",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::SYSTEM_UPDATE,
            "SYSTEM UPDATE",
        },
        {
            "ZirconCleanGracefulNetstackMigration",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::NETSTACK_MIGRATION,
            "NETSTACK MIGRATION",
        },
        {
            "ZirconCleanGracefulHighTemperature",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::HIGH_TEMPERATURE,
            "HIGH TEMPERATURE",
        },
        {
            "ZirconCleanGracefulSessionFailure",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::SESSION_FAILURE,
            "SESSION FAILURE",
        },
        {
            "ZirconCleanGracefulNotSupported",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            static_cast<ShutdownReason>(1000u),
            "GENERIC GRACEFUL",
        },
        {
            "Cold",
            std::nullopt,
            ShutdownReason::USER_REQUEST,
            "COLD",
        },
        {
            "ColdWithHwReason",
            "HW REBOOT REASON (COLD BOOT)\n\n",
            ShutdownReason::USER_REQUEST,
            "COLD",
        },
        {
            "ColdWithHwReasonAndWarning",
            "HW REBOOT REASON (COLD BOOT)\n\n"
            "WARNING - Could not recover crashlog from RAM. Only HW reboot reason is available.",
            std::nullopt,
            "COLD",
        },
        {
            "KernelPanic",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::USER_REQUEST,
            "KERNEL PANIC",
        },
        {
            "OOM",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (OOM)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::USER_REQUEST,
            "OOM",
        },
        {
            "SwWatchdog",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (SW WATCHDOG)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::USER_REQUEST,
            "SOFTWARE WATCHDOG TIMEOUT",
        },
        {
            "HwWatchdog",
            "HW REBOOT REASON (HW WATCHDOG)\n\n",
            ShutdownReason::USER_REQUEST,
            "HARDWARE WATCHDOG TIMEOUT",
        },
        {
            "Brownout",
            "HW REBOOT REASON (BROWNOUT)\n\n",
            ShutdownReason::USER_REQUEST,
            "BROWNOUT",
        },
        {
            "Spontaneous",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (UNKNOWN)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::USER_REQUEST,
            "SPONTANEOUS",
        },
        {
            "RootJobTermination",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            ShutdownReason::USER_REQUEST,
            "ROOT JOB TERMINATION",
        },
        {
            "UserHardReset",
            "HW REBOOT REASON (USER HARD RESET)\n\n",
            std::nullopt,
            "USER HARD RESET",
        },
        {
            "UserHardResetTrumpsKernelPanic",
            "HW REBOOT REASON (USER HARD RESET)\n\n"
            "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            std::nullopt,
            "USER HARD RESET",
        },
        {
            "NotParseable",
            "NOT PARSEABLE",
            ShutdownReason::USER_REQUEST,
            "NOT PARSEABLE",
        },
    })),
    [](const testing::TestParamInfo<RebootReasonTestParam>& info) { return info.param.test_name; });

TEST_P(RebootLogReasonTest, Succeed) {
  const RebootReasonTestParam& param = GetParam();
  if (param.zircon_reboot_log.has_value()) {
    WriteZirconRebootLogContents(param.zircon_reboot_log.value());
  }

  if (param.shutdown_reason.has_value()) {
    WriteGracefulShutdownInfoContents(
        NewShutdownOptions(ShutdownAction::REBOOT, {param.shutdown_reason.value()}));
  }

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), param.output_reboot_reason);
}

class ColdBootTest : public RebootLogTest,
                     public testing::WithParamInterface<ShutdownActionTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownActions, ColdBootTest,
                         ::testing::ValuesIn(std::vector<ShutdownActionTestParam>({
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
                         })),
                         [](const testing::TestParamInfo<ShutdownActionTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(ColdBootTest, ActionPreservedForColdBoot) {
  const ShutdownActionTestParam& param = GetParam();

  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(*param.shutdown_action, {ShutdownReason::USER_REQUEST}));

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToGracefulShutdownAction(),
            param.output_shutdown_action);
  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "COLD");
}

TEST_F(ColdBootTest, UsesGracefulPoweroffReasons) {
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::POWEROFF, {ShutdownReason::USER_REQUEST}));

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToGracefulShutdownAction(),
            GracefulShutdownAction::kPoweroff);
  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "USER REQUEST");
}

TEST_F(ColdBootTest, EmptyGracefulReasonsIsGenericGraceful) {
  WriteGracefulShutdownInfoContents(NewShutdownOptions(ShutdownAction::POWEROFF, {}));

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToGracefulShutdownAction(),
            GracefulShutdownAction::kPoweroff);
  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "GENERIC GRACEFUL");
}

TEST_F(ColdBootTest, EmptyGracefulInfoIsCold) {
  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_FALSE(reboot_log.GetFinalShutdownInfo().ToGracefulShutdownAction().has_value());
  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "COLD");
}

TEST_F(ColdBootTest, EmptyGracefulInfoIsSpontaneous) {
  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/true));

  EXPECT_FALSE(reboot_log.GetFinalShutdownInfo().ToGracefulShutdownAction().has_value());
  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "SPONTANEOUS");
}

class ShutdownActionTest : public RebootLogTest,
                           public testing::WithParamInterface<ShutdownActionTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownAction, ShutdownActionTest,
                         ::testing::ValuesIn(std::vector<ShutdownActionTestParam>({
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
                                 "Ungraceful",
                                 std::nullopt,
                                 std::nullopt,
                             },
                         })),
                         [](const testing::TestParamInfo<ShutdownActionTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(ShutdownActionTest, ActionParsed) {
  const ShutdownActionTestParam& param = GetParam();
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");

  if (param.shutdown_action.has_value()) {
    WriteGracefulShutdownInfoContents(NewShutdownOptions(*param.shutdown_action, {}));
  }

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToGracefulShutdownAction(),
            param.output_shutdown_action);
}

TEST_P(RebootLogReasonTest, LegacyTxtFallback) {
  const RebootReasonTestParam& param = GetParam();
  if (param.zircon_reboot_log.has_value()) {
    WriteZirconRebootLogContents(param.zircon_reboot_log.value());
  }

  std::string legacy_graceful_reboot_log_path;
  if (param.shutdown_reason.has_value()) {
    legacy_graceful_reboot_log_path = WriteLegacyGracefulRebootLogContents(
        NewShutdownOptions(ShutdownAction::REBOOT, {param.shutdown_reason.value()}));
  }

  const RebootLog reboot_log(ParseRebootLog(
      zircon_reboot_log_path_, graceful_shutdown_info_path_, legacy_graceful_reboot_log_path,
      previous_system_time_path_, /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), param.output_reboot_reason);
}

TEST_F(RebootLogReasonTest, Succeed_ZirconCleanGracefulFdr) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::REBOOT, {ShutdownReason::SYSTEM_UPDATE}));

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/false, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "FACTORY DATA RESET");
}

TEST_F(RebootLogReasonTest, Succeed_ZirconCleanGracefulNotParseable) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");
  WriteGracefulShutdownInfoContents("NOT PARSEABLE");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "GENERIC GRACEFUL");

  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Uptime(), zx::msec(1234));

  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Runtime(), zx::msec(1098));
}

TEST_F(RebootLogReasonTest, Succeed_RebootReasonsUnset) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");
  fuchsia::hardware::power::statecontrol::ShutdownOptions options;
  options.set_action(ShutdownAction::REBOOT);
  WriteGracefulShutdownInfoContents(std::move(options));
  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "GENERIC GRACEFUL");
}

TEST_F(RebootLogReasonTest, Succeed_RebootReasonsEmpty) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");
  WriteGracefulShutdownInfoContents(NewShutdownOptions(ShutdownAction::REBOOT, {}));
  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "GENERIC GRACEFUL");
}

TEST_F(RebootLogReasonTest, SucceedNoGracefulShutdownInfo) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToGracefulShutdownAction(), std::nullopt);
  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "GENERIC GRACEFUL");
}

class RebootLogMultiReasonTest : public RebootLogTest,
                                 public testing::WithParamInterface<RebootMultiReasonTestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousRebootLogs, RebootLogMultiReasonTest,
                         ::testing::ValuesIn(std::vector<RebootMultiReasonTestParam>(
                             {{
                                  "SystemUpdateThenNetstackMigration",
                                  {
                                      ShutdownReason::SYSTEM_UPDATE,
                                      ShutdownReason::NETSTACK_MIGRATION,
                                  },
                                  "SYSTEM UPDATE",
                              },
                              {
                                  "NetstackMigrationThenSystemUpdate",
                                  {
                                      ShutdownReason::NETSTACK_MIGRATION,
                                      ShutdownReason::SYSTEM_UPDATE,
                                  },
                                  "SYSTEM UPDATE",
                              },
                              {
                                  "UnexpectedCombination",
                                  {
                                      ShutdownReason::OUT_OF_MEMORY,
                                      ShutdownReason::SYSTEM_UPDATE,
                                  },
                                  "UNEXPECTED REASON GRACEFUL",
                              }})),
                         [](const testing::TestParamInfo<RebootMultiReasonTestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(RebootLogMultiReasonTest, Succeed) {
  const RebootMultiReasonTestParam& param = GetParam();

  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");

  WriteGracefulShutdownInfoContents(NewShutdownOptions(ShutdownAction::REBOOT, param.reasons));

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), param.output_reboot_reason);
}

class RebootLogTimeTest : public RebootLogTest,
                          public testing::WithParamInterface<TimeTestParam> {};

INSTANTIATE_TEST_SUITE_P(
    WithVariousRebootLogs, RebootLogTimeTest,
    ::testing::ValuesIn(std::vector<TimeTestParam>({
        {
            "WellFormedLog",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            zx::msec(1234),
            zx::msec(1098),
        },
        {
            "NoZirconRebootLog",
            std::nullopt,
            std::nullopt,
            std::nullopt,
        },
        {
            "EmptyZirconRebootLog",
            "",
            std::nullopt,
            std::nullopt,
        },
        {
            "OnlyHwRebootReason",
            "HW REBOOT REASON (COLD BOOT)\n\n",
            std::nullopt,
            std::nullopt,
        },
        {
            "MalformedLog",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "BAD REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            std::nullopt,
            std::nullopt,
        },
        {
            "TooFewLinesForUptime",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n",
            std::nullopt,
            std::nullopt,
        },
        {
            "BadUptimeString",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nDOWNTIME (ms)\n1234",
            std::nullopt,
            std::nullopt,
        },
        {
            "TooFewLinesForRuntime",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n",
            zx::msec(1234),
            std::nullopt,
        },
        {
            "BadRuntimeString",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nWALKTIME (ms)\n1098",
            zx::msec(1234),
            std::nullopt,
        },
        {
            "NegativeUptime",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n-1234\nRUNTIME (ms)\n1098",
            std::nullopt,
            zx::msec(1098),
        },
        {
            "NegativeRuntime",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n-1098",
            zx::msec(1234),
            std::nullopt,
        },
        {
            "NegativeUptimeRuntime",
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n-1234\nRUNTIME (ms)\n-1098",
            std::nullopt,
            std::nullopt,
        },
    })),
    [](const testing::TestParamInfo<TimeTestParam>& info) { return info.param.test_name; });

TEST_P(RebootLogTimeTest, Succeed) {
  const TimeTestParam& param = GetParam();
  if (param.zircon_reboot_log.has_value()) {
    WriteZirconRebootLogContents(param.zircon_reboot_log.value());
  }

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  if (param.output_uptime.has_value()) {
    ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
    EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Uptime(), param.output_uptime.value());
  } else {
    EXPECT_FALSE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
  }

  if (param.output_runtime.has_value()) {
    ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
    EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Runtime(), param.output_runtime.value());
  } else {
    EXPECT_FALSE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
  }
}

TEST_F(RebootLogTest, FallbackToSystemTimeTracker_NoZirconValues) {
  WritePreviousSystemTimeContents(R"({"uptime_ms":9876,"runtime_ms":8765})");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Uptime(), zx::msec(9876));
  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Runtime(), zx::msec(8765));

  EXPECT_THAT(reboot_log.RebootLogStr(), HasSubstr("FALLBACK UPTIME (ms)\n9876"));
  EXPECT_THAT(reboot_log.RebootLogStr(), HasSubstr("FALLBACK RUNTIME (ms)\n8765"));
}

TEST_F(RebootLogTest, FallbackToSystemTimeTracker_NegativeZirconValues) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n-1234\nRUNTIME (ms)\n-1098");
  WritePreviousSystemTimeContents(R"({"uptime_ms":9876,"runtime_ms":8765})");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Uptime(), zx::msec(9876));
  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Runtime(), zx::msec(8765));

  EXPECT_THAT(reboot_log.RebootLogStr(), HasSubstr("FALLBACK UPTIME (ms)\n9876"));
  EXPECT_THAT(reboot_log.RebootLogStr(), HasSubstr("FALLBACK RUNTIME (ms)\n8765"));
}

TEST_F(RebootLogTest, NoPreviousSystemTimeFile) {
  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  EXPECT_FALSE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
  EXPECT_FALSE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
}

TEST_F(RebootLogTest, NoFallbackToSystemTimeTracker_MissingRuntime) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n");
  WritePreviousSystemTimeContents(R"({"uptime_ms":9876,"runtime_ms":8765})");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Uptime(), zx::msec(1234));
  ASSERT_FALSE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
}

TEST_F(RebootLogTest, NoFallbackToSystemTimeTracker_MissingUptime) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nDOWNTIME (ms)\n1234\nRUNTIME (ms)\n5678");
  WritePreviousSystemTimeContents(R"({"uptime_ms":9876,"runtime_ms":8765})");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  ASSERT_FALSE(reboot_log.GetFinalShutdownInfo().Uptime().has_value());
  ASSERT_TRUE(reboot_log.GetFinalShutdownInfo().Runtime().has_value());
  EXPECT_EQ(*reboot_log.GetFinalShutdownInfo().Runtime(), zx::msec(5678));
}

TEST_F(RebootLogTest, ParsesCriticalProcess) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098\n"
      "ROOT JOB TERMINATED BY CRITICAL PROCESS DEATH: foo (1)");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  const FinalShutdownInfo& shutdown_info = reboot_log.GetFinalShutdownInfo();
  ASSERT_TRUE(shutdown_info.IsCrash());
  EXPECT_EQ(shutdown_info.ToCrashSignature(SpontaneousRebootReason::kSpontaneous),
            "fuchsia-reboot-foo-terminated");
}

class RebootLogStrTest : public RebootLogTest,
                         public testing::WithParamInterface<RebootLogStrTestParam> {};

INSTANTIATE_TEST_SUITE_P(
    WithVariousRebootLogs, RebootLogStrTest,
    ::testing::ValuesIn(std::vector<RebootLogStrTestParam>({
        {
            "ConcatenatesZirconAndGraceful",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            {ShutdownReason::USER_REQUEST},
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098\n"
            "GRACEFUL SHUTDOWN ACTION: (REBOOT)\n"
            "GRACEFUL REBOOT REASONS: (USER REQUEST)\n\nFINAL REBOOT REASON (USER REQUEST)",
        },
        {
            // This test is the same as the above test, but is used to show that there may be an
            // ungraceful zircon reboot reason and a graceful reboot reason.
            "ConcatenatesZirconUngracefulAndGraceful",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            {ShutdownReason::USER_REQUEST},
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (KERNEL PANIC)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098\n"
            "GRACEFUL SHUTDOWN ACTION: (REBOOT)\n"
            "GRACEFUL REBOOT REASONS: (USER REQUEST)\n\nFINAL REBOOT REASON (KERNEL PANIC)",
        },
        {
            "NoGracefulRebootLog",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            {},
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098\n"
            "GRACEFUL SHUTDOWN ACTION: (NONE)\n"
            "GRACEFUL REBOOT REASONS: (NONE)\n\nFINAL REBOOT REASON (GENERIC GRACEFUL)",
        },
        {
            "MultipleGracefulRebootLog",
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098",
            {ShutdownReason::NETSTACK_MIGRATION, ShutdownReason::SYSTEM_UPDATE},
            "HW REBOOT REASON (WARM BOOT)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098\n"
            "GRACEFUL SHUTDOWN ACTION: (REBOOT)\n"
            "GRACEFUL REBOOT REASONS: (NETSTACK MIGRATION,SYSTEM UPDATE)\n\nFINAL REBOOT REASON (SYSTEM UPDATE)",
        },
        {
            "NoZirconRebootLog",
            std::nullopt,
            {ShutdownReason::USER_REQUEST},
            "HW REBOOT REASON (COLD BOOT)\n"
            "GRACEFUL SHUTDOWN ACTION: (REBOOT)\n"
            "GRACEFUL REBOOT REASONS: (USER REQUEST)\n\nFINAL REBOOT REASON (COLD)",
        },
    })),
    [](const testing::TestParamInfo<RebootLogStrTestParam>& info) { return info.param.test_name; });

TEST_P(RebootLogStrTest, Succeed) {
  const RebootLogStrTestParam& param = GetParam();
  if (param.zircon_reboot_log.has_value()) {
    WriteZirconRebootLogContents(param.zircon_reboot_log.value());
  }

  if (!param.shutdown_reasons.empty()) {
    WriteGracefulShutdownInfoContents(
        NewShutdownOptions(ShutdownAction::REBOOT, param.shutdown_reasons));
  }

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));

  if (param.output_reboot_log_str.has_value()) {
    EXPECT_EQ(reboot_log.RebootLogStr(), param.output_reboot_log_str.value());
  }
}

TEST_F(RebootLogStrTest, Succeed_SetGracefulFDR) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::REBOOT, {ShutdownReason::FACTORY_DATA_RESET}));

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false));
  EXPECT_EQ(reboot_log.RebootLogStr(),
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098\n"
            "GRACEFUL SHUTDOWN ACTION: (REBOOT)\n"
            "GRACEFUL REBOOT REASONS: (FACTORY DATA RESET)\n\n"
            "FINAL REBOOT REASON (FACTORY DATA RESET)");
}

TEST_F(RebootLogStrTest, Succeed_InferFDR) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");

  const RebootLog reboot_log(
      ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                     /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                     /*not_a_fdr=*/false, /*supports_user_initiated_poweroffs=*/false));
  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "FACTORY DATA RESET");
  EXPECT_EQ(reboot_log.RebootLogStr(),
            "HW REBOOT REASON (UNKNOWN)\n\n"
            "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098\n"
            "GRACEFUL SHUTDOWN ACTION: (NONE)\n"
            "GRACEFUL REBOOT REASONS: (NONE)\n\n"
            "FINAL REBOOT REASON (FACTORY DATA RESET)");
}

TEST_F(RebootLogStrTest, Succeed_SetDlog) {
  constexpr std::string_view kContents =
      R"(HW REBOOT REASON (UNKNOWN)

ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)

UPTIME (ms)
1234
RUNTIME (ms)
1098

--- BEGIN DLOG DUMP ---
test dlog dump line1
test dlog dump line2

--- END DLOG DUMP ---

GRACEFUL SHUTDOWN ACTION: (REBOOT)
GRACEFUL REBOOT REASONS: (NONE)

FINAL REBOOT REASON (ROOT JOB TERMINATION))";

  WriteZirconRebootLogContents(std::string(kContents));
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::REBOOT, {ShutdownReason::CRITICAL_COMPONENT_FAILURE}));

  ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                 /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                 /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false);

  std::string dlog;
  ASSERT_TRUE(files::ReadFileToString(previous_boot_kernel_log_path_, &dlog));
  EXPECT_EQ(dlog, "test dlog dump line1\ntest dlog dump line2");
}

TEST_F(RebootLogStrTest, Succeed_EmptyDlog) {
  constexpr std::string_view kContents =
      R"(HW REBOOT REASON (UNKNOWN)

ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)

  UPTIME (ms)
  1234
  RUNTIME (ms)
  1098

  --- BEGIN DLOG DUMP ---
  --- END DLOG DUMP ---

  GRACEFUL SHUTDOWN ACTION: (REBOOT)
  GRACEFUL REBOOT REASONS: (NONE)

  FINAL REBOOT REASON (ROOT JOB TERMINATION))";

  WriteZirconRebootLogContents(std::string(kContents));
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::REBOOT, {ShutdownReason::CRITICAL_COMPONENT_FAILURE}));

  ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                 /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                 /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false);

  std::string dlog;
  ASSERT_TRUE(files::ReadFileToString(previous_boot_kernel_log_path_, &dlog));
  EXPECT_EQ(dlog, "");
}

TEST_F(RebootLogStrTest, Succeed_NoDlog) {
  constexpr std::string_view kContents =
      R"(HW REBOOT REASON (UNKNOWN)

ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)

  UPTIME (ms)
  1234
  RUNTIME (ms)
  1098

  GRACEFUL SHUTDOWN ACTION: (REBOOT)
  GRACEFUL REBOOT REASONS: (NONE)

  FINAL REBOOT REASON (ROOT JOB TERMINATION))";

  WriteZirconRebootLogContents(std::string(kContents));
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::REBOOT, {ShutdownReason::CRITICAL_COMPONENT_FAILURE}));

  ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                 /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                 /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false);

  EXPECT_FALSE(files::IsFile(previous_boot_kernel_log_path_));
}

TEST_F(RebootLogStrTest, ParsesDlogOnFirstComponentInstanceOnly) {
  constexpr std::string_view kNoDlogContents =
      R"(HW REBOOT REASON (UNKNOWN)

ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)

  UPTIME (ms)
  1234
  RUNTIME (ms)
  1098

  GRACEFUL SHUTDOWN ACTION: (REBOOT)
  GRACEFUL REBOOT REASONS: (NONE)

  FINAL REBOOT REASON (ROOT JOB TERMINATION))";

  WriteZirconRebootLogContents(std::string(kNoDlogContents));
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::REBOOT, {ShutdownReason::CRITICAL_COMPONENT_FAILURE}));

  RebootLog::ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                            /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                            previous_boot_kernel_log_path_, final_shutdown_info_path_,
                            /*not_a_fdr=*/true,
                            /*supports_user_initiated_poweroffs=*/true,
                            /*first_component_instance=*/true, redactor_.get());

  EXPECT_FALSE(files::IsFile(previous_boot_kernel_log_path_));

  constexpr std::string_view kContents =
      R"(HW REBOOT REASON (UNKNOWN)

ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)

UPTIME (ms)
1234
RUNTIME (ms)
1098

--- BEGIN DLOG DUMP ---
test dlog dump line1
test dlog dump line2

--- END DLOG DUMP ---

GRACEFUL SHUTDOWN ACTION: (REBOOT)
GRACEFUL REBOOT REASONS: (NONE)

FINAL REBOOT REASON (ROOT JOB TERMINATION))";

  WriteZirconRebootLogContents(std::string(kContents));

  RebootLog::ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                            /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                            previous_boot_kernel_log_path_, final_shutdown_info_path_,
                            /*not_a_fdr=*/true,
                            /*supports_user_initiated_poweroffs=*/true,
                            /*first_component_instance=*/false, redactor_.get());

  EXPECT_FALSE(files::IsFile(previous_boot_kernel_log_path_));
}

TEST_F(RebootLogStrTest, DlogIsRedacted) {
  constexpr std::string_view kContents =
      R"(HW REBOOT REASON (UNKNOWN)

ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)

UPTIME (ms)
1234
RUNTIME (ms)
1098

--- BEGIN DLOG DUMP ---
test dlog dump line1
test dlog dump line2

--- END DLOG DUMP ---

GRACEFUL SHUTDOWN ACTION: (REBOOT)
GRACEFUL REBOOT REASONS: (NONE)

FINAL REBOOT REASON (ROOT JOB TERMINATION))";

  WriteZirconRebootLogContents(std::string(kContents));
  WriteGracefulShutdownInfoContents(
      NewShutdownOptions(ShutdownAction::REBOOT, {ShutdownReason::CRITICAL_COMPONENT_FAILURE}));

  redactor_ = std::make_unique<SimpleRedactor>();
  ParseRebootLog(zircon_reboot_log_path_, graceful_shutdown_info_path_,
                 /*legacy_graceful_reboot_log_path=*/"", previous_system_time_path_,
                 /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false);

  std::string dlog;
  ASSERT_TRUE(files::ReadFileToString(previous_boot_kernel_log_path_, &dlog));
  EXPECT_EQ(dlog, "<REDACTED>");
}

TEST_F(RebootLogTest, UsesPersistedFinalShutdownInfo) {
  WriteZirconRebootLogContents(
      "HW REBOOT REASON (UNKNOWN)\n\n"
      "ZIRCON REBOOT REASON (NO CRASH)\n\nUPTIME (ms)\n1234\nRUNTIME (ms)\n1098");

  RebootLog reboot_log = RebootLog::ParseRebootLog(
      zircon_reboot_log_path_, graceful_shutdown_info_path_, "", previous_system_time_path_,
      previous_boot_kernel_log_path_, final_shutdown_info_path_,
      /*not_a_fdr=*/false, /*supports_user_initiated_poweroffs=*/false,
      /*first_component_instance=*/true, redactor_.get());

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "FACTORY DATA RESET");
  ASSERT_TRUE(files::IsFile(final_shutdown_info_path_));

  reboot_log = RebootLog::ParseRebootLog(
      zircon_reboot_log_path_, graceful_shutdown_info_path_, "", previous_system_time_path_,
      previous_boot_kernel_log_path_, final_shutdown_info_path_,
      /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false,
      /*first_component_instance=*/false, redactor_.get());

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "FACTORY DATA RESET");
}

TEST_F(RebootLogTest, DoNotReparseIfFinalShutdownInfoIsUnparseable) {
  WriteZirconRebootLogContents("HW REBOOT REASON (BROWNOUT)\n\n");

  ASSERT_TRUE(files::WriteFile(final_shutdown_info_path_, "INVALID JSON"));

  const RebootLog reboot_log = RebootLog::ParseRebootLog(
      zircon_reboot_log_path_, graceful_shutdown_info_path_, "", previous_system_time_path_,
      previous_boot_kernel_log_path_, final_shutdown_info_path_,
      /*not_a_fdr=*/true, /*supports_user_initiated_poweroffs=*/false,
      /*first_component_instance=*/false, redactor_.get());

  EXPECT_EQ(reboot_log.GetFinalShutdownInfo().ToRebootReasonString(), "NOT PARSEABLE");
}

}  // namespace
}  // namespace feedback
}  // namespace forensics
