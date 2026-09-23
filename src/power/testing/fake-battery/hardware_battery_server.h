// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_TESTING_FAKE_BATTERY_HARDWARE_BATTERY_SERVER_H_
#define SRC_POWER_TESTING_FAKE_BATTERY_HARDWARE_BATTERY_SERVER_H_

#include <fidl/fuchsia.hardware.power.battery/cpp/fidl.h>
#include <fidl/test.hardwarepowercontrol/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/zx/result.h>

#include <memory>
#include <optional>
#include <vector>

namespace fake_battery {

class HardwareBatteryServer;

class BatteryConnection : public fidl::Server<fuchsia_hardware_power_battery::Battery> {
 public:
  explicit BatteryConnection(HardwareBatteryServer* server);

  // fuchsia.hardware.power.battery.Battery implementation
  void GetSpec(GetSpecCompleter::Sync& completer) override;
  void GetStatus(GetStatusCompleter::Sync& completer) override;
  void ConfigureWatch(ConfigureWatchRequest& request,
                      ConfigureWatchCompleter::Sync& completer) override;
  void Watch(WatchRequest& request, WatchCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_power_battery::Battery> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void Notify(const fuchsia_hardware_power_battery::Status& status);

 private:
  HardwareBatteryServer* server_;
  bool first_watch_ = true;
  bool state_changed_ = false;
  std::optional<WatchCompleter::Async> watch_completer_;

  // Fields whose changes resolve this connection's `Watch`. Seeded with everything the fake
  // advertises in `Spec.supported_options`, matching the FIDL default for an absent `interest`.
  // An empty table means "no fields", which is a valid configuration.
  fuchsia_hardware_power_battery::Status interest_;
  fuchsia_hardware_power_battery::Status wake_on_;

  // Status last delivered on this connection, used for change detection. Absent until the first
  // response, which always resolves immediately.
  std::optional<fuchsia_hardware_power_battery::Status> last_status_;
};

class HardwareBatteryServer : public fidl::Server<test_hardwarepowercontrol::Control> {
 public:
  explicit HardwareBatteryServer(async_dispatcher_t* dispatcher);

  zx_status_t Init(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing);
  void NotifyAll(const fuchsia_hardware_power_battery::Status& status);

  const fuchsia_hardware_power_battery::Spec& battery_spec() const { return battery_spec_; }
  const fuchsia_hardware_power_battery::Status& battery_status() const { return battery_status_; }

 private:
  using scontrol = fidl::Server<test_hardwarepowercontrol::Control>;

  // test.hardwarepowercontrol.Control implementation
  void SetBatteryStatus(scontrol::SetBatteryStatusRequest& request,
                        scontrol::SetBatteryStatusCompleter::Sync& completer) override;

  fuchsia_hardware_power_battery::Spec battery_spec_;
  fuchsia_hardware_power_battery::Status battery_status_;

  async_dispatcher_t* dispatcher_ = nullptr;
  fidl::ServerBindingGroup<test_hardwarepowercontrol::Control> control_bindings_;
  std::vector<std::shared_ptr<BatteryConnection>> battery_connections_;
};

}  // namespace fake_battery

#endif  // SRC_POWER_TESTING_FAKE_BATTERY_HARDWARE_BATTERY_SERVER_H_
