// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_BATTERY_INFO_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_BATTERY_INFO_PROVIDER_H_

#include <fidl/fuchsia.power.battery/cpp/fidl.h>

#include "src/developer/forensics/feedback/annotations/fidl_provider.h"
#include "src/developer/forensics/feedback/annotations/types.h"

namespace forensics::feedback {

namespace internal {

inline auto GetBatteryInfo(fidl::Client<fuchsia_power_battery::BatteryManager>& client) {
  return client->GetBatteryInfo();
}

}  // namespace internal

struct BatteryInfoToAnnotations {
  Annotations operator()(
      const fuchsia_power_battery::BatteryInfoProviderGetBatteryInfoResponse& response);
  Annotations operator()(Error error);
};

// Responsible for collecting annotations from
// fuchsia.power.battery/BatteryInfoProvider::GetBatteryInfo.
class BatteryInfoProvider
    : public DynamicSingleFidlMethodAnnotationProvider<fuchsia_power_battery::BatteryManager,
                                                       &internal::GetBatteryInfo,
                                                       BatteryInfoToAnnotations> {
 public:
  using DynamicSingleFidlMethodAnnotationProvider::DynamicSingleFidlMethodAnnotationProvider;

  virtual ~BatteryInfoProvider() = default;

  static std::set<std::string> GetAnnotationKeys();
  std::set<std::string> GetKeys() const override;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_BATTERY_INFO_PROVIDER_H_
