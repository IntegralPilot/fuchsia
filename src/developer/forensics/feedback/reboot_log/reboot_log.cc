// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/reboot_log/reboot_log.h"

#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>

#include <utility>

#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"
#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"
#include "src/developer/forensics/feedback/reboot_log/hw_shutdown_reason.h"
#include "src/developer/forensics/feedback/reboot_log/zircon_shutdown_reason.h"
#include "src/developer/forensics/feedback/system_time_tracker.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/fxl/strings/trim.h"

namespace forensics {
namespace feedback {
namespace {

// The kernel adds this line to indicate which process caused the root job to terminate.
//
// It can be found at
// https://osscs.corp.google.com/fuchsia/fuchsia/+/main:zircon/kernel/lib/crashlog/crashlog.cc;l=146;drc=e81b291e80479976c2cca9f87b600917fda48475
constexpr std::string_view kCriticalProcessPrefix =
    "ROOT JOB TERMINATED BY CRITICAL PROCESS DEATH: ";
constexpr std::string_view kBeginDlog = "--- BEGIN DLOG DUMP ---";
constexpr std::string_view kEndDlog = "--- END DLOG DUMP ---";

std::optional<zx::duration> ExtractTime(const std::string_view line) {
  int64_t val;
  if (!fxl::StringToNumberWithError(line, &val)) {
    return std::nullopt;
  }

  // Sometimes (e.g., https://fxbug.dev/458140022), the time is negative and we know it's bogus in
  // these cases.
  if (val < 0) {
    return std::nullopt;
  }
  return zx::msec(val);
}

HwShutdownReason ExtractHwShutdownReason(const std::string_view line) {
  if (line == "HW REBOOT REASON (COLD BOOT)") {
    return HwShutdownReason::kCold;
  } else if (line == "HW REBOOT REASON (WARM BOOT)") {
    return HwShutdownReason::kWarm;
  } else if (line == "HW REBOOT REASON (BROWNOUT)") {
    return HwShutdownReason::kBrownout;
  } else if (line == "HW REBOOT REASON (HW WATCHDOG)") {
    return HwShutdownReason::kWatchdog;
  } else if (line == "HW REBOOT REASON (USER HARD RESET)") {
    return HwShutdownReason::kUserHardReset;
  } else if (line == "HW REBOOT REASON (UNKNOWN)") {
    return HwShutdownReason::kUndefined;
  }

  FX_LOGS(ERROR) << "Failed to extract a hardware reboot reason from Zircon reboot log";
  return HwShutdownReason::kNotParseable;
}

ZirconShutdownReason ExtractZirconShutdownReason(const std::string_view line) {
  if (line == "ZIRCON REBOOT REASON (NO CRASH)") {
    return ZirconShutdownReason::kNoCrash;
  } else if (line == "ZIRCON REBOOT REASON (KERNEL PANIC)") {
    return ZirconShutdownReason::kKernelPanic;
  } else if (line == "ZIRCON REBOOT REASON (OOM)") {
    return ZirconShutdownReason::kOOM;
  } else if (line == "ZIRCON REBOOT REASON (SW WATCHDOG)") {
    return ZirconShutdownReason::kSwWatchdog;
  } else if (line == "ZIRCON REBOOT REASON (UNKNOWN)") {
    return ZirconShutdownReason::kUnknown;
  } else if (line == "ZIRCON REBOOT REASON (USERSPACE ROOT JOB TERMINATION)") {
    return ZirconShutdownReason::kRootJobTermination;
  }

  return ZirconShutdownReason::kNotParseable;
}

void ExtractZirconRebootInfo(const std::string& path, HwShutdownReason* out_hw_reason,
                             ZirconShutdownReason* out_zircon_reason,
                             std::optional<std::string>* content,
                             std::optional<zx::duration>* uptime,
                             std::optional<zx::duration>* runtime,
                             std::optional<std::string>* crashed_process) {
  *out_hw_reason = HwShutdownReason::kNotSet;
  *out_zircon_reason = ZirconShutdownReason::kNotSet;

  if (!files::IsFile(path)) {
    *out_hw_reason = HwShutdownReason::kCold;
    *content = "HW REBOOT REASON (COLD BOOT)";
    return;
  }

  std::string file_content;
  if (!files::ReadFileToString(path, &file_content)) {
    FX_LOGS(ERROR) << "Failed to read Zircon reboot log from " << path;
    *out_hw_reason = HwShutdownReason::kNotParseable;
    *out_zircon_reason = ZirconShutdownReason::kNotParseable;
    return;
  }

  if (file_content.empty()) {
    FX_LOGS(ERROR) << "Found empty Zircon reboot log at " << path;
    *out_hw_reason = HwShutdownReason::kNotParseable;
    *out_zircon_reason = ZirconShutdownReason::kNotParseable;
    return;
  }

  *content = file_content;
  (*content)->erase(std::find((*content)->begin(), (*content)->end(), '\0'), (*content)->end());

  const std::vector<std::string_view> lines =
      fxl::SplitString(content->value(), "\n", fxl::WhiteSpaceHandling::kTrimWhitespace,
                       fxl::SplitResult::kSplitWantNonEmpty);

  if (lines.size() == 0) {
    FX_LOGS(ERROR) << "Zircon reboot log has no content";
    *out_hw_reason = HwShutdownReason::kNotParseable;
    *out_zircon_reason = ZirconShutdownReason::kNotParseable;
    return;
  }

  // We expect the format to be:
  //
  // HW REBOOT REASON (<SOME REASON>)
  // ZIRCON REBOOT REASON (<SOME REASON>)
  // UPTIME (ms)
  // <SOME UPTIME>
  // RUNTIME (ms)
  // <SOME RUNTIME>

  *out_hw_reason = ExtractHwShutdownReason(lines[0]);
  if (*out_hw_reason == HwShutdownReason::kNotParseable) {
    return;
  }

  if (lines.size() < 2) {
    return;
  }

  // From //zircon/kernel/platform/mapped_crashlog.cc
  if (lines[1] ==
      "WARNING - Could not recover crashlog from RAM. Only HW reboot reason is available.") {
    return;
  }

  *out_zircon_reason = ExtractZirconShutdownReason(lines[1]);
  if (*out_zircon_reason == ZirconShutdownReason::kNotParseable) {
    return;
  }

  if (lines.size() < 4) {
    return;
  } else if (lines[2] != "UPTIME (ms)") {
    FX_LOGS(ERROR) << "'UPTIME (ms)' not present, found '" << lines[2] << "'";
  } else {
    *uptime = ExtractTime(lines[3]);
  }

  if (lines.size() < 6) {
    return;
  } else if (lines[4] != "RUNTIME (ms)") {
    FX_LOGS(ERROR) << "'RUNTIME (ms)' not present, found '" << lines[4] << "'";
  } else {
    *runtime = ExtractTime(lines[5]);
  }

  // We expect the critical process to look like:
  //
  // ROOT JOB TERMINATED BY CRITICAL PROCESS DEATH: <PROCESS> (<KOID>)
  for (std::string_view line : lines) {
    if (line.substr(0, kCriticalProcessPrefix.size()) != kCriticalProcessPrefix) {
      continue;
    }

    line.remove_prefix(kCriticalProcessPrefix.size());

    if (const size_t r_paren = line.find_last_of('('); r_paren == line.npos) {
      continue;
    } else {
      line.remove_suffix(line.size() - r_paren);
    }

    if (line.empty() || line.back() != ' ') {
      continue;
    }
    line.remove_suffix(1);

    *crashed_process = line;
    break;
  }
}

// Prints |reboot_log| with the DLOG removed. Returns the removed DLOG, if present.
std::optional<std::string> ExtractDlogAndLogRebootLog(const std::string& reboot_log) {
  auto fallback_log =
      fit::defer([&reboot_log] { FX_LOGS(INFO) << "Reboot info:\n"
                                               << reboot_log; });

  const size_t begin_header_pos = reboot_log.find(kBeginDlog);
  if (begin_header_pos == std::string::npos) {
    return std::nullopt;
  }

  const size_t payload_begin = begin_header_pos + kBeginDlog.size();
  const size_t payload_end = reboot_log.find(kEndDlog, begin_header_pos);

  if (payload_end == std::string::npos) {
    // For some reason the DLOG dump started, but never finished.
    return std::nullopt;
  }

  const size_t end_footer_pos = payload_end + kEndDlog.size();

  fallback_log.cancel();
  FX_LOGS(INFO) << "Reboot info:\n"
                << reboot_log.substr(0, begin_header_pos)
                << "DLOG dump can be found in the snapshot file: "
                << feedback_data::kAttachmentLogKernelPrevious << reboot_log.substr(end_footer_pos);

  const std::string dlog = reboot_log.substr(payload_begin, payload_end - payload_begin);

  return std::string(fxl::TrimString(dlog, " \f\n\r\t\v"));
}

std::optional<GracefulShutdownInfo> ExtractLegacyGracefulRebootInfo(
    const std::string& legacy_graceful_reboot_log_path) {
  if (!files::IsFile(legacy_graceful_reboot_log_path)) {
    return std::nullopt;
  }

  std::string file_content;
  if (!files::ReadFileToString(legacy_graceful_reboot_log_path, &file_content)) {
    return GracefulShutdownInfo{
        .action = GracefulShutdownAction::kNotParseable,
        .reasons = {GracefulShutdownReason::kNotParseable},
    };
  }

  if (file_content.empty()) {
    return GracefulShutdownInfo{
        .action = GracefulShutdownAction::kNotParseable,
        .reasons = {GracefulShutdownReason::kNotParseable},
    };
  }

  // If an older version of Fuchsia persisted a legacy .txt file, then that means that the _reboot_
  // signal was received since other shutdown actions were not yet sending the shutdown signal to
  // Feedback.
  return GracefulShutdownInfo{
      .action = GracefulShutdownAction::kReboot,
      .reasons = FromLegacyTxtFile(file_content),
  };
}

// Returns std::nullopt if neither the json nor legacy txt file is present.
std::optional<GracefulShutdownInfo> ExtractGracefulShutdownInfo(
    const std::string& graceful_shutdown_info_path,
    const std::string& legacy_graceful_reboot_log_path) {
  if (!files::IsFile(graceful_shutdown_info_path)) {
    return ExtractLegacyGracefulRebootInfo(legacy_graceful_reboot_log_path);
  }

  std::string file_content;
  if (!files::ReadFileToString(graceful_shutdown_info_path, &file_content)) {
    return GracefulShutdownInfo{
        .action = GracefulShutdownAction::kNotParseable,
        .reasons = {GracefulShutdownReason::kNotParseable},
    };
  }

  if (file_content.empty()) {
    return GracefulShutdownInfo{
        .action = GracefulShutdownAction::kNotParseable,
        .reasons = {GracefulShutdownReason::kNotParseable},
    };
  }

  return FromJson(file_content);
}

std::string MakeRebootLog(const std::optional<std::string>& zircon_reboot_log,
                          const std::optional<GracefulShutdownInfo>& graceful_info,
                          const std::string& reboot_reason,
                          std::optional<zx::duration> fallback_uptime,
                          std::optional<zx::duration> fallback_runtime) {
  std::vector<std::string> lines;

  if (zircon_reboot_log.has_value()) {
    lines.push_back(zircon_reboot_log.value());
  }

  if (fallback_uptime.has_value()) {
    lines.push_back(fxl::StringPrintf("FALLBACK UPTIME (ms)\n%ld", fallback_uptime->to_msecs()));
  }

  if (fallback_runtime.has_value()) {
    lines.push_back(
        fxl::StringPrintf("FALLBACK RUNTIME (ms)\n%ld\n", fallback_runtime->to_msecs()));
  }

  const std::string graceful_action =
      graceful_info.has_value() ? ToString(graceful_info->action) : "NONE";
  const std::vector<GracefulShutdownReason> graceful_reasons =
      graceful_info.has_value() ? graceful_info->reasons : std::vector<GracefulShutdownReason>();

  lines.push_back(fxl::StringPrintf("GRACEFUL SHUTDOWN ACTION: (%s)", graceful_action.c_str()));

  // TODO(https://fxbug.dev/414413282): rename output to "shutdown" reasons once any
  // dependencies are ready for the migration. To make it a cleaner break this can be done once the
  // LastRebootInfoProvider protocol is renamed.
  lines.push_back(
      fxl::StringPrintf("GRACEFUL REBOOT REASONS: (%s)\n", ToRawStrings(graceful_reasons).c_str()));

  lines.push_back(fxl::StringPrintf("FINAL REBOOT REASON (%s)", reboot_reason.c_str()));

  return fxl::JoinStrings(lines, "\n");
}

void PersistDlog(const std::optional<std::string>& dlog, RedactorBase* redactor,
                 const std::string& path) {
  if (!dlog.has_value()) {
    return;
  }

  std::string redacted_dlog = *dlog;
  redactor->Redact(redacted_dlog);
  if (!files::WriteFile(path, redacted_dlog)) {
    FX_LOGS(ERROR) << "Failed to write dlog to: " << path;
  }
}

}  // namespace

// static
RebootLog RebootLog::ParseRebootLog(const std::string& zircon_reboot_log_path,
                                    const std::string& graceful_shutdown_info_path,
                                    const std::string& legacy_graceful_reboot_log_path,
                                    const std::string& previous_system_time_path,
                                    const std::string& previous_boot_kernel_log_path,
                                    const std::string& final_shutdown_info_path,
                                    const bool not_a_fdr,
                                    const bool supports_user_initiated_poweroffs,
                                    const bool first_component_instance, RedactorBase* redactor) {
  if (!first_component_instance) {
    std::string content;
    if (files::ReadFileToString(final_shutdown_info_path, &content)) {
      // We shouldn't need the reboot log after the first component instance because it's only
      // used for reboot reports.
      return RebootLog(FinalShutdownInfo::FromJson(content), /*reboot_log_str=*/"");
    }
  }

  std::optional<std::string> zircon_reboot_log;
  std::optional<zx::duration> last_boot_uptime;
  std::optional<zx::duration> last_boot_runtime;
  std::optional<std::string> critical_process;
  HwShutdownReason hw_reason = HwShutdownReason::kNotSet;
  ZirconShutdownReason zircon_reason = ZirconShutdownReason::kNotSet;
  ExtractZirconRebootInfo(zircon_reboot_log_path, &hw_reason, &zircon_reason, &zircon_reboot_log,
                          &last_boot_uptime, &last_boot_runtime, &critical_process);

  std::optional<zx::duration> fallback_uptime;
  std::optional<zx::duration> fallback_runtime;

  if (!last_boot_uptime.has_value() && !last_boot_runtime.has_value()) {
    if (const std::optional<SystemTime> system_time =
            GetPreviousSystemTime(previous_system_time_path);
        system_time.has_value()) {
      last_boot_uptime = system_time->uptime;
      last_boot_runtime = system_time->runtime;
      fallback_uptime = system_time->uptime;
      fallback_runtime = system_time->runtime;
    }
  }

  const std::optional<GracefulShutdownInfo> graceful_info =
      ExtractGracefulShutdownInfo(graceful_shutdown_info_path, legacy_graceful_reboot_log_path);

  FinalShutdownInfo final_shutdown_info = FinalShutdownInfo::MakeFinalShutdownInfo(
      hw_reason, zircon_reason, graceful_info, not_a_fdr, supports_user_initiated_poweroffs,
      last_boot_uptime, last_boot_runtime, critical_process);
  const std::string reboot_log =
      MakeRebootLog(zircon_reboot_log, graceful_info, final_shutdown_info.ToRebootReasonString(),
                    fallback_uptime, fallback_runtime);

  if (first_component_instance) {
    const std::optional<std::string> dlog = ExtractDlogAndLogRebootLog(reboot_log);
    PersistDlog(dlog, redactor, previous_boot_kernel_log_path);

    if (!files::WriteFile(final_shutdown_info_path, final_shutdown_info.ToJson())) {
      FX_LOGS(ERROR) << "Failed to persist FinalShutdownInfo";
    }
  }

  return RebootLog(final_shutdown_info, reboot_log);
}

RebootLog::RebootLog(FinalShutdownInfo final_shutdown_info, std::string reboot_log_str)
    : final_shutdown_info_(std::move(final_shutdown_info)), reboot_log_str_(reboot_log_str) {}

}  // namespace feedback
}  // namespace forensics
