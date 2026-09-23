// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_BATTERY_INFO_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_BATTERY_INFO_PROVIDER_H_

#include <fidl/fuchsia.power.battery/cpp/fidl.h>

namespace forensics::stubs {

class StubBatteryInfoProvider : public fidl::Server<::fuchsia_power_battery::BatteryManager> {
 public:
  void GetBatteryInfo(GetBatteryInfoCompleter::Sync& completer) override;
  void Watch(WatchRequest& request, WatchCompleter::Sync& completer) override;

  void set_battery_info(::fuchsia_power_battery::BatteryInfo info) { info_ = std::move(info); }

  void set_level(float level) { info_.level_percent(level); }

  void set_charge_status(::fuchsia_power_battery::ChargeStatus status) {
    info_.charge_status(status);
  }

  void set_charge_source(::fuchsia_power_battery::ChargeSource source) {
    info_.charge_source(source);
  }

 private:
  ::fuchsia_power_battery::BatteryInfo info_;
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_BATTERY_INFO_PROVIDER_H_
