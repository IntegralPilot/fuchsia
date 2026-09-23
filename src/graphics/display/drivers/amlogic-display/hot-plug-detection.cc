// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/hot-plug-detection.h"

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>

namespace amlogic_display {

// static
zx::result<std::unique_ptr<HotPlugDetection>> HotPlugDetection::Create(
    fdf::Namespace& incoming, HotPlugDetection::OnStateChangeHandler on_state_change) {
  static const char kHpdGpioFragmentName[] = "gpio-hdmi-hotplug-detect";
  zx::result<fidl::ClientEnd<fuchsia_hardware_gpio::Gpio>> pin_gpio_result =
      incoming.Connect<fuchsia_hardware_gpio::Service::Device>(kHpdGpioFragmentName);
  if (pin_gpio_result.is_error()) {
    fdf::error("Failed to get gpio protocol from fragment {}: {}", kHpdGpioFragmentName,
               pin_gpio_result);
    return pin_gpio_result.take_error();
  }

  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> pin_gpio(std::move(pin_gpio_result.value()));

  fidl::Arena arena;
  auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                              .mode(fuchsia_hardware_gpio::InterruptMode::kLevelHigh)
                              .Build();
  fidl::WireResult configure_interrupt_result = pin_gpio->ConfigureInterrupt(interrupt_config);
  if (!configure_interrupt_result.ok()) {
    fdf::error("Failed to send ConfigureInterrupt request to HPD GPIO: {}",
               configure_interrupt_result.status_string());
    return zx::error(configure_interrupt_result.status());
  }
  fidl::WireResultUnwrapType<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>&
      configure_interrupt_response = configure_interrupt_result.value();
  if (configure_interrupt_response.is_error()) {
    fdf::error("Failed to configure HPD GPIO interrupt: {}",
               zx::make_result(configure_interrupt_response.error_value()));
    return configure_interrupt_response.take_error();
  }

  fidl::WireResult interrupt_result = pin_gpio->GetInterrupt({});
  if (interrupt_result->is_error()) {
    fdf::error("Failed to send GetInterrupt request to HPD GPIO: {}",
               interrupt_result.status_string());
    return interrupt_result->take_error();
  }
  fidl::WireResultUnwrapType<fuchsia_hardware_gpio::Gpio::GetInterrupt>& interrupt_response =
      interrupt_result.value();
  if (interrupt_response.is_error()) {
    fdf::error("Failed to get interrupt from HPD GPIO: {}",
               zx::make_result(interrupt_response.error_value()));
    return interrupt_response.take_error();
  }

  static constexpr std::string_view kDispatcherName = "hot-plug-detection-interrupt-thread";
  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls,
                                          kDispatcherName,
                                          /*shutdown_handler=*/[](fdf_dispatcher_t*) {},
                                          /*scheduler_role=*/{});
  if (create_dispatcher_result.is_error()) {
    fdf::error("Failed to create vsync Dispatcher: {}", create_dispatcher_result);
    return create_dispatcher_result.take_error();
  }
  fdf::SynchronizedDispatcher dispatcher = std::move(create_dispatcher_result).value();

