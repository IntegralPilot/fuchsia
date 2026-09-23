// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/battery_info_provider.h"

#include <fidl/fuchsia.power.battery/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <string>

#include "src/developer/forensics/feedback/annotations/constants.h"

namespace forensics::feedback {
namespace {

Annotations BuildAnnotations(const Error error) {
  Annotations annotations;
  for (const std::string& key : BatteryInfoProvider::GetAnnotationKeys()) {
    annotations.insert_or_assign(key, ErrorOrString(error));
  }

  return annotations;
}

}  // namespace

Annotations BatteryInfoToAnnotations::operator()(
    const fuchsia_power_battery::BatteryInfoProviderGetBatteryInfoResponse& response) {
  Annotations annotations = BuildAnnotations(Error::kMissingValue);
  const fuchsia_power_battery::BatteryInfo& info = response.info();

  if (!info.status().has_value()) {
    FX_LOGS(WARNING) << "GetBatteryInfo returned no status";
    return annotations;
  }

  if (*info.status() == fuchsia_power_battery::BatteryStatus::kNotPresent) {
    // This should never happen. It means we're trying to collect battery annotations on a device
    // that doesn't have a battery, but serves the battery info protocol.
    FX_LOGS(ERROR) << "GetBatteryInfo returned BatteryStatus::kNotPresent";
    return BuildAnnotations(Error::kLogicError);
  }

  if (info.level_percent().has_value()) {
    annotations.insert_or_assign(
        kDeviceBatteryLevelKey,
        ErrorOrString(std::to_string(static_cast<int>(*info.level_percent()))));
  }

  if (info.charge_status().has_value()) {
    std::string state = "unrecognized";
    switch (*info.charge_status()) {
      case fuchsia_power_battery::ChargeStatus::kUnknown:
        state = "unknown";
        break;
      case fuchsia_power_battery::ChargeStatus::kNotCharging:
        state = "not charging";
        break;
      case fuchsia_power_battery::ChargeStatus::kCharging:
        state = "charging";
        break;
      case fuchsia_power_battery::ChargeStatus::kDischarging:
        state = "discharging";
        break;
      case fuchsia_power_battery::ChargeStatus::kFull:
        state = "full";
        break;
    }
    annotations.insert_or_assign(kDeviceBatteryStateKey, ErrorOrString(state));
  }

  if (info.charge_source().has_value()) {
    const bool is_on_charger =
        (*info.charge_source() != fuchsia_power_battery::ChargeSource::kUnknown &&
         *info.charge_source() != fuchsia_power_battery::ChargeSource::kNone);
    annotations.insert_or_assign(kDeviceBatteryOnChargerKey,
                                 ErrorOrString(is_on_charger ? "true" : "false"));
  }

  return annotations;
}

Annotations BatteryInfoToAnnotations::operator()(const Error error) {
  return BuildAnnotations(error);
}

std::set<std::string> BatteryInfoProvider::GetAnnotationKeys() {
  return {
      kDeviceBatteryLevelKey,
      kDeviceBatteryStateKey,
      kDeviceBatteryOnChargerKey,
  };
}

std::set<std::string> BatteryInfoProvider::GetKeys() const {
  return BatteryInfoProvider::GetAnnotationKeys();
}

}  // namespace forensics::feedback
