// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"

#include <lib/syslog/cpp/macros.h>

#include <unordered_set>
#include <utility>

#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"
#include "src/developer/forensics/feedback/reboot_log/hw_shutdown_reason.h"
#include "src/developer/forensics/feedback/reboot_log/zircon_shutdown_reason.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/developer/forensics/utils/time.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/error/en.h"
#include "third_party/rapidjson/include/rapidjson/prettywriter.h"
#include "third_party/rapidjson/include/rapidjson/stringbuffer.h"

namespace forensics::feedback {
namespace {

std::string GetSpontaneousRebootCrashSignature(
    const SpontaneousRebootReason spontaneous_reboot_reason) {
  switch (spontaneous_reboot_reason) {
    case SpontaneousRebootReason::kSpontaneous:
      return "fuchsia-spontaneous-reboot";
    case SpontaneousRebootReason::kBriefPowerLoss:
      return "fuchsia-brief-power-loss";
    case SpontaneousRebootReason::kHardReset:
      return "fuchsia-hard-reset";
  }
}

std::string GetSpontaneousRebootReason(const SpontaneousRebootReason spontaneous_reboot_reason) {
  switch (spontaneous_reboot_reason) {
    case SpontaneousRebootReason::kSpontaneous:
      return "spontaneous";
    case SpontaneousRebootReason::kBriefPowerLoss:
      return "brief loss of power";
    case SpontaneousRebootReason::kHardReset:
      return "hard reset";
  }
}

FinalShutdownReason FromRebootReasonString(const std::string& reason) {
  if (reason == "COLD") {
    return FinalShutdownReason::kCold;
  }
  if (reason == "KERNEL PANIC") {
    return FinalShutdownReason::kKernelPanic;
  }
  if (reason == "OOM") {
    return FinalShutdownReason::kOom;
  }
  if (reason == "HARDWARE WATCHDOG TIMEOUT") {
    return FinalShutdownReason::kHwWatchdog;
  }
  if (reason == "SOFTWARE WATCHDOG TIMEOUT") {
    return FinalShutdownReason::kSwWatchdog;
  }
  if (reason == "BROWNOUT") {
    return FinalShutdownReason::kBrownout;
  }
  if (reason == "SPONTANEOUS") {
    return FinalShutdownReason::kSpontaneousReboot;
  }
  if (reason == "ROOT JOB TERMINATION") {
    return FinalShutdownReason::kRootJobTermination;
  }
  if (reason == "USER HARD RESET") {
    return FinalShutdownReason::kUserHardReset;
  }
  if (reason == "GENERIC GRACEFUL") {
    return FinalShutdownReason::kGenericGraceful;
  }
  if (reason == "UNEXPECTED REASON GRACEFUL") {
    return FinalShutdownReason::kUnexpectedReasonGraceful;
  }
  if (reason == "USER REQUEST") {
    return FinalShutdownReason::kUserRequest;
  }
  if (reason == "SYSTEM UPDATE") {
    return FinalShutdownReason::kSystemUpdate;
  }
  if (reason == "RETRY SYSTEM UPDATE") {
    return FinalShutdownReason::kRetrySystemUpdate;
  }
  if (reason == "HIGH TEMPERATURE") {
    return FinalShutdownReason::kHighTemperature;
  }
  if (reason == "SESSION FAILURE") {
    return FinalShutdownReason::kSessionFailure;
  }
  if (reason == "SYSMGR FAILURE") {
    return FinalShutdownReason::kSysmgrFailure;
  }
  if (reason == "CRITICAL COMPONENT FAILURE") {
    return FinalShutdownReason::kCriticalComponentFailure;
  }
  if (reason == "FACTORY DATA RESET") {
    return FinalShutdownReason::kFdr;
  }
  if (reason == "ZBI SWAP") {
    return FinalShutdownReason::kZbiSwap;
  }
  if (reason == "NETSTACK MIGRATION") {
    return FinalShutdownReason::kNetstackMigration;
  }
  if (reason == "ANDROID UNEXPECTED REASON") {
    return FinalShutdownReason::kAndroidUnexpectedReason;
  }
  if (reason == "ANDROID NO REASON") {
    return FinalShutdownReason::kAndroidNoReason;
  }
  if (reason == "ANDROID RESCUE PARTY") {
    return FinalShutdownReason::kAndroidRescueParty;
  }
  if (reason == "ANDROID CRITICAL PROCESS FAILURE") {
    return FinalShutdownReason::kAndroidCriticalProcessFailure;
  }
  if (reason == "DEVELOPER REQUEST") {
    return FinalShutdownReason::kDeveloperRequest;
  }
  if (reason == "USER REQUEST DEVICE STUCK") {
    return FinalShutdownReason::kUserRequestDeviceStuck;
  }
  if (reason == "SUSPENSION FAILURE") {
    return FinalShutdownReason::kSuspensionFailure;
  }
  if (reason == "BATTERY DRAINED") {
    return FinalShutdownReason::kBatteryDrained;
  }
  if (reason == "CRITICAL DRIVER FAILURE") {
    return FinalShutdownReason::kCriticalDriverFailure;
  }

  return FinalShutdownReason::kNotParseable;
}

}  // namespace

FinalShutdownInfo::FinalShutdownInfo(FinalShutdownReason reason)
    : reason_(reason),
      graceful_shutdown_action_(std::nullopt),
      uptime_(std::nullopt),
      runtime_(std::nullopt),
      critical_process_(std::nullopt) {}

FinalShutdownInfo::FinalShutdownInfo(FinalShutdownReason reason,
                                     std::optional<GracefulShutdownAction> graceful_shutdown_action)
    : reason_(reason),
      graceful_shutdown_action_(graceful_shutdown_action),
      uptime_(std::nullopt),
      runtime_(std::nullopt),
      critical_process_(std::nullopt) {}

FinalShutdownInfo::FinalShutdownInfo(FinalShutdownReason reason,
                                     std::optional<GracefulShutdownAction> graceful_shutdown_action,
                                     std::optional<zx::duration> uptime,
                                     std::optional<zx::duration> runtime)
    : reason_(reason),
      graceful_shutdown_action_(graceful_shutdown_action),
      uptime_(uptime),
      runtime_(runtime),
      critical_process_(std::nullopt) {}

FinalShutdownInfo::FinalShutdownInfo(FinalShutdownReason reason, std::optional<zx::duration> uptime,
                                     std::optional<zx::duration> runtime)
    : reason_(reason),
      graceful_shutdown_action_(std::nullopt),
      uptime_(uptime),
      runtime_(runtime),
      critical_process_(std::nullopt) {}

FinalShutdownInfo::FinalShutdownInfo(FinalShutdownReason reason, std::optional<zx::duration> uptime,
                                     std::optional<zx::duration> runtime,
                                     std::optional<std::string> critical_process)
    : reason_(reason),
      graceful_shutdown_action_(std::nullopt),
      uptime_(uptime),
      runtime_(runtime),
      critical_process_(std::move(critical_process)) {
  if (critical_process_.has_value() && reason_ != FinalShutdownReason::kRootJobTermination) {
    FX_LOGS(ERROR) << "Critical process provided for an invalid reason: " << ToRebootReasonString();
  }
}

bool FinalShutdownInfo::IsOom() const { return reason_ == FinalShutdownReason::kOom; }

bool FinalShutdownInfo::IsCrash() const {
  switch (reason_) {
    case FinalShutdownReason::kKernelPanic:
    case FinalShutdownReason::kOom:
    case FinalShutdownReason::kHwWatchdog:
    case FinalShutdownReason::kSwWatchdog:
    case FinalShutdownReason::kBrownout:
    case FinalShutdownReason::kSpontaneousReboot:
    case FinalShutdownReason::kRootJobTermination:
    case FinalShutdownReason::kUserHardReset:
    case FinalShutdownReason::kNotParseable:
    case FinalShutdownReason::kGenericGraceful:
    case FinalShutdownReason::kUnexpectedReasonGraceful:
    case FinalShutdownReason::kRetrySystemUpdate:
    case FinalShutdownReason::kHighTemperature:
    case FinalShutdownReason::kSessionFailure:
    case FinalShutdownReason::kSysmgrFailure:
    case FinalShutdownReason::kCriticalComponentFailure:
    case FinalShutdownReason::kCriticalDriverFailure:
    case FinalShutdownReason::kAndroidUnexpectedReason:
    case FinalShutdownReason::kAndroidRescueParty:
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
    case FinalShutdownReason::kUserRequestDeviceStuck:
    case FinalShutdownReason::kSuspensionFailure:
      return true;
    case FinalShutdownReason::kCold:
    case FinalShutdownReason::kUserRequest:
    case FinalShutdownReason::kSystemUpdate:
    case FinalShutdownReason::kFdr:
    case FinalShutdownReason::kZbiSwap:
    case FinalShutdownReason::kNetstackMigration:
    case FinalShutdownReason::kDeveloperRequest:
    case FinalShutdownReason::kAndroidNoReason:
    case FinalShutdownReason::kBatteryDrained:
      return false;
  }
}

std::optional<bool> FinalShutdownInfo::OptionallyGraceful() const {
  switch (reason_) {
    case FinalShutdownReason::kCold:
    case FinalShutdownReason::kKernelPanic:
    case FinalShutdownReason::kOom:
    case FinalShutdownReason::kHwWatchdog:
    case FinalShutdownReason::kSwWatchdog:
    case FinalShutdownReason::kBrownout:
    case FinalShutdownReason::kSpontaneousReboot:
    case FinalShutdownReason::kRootJobTermination:
    case FinalShutdownReason::kUserHardReset:
      return false;
    case FinalShutdownReason::kNotParseable:
      return std::nullopt;
    case FinalShutdownReason::kGenericGraceful:
    case FinalShutdownReason::kUnexpectedReasonGraceful:
    case FinalShutdownReason::kUserRequest:
    case FinalShutdownReason::kSystemUpdate:
    case FinalShutdownReason::kRetrySystemUpdate:
    case FinalShutdownReason::kHighTemperature:
    case FinalShutdownReason::kSessionFailure:
    case FinalShutdownReason::kSysmgrFailure:
    case FinalShutdownReason::kCriticalComponentFailure:
    case FinalShutdownReason::kCriticalDriverFailure:
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
      return true;
  }
}

std::optional<bool> FinalShutdownInfo::OptionallyPlanned() const {
  switch (reason_) {
    case FinalShutdownReason::kCold:
    case FinalShutdownReason::kKernelPanic:
    case FinalShutdownReason::kOom:
    case FinalShutdownReason::kHwWatchdog:
    case FinalShutdownReason::kSwWatchdog:
    case FinalShutdownReason::kBrownout:
    case FinalShutdownReason::kSpontaneousReboot:
    case FinalShutdownReason::kRootJobTermination:
    case FinalShutdownReason::kUserHardReset:
    case FinalShutdownReason::kGenericGraceful:
    case FinalShutdownReason::kUnexpectedReasonGraceful:
    case FinalShutdownReason::kUserRequest:
    case FinalShutdownReason::kRetrySystemUpdate:
    case FinalShutdownReason::kHighTemperature:
    case FinalShutdownReason::kSessionFailure:
    case FinalShutdownReason::kSysmgrFailure:
    case FinalShutdownReason::kCriticalComponentFailure:
    case FinalShutdownReason::kCriticalDriverFailure:
    case FinalShutdownReason::kFdr:
    case FinalShutdownReason::kZbiSwap:
    case FinalShutdownReason::kAndroidUnexpectedReason:
    case FinalShutdownReason::kAndroidNoReason:
    case FinalShutdownReason::kAndroidRescueParty:
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
    case FinalShutdownReason::kDeveloperRequest:
    case FinalShutdownReason::kUserRequestDeviceStuck:
    case FinalShutdownReason::kSuspensionFailure:
    case FinalShutdownReason::kBatteryDrained:
      return false;
    case FinalShutdownReason::kNotParseable:
      return std::nullopt;
    case FinalShutdownReason::kSystemUpdate:
    case FinalShutdownReason::kNetstackMigration:
      return true;
  }
}

std::optional<GracefulShutdownAction> FinalShutdownInfo::ToGracefulShutdownAction() const {
  return graceful_shutdown_action_;
}

std::string FinalShutdownInfo::ToRebootReasonString() const {
  switch (reason_) {
    case FinalShutdownReason::kCold:
      return "COLD";
    case FinalShutdownReason::kKernelPanic:
      return "KERNEL PANIC";
    case FinalShutdownReason::kOom:
      return "OOM";
    case FinalShutdownReason::kHwWatchdog:
      return "HARDWARE WATCHDOG TIMEOUT";
    case FinalShutdownReason::kSwWatchdog:
      return "SOFTWARE WATCHDOG TIMEOUT";
    case FinalShutdownReason::kBrownout:
      return "BROWNOUT";
    case FinalShutdownReason::kSpontaneousReboot:
      return "SPONTANEOUS";
    case FinalShutdownReason::kRootJobTermination:
      return "ROOT JOB TERMINATION";
    case FinalShutdownReason::kUserHardReset:
      return "USER HARD RESET";
    case FinalShutdownReason::kNotParseable:
      return "NOT PARSEABLE";
    case FinalShutdownReason::kGenericGraceful:
      return "GENERIC GRACEFUL";
    case FinalShutdownReason::kUnexpectedReasonGraceful:
      return "UNEXPECTED REASON GRACEFUL";
    case FinalShutdownReason::kUserRequest:
      return "USER REQUEST";
    case FinalShutdownReason::kSystemUpdate:
      return "SYSTEM UPDATE";
    case FinalShutdownReason::kRetrySystemUpdate:
      return "RETRY SYSTEM UPDATE";
    case FinalShutdownReason::kHighTemperature:
      return "HIGH TEMPERATURE";
    case FinalShutdownReason::kSessionFailure:
      return "SESSION FAILURE";
    case FinalShutdownReason::kSysmgrFailure:
      return "SYSMGR FAILURE";
    case FinalShutdownReason::kCriticalComponentFailure:
      return "CRITICAL COMPONENT FAILURE";
    case FinalShutdownReason::kFdr:
      return "FACTORY DATA RESET";
    case FinalShutdownReason::kZbiSwap:
      return "ZBI SWAP";
    case FinalShutdownReason::kNetstackMigration:
      return "NETSTACK MIGRATION";
    case FinalShutdownReason::kAndroidUnexpectedReason:
      return "ANDROID UNEXPECTED REASON";
    case FinalShutdownReason::kAndroidNoReason:
      return "ANDROID NO REASON";
    case FinalShutdownReason::kAndroidRescueParty:
      return "ANDROID RESCUE PARTY";
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
      return "ANDROID CRITICAL PROCESS FAILURE";
    case FinalShutdownReason::kDeveloperRequest:
      return "DEVELOPER REQUEST";
    case FinalShutdownReason::kUserRequestDeviceStuck:
      return "USER REQUEST DEVICE STUCK";
    case FinalShutdownReason::kSuspensionFailure:
      return "SUSPENSION FAILURE";
    case FinalShutdownReason::kBatteryDrained:
      return "BATTERY DRAINED";
    case FinalShutdownReason::kCriticalDriverFailure:
      return "CRITICAL DRIVER FAILURE";
  }
}

std::optional<fuchsia::feedback::RebootReason> FinalShutdownInfo::ToFidlRebootReason() const {
  switch (reason_) {
    case FinalShutdownReason::kCold:
      return fuchsia::feedback::RebootReason::COLD;
    case FinalShutdownReason::kKernelPanic:
      return fuchsia::feedback::RebootReason::KERNEL_PANIC;
    case FinalShutdownReason::kOom:
      return fuchsia::feedback::RebootReason::SYSTEM_OUT_OF_MEMORY;
    case FinalShutdownReason::kHwWatchdog:
      return fuchsia::feedback::RebootReason::HARDWARE_WATCHDOG_TIMEOUT;
    case FinalShutdownReason::kSwWatchdog:
      return fuchsia::feedback::RebootReason::SOFTWARE_WATCHDOG_TIMEOUT;
    case FinalShutdownReason::kBrownout:
      return fuchsia::feedback::RebootReason::BROWNOUT;
    case FinalShutdownReason::kSpontaneousReboot:
      return fuchsia::feedback::RebootReason::BRIEF_POWER_LOSS;
    case FinalShutdownReason::kRootJobTermination:
      return fuchsia::feedback::RebootReason::ROOT_JOB_TERMINATION;
    case FinalShutdownReason::kUserHardReset:
      return fuchsia::feedback::RebootReason::USER_HARD_RESET;
    case FinalShutdownReason::kNotParseable:
    case FinalShutdownReason::kGenericGraceful:
    case FinalShutdownReason::kUnexpectedReasonGraceful:
      return std::nullopt;
    case FinalShutdownReason::kSuspensionFailure:
      return fuchsia::feedback::RebootReason::SUSPENSION_FAILURE;
    case FinalShutdownReason::kUserRequest:
      return fuchsia::feedback::RebootReason::USER_REQUEST;
    case FinalShutdownReason::kSystemUpdate:
      return fuchsia::feedback::RebootReason::SYSTEM_UPDATE;
    case FinalShutdownReason::kRetrySystemUpdate:
      return fuchsia::feedback::RebootReason::RETRY_SYSTEM_UPDATE;
    case FinalShutdownReason::kHighTemperature:
      return fuchsia::feedback::RebootReason::HIGH_TEMPERATURE;
    case FinalShutdownReason::kSessionFailure:
      return fuchsia::feedback::RebootReason::SESSION_FAILURE;
    case FinalShutdownReason::kSysmgrFailure:
      return fuchsia::feedback::RebootReason::SYSMGR_FAILURE;
    case FinalShutdownReason::kCriticalComponentFailure:
      return fuchsia::feedback::RebootReason::CRITICAL_COMPONENT_FAILURE;
    case FinalShutdownReason::kCriticalDriverFailure:
      return fuchsia::feedback::RebootReason::CRITICAL_DRIVER_FAILURE;
    case FinalShutdownReason::kFdr:
      return fuchsia::feedback::RebootReason::FACTORY_DATA_RESET;
    case FinalShutdownReason::kZbiSwap:
      return fuchsia::feedback::RebootReason::ZBI_SWAP;
    case FinalShutdownReason::kNetstackMigration:
      return fuchsia::feedback::RebootReason::NETSTACK_MIGRATION;
    case FinalShutdownReason::kAndroidUnexpectedReason:
      return fuchsia::feedback::RebootReason::ANDROID_UNEXPECTED_REASON;
    case FinalShutdownReason::kAndroidNoReason:
      return fuchsia::feedback::RebootReason::ANDROID_NO_REASON;
    case FinalShutdownReason::kAndroidRescueParty:
      return fuchsia::feedback::RebootReason::ANDROID_RESCUE_PARTY;
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
      return fuchsia::feedback::RebootReason::ANDROID_CRITICAL_PROCESS_FAILURE;
    case FinalShutdownReason::kDeveloperRequest:
      return fuchsia::feedback::RebootReason::DEVELOPER_REQUEST;
    case FinalShutdownReason::kUserRequestDeviceStuck:
      return fuchsia::feedback::RebootReason::USER_REQUEST_DEVICE_STUCK;
    case FinalShutdownReason::kBatteryDrained:
      return fuchsia::feedback::RebootReason::BATTERY_DRAINED;
  }
}

cobalt::LastRebootReason FinalShutdownInfo::ToCobaltLastRebootReason() const {
  switch (reason_) {
    case FinalShutdownReason::kCold:
      return cobalt::LastRebootReason::kCold;
    case FinalShutdownReason::kKernelPanic:
      return cobalt::LastRebootReason::kKernelPanic;
    case FinalShutdownReason::kOom:
      return cobalt::LastRebootReason::kSystemOutOfMemory;
    case FinalShutdownReason::kHwWatchdog:
      return cobalt::LastRebootReason::kHardwareWatchdogTimeout;
    case FinalShutdownReason::kSwWatchdog:
      return cobalt::LastRebootReason::kSoftwareWatchdogTimeout;
    case FinalShutdownReason::kBrownout:
      return cobalt::LastRebootReason::kBrownout;
    case FinalShutdownReason::kSpontaneousReboot:
      return cobalt::LastRebootReason::kBriefPowerLoss;
    case FinalShutdownReason::kRootJobTermination:
      return cobalt::LastRebootReason::kRootJobTermination;
    case FinalShutdownReason::kUserHardReset:
      return cobalt::LastRebootReason::kUserHardReset;
    case FinalShutdownReason::kNotParseable:
      return cobalt::LastRebootReason::kUnknown;
    case FinalShutdownReason::kGenericGraceful:
      return cobalt::LastRebootReason::kGenericGraceful;
    case FinalShutdownReason::kUnexpectedReasonGraceful:
      return cobalt::LastRebootReason::kUnexpectedReasonGraceful;
    case FinalShutdownReason::kUserRequest:
      return cobalt::LastRebootReason::kUserRequest;
    case FinalShutdownReason::kSystemUpdate:
      return cobalt::LastRebootReason::kSystemUpdate;
    case FinalShutdownReason::kRetrySystemUpdate:
      return cobalt::LastRebootReason::kRetrySystemUpdate;
    case FinalShutdownReason::kHighTemperature:
      return cobalt::LastRebootReason::kHighTemperature;
    case FinalShutdownReason::kSessionFailure:
      return cobalt::LastRebootReason::kSessionFailure;
    case FinalShutdownReason::kSysmgrFailure:
      return cobalt::LastRebootReason::kSysmgrFailure;
    case FinalShutdownReason::kCriticalComponentFailure:
      return cobalt::LastRebootReason::kCriticalComponentFailure;
    case FinalShutdownReason::kCriticalDriverFailure:
      return cobalt::LastRebootReason::kCriticalDriverFailure;
    case FinalShutdownReason::kFdr:
      return cobalt::LastRebootReason::kFactoryDataReset;
    case FinalShutdownReason::kZbiSwap:
      return cobalt::LastRebootReason::kZbiSwap;
    case FinalShutdownReason::kNetstackMigration:
      return cobalt::LastRebootReason::kNetstackMigration;
    case FinalShutdownReason::kAndroidUnexpectedReason:
      return cobalt::LastRebootReason::kAndroidUnexpectedReason;
    case FinalShutdownReason::kAndroidNoReason:
      return cobalt::LastRebootReason::kAndroidNoReason;
    case FinalShutdownReason::kAndroidRescueParty:
      return cobalt::LastRebootReason::kAndroidRescueParty;
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
      return cobalt::LastRebootReason::kAndroidCriticalProcessFailure;
    case FinalShutdownReason::kDeveloperRequest:
      return cobalt::LastRebootReason::kDeveloperRequest;
    case FinalShutdownReason::kUserRequestDeviceStuck:
      return cobalt::LastRebootReason::kUserRequestDeviceStuck;
    case FinalShutdownReason::kSuspensionFailure:
      return cobalt::LastRebootReason::kSuspensionFailure;
    case FinalShutdownReason::kBatteryDrained:
      return cobalt::LastRebootReason::kBatteryDrained;
  }
}

std::string FinalShutdownInfo::ToCrashProgramName() const {
  switch (reason_) {
    case FinalShutdownReason::kNotParseable:
      return "reboot-log";
    case FinalShutdownReason::kKernelPanic:
      return "kernel";
    case FinalShutdownReason::kHwWatchdog:
    case FinalShutdownReason::kBrownout:
    case FinalShutdownReason::kSpontaneousReboot:
    case FinalShutdownReason::kUserHardReset:
      return "device";
    case FinalShutdownReason::kOom:
    case FinalShutdownReason::kSwWatchdog:
    case FinalShutdownReason::kRootJobTermination:
    case FinalShutdownReason::kGenericGraceful:
    case FinalShutdownReason::kUnexpectedReasonGraceful:
    case FinalShutdownReason::kRetrySystemUpdate:
    case FinalShutdownReason::kHighTemperature:
    case FinalShutdownReason::kSessionFailure:
    case FinalShutdownReason::kSysmgrFailure:
    case FinalShutdownReason::kCriticalComponentFailure:
    case FinalShutdownReason::kCriticalDriverFailure:
    case FinalShutdownReason::kUserRequestDeviceStuck:
    case FinalShutdownReason::kSuspensionFailure:
      return "system";
    case FinalShutdownReason::kAndroidUnexpectedReason:
    case FinalShutdownReason::kAndroidNoReason:
    case FinalShutdownReason::kAndroidRescueParty:
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
      return "android";
    case FinalShutdownReason::kCold:
    case FinalShutdownReason::kUserRequest:
    case FinalShutdownReason::kDeveloperRequest:
    case FinalShutdownReason::kSystemUpdate:
    case FinalShutdownReason::kNetstackMigration:
    case FinalShutdownReason::kZbiSwap:
    case FinalShutdownReason::kFdr:
    case FinalShutdownReason::kBatteryDrained:
      FX_LOGS(FATAL) << "Not expecting a program name request for reboot reason: "
                     << ToRebootReasonString();
      return "FATAL ERROR";
  }
}

std::string FinalShutdownInfo::ToCrashSignature(
    const SpontaneousRebootReason spontaneous_reboot_reason) const {
  switch (reason_) {
    case FinalShutdownReason::kNotParseable:
      return "fuchsia-reboot-log-not-parseable";
    case FinalShutdownReason::kSpontaneousReboot:
      return GetSpontaneousRebootCrashSignature(spontaneous_reboot_reason);
    case FinalShutdownReason::kKernelPanic:
      return "fuchsia-kernel-panic";
    case FinalShutdownReason::kOom:
      return "fuchsia-oom";
    case FinalShutdownReason::kHwWatchdog:
      return "fuchsia-hw-watchdog-timeout";
    case FinalShutdownReason::kSwWatchdog:
      return "fuchsia-sw-watchdog-timeout";
    case FinalShutdownReason::kBrownout:
      return "fuchsia-brownout";
    case FinalShutdownReason::kRootJobTermination:
      return (!critical_process_.has_value())
                 ? "fuchsia-root-job-termination"
                 : std::string("fuchsia-reboot-").append(*critical_process_).append("-terminated");
    case FinalShutdownReason::kUserHardReset:
      return "fuchsia-hard-reset-user-requested";
    case FinalShutdownReason::kRetrySystemUpdate:
      return "fuchsia-retry-system-update";
    case FinalShutdownReason::kHighTemperature:
      return "fuchsia-shutdown-high-temperature";
    case FinalShutdownReason::kSessionFailure:
      return "fuchsia-session-failure";
    case FinalShutdownReason::kSysmgrFailure:
      return "fuchsia-sysmgr-failure";
    case FinalShutdownReason::kCriticalComponentFailure:
      return "fuchsia-critical-component-failure";
    case FinalShutdownReason::kCriticalDriverFailure:
      return "fuchsia-critical-driver-failure";
    case FinalShutdownReason::kAndroidUnexpectedReason:
      return "fuchsia-shutdown-android-unexpected-reason";
    case FinalShutdownReason::kAndroidRescueParty:
      return "fuchsia-shutdown-android-rescue-party";
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
      return "fuchsia-shutdown-android-critical-process-failure";
    case FinalShutdownReason::kUserRequestDeviceStuck:
      return "fuchsia-shutdown-user-request-device-stuck";
    case FinalShutdownReason::kSuspensionFailure:
      return "fuchsia-shutdown-suspension-failure";
    case FinalShutdownReason::kGenericGraceful:
      return "fuchsia-shutdown-undetermined-userspace-reason";
    case FinalShutdownReason::kUnexpectedReasonGraceful:
      return "fuchsia-shutdown-unexpected-userspace-reason";
    case FinalShutdownReason::kCold:
    case FinalShutdownReason::kUserRequest:
    case FinalShutdownReason::kSystemUpdate:
    case FinalShutdownReason::kFdr:
    case FinalShutdownReason::kZbiSwap:
    case FinalShutdownReason::kNetstackMigration:
    case FinalShutdownReason::kDeveloperRequest:
    case FinalShutdownReason::kAndroidNoReason:
    case FinalShutdownReason::kBatteryDrained:
      FX_LOGS(FATAL) << "Not expecting a crash for reason: " << ToRebootReasonString();
      return "FATAL ERROR";
  }
}

namespace {

FinalShutdownReason ConsolidateGracefulShutdownReasons(
    const std::vector<GracefulShutdownReason>& reasons) {
  if (reasons.empty()) {
    return FinalShutdownReason::kGenericGraceful;
  }

  // If there's only one reason, consolidation is trivial.
  if (reasons.size() == 1) {
    switch (reasons[0]) {
      case GracefulShutdownReason::kUserRequest:
        return FinalShutdownReason::kUserRequest;
      case GracefulShutdownReason::kSystemUpdate:
        return FinalShutdownReason::kSystemUpdate;
      case GracefulShutdownReason::kRetrySystemUpdate:
        return FinalShutdownReason::kRetrySystemUpdate;
      case GracefulShutdownReason::kHighTemperature:
        return FinalShutdownReason::kHighTemperature;
      case GracefulShutdownReason::kSessionFailure:
        return FinalShutdownReason::kSessionFailure;
      case GracefulShutdownReason::kSysmgrFailure:
        return FinalShutdownReason::kSysmgrFailure;
      case GracefulShutdownReason::kCriticalComponentFailure:
        return FinalShutdownReason::kCriticalComponentFailure;
      case GracefulShutdownReason::kFdr:
        return FinalShutdownReason::kFdr;
      case GracefulShutdownReason::kZbiSwap:
        return FinalShutdownReason::kZbiSwap;
      case GracefulShutdownReason::kNotSupported:
      case GracefulShutdownReason::kNotParseable:
        return FinalShutdownReason::kGenericGraceful;
      case GracefulShutdownReason::kOutOfMemory:
        return FinalShutdownReason::kOom;
      case GracefulShutdownReason::kNetstackMigration:
        return FinalShutdownReason::kNetstackMigration;
      case GracefulShutdownReason::kAndroidUnexpectedReason:
        return FinalShutdownReason::kAndroidUnexpectedReason;
      case GracefulShutdownReason::kAndroidNoReason:
        return FinalShutdownReason::kAndroidNoReason;
      case GracefulShutdownReason::kAndroidRescueParty:
        return FinalShutdownReason::kAndroidRescueParty;
      case GracefulShutdownReason::kAndroidCriticalProcessFailure:
        return FinalShutdownReason::kAndroidCriticalProcessFailure;
      case GracefulShutdownReason::kDeveloperRequest:
        return FinalShutdownReason::kDeveloperRequest;
      case GracefulShutdownReason::kUserRequestDeviceStuck:
        return FinalShutdownReason::kUserRequestDeviceStuck;
      case GracefulShutdownReason::kSuspensionFailure:
        return FinalShutdownReason::kSuspensionFailure;
      case GracefulShutdownReason::kBatteryDrained:
        return FinalShutdownReason::kBatteryDrained;
      case GracefulShutdownReason::kCriticalDriverFailure:
        return FinalShutdownReason::kCriticalDriverFailure;
      case GracefulShutdownReason::kNotSet:
        FX_LOGS(FATAL) << "Graceful shutdown reason must be set";
        return FinalShutdownReason::kUnexpectedReasonGraceful;
    }
  }

  // Otherwise, verify it's an expected combination of reasons.
  std::unordered_set<GracefulShutdownReason> reasons_set(reasons.begin(), reasons.end());
  if (reasons_set.size() == 2 && reasons_set.contains(GracefulShutdownReason::kNetstackMigration) &&
      reasons_set.contains(GracefulShutdownReason::kSystemUpdate)) {
    // Netstack Migration + System Update is consolidated to System Update.
    return FinalShutdownReason::kSystemUpdate;
  }

  FX_LOGS(WARNING) << "Unexpected combination of graceful shutdown reasons: "
                   << ToRawStrings(reasons);
  return FinalShutdownReason::kUnexpectedReasonGraceful;
}

}  // namespace

// static
FinalShutdownInfo FinalShutdownInfo::MakeFinalShutdownInfo(
    const HwShutdownReason hw_reason, const ZirconShutdownReason zircon_reason,
    std::optional<GracefulShutdownInfo> graceful_shutdown_info, const bool not_a_fdr,
    bool supports_user_initiated_poweroffs, std::optional<zx::duration> uptime,
    std::optional<zx::duration> runtime, const std::optional<std::string>& critical_process) {
  switch (hw_reason) {
    case HwShutdownReason::kNotParseable:
      return FinalShutdownInfo(FinalShutdownReason::kNotParseable, uptime, runtime);
    case HwShutdownReason::kWatchdog:
      return FinalShutdownInfo(FinalShutdownReason::kHwWatchdog, uptime, runtime);
    case HwShutdownReason::kBrownout:
      return FinalShutdownInfo(FinalShutdownReason::kBrownout, uptime, runtime);
    case HwShutdownReason::kUserHardReset:
      return FinalShutdownInfo(FinalShutdownReason::kUserHardReset, uptime, runtime);
    case HwShutdownReason::kNotSet:
    case HwShutdownReason::kUndefined:
    case HwShutdownReason::kCold:
    case HwShutdownReason::kWarm:
      break;
  }

  switch (zircon_reason) {
    case ZirconShutdownReason::kKernelPanic:
      return FinalShutdownInfo(FinalShutdownReason::kKernelPanic, uptime, runtime);
    case ZirconShutdownReason::kOOM:
      return FinalShutdownInfo(FinalShutdownReason::kOom, uptime, runtime);
    case ZirconShutdownReason::kSwWatchdog:
      return FinalShutdownInfo(FinalShutdownReason::kSwWatchdog, uptime, runtime);
    case ZirconShutdownReason::kUnknown:
      return FinalShutdownInfo(FinalShutdownReason::kSpontaneousReboot, uptime, runtime);
    case ZirconShutdownReason::kNotParseable:
      return FinalShutdownInfo(FinalShutdownReason::kNotParseable, uptime, runtime);
    case ZirconShutdownReason::kRootJobTermination:
      return FinalShutdownInfo(FinalShutdownReason::kRootJobTermination, uptime, runtime,
                               critical_process);
    case ZirconShutdownReason::kNoCrash: {
      if (!not_a_fdr) {
        return FinalShutdownInfo(FinalShutdownReason::kFdr, uptime, runtime);
      }
      if (graceful_shutdown_info.has_value()) {
        return FinalShutdownInfo(
            ConsolidateGracefulShutdownReasons(graceful_shutdown_info->reasons),
            graceful_shutdown_info->action, uptime, runtime);
      }
      return FinalShutdownInfo(FinalShutdownReason::kGenericGraceful, uptime, runtime);
    }
    case ZirconShutdownReason::kNotSet:
      break;
  }

  if (!not_a_fdr) {
    return FinalShutdownInfo(FinalShutdownReason::kFdr, uptime, runtime);
  }

  // If there is no graceful shutdown info, it means the device was abruptly shut down.
  if (!graceful_shutdown_info.has_value()) {
    // Now, it's either a spontaneous reboot or a cold boot depending on whether the user
    // can initiate a poweroff. E.g., if there no power button and they have to yank the cable
    // off a wall-powered device, it's likely more a cold boot.
    if (supports_user_initiated_poweroffs) {
      return FinalShutdownInfo(FinalShutdownReason::kSpontaneousReboot, uptime, runtime);
    } else {
      return FinalShutdownInfo(FinalShutdownReason::kCold, uptime, runtime);
    }
  }

  // Graceful poweroffs will likely result in a cold boot. If so, the graceful info might have
  // reasons more informative than just "cold."
  if (graceful_shutdown_info->action == GracefulShutdownAction::kPoweroff) {
    return FinalShutdownInfo(ConsolidateGracefulShutdownReasons(graceful_shutdown_info->reasons),
                             graceful_shutdown_info->action, uptime, runtime);
  }

  // While we now distinguish HwShutdownReason being cold, warm, undefined and not set,
  // for now we still report all of them as cold boots.
  return FinalShutdownInfo(FinalShutdownReason::kCold,
                           std::make_optional(graceful_shutdown_info->action), uptime, runtime);
}

std::string FinalShutdownInfo::ToSnapshotAnnotationReason(
    const SpontaneousRebootReason spontaneous_reboot_reason) const {
  switch (reason_) {
    case FinalShutdownReason::kCold:
      return "cold";
    case FinalShutdownReason::kSpontaneousReboot:
      return GetSpontaneousRebootReason(spontaneous_reboot_reason);
    case FinalShutdownReason::kBrownout:
      return "brownout";
    case FinalShutdownReason::kKernelPanic:
      return "kernel panic";
    case FinalShutdownReason::kOom:
      return "system out of memory";
    case FinalShutdownReason::kHwWatchdog:
      return "hardware watchdog timeout";
    case FinalShutdownReason::kSwWatchdog:
      return "software watchdog timeout";
    case FinalShutdownReason::kUserRequest:
      return "user request";
    case FinalShutdownReason::kUserRequestDeviceStuck:
      return "user request device stuck";
    case FinalShutdownReason::kSuspensionFailure:
      return "suspension failure";
    case FinalShutdownReason::kUserHardReset:
      return "user request hard reset";
    case FinalShutdownReason::kSystemUpdate:
      return "system update";
    case FinalShutdownReason::kRetrySystemUpdate:
      return "retry system update";
    case FinalShutdownReason::kZbiSwap:
      return "ZBI swap";
    case FinalShutdownReason::kHighTemperature:
      return "device too hot";
    case FinalShutdownReason::kSessionFailure:
      return "fatal session failure";
    case FinalShutdownReason::kSysmgrFailure:
      return "fatal sysmgr failure";
    case FinalShutdownReason::kCriticalComponentFailure:
      return "fatal critical component failure";
    case FinalShutdownReason::kCriticalDriverFailure:
      return "fatal critical driver failure";
    case FinalShutdownReason::kFdr:
      return "factory data reset";
    case FinalShutdownReason::kRootJobTermination:
      return "root job termination";
    case FinalShutdownReason::kNetstackMigration:
      return "netstack migration";
    case FinalShutdownReason::kAndroidUnexpectedReason:
      return "android unexpected reason";
    case FinalShutdownReason::kAndroidNoReason:
      return "android no reason";
    case FinalShutdownReason::kAndroidRescueParty:
      return "android rescue party";
    case FinalShutdownReason::kAndroidCriticalProcessFailure:
      return "android critical process failure";
    case FinalShutdownReason::kDeveloperRequest:
      return "developer request";
    case FinalShutdownReason::kBatteryDrained:
      return "battery drained";
    case FinalShutdownReason::kNotParseable:
      return "not parseable";
    case FinalShutdownReason::kGenericGraceful:
    case FinalShutdownReason::kUnexpectedReasonGraceful:
      return "graceful";
  }
}

ErrorOrString FinalShutdownInfo::ToSnapshotAnnotationUptime() const {
  if (Uptime().has_value()) {
    const std::optional<std::string> uptime = FormatDuration(*Uptime());
    if (uptime.has_value()) {
      return ErrorOrString(*uptime);
    }
  }

  return ErrorOrString(Error::kMissingValue);
}

ErrorOrString FinalShutdownInfo::ToSnapshotAnnotationRuntime() const {
  if (Runtime().has_value()) {
    const std::optional<std::string> runtime = FormatDuration(*Runtime());
    if (runtime.has_value()) {
      return ErrorOrString(*runtime);
    }
  }

  return ErrorOrString(Error::kMissingValue);
}

ErrorOrString FinalShutdownInfo::ToSnapshotAnnotationTotalSuspendedTime() const {
  if (Uptime().has_value() && Runtime().has_value()) {
    const std::optional<std::string> suspended_time = FormatDuration(*Uptime() - *Runtime());
    if (suspended_time.has_value()) {
      return ErrorOrString(*suspended_time);
    }
  }

  return ErrorOrString(Error::kMissingValue);
}

ErrorOrString FinalShutdownInfo::ToSnapshotAnnotationGracefulAction() const {
  const std::optional<GracefulShutdownAction> action = ToGracefulShutdownAction();
  if (!action.has_value()) {
    return ErrorOrString(Error::kMissingValue);
  }

  switch (*action) {
    case GracefulShutdownAction::kPoweroff:
      return ErrorOrString("poweroff");
    case GracefulShutdownAction::kReboot:
      return ErrorOrString("reboot");
    case GracefulShutdownAction::kRebootToRecovery:
      return ErrorOrString("reboot to recovery");
    case GracefulShutdownAction::kRebootToBootloader:
      return ErrorOrString("reboot to bootloader");
    case GracefulShutdownAction::kNotSupported:
    case GracefulShutdownAction::kNotParseable:
      return ErrorOrString(Error::kBadValue);
  }
}

std::string FinalShutdownInfo::ToJson() const {
  rapidjson::Document json;
  json.SetObject();
  auto& allocator = json.GetAllocator();

  json.AddMember("reason", ToRebootReasonString(), allocator);

  if (graceful_shutdown_action_.has_value()) {
    json.AddMember("graceful_action", ToString(*graceful_shutdown_action_), allocator);
  }

  if (uptime_.has_value()) {
    json.AddMember("uptime_ms", uptime_->to_msecs(), allocator);
  }

  if (runtime_.has_value()) {
    json.AddMember("runtime_ms", runtime_->to_msecs(), allocator);
  }

  if (critical_process_.has_value()) {
    json.AddMember("critical_process", *critical_process_, allocator);
  }

  rapidjson::StringBuffer buffer;
  rapidjson::PrettyWriter<rapidjson::StringBuffer> writer(buffer);
  json.Accept(writer);

  return buffer.GetString();
}

FinalShutdownInfo FinalShutdownInfo::FromJson(const std::string& content) {
  rapidjson::Document json;
  if (const rapidjson::ParseResult result = json.Parse(content.c_str()); !result) {
    FX_LOGS(ERROR) << "Error parsing final shutdown info as JSON at offset " << result.Offset()
                   << ", " << rapidjson::GetParseError_En(result.Code()) << ". Content: \n"
                   << content;
    return FinalShutdownInfo(FinalShutdownReason::kNotParseable);
  }

  if (!json.HasMember("reason") || !json["reason"].IsString()) {
    FX_LOGS(ERROR) << "Failed to parse reason string from persisted final shutdown info json";
    return FinalShutdownInfo(FinalShutdownReason::kNotParseable);
  }

  const FinalShutdownReason reason = FromRebootReasonString(json["reason"].GetString());

  const std::optional<GracefulShutdownAction> graceful_action =
      json.HasMember("graceful_action") && json["graceful_action"].IsString()
          ? std::make_optional(
                FromGracefulShutdownActionString(json["graceful_action"].GetString()))
          : std::nullopt;

  const std::optional<zx::duration> uptime =
      json.HasMember("uptime_ms") && json["uptime_ms"].IsInt64()
          ? std::make_optional(zx::msec(json["uptime_ms"].GetInt64()))
          : std::nullopt;

  const std::optional<zx::duration> runtime =
      json.HasMember("runtime_ms") && json["runtime_ms"].IsInt64()
          ? std::make_optional(zx::msec(json["runtime_ms"].GetInt64()))
          : std::nullopt;

  const std::optional<std::string> critical_process =
      json.HasMember("critical_process") && json["critical_process"].IsString()
          ? std::make_optional(json["critical_process"].GetString())
          : std::nullopt;

  if (critical_process.has_value() && graceful_action.has_value()) {
    FX_LOGS(ERROR)
        << "Persisted final shutdown info json has both a critical process and graceful action";
    return FinalShutdownInfo(FinalShutdownReason::kNotParseable);
  }

  if (critical_process.has_value()) {
    return FinalShutdownInfo(reason, uptime, runtime, critical_process);
  }

  if (graceful_action.has_value()) {
    return FinalShutdownInfo(reason, graceful_action, uptime, runtime);
  }

  return FinalShutdownInfo(reason, uptime, runtime);
}

}  // namespace forensics::feedback
