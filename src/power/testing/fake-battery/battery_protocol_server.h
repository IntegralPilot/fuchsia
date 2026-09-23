// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_TESTING_FAKE_BATTERY_BATTERY_PROTOCOL_SERVER_H_
#define SRC_POWER_TESTING_FAKE_BATTERY_BATTERY_PROTOCOL_SERVER_H_

#include <fidl/fuchsia.power.battery/cpp/fidl.h>
#include <fidl/test.hardwarepowercontrol/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <zircon/types.h>

namespace fake_battery {

// Protocol served to client components over devfs.
class BatteryProtocolServer : public fidl::Server<fuchsia_power_battery::BatteryInfoProvider> {
 public:
  explicit BatteryProtocolServer(async_dispatcher_t* dispatcher);
  ~BatteryProtocolServer() override;

  zx_status_t Init(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing);
  void GetBatteryInfo(GetBatteryInfoCompleter::Sync& completer) override;

  void Watch(WatchRequest& request, WatchCompleter::Sync& completer) override;

  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_power_battery::BatteryInfoProvider> server);

  void NotifyOnceAsync(fidl::Client<fuchsia_power_battery::BatteryInfoWatcher> watcher);

 private:
  fidl::ServerBindingGroup<fuchsia_power_battery::BatteryInfoProvider> bindings_;
  async_dispatcher_t* dispatcher_ = nullptr;
  std::vector<fidl::Client<fuchsia_power_battery::BatteryInfoWatcher>> watcher_clients_;

  fuchsia_power_battery::BatteryInfo faked_battery_info_{{
      .status = fuchsia_power_battery::BatteryStatus::kOk,
      .charge_status = fuchsia_power_battery::ChargeStatus::kCharging,
      .charge_source = fuchsia_power_battery::ChargeSource::kAcAdapter,
      .level_percent = test_hardwarepowercontrol::kDefaultLevelPercent,
      .level_status = fuchsia_power_battery::LevelStatus::kOk,
      .health = fuchsia_power_battery::HealthStatus::kGood,
      .time_remaining =
          fuchsia_power_battery::TimeRemaining::WithFullCharge(zx::sec(59).to_nsecs()),
      .present_voltage_mv = test_hardwarepowercontrol::kDefaultPresentVoltageMv,
      .remaining_charge_uah = test_hardwarepowercontrol::kDefaultRemainingChargeUah,
      .full_capacity_uah = test_hardwarepowercontrol::kDefaultFullCapacityUah,
      .temperature_mc = test_hardwarepowercontrol::kDefaultTemperatureMc,
      .present_charging_current_ua = test_hardwarepowercontrol::kDefaultChargingCurrentUa,
  }};
};

}  // namespace fake_battery

#endif  // SRC_POWER_TESTING_FAKE_BATTERY_BATTERY_PROTOCOL_SERVER_H_
