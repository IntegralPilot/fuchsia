// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_GRACEFUL_SHUTDOWN_INFO_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_GRACEFUL_SHUTDOWN_INFO_H_

#include <fuchsia/hardware/power/statecontrol/cpp/fidl.h>

#include <string>

namespace forensics {
namespace feedback {

// Feedback's internal representation of why a device shutdown gracefully.
//
// These values should not be used to understand why a device has shutdown outside of this
// component.
enum class GracefulShutdownReason {
  kNotSet,
  kUserRequest,
  kSystemUpdate,
  kRetrySystemUpdate,
  kHighTemperature,
  kSessionFailure,
  // TODO(https://fxbug.dev/394392398): kSysmgrFailure is from CFv1, remove once it's no longer
  // written as a graceful shutdown reason or once deprecated reasons can be ignored.
  kSysmgrFailure,
  kCriticalComponentFailure,
  kFdr,
  kZbiSwap,
  kOutOfMemory,
  // TODO(https://fxbug.dev/42081574): Remove this reason once Netstack2 is
  // fully migrated to Netstack3.
  kNetstackMigration,
  kAndroidUnexpectedReason,
  kAndroidNoReason,
  kAndroidRescueParty,
  kAndroidCriticalProcessFailure,
  kDeveloperRequest,
  kUserRequestDeviceStuck,
  kSuspensionFailure,
  kBatteryDrained,
  kCriticalDriverFailure,
  kNotSupported,
  kNotParseable,
};

// Feedback's internal representation of how a device shutdown gracefully.
//
// These values should not be used to understand how a device has shutdown outside of this
// component.
enum class GracefulShutdownAction : std::uint8_t {
  kPoweroff,
  kReboot,
  kRebootToRecovery,
  kRebootToBootloader,
  kNotSupported,
  kNotParseable,
};

struct GracefulShutdownInfo {
  GracefulShutdownAction action;
  std::vector<GracefulShutdownReason> reasons;

  auto operator<=>(const GracefulShutdownInfo&) const = default;
};

std::string ToString(GracefulShutdownAction action);
std::string ToString(GracefulShutdownReason reason);

GracefulShutdownAction FromGracefulShutdownActionString(std::string_view action);

// Extracts the ShutdownAction from |options| and returns the action as a GracefulShutdownAction.
// Check-fails that |options| has the action set.
GracefulShutdownAction ToGracefulShutdownAction(
    const fuchsia::hardware::power::statecontrol::ShutdownOptions& options);

std::vector<GracefulShutdownReason> ToGracefulShutdownReasons(
    const fuchsia::hardware::power::statecontrol::ShutdownOptions& options);

// The input is limited to values corresponding to |power::statecontrol::ShutdownReason|.
std::vector<GracefulShutdownReason> FromLegacyTxtFile(std::string_view content);

// Only used for testing legacy functionality. The input is limited to GracefulShutdownReasons that
// map to |power::statecontrol::ShutdownReason|.
std::string ToLegacyFileContentForTesting(const std::vector<GracefulShutdownReason>& reasons);

// The input is limited to GracefulShutdownReasons that map to
// |power::statecontrol::ShutdownReason|.
//
// Note that some variants that should not be persisted (e.g. `kNotParseable`) are translated to
// `kNotSupported`.
std::vector<std::string> ToReasonStrings(const std::vector<GracefulShutdownReason>& reasons);

// The input is limited to GracefulShutdownActions that map to |power::statecontrol::ShutdownAction|
// and GracefulShutdownReasons that map to |power::statecontrol::ShutdownReason|.
//
// The format is expected to be:
// {
//   action: "action",
//   reasons: [
//     "Reason 1",
//     "Reason 2"
//   ]
// }
GracefulShutdownInfo FromJson(const std::string& content);

// The input is limited to GracefulShutdownActions that map to |power::statecontrol::ShutdownAction|
// and GracefulShutdownReasons that map to |power::statecontrol::ShutdownReason|.
std::string ToJson(GracefulShutdownAction action,
                   const std::vector<GracefulShutdownReason>& reasons);

// Converts the list of `GracefulShutdownReasons` into a single comma-separated string, like
// "Reason 1,Reason 2,Reason 3".
std::string ToRawStrings(const std::vector<GracefulShutdownReason>& reasons);

// Writes the graceful shutdown action and reasons to `path`.
void WriteGracefulShutdownInfo(GracefulShutdownAction action,
                               const std::vector<GracefulShutdownReason>& reasons,
                               const std::string& path);

}  // namespace feedback
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_GRACEFUL_SHUTDOWN_INFO_H_
