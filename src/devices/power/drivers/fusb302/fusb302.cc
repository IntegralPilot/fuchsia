// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302.h"

#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/profile.h>
#include <lib/zx/result.h>
#include <lib/zx/timer.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/threads.h>
#include <zircon/types.h>

#include <cinttypes>
#include <cstddef>
#include <cstdint>
#include <string>

#include <fbl/alloc_checker.h>
#include <fbl/string_buffer.h>

#include "src/devices/power/drivers/fusb302/fusb302-controls.h"
#include "src/devices/power/drivers/fusb302/pd-sink-state-machine.h"
#include "src/devices/power/drivers/fusb302/registers.h"
#include "src/devices/power/drivers/fusb302/state-machine-base.h"
#include "src/devices/power/drivers/fusb302/typec-port-state-machine.h"

namespace fusb302 {

void Fusb302::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                        const zx_packet_interrupt_t* interrupt) {
  ProcessStateChanges(signals_.ServiceInterrupts());
  irq_.ack();
}

void Fusb302::HandleTimeout(async_dispatcher_t*, async::WaitBase*, zx_status_t status,
                            const zx_packet_signal_t*) {
  HardwareStateChanges changes;
  fdf::trace("State machine timer fired off");
  changes.timer_signaled = true;
  ProcessStateChanges(changes);
}

void Fusb302::ProcessStateChanges(HardwareStateChanges changes) {
  if (changes.received_reset) {
    pd_state_machine_.DidReceiveSoftReset();
    pd_state_machine_.Run(SinkPolicyEngineInput::kInitialized);
  }

  while (protocol_.HasUnreadMessage()) {
    pd_state_machine_.Run(SinkPolicyEngineInput::kMessageReceived);

    if (protocol_.HasUnreadMessage()) {
      pd_state_machine_.ProcessUnexpectedMessage();
      pd_state_machine_.Run(SinkPolicyEngineInput::kInitialized);
    }
  }

  if (changes.port_state_changed) {
    port_state_machine_.Run(TypeCPortInput::kPortStateChanged);

    if (port_state_machine_.current_state() == TypeCPortState::kSinkAttached) {
      pd_state_machine_.Run(SinkPolicyEngineInput::kInitialized);
    } else {
      pd_state_machine_.Reset();
    }
  }

  if (changes.timer_signaled &&
      port_state_machine_.current_state() == TypeCPortState::kSinkAttached) {
    pd_state_machine_.Run(SinkPolicyEngineInput::kTimerFired);
  }
}

zx_status_t Fusb302::ResetHardwareAndStartPowerRoleDetection() {
  auto status = ResetReg::Get().FromValue(0).set_sw_res(true).WriteTo(i2c_);
  if (status != ZX_OK) {
    fdf::error("Failed to write Reset register: {}", zx_status_get_string(status));
    return status;
  }

  zx::result<> result = signals_.InitInterruptUnit();
  if (!result.is_ok()) {
    return result.error_value();
  }

  result = controls_.ResetIntoPowerRoleDiscovery();
  if (!result.is_ok()) {
    return result.error_value();
  }

  return ZX_OK;
}

zx_status_t Fusb302::Init() {
  zx::result<> result = identity_.ReadIdentity();
  if (result.is_error()) {
    fdf::error("Failed to initialize inspect: {}", result);
    return result.error_value();
  }

  zx_status_t status = ResetHardwareAndStartPowerRoleDetection();
  if (status != ZX_OK) {
    fdf::error("ResetHardwareAndStartPowerRoleDetection() failed: {}",
               zx_status_get_string(status));
    return status;
  }

  irq_handler_.set_object(irq_.get());
  irq_handler_.Begin(dispatcher_.async_dispatcher());

  return ZX_OK;
}

zx::result<> Fusb302::WaitAsyncForTimer(zx::timer& timer) {
  timeout_handler_.set_object(timer.get());
  timeout_handler_.set_trigger(ZX_TIMER_SIGNALED);
  auto status = timeout_handler_.Begin(dispatcher_.async_dispatcher(),
                                       fit::bind_member(this, &Fusb302::HandleTimeout));
  if (status != ZX_OK) {
    fdf::warn("Failed to wait on timer: {}", zx_status_get_string(status));
  }
  return zx::make_result(status);
}

zx::result<> Fusb302Device::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  // Map hardware resources.
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> i2c;
  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio;
  zx::interrupt irq;
  {
    zx::result result = incoming->Connect<fuchsia_hardware_i2c::Service::Device>("i2c");
    if (result.is_error()) {
      fdf::error("Failed to open i2c service: {}", result);
      return result.take_error();
    }
    i2c = std::move(result.value());
  }
  {
    zx::result result = incoming->Connect<fuchsia_hardware_gpio::Service::Device>("gpio-int");
    if (result.is_error()) {
      fdf::error("Failed to open gpio service: {}", result);
      return result.take_error();
    }

    gpio = std::move(result.value());

    fidl::Arena arena;
    auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                                .mode(fuchsia_hardware_gpio::InterruptMode::kLevelLow)
                                .Build();
    if (auto result = fidl::WireCall(gpio)->ConfigureInterrupt(interrupt_config);
        !result.ok() || result->is_error()) {
      fdf::error("GPIO ConfigureInterrupt() failed: {}",
                 result.ok() ? zx_status_get_string(result->error_value())
                             : result.FormatDescription().c_str());
    }

    if (auto result = fidl::WireCall(gpio)->GetInterrupt({}); !result.ok() || result->is_error()) {
      fdf::error("GPIO GetInterrupt() failed: {}", result.ok()
                                                       ? zx_status_get_string(result->error_value())
                                                       : result.FormatDescription().c_str());
    } else {
      irq = std::move(result->value()->interrupt);
    }
  }

  auto fusb302_dispatcher = fdf::SynchronizedDispatcher::Create(
      {}, "fusb302", [&](fdf_dispatcher_t*) {}, "fuchsia.devices.power.drivers.fusb302.interrupt");
  ZX_ASSERT_MSG(!fusb302_dispatcher.is_error(), "Creating dispatcher error: %s",
                zx_status_get_string(fusb302_dispatcher.status_value()));

  device_ = std::make_unique<fusb302::Fusb302>(std::move(*fusb302_dispatcher), std::move(i2c),
                                               std::move(gpio), std::move(irq));
  auto status = device_->Init();
  if (status != ZX_OK) {
    fdf::error("Init() failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  auto result = outgoing()->component().AddUnmanagedProtocol<fuchsia_hardware_powersource::Source>(
      source_bindings_.CreateHandler(device_.get(), dispatcher(), fidl::kIgnoreBindingClosure),
      kDeviceName);
  if (result.is_error()) {
    fdf::error("Failed to add Device service {}", result);
    return result.take_error();
  }

  if (zx::result result = CreateDevfsNode(); result.is_error()) {
    fdf::error("Failed to export to devfs {}", result);
    return result.take_error();
  }

  return zx::ok();
}

Fusb302Device::~Fusb302Device() { device_.reset(); }

zx::result<> Fusb302Device::CreateDevfsNode() {
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args(
      {.connector = std::move(connector.value()), .class_name = "power"});

  zx::result child = AddOwnedChild(kDeviceName, devfs_args);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child.status_string());
    return child.take_error();
  }

  controller_.Bind(std::move(child->node_controller_));
  node_.Bind(std::move(child->node_));

  return zx::ok();
}

}  // namespace fusb302

FUCHSIA_DRIVER_EXPORT2(fusb302::Fusb302Device);
