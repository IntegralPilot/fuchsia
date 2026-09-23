// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_H_

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <fidl/fuchsia.hardware.powersource/cpp/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <lib/zx/timer.h>
#include <threads.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include "lib/fdf/cpp/dispatcher.h"
#include "src/devices/power/drivers/fusb302/fusb302-controls.h"
#include "src/devices/power/drivers/fusb302/fusb302-fifos.h"
#include "src/devices/power/drivers/fusb302/fusb302-identity.h"
#include "src/devices/power/drivers/fusb302/fusb302-protocol.h"
#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"
#include "src/devices/power/drivers/fusb302/fusb302-signals.h"
#include "src/devices/power/drivers/fusb302/pd-sink-state-machine.h"
#include "src/devices/power/drivers/fusb302/typec-port-state-machine.h"
#include "src/devices/power/drivers/fusb302/usb-pd-sink-policy.h"

namespace fusb302 {

// Fusb302: Device that keeps track of the state of the HW, services FIDL requests, and runs the IRQ
// thread, which in turn runs StateMachine when called on.
class Fusb302 : public fidl::WireServer<fuchsia_hardware_powersource::Source> {
 public:
  Fusb302(fdf::Dispatcher dispatcher, fidl::ClientEnd<fuchsia_hardware_i2c::Device> i2c,
          fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio, zx::interrupt irq)
      : dispatcher_(std::move(dispatcher)),
        i2c_(std::move(i2c)),
        gpio_(std::move(gpio)),
        irq_(std::move(irq)),
        identity_(i2c_, inspect_.GetRoot().CreateChild("Identity")),
        sensors_(i2c_, inspect_.GetRoot().CreateChild("Sensors")),
        fifos_(i2c_),
        // GoodCrcGenerationMode::kSoftware is not fast enough for meeting the
        // reaction time mandated by the USB PD specification.
        protocol_(GoodCrcGenerationMode::kAssumed, fifos_),
        signals_(i2c_, sensors_, protocol_),
        controls_(i2c_, sensors_, inspect_.GetRoot().CreateChild("Controls")),
        sink_policy_({.min_voltage_mv = 5'000, .max_voltage_mv = 12'000, .max_power_mw = 24'000}),
        port_state_machine_(*this, inspect_.GetRoot().CreateChild("PortStateMachine")),
        pd_state_machine_(sink_policy_, *this,
                          inspect_.GetRoot().CreateChild("SinkPolicyEngineStateMachine")) {
    ZX_DEBUG_ASSERT(i2c_.is_valid());
    ZX_DEBUG_ASSERT(irq_.is_valid());
  }

  Fusb302(const Fusb302&) = delete;
  Fusb302& operator=(const Fusb302&) = delete;

  ~Fusb302() override {
    irq_handler_.Cancel();
    const zx_status_t status = irq_.destroy();
    if (status != ZX_OK) {
      fdf::warn("zx::interrupt::destroy() failed: {}", zx_status_get_string(status));
    }
    timeout_handler_.Cancel();
  }

  // Initialization Functions and Variables
  zx_status_t Init();

  // TODO(rdzhuang): change power FIDL to supply required values in SourceInfo
  void GetPowerInfo(GetPowerInfoCompleter::Sync& completer) override {
    // For now, we hardcode this value. When we support other power types in the future, this will
    // need to vary depending on selected source capability.
    completer.Reply(ZX_OK, {.type = fuchsia_hardware_powersource::PowerType::kAc,
                            .state = fuchsia_hardware_powersource::wire::kPowerStateCharging});
  }
  void GetStateChangeEvent(GetStateChangeEventCompleter::Sync& completer) override {
    completer.Reply(ZX_ERR_NOT_SUPPORTED, {});
  }
  void GetBatteryInfo(GetBatteryInfoCompleter::Sync& completer) override {
    completer.Reply(ZX_ERR_NOT_SUPPORTED, {});
  }

  Fusb302Sensors& sensors() { return sensors_; }
  Fusb302Protocol& protocol() { return protocol_; }
  Fusb302Controls& controls() { return controls_; }

  inspect::Inspector& InspectorForTesting() { return inspect_; }

  // Asynchronously waits for a timer to be signaled once.
  //
  // When the timer is signaled, the state machines connected to this instance
  // will be run with an indication that a timer was signaled.
  zx::result<> WaitAsyncForTimer(zx::timer& timer);

 private:
  zx_status_t ResetHardwareAndStartPowerRoleDetection();

  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);
  void HandleTimeout(async_dispatcher_t*, async::WaitBase*, zx_status_t status,
                     const zx_packet_signal_t*);

  void ProcessStateChanges(HardwareStateChanges changes);

  fdf::Dispatcher dispatcher_;

  fidl::ClientEnd<fuchsia_hardware_i2c::Device> i2c_;
  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio_;
  zx::interrupt irq_;
  async::IrqMethod<Fusb302, &Fusb302::HandleIrq> irq_handler_{this};
  async::WaitOnce timeout_handler_;

  inspect::Inspector inspect_;

  Fusb302Identity identity_;
  Fusb302Sensors sensors_;
  Fusb302Fifos fifos_;
  Fusb302Protocol protocol_;
  Fusb302Signals signals_;
  Fusb302Controls controls_;

  usb_pd::SinkPolicy sink_policy_;

  TypeCPortStateMachine port_state_machine_;
  SinkPolicyEngineStateMachine pd_state_machine_;
};

constexpr char kDeviceName[] = "fusb302";

class Fusb302Device : public fdf::DriverBase2 {
 public:
  explicit Fusb302Device()
      : fdf::DriverBase2(kDeviceName),
        devfs_connector_(fit::bind_member<&Fusb302Device::Serve>(this)) {}

  ~Fusb302Device() override;

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  zx::result<> CreateDevfsNode();
  void Serve(fidl::ServerEnd<fuchsia_hardware_powersource::Source> server) {
    source_bindings_.AddBinding(dispatcher(), std::move(server), device_.get(),
                                fidl::kIgnoreBindingClosure);
  }

  std::unique_ptr<Fusb302> device_;
  fidl::ServerBindingGroup<fuchsia_hardware_powersource::Source> source_bindings_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  driver_devfs::Connector<fuchsia_hardware_powersource::Source> devfs_connector_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_H_
