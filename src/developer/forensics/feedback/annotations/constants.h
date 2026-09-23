// Copyright 2022 The Fuchsia Authors.All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_CONSTANTS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_CONSTANTS_H_

#include <fuchsia/feedback/cpp/fidl.h>

#include <set>
#include <string>

namespace forensics::feedback {

///////////////////////////////////////////////////////////////////////////////////////////////////
// KEYS
///////////////////////////////////////////////////////////////////////////////////////////////////
constexpr const char kBuildBoardKey[] = "build.board";
constexpr const char kBuildCompilationModeKey[] = "build.compilation-mode";
constexpr const char kBuildVersionKey[] = "build.version";
constexpr const char kBuildVersionPreviousBootKey[] = "build.version.previous-boot";
constexpr const char kBuildPlatformVersionKey[] = "build.platform.version";
constexpr const char kBuildPlatformVersionPreviousBootKey[] =
    "build.platform.version.previous-boot";
constexpr const char kBuildProductVersionKey[] = "build.product.version";
constexpr const char kBuildProductVersionPreviousBootKey[] = "build.product.version.previous-boot";
constexpr const char kBuildProductKey[] = "build.product";
constexpr const char kBuildLatestCommitDateKey[] = "build.latest-commit-date";
constexpr const char kBuildPlatformBackstopKey[] = "build.platform.utc-backstop";
constexpr const char kDebugReportUploadBootId[] = "debug.report.upload.boot-id";
constexpr const char kDebugReportUploadUptime[] = "debug.report.upload.uptime";
constexpr const char kDebugReportUuid[] = "debug.report.uuid";
constexpr const char kDebugSnapshotErrorKey[] = "debug.snapshot.error";
constexpr const char kDebugSnapshotPresentKey[] = "debug.snapshot.present";
constexpr const char kDeviceBatteryLevelKey[] = "device.battery.level";
constexpr const char kDeviceBatteryOnChargerKey[] = "device.battery.on-charger";
constexpr const char kDeviceBatteryStateKey[] = "device.battery.state";
constexpr const char kDeviceBoardNameKey[] = "device.board-name";
constexpr const char kDeviceFeedbackIdKey[] = "device.feedback-id";
constexpr const char kDeviceNumCPUsKey[] = "device.num-cpus";
constexpr const char kDeviceRuntimeKey[] = "device.runtime";
constexpr const char kDeviceTotalSuspendedTimeKey[] = "device.total-suspended-time";
constexpr const char kDeviceUptimeKey[] = "device.uptime";
constexpr const char kDeviceUtcTimeKey[] = "device.utc-time";
constexpr const char kHardwareBoardNameKey[] = "hardware.board.name";
constexpr const char kHardwareBoardRevisionKey[] = "hardware.board.revision";
constexpr const char kHardwareProductLanguageKey[] = "hardware.product.language";
constexpr const char kHardwareProductLocaleListKey[] = "hardware.product.locale-list";
constexpr const char kHardwareProductManufacturerKey[] = "hardware.product.manufacturer";
constexpr const char kHardwareProductModelKey[] = "hardware.product.model";
constexpr const char kHardwareProductNameKey[] = "hardware.product.name";
constexpr const char kHardwareProductRegulatoryDomainKey[] = "hardware.product.regulatory-domain";
constexpr const char kHardwareProductSKUKey[] = "hardware.product.sku";
constexpr const char kOSNameKey[] = "osName";
constexpr const char kOSVersionKey[] = "osVersion";
constexpr const char kOSChannelKey[] = "osChannel";
constexpr const char kSnapshotUuid[] = "snapshot.uuid";
constexpr const char kSystemBootIdCurrentKey[] = "system.boot-id.current";
constexpr const char kSystemBootIdPreviousKey[] = "system.boot-id.previous";
constexpr const char kSystemBootIdTimelineKey[] = "system.boot-id.timeline";
constexpr const char kSystemLastRebootReasonKey[] = "system.last-reboot.reason";
constexpr const char kSystemLastRebootRuntimeKey[] = "system.last-reboot.runtime";
constexpr const char kSystemLastRebootTotalSuspendedTimeKey[] =
    "system.last-reboot.total-suspended-time";
constexpr const char kSystemLastRebootUptimeKey[] = "system.last-reboot.uptime";
constexpr const char kSystemLastShutdownGracefulActionKey[] =
    "system.last-shutdown.graceful-action";
constexpr const char kSystemLocalePrimaryKey[] = "system.locale.primary";
constexpr const char kSystemTimezonePrimaryKey[] = "system.timezone.primary";
constexpr const char kSystemUpdateChannelCurrentKey[] = "system.update-channel.current";
constexpr const char kSystemUpdateChannelTargetKey[] = "system.update-channel.target";
constexpr const char kSystemUserActivityCurrentStateKey[] = "system.user-activity.current.state";
constexpr const char kSystemUserActivityCurrentDurationKey[] =
    "system.user-activity.current.duration";

///////////////////////////////////////////////////////////////////////////////////////////////////
// RESTRICTIONS
///////////////////////////////////////////////////////////////////////////////////////////////////

// 256 annotations may be collected by the platform.
constexpr uint32_t kMaxNumPlatformAnnotations = 256u;

// 128 non-platform annotations may be registered by non-platform components.
constexpr uint32_t kMaxNumNonPlatformAnnotations = 128u;

// 128 annotations are permitted to be from Feedback itself for debugging purposes.
constexpr uint32_t kMaxNumDebugAnnotations = 128u;

static_assert(kMaxNumPlatformAnnotations + kMaxNumNonPlatformAnnotations +
                      kMaxNumDebugAnnotations ==
                  fuchsia::feedback::MAX_NUM_ANNOTATIONS2_PROVIDED,
              "Annotations must be allocated to the platform, non-platform components, and "
              "Feedback itself (for debugging)");

// Reserved namespaces for platform annotations. Components are not allowed to use these namespaces
// when supplying non-platform annotations.
const std::set<std::string> kReservedAnnotationNamespaces({
    "build",
    "device",
    "hardware",
    "hardware.board",
    "hardware.product",
    "misc",
    "system",
});

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_CONSTANTS_H_