  fbl::AllocChecker alloc_checker;
  std::unique_ptr<HotPlugDetection> hot_plug_detection = fbl::make_unique_checked<HotPlugDetection>(
      &alloc_checker, pin_gpio.TakeClientEnd(), std::move(interrupt_response->interrupt),
      std::move(on_state_change), std::move(dispatcher));
  if (!alloc_checker.check()) {
    fdf::error("Out of memory while allocating HotPlugDetection");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> init_result = hot_plug_detection->Init();
  if (init_result.is_error()) {
    fdf::error("Failed to initalize HPD: {}", init_result);
    return init_result.take_error();
  }

  return zx::ok(std::move(hot_plug_detection));
}

HotPlugDetection::HotPlugDetection(fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> pin_gpio,
                                   zx::interrupt pin_gpio_interrupt,
                                   HotPlugDetection::OnStateChangeHandler on_state_change,
                                   fdf::SynchronizedDispatcher irq_handler_dispatcher)
    : pin_gpio_(std::move(pin_gpio)),
      pin_gpio_irq_(std::move(pin_gpio_interrupt)),
      on_state_change_(std::move(on_state_change)),
      irq_handler_dispatcher_(std::move(irq_handler_dispatcher)) {
  ZX_DEBUG_ASSERT(on_state_change_);
  pin_gpio_irq_handler_.set_object(pin_gpio_irq_.get());
}

HotPlugDetection::~HotPlugDetection() {
  // In order to shut down the interrupt handler and join the thread, the
  // interrupt must be destroyed first.
  if (pin_gpio_irq_.is_valid()) {
    zx_status_t status = pin_gpio_irq_.destroy();
    if (status != ZX_OK) {
      fdf::error("GPIO interrupt destroy failed: {}", zx::make_result(status));
    }
  }

  irq_handler_dispatcher_.reset();

  // After the interrupt handler loop is shut down, the interrupt is unused
  // and we can safely release the interrupt.
  if (pin_gpio_.is_valid()) {
    fidl::WireResult release_result = pin_gpio_->ReleaseInterrupt();
    if (!release_result.ok()) {
      fdf::error("Failed to connect to GPIO FIDL protocol: {}", release_result.status_string());
    } else {
      fidl::WireResultUnwrapType<fuchsia_hardware_gpio::Gpio::ReleaseInterrupt>& release_response =
          release_result.value();
      if (release_response.is_error()) {
        fdf::error("Failed to release GPIO interrupt: {}",
                   zx::make_result(release_response.error_value()));
      }
    }
  }
}

HotPlugDetectionState HotPlugDetection::CurrentState() {
  fbl::AutoLock lock(&mutex_);
  return current_pin_state_;
}

zx::result<> HotPlugDetection::Init() {
  fidl::WireResult<fuchsia_hardware_gpio::Gpio::SetBufferMode> set_buffer_mode_result =
      pin_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
  if (!set_buffer_mode_result.ok()) {
    fdf::error("Failed to send SetBufferMode request to hpd gpio: {}",
               set_buffer_mode_result.status_string());
    return zx::error(set_buffer_mode_result.status());
  }
  fidl::WireResultUnwrapType<fuchsia_hardware_gpio::Gpio::SetBufferMode>& set_buffer_mode_response =
      set_buffer_mode_result.value();
  if (set_buffer_mode_response.is_error()) {
    fdf::error("Failed to configure hpd gpio to input: {}",
               zx::make_result(set_buffer_mode_response.error_value()));
    return set_buffer_mode_response.take_error();
  }

  zx_status_t status = pin_gpio_irq_handler_.Begin(irq_handler_dispatcher_.async_dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to bind the GPIO IRQ to the loop dispatcher: {}", zx::make_result(status));
    return zx::error(status);
  }
  return zx::ok();
}

// static
HotPlugDetectionState HotPlugDetection::GpioValueToState(bool gpio_value) {
  return gpio_value ? HotPlugDetectionState::kDetected : HotPlugDetectionState::kNotDetected;
}

// static
fuchsia_hardware_gpio::InterruptMode HotPlugDetection::InterruptModeForStateChange(
    HotPlugDetectionState current_state) {
  return (current_state == HotPlugDetectionState::kDetected)
             ? fuchsia_hardware_gpio::InterruptMode::kLevelLow
             : fuchsia_hardware_gpio::InterruptMode::kLevelHigh;
}

zx::result<> HotPlugDetection::UpdateState() {
  zx::result<HotPlugDetectionState> pin_state_result = ReadPinGpioState();
  if (pin_state_result.is_error()) {
    // ReadPinGpioState() already logged the error.
    return pin_state_result.take_error();
  }

  HotPlugDetectionState current_pin_state = pin_state_result.value();
  zx::result<> interrupt_mode_change_result;
  {
    fbl::AutoLock lock(&mutex_);
    if (current_pin_state == current_pin_state_) {
      return zx::ok();
    }
    current_pin_state_ = current_pin_state;

    interrupt_mode_change_result =
        SetPinInterruptMode(InterruptModeForStateChange(current_pin_state));
  }

  // We call the state change handler after setting the GPIO polarity so that
  // we don't miss a GPIO state change even if running the state change
  // handler takes a long time.
  on_state_change_(current_pin_state);

  return interrupt_mode_change_result;
}

void HotPlugDetection::InterruptHandler(async_dispatcher_t* dispatcher, async::IrqBase* irq,
                                        zx_status_t status,
                                        const zx_packet_interrupt_t* interrupt) {
  if (status == ZX_ERR_CANCELED) {
    fdf::info("Hotplug interrupt wait is cancelled.");
    return;
  }
  if (status != ZX_OK) {
    fdf::error("Hotplug interrupt wait failed: {}", zx::make_result(status));
    // A failed async interrupt wait doesn't remove the interrupt from the
    // async loop, so we have to manually cancel it.
    irq->Cancel();
    return;
  }

  // Undocumented magic. Probably a very simple approximation of debouncing.
  usleep(500000);

  [[maybe_unused]] zx::result<> result = UpdateState();
  // UpdateState() already logged the error.

  // For interrupts bound to ports (including those bound to async loops), the
  // interrupt must be re-armed using zx_interrupt_ack() for each incoming
  // interrupt request. This is best done after the interrupt has been fully
  // processed.
  zx::unowned_interrupt(irq->object())->ack();
}

zx::result<HotPlugDetectionState> HotPlugDetection::ReadPinGpioState() {
  fidl::WireResult<fuchsia_hardware_gpio::Gpio::Read> read_result = pin_gpio_->Read();
  if (!read_result.ok()) {
    fdf::error("Failed to send Read request to pin GPIO: {}", read_result.status_string());
    return zx::error(read_result.status());
  }

  fidl::WireResultUnwrapType<fuchsia_hardware_gpio::Gpio::Read>& read_response =
      read_result.value();
  if (read_response.is_error()) {
    fdf::error("Failed to read pin GPIO: {}", zx::make_result(read_response.error_value()));
    return read_response.take_error();
  }
  return zx::ok(GpioValueToState(read_response->value));
}

zx::result<> HotPlugDetection::SetPinInterruptMode(fuchsia_hardware_gpio::InterruptMode mode) {
  fidl::Arena arena;
  auto interrupt_config =
      fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena).mode(mode).Build();
  fidl::WireResult result = pin_gpio_->ConfigureInterrupt(interrupt_config);
  if (!result.ok()) {
    fdf::error("Failed to send ConfigureInterrupt request to hpd gpio: {}", result.status_string());
    return zx::error(result.status());
  }

  fidl::WireResultUnwrapType<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& response =
      result.value();
  if (response.is_error()) {
    fdf::error("Failed to reconfigure interrupt of hpd gpio: {}",
               zx::make_result(response.error_value()));
    return response.take_error();
  }
  return zx::ok();
}

}  // namespace amlogic_display
