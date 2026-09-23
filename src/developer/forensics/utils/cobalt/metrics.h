// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_COBALT_METRICS_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_COBALT_METRICS_H_

#include "src/developer/forensics/utils/cobalt/metrics_registry.cb.h"

namespace forensics {
namespace cobalt {

constexpr uint32_t kProjectId = cobalt_registry::kProjectId;
constexpr uint32_t kInspectBudgetMetricId = cobalt_registry::kMaxInputInspectBudgetMigratedMetricId;

enum class CrashState {
  kUnknown = cobalt_registry::CrashMigratedMetricDimensionState::Unknown,
  kFiled = cobalt_registry::CrashMigratedMetricDimensionState::Filed,
  kUploaded = cobalt_registry::CrashMigratedMetricDimensionState::Uploaded,
  kArchived = cobalt_registry::CrashMigratedMetricDimensionState::Archived,
  kGarbageCollected = cobalt_registry::CrashMigratedMetricDimensionState::GarbageCollected,
  kDropped = cobalt_registry::CrashMigratedMetricDimensionState::Dropped,
  kUploadThrottled = cobalt_registry::CrashMigratedMetricDimensionState::UploadThrottled,
  kOnDeviceQuotaReached = cobalt_registry::CrashMigratedMetricDimensionState::OnDeviceQuotaReached,
  kDeleted = cobalt_registry::CrashMigratedMetricDimensionState::Deleted,
  kUploadTimedOut = cobalt_registry::CrashMigratedMetricDimensionState::UploadTimedOut,
};

enum class UploadAttemptState {
  kUnknown = cobalt_registry::CrashUploadAttemptsMigratedMetricDimensionState::Unknown,
  kUploadAttempt = cobalt_registry::CrashUploadAttemptsMigratedMetricDimensionState::UploadAttempt,
  kUploaded = cobalt_registry::CrashUploadAttemptsMigratedMetricDimensionState::Uploaded,
  kDeleted = cobalt_registry::CrashUploadAttemptsMigratedMetricDimensionState::Deleted,
  kGarbageCollected =
      cobalt_registry::CrashUploadAttemptsMigratedMetricDimensionState::GarbageCollected,
  kUploadThrottled =
      cobalt_registry::CrashUploadAttemptsMigratedMetricDimensionState::UploadThrottled,
  kUploadTimedOut =
      cobalt_registry::CrashUploadAttemptsMigratedMetricDimensionState::UploadTimedOut,
};

enum class TimedOutData {
  kUnknown = cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::Unknown,
  kSystemLog = cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::SystemLog,
  kKernelLog = cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::KernelLog,
  kScreenshot =
      cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::Screenshot,
  kInspect = cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::Inspect,
  kChannel = cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::Channel,
  kProductInfo =
      cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::ProductInfo,
  kBoardInfo = cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::BoardInfo,
  kLastRebootInfo =
      cobalt_registry::FeedbackDataCollectionTimeoutMigratedMetricDimensionData::LastRebootInfo,
};

enum class SnapshotGenerationFlow {
  kUnknown = cobalt_registry::SnapshotGenerationDurationUsecsMigratedMetricDimensionFlow::Unknown,
  kSuccess = cobalt_registry::SnapshotGenerationDurationUsecsMigratedMetricDimensionFlow::Success,
  kFailure = cobalt_registry::SnapshotGenerationDurationUsecsMigratedMetricDimensionFlow::Failure,
};

enum class LastRebootReason {
  kUnknown = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::Unknown,
  kGenericGraceful =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::GenericGraceful,
  kUnexpectedReasonGraceful =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::UnexpectedReasonGraceful,
  kGenericUngraceful =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::GenericUngraceful,
  kCold = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::Cold,
  kBriefPowerLoss = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::BriefPowerLoss,
  kBrownout = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::Brownout,
  kKernelPanic = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::KernelPanic,
  kSystemOutOfMemory =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::SystemOutOfMemory,
  kHardwareWatchdogTimeout =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::HardwareWatchdogTimeout,
  kSoftwareWatchdogTimeout =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::SoftwareWatchdogTimeout,
  kUserRequest = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::UserRequest,
  kSystemUpdate = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::SystemUpdate,
  kRetrySystemUpdate =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::RetrySystemUpdate,
  kZbiSwap = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::ZbiSwap,
  kHighTemperature =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::HighTemperature,
  kSessionFailure = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::SessionFailure,
  kSysmgrFailure = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::SysmgrFailure,
  kFactoryDataReset =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::FactoryDataReset,
  kCriticalComponentFailure =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::CriticalComponentFailure,
  kRootJobTermination =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::RootJobTermination,
  kNetstackMigration =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::NetstackMigration,
  kAndroidUnexpectedReason =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::AndroidUnexpectedReason,
  kAndroidNoReason =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::AndroidNoReason,
  kAndroidRescueParty =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::AndroidRescueParty,
  kAndroidCriticalProcessFailure =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::AndroidCriticalProcessFailure,
  kDeveloperRequest =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::DeveloperRequest,
  kUserRequestDeviceStuck =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::UserRequestDeviceStuck,
  kBatteryDrained = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::BatteryDrained,
  kUserHardReset = cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::UserHardReset,
  kSuspensionFailure =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::SuspensionFailure,
  kCriticalDriverFailure =
      cobalt_registry::LastRebootUptimeMigratedMetricDimensionReason::CriticalDriverFailure,
};

enum class PreviousBootEncodingVersion {
  kUnknown = cobalt_registry::
      PreviousBootLogCompressionRatioMigratedMetricDimensionEncodingVersion::Unknown,
  kV_01 =
      cobalt_registry::PreviousBootLogCompressionRatioMigratedMetricDimensionEncodingVersion::V01,
  kV_02 =
      cobalt_registry::PreviousBootLogCompressionRatioMigratedMetricDimensionEncodingVersion::V02,
  kV_03 =
      cobalt_registry::PreviousBootLogCompressionRatioMigratedMetricDimensionEncodingVersion::V03,
  kV_04 =
      cobalt_registry::PreviousBootLogCompressionRatioMigratedMetricDimensionEncodingVersion::V04,
  kV_05 =
      cobalt_registry::PreviousBootLogCompressionRatioMigratedMetricDimensionEncodingVersion::V05,
};

enum class SnapshotVersion {
  kUnknown = cobalt_registry::SnapshotSizeMigratedMetricDimensionVersion::Unknown,
  kV_01 = cobalt_registry::SnapshotSizeMigratedMetricDimensionVersion::V01,
  kV_02 = cobalt_registry::SnapshotSizeMigratedMetricDimensionVersion::V02,
  kV_03 = cobalt_registry::SnapshotSizeMigratedMetricDimensionVersion::V03,
};

enum class ReportUploadStatus : std::uint8_t {
  kUnknown = cobalt_registry::ReportUploadDurationMetricDimensionUploadStatus::Unknown,
  kSuccess = cobalt_registry::ReportUploadDurationMetricDimensionUploadStatus::Success,
  kFailure = cobalt_registry::ReportUploadDurationMetricDimensionUploadStatus::Failure,
  kThrottled = cobalt_registry::ReportUploadDurationMetricDimensionUploadStatus::Throttled,
  kTimedOut = cobalt_registry::ReportUploadDurationMetricDimensionUploadStatus::TimedOut,
};

enum class ReportUploadSize : std::uint8_t {
  kUnknown = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::Unknown,
  kLessThan250 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan250,
  kLessThan500 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan500,
  kLessThan750 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan750,
  kLessThan1000 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan1000,
  kLessThan1250 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan1250,
  kLessThan1500 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan1500,
  kLessThan1750 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan1750,
  kLessThan2000 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan2000,
  kLessThan2250 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan2250,
  kLessThan2500 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan2500,
  kLessThan2750 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan2750,
  kLessThan3000 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan3000,
  kLessThan3250 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan3250,
  kLessThan3500 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan3500,
  kLessThan3750 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan3750,
  kLessThan4000 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan4000,
  kLessThan4250 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan4250,
  kLessThan4500 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan4500,
  kLessThan4750 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan4750,
  kLessThan5000 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::LessThan5000,
  kGreaterThan5000 = cobalt_registry::ReportUploadDurationMetricDimensionSizeKib::GreaterThan5000,
};

inline constexpr uint32_t MetricIDForEventCode(const SnapshotVersion version) {
  return cobalt_registry::kSnapshotSizeMigratedMetricId;
}

inline constexpr uint32_t MetricIDForEventCode(const PreviousBootEncodingVersion version) {
  return cobalt_registry::kPreviousBootLogCompressionRatioMigratedMetricId;
}

inline constexpr uint32_t MetricIDForEventCode(const LastRebootReason reason) {
  return cobalt_registry::kLastRebootUptimeMigratedMetricId;
}

inline constexpr uint32_t MetricIDForEventCode(const SnapshotGenerationFlow snapshot) {
  return cobalt_registry::kSnapshotGenerationDurationUsecsMigratedMetricId;
}

inline constexpr uint32_t MetricIDForEventCode(const TimedOutData data) {
  return cobalt_registry::kFeedbackDataCollectionTimeoutMigratedMetricId;
}

inline constexpr uint32_t MetricIDForEventCode(const CrashState state) {
  return cobalt_registry::kCrashMigratedMetricId;
}

inline constexpr uint32_t MetricIDForEventCode(const UploadAttemptState state) {
  return cobalt_registry::kCrashUploadAttemptsMigratedMetricId;
}

namespace internal {

// Determines if all passed event code types correspond to the same metric ids.
//
// The base case needs to be provided with a default value to return.
template <typename EventCodeTypeDefault, typename... EventCodeTypes>
struct MetricIDChecker {
  static constexpr uint32_t metric_id = MetricIDForEventCode(static_cast<EventCodeTypeDefault>(0));
  static constexpr bool all_same = true;
};

// Uses the first event code type as the default for the base case and check if all of the metric
// ids for the event codes in the parameter pack are the same.
template <typename EventCodeTypeDefault, typename EventCodeTypesH, typename... EventCodeTypesT>
struct MetricIDChecker<EventCodeTypeDefault, EventCodeTypesH, EventCodeTypesT...> {
  static constexpr uint32_t metric_id = MetricIDForEventCode(static_cast<EventCodeTypesH>(0));
  static constexpr bool all_same =
      metric_id == MetricIDChecker<EventCodeTypeDefault, EventCodeTypesT...>::metric_id;
};

}  // namespace internal

template <typename EventCodeTypesH, typename... EventCodeTypesT>
inline constexpr uint32_t MetricIDForEventCode(const EventCodeTypesH event_code,
                                               const EventCodeTypesT... event_codes_t) {
  constexpr internal::MetricIDChecker<EventCodeTypesH, EventCodeTypesH, EventCodeTypesT...> checker;
  static_assert(checker.all_same, "All event codes need to have the same metric id");
  return checker.metric_id;
}

// Corresponds to |fuchsia::metrics::MetricEventLogger| public methods.
enum class EventType {
  kInteger,
  kOccurrence,
};

inline constexpr EventType EventTypeForEventCode(const SnapshotVersion version) {
  return EventType::kInteger;
}

inline constexpr EventType EventTypeForEventCode(const PreviousBootEncodingVersion version) {
  return EventType::kInteger;
}

inline constexpr EventType EventTypeForEventCode(const LastRebootReason reason) {
  return EventType::kInteger;
}

inline constexpr EventType EventTypeForEventCode(const SnapshotGenerationFlow snapshot) {
  return EventType::kInteger;
}

inline constexpr EventType EventTypeForEventCode(const TimedOutData data) {
  return EventType::kOccurrence;
}

inline constexpr EventType EventTypeForEventCode(const CrashState state) {
  return EventType::kOccurrence;
}

inline constexpr EventType EventTypeForEventCode(const UploadAttemptState state) {
  return EventType::kOccurrence;
}

}  // namespace cobalt
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_COBALT_METRICS_H_
