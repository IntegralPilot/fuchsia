// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "buttons-device.h"

#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/clock.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <algorithm>
#include <cinttypes>
#include <cstddef>

#include <fbl/alloc_checker.h>

namespace buttons {

void ButtonsDevice::ButtonsInputReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  fidl::VectorView<fuchsia_input_report::wire::ConsumerControlButton> buttons_rpt(
      allocator, fuchsia_input_report::wire::kConsumerControlMaxNumButtons);
  size_t count = 0;
  bool mic_mute = false;
  bool cam_mute = false;
  for (uint32_t id = 0; id < buttons.size(); id++) {
    if (!buttons[id]) {
      continue;
    }

    switch (static_cast<fuchsia_buttons::GpioButtonId>(id)) {
      case fuchsia_buttons::GpioButtonId::kPower:
        buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kPower;
        count++;
        break;
      case fuchsia_buttons::GpioButtonId::kVolumeUp:
        buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kVolumeUp;
        count++;
        break;
      case fuchsia_buttons::GpioButtonId::kVolumeDown:
        buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kVolumeDown;
        count++;
        break;
      case fuchsia_buttons::GpioButtonId::kFdr:
        buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kFactoryReset;
        count++;
        break;
      case fuchsia_buttons::GpioButtonId::kMicMute:
        if (!mic_mute) {
          buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kMicMute;
          count++;
          mic_mute = true;
        }
        break;
      case fuchsia_buttons::GpioButtonId::kCamMute:
        if (!cam_mute) {
          buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kCameraDisable;
          count++;
          cam_mute = true;
        }
        break;
      case fuchsia_buttons::GpioButtonId::kMicAndCamMute:
        if (!mic_mute) {
          buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kMicMute;
          count++;
          mic_mute = true;
        }
        if (!cam_mute) {
          buttons_rpt[count] = fuchsia_input_report::ConsumerControlButton::kCameraDisable;
          count++;
          cam_mute = true;
        }
        break;
      default:
        fdf::error("Invalid Button ID encountered {}", static_cast<uint32_t>(id));
    }
  }
  buttons_rpt.set_size(count);

  auto consumer_control =
      fuchsia_input_report::wire::ConsumerControlInputReport::Builder(allocator).pressed_buttons(
          buttons_rpt);
  input_report.event_time(event_time.get()).consumer_control(consumer_control.Build());
}

void ButtonsDevice::Notify(size_t button_index) {
  zx::result<ButtonsInputReport> result = GetInputReportInternal();
  if (result.is_error()) {
    fdf::error("GetInputReport failed {}", zx_status_get_string(result.error_value()));
  } else if (!last_report_.has_value() || *last_report_ != result.value()) {
    last_report_ = result.value();
    readers_.SendReportToAllReaders(*last_report_);

    if (debounce_states_[button_index].timestamp != zx::time::infinite_past()) {
      const zx::duration latency =
          zx::clock::get_monotonic() - debounce_states_[button_index].timestamp;

      total_latency_ += latency;
      report_count_++;
      average_latency_usecs_.Set(total_latency_.to_usecs() / report_count_);

      if (latency > max_latency_) {
        max_latency_ = latency;
        max_latency_usecs_.Set(max_latency_.to_usecs());
      }
    }

    if (!last_report_->empty()) {
      total_report_count_.Add(1);
      last_event_timestamp_.Set(last_report_->event_time.get());
    }
  }
  if (buttons_[button_index].id() == fuchsia_buttons::GpioButtonId::kFdr) {
    fdf::info("FDR (up and down buttons) pressed");
  }

  debounce_states_[button_index].enqueued = false;
  debounce_states_[button_index].timestamp = zx::time::infinite_past();
}

int ButtonsDevice::Thread() {
  thread_started_.Signal();
  if (poll_period_ != zx::duration::infinite()) {
    poll_timer_.set(zx::deadline_after(poll_period_), zx::duration(0));
    poll_timer_.wait_async(port_, kPortKeyPollTimer, ZX_TIMER_SIGNALED, 0);
  }

  while (true) {
    zx_port_packet_t packet;
    zx_status_t status = port_.wait(zx::time::infinite(), &packet);
    fdf::debug("msg received on port key {}", packet.key);
    if (status != ZX_OK) {
      fdf::error("port wait failed {}", status);
      return thrd_error;
    }

    if (packet.key == kPortKeyShutDown) {
      fdf::info("shutting down");
      return thrd_success;
    }

    if (packet.key >= kPortKeyInterruptStart &&
        packet.key < (kPortKeyInterruptStart + buttons_.size())) {
      uint32_t type = static_cast<uint32_t>(packet.key - kPortKeyInterruptStart);
      const std::optional<fuchsia_buttons::GpioType>& gpio_type = gpios_[type].config.type();
      if (!gpio_type.has_value()) {
        fdf::error("Config for gpio {} missing type", type);
        return thrd_error;
      }
      if (gpio_type.value().Which() == fuchsia_buttons::GpioType::Tag::kInterrupt) {
        const zx::duration kLeaseTimeout = zx::msec(100);
        wake_lease_.HandleInterrupt(kLeaseTimeout);

        // We need to reconfigure the GPIO to catch the opposite polarity.
        auto reconfig_result = ReconfigurePolarity(type, packet.key);
        if (!reconfig_result.is_ok()) {
          return reconfig_result.error_value();
        }
        debounce_states_[type].value = *reconfig_result;

        // Notify
        debounce_states_[type].timer.set(zx::deadline_after(zx::duration(kDebounceThresholdNs)),
                                         zx::duration(0));
        if (!debounce_states_[type].enqueued) {
          debounce_states_[type].timer.wait_async(port_, kPortKeyTimerStart + type,
                                                  ZX_TIMER_SIGNALED, 0);
          debounce_states_[type].timestamp = zx::time(packet.interrupt.timestamp);
        }
        debounce_states_[type].enqueued = true;
      }

      gpios_[type].irq.ack();
    }

    if (packet.key >= kPortKeyTimerStart && packet.key < (kPortKeyTimerStart + buttons_.size())) {
      Notify(packet.key - kPortKeyTimerStart);
    }

    if (packet.key == kPortKeyPollTimer) {
      for (size_t i = 0; i < gpios_.size(); i++) {
        const std::optional<fuchsia_buttons::GpioType>& gpio_type = gpios_[i].config.type();
        if (!gpio_type.has_value()) {
          fdf::error("Config for gpio {} missing type", i);
          return thrd_error;
        }
        if (gpio_type.value().Which() != fuchsia_buttons::GpioType::Tag::kPoll) {
          continue;
        }

        fidl::WireResult read_result = gpios_[i].client->Read();
        if (!read_result.ok()) {
          fdf::error("Failed to send Read request to gpio {}: {}", i, read_result.status_string());
          return read_result.status();
        }
        if (read_result->is_error()) {
          fdf::error("Failed to read gpio {}: {}", i,
                     zx_status_get_string(read_result->error_value()));
          return read_result->error_value();
        }
        if (read_result.value()->value != debounce_states_[i].value) {
          Notify(i);
        }
        debounce_states_[i].value = read_result.value()->value;
      }

      poll_timer_.set(zx::deadline_after(poll_period_), zx::duration(0));
      poll_timer_.wait_async(port_, kPortKeyPollTimer, ZX_TIMER_SIGNALED, 0);
    }
  }
  return thrd_success;
}

void ButtonsDevice::GetInputReportsReader(GetInputReportsReaderRequestView request,
                                          GetInputReportsReaderCompleter::Sync& completer) {
  ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
}

void ButtonsDevice::GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                                            GetInputReportsReaderV2Completer::Sync& completer) {
  zx::result<ButtonsInputReport> initial_report = GetInputReportInternal();
  if (initial_report.is_error()) {
    fdf::error("Failed to get initial report {}", initial_report.status_string());
  }
  const uint16_t max_unacknowledged_reports =
      std::clamp<uint16_t>(request->max_unacknowledged_reports_limit, 1, kMaxReportsPerHalfSecond);
  if (request->max_unacknowledged_reports_limit != max_unacknowledged_reports) {
    fdf::warn("GetInputReportsReaderV2: requested limit {} clamped to {}",
              request->max_unacknowledged_reports_limit, max_unacknowledged_reports);
  }
  zx_status_t status = readers_.CreateReaderV2(
      dispatcher_, std::move(request->reader), max_unacknowledged_reports,
      initial_report.is_ok() ? std::make_optional(initial_report.value()) : std::nullopt);
  if (status != ZX_OK) {
    fdf::error("Failed to create a reader v2 {}", zx_status_get_string(status));
    completer.Close(status);
    return;
  }
  completer.Reply(max_unacknowledged_reports);
}

void ButtonsDevice::GetDescriptor(GetDescriptorCompleter::Sync& completer) {
  fidl::Arena<kFeatureAndDescriptorBufferSize> arena;

  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(arena);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::VendorId::kGoogle));
  // Product id is "HID" buttons only for backward compatibility with users of this driver.
  // There is no HID support in this driver anymore.
  device_info.product_id(
      static_cast<uint32_t>(fuchsia_input_report::VendorGoogleProductId::kHidButtons));

  const std::vector<fuchsia_input_report::ConsumerControlButton> buttons = {
      fuchsia_input_report::ConsumerControlButton::kVolumeUp,
      fuchsia_input_report::ConsumerControlButton::kVolumeDown,
      fuchsia_input_report::ConsumerControlButton::kFactoryReset,
      fuchsia_input_report::ConsumerControlButton::kCameraDisable,
      fuchsia_input_report::ConsumerControlButton::kMicMute,
      fuchsia_input_report::ConsumerControlButton::kPower};

  const auto input = fuchsia_input_report::wire::ConsumerControlInputDescriptor::Builder(arena)
                         .buttons(buttons)
                         .Build();

  const auto consumer_control =
      fuchsia_input_report::wire::ConsumerControlDescriptor::Builder(arena).input(input).Build();

  completer.Reply(fuchsia_input_report::wire::DeviceDescriptor::Builder(arena)
                      .device_information(device_info.Build())
                      .consumer_control(consumer_control)
                      .Build());
}

// Requires interrupts to be disabled for all rows/cols.
zx::result<bool> ButtonsDevice::MatrixScan(uint32_t row, uint32_t col, zx_duration_t delay) {
  auto& gpio_col = gpios_[col];
  {
    fidl::WireResult result = gpio_col.client->SetBufferMode(
        fuchsia_hardware_gpio::BufferMode::kInput);  // Float column to find row in use.
    if (!result.ok()) {
      fdf::error("Failed to send SetBufferMode request to gpio {}: {}", col,
                 result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("Failed to configuire gpio {} to input: {}", col,
                 zx_status_get_string(result->error_value()));
      return zx::error(result->error_value());
    }
  }
  zx::nanosleep(zx::deadline_after(zx::duration(delay)));

  fidl::WireResult read_result = gpios_[row].client->Read();
  if (!read_result.ok()) {
    fdf::error("Failed to send Read request to gpio {}: {}", row, read_result.status_string());
    return zx::error(read_result.status());
  }
  if (read_result->is_error()) {
    fdf::error("Failed to read gpio {}: {}", row, zx_status_get_string(read_result->error_value()));
    return zx::error(read_result->error_value());
  }

  {
    const std::optional<fuchsia_buttons::GpioType>& type = gpio_col.config.type();
    if (!type.has_value()) {
      fdf::error("Gpio config missing type");
      return zx::error(ZX_ERR_BAD_STATE);
    }
    const std::optional<uint8_t> output_value =
        gpio_col.config.type()->matrix_output()->output_value();
    if (!output_value.has_value()) {
      fdf::error("Gpio config missing output value");
      return zx::error(ZX_ERR_BAD_STATE);
    }
    fidl::WireResult result = gpio_col.client->SetBufferMode(
        output_value.value() ? fuchsia_hardware_gpio::BufferMode::kOutputHigh
                             : fuchsia_hardware_gpio::BufferMode::kOutputLow);
    if (!result.ok()) {
      fdf::error("Failed to send SetBufferMode request to gpio {}: {}", col,
                 result.status_string());
      return zx::error(read_result.status());
    }
    if (result->is_error()) {
      fdf::error("Failed to configuire gpio {} to output: {}", col,
                 zx_status_get_string(result->error_value()));
      return zx::error(result->error_value());
    }
  }
  fdf::debug("row {} col {} val {}", row, col, read_result.value()->value);
  return zx::ok(read_result.value()->value);
}

zx::result<ButtonsDevice::ButtonsInputReport> ButtonsDevice::GetInputReportInternal() {
  ButtonsInputReport input_rpt;

  for (size_t i = 0; i < buttons_.size(); ++i) {
    bool new_value = false;  // A value true means a button is pressed.
    const fuchsia_buttons::GpioButtonConfig& button = buttons_[i];
    const std::optional<fuchsia_buttons::GpioButtonType>& button_type = button.type();
    if (!button_type.has_value()) {
      fdf::error("Button {} missing type", i);
      return zx::error(ZX_ERR_BAD_STATE);
    }
    const std::optional<uint8_t>& gpio_a_index = button.gpio_a_index();
    if (!gpio_a_index.has_value()) {
      fdf::error("Button {} missing gpio A index", i);
      return zx::error(ZX_ERR_BAD_STATE);
    }
    const std::optional<uint8_t>& gpio_delay = button.gpio_a_index();
    if (!gpio_delay.has_value()) {
      fdf::error("Button {} missing gpio delay", i);
      return zx::error(ZX_ERR_BAD_STATE);
    }
    switch (button_type.value().Which()) {
      case fuchsia_buttons::GpioButtonType::Tag::kMatrix: {
        const fuchsia_buttons::MatrixGpioButton& matrix = button_type.value().matrix().value();
        const std::optional<uint8_t>& gpio_b_index = matrix.gpio_b_index();
        if (!gpio_b_index.has_value()) {
          fdf::error("Button {} missing gpio B index", i);
          return zx::error(ZX_ERR_BAD_STATE);
        }

        zx::result scan_result =
            MatrixScan(gpio_a_index.value(), gpio_b_index.value(), gpio_delay.value());
        if (!scan_result.is_ok()) {
          return zx::error(scan_result.error_value());
        }
        new_value = *scan_result;
        break;
      }
      case fuchsia_buttons::GpioButtonType::Tag::kDirect: {
        const uint8_t gpio_index = gpio_a_index.value();
        fidl::WireResult read_result = gpios_[gpio_index].client->Read();
        if (!read_result.ok()) {
          fdf::error("Failed to send Read request to gpio {}: {}", gpio_index,
                     read_result.status_string());
          return zx::error(read_result.status());
        }
        if (read_result->is_error()) {
          fdf::error("Failed to read gpio {}: {}", gpio_index,
                     zx_status_get_string(read_result->error_value()));
          return zx::error(read_result->error_value());
        }

        new_value = read_result.value()->value;
        fdf::debug("GPIO direct read {} for button {}", new_value, i);
        break;
      }
      default:
        fdf::error("Button {} has unknown type {}", i, static_cast<uint32_t>(button_type->Which()));
        return zx::error(ZX_ERR_INTERNAL);
    }

    const std::optional<fuchsia_buttons::GpioFlag>& flags = gpios_[i].config.flags();
    if (!flags.has_value()) {
      fdf::error("Config for gpio {} missing flags", i);
      return zx::error(ZX_ERR_BAD_STATE);
    }
    if (flags.value() & fuchsia_buttons::GpioFlag::kInverted) {
      new_value = !new_value;
    }

    fdf::debug("GPIO new value {} for button {}", new_value, i);
    const std::optional<fuchsia_buttons::GpioButtonId>& button_id = button.id();
    if (!button_id.has_value()) {
      fdf::error("Button {} missing id", i);
      return zx::error(ZX_ERR_BAD_STATE);
    }
    input_rpt.set(static_cast<uint32_t>(button_id.value()), new_value);
  }
  input_rpt.event_time = zx::clock::get_monotonic();

  return zx::ok(input_rpt);
}

void ButtonsDevice::GetInputReport(GetInputReportRequestView request,
                                   GetInputReportCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(false);
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

zx::result<bool> ButtonsDevice::ReconfigurePolarity(size_t idx, uint64_t int_port) {
  fdf::debug("gpio {} port {}", idx, int_port);
  bool current = false, old;
  auto& gpio = gpios_[idx];

  fidl::WireResult read_result1 = gpio.client->Read();
  if (!read_result1.ok()) {
    fdf::error("Failed to send Read request to gpio {}: {}", idx, read_result1.status_string());
    return zx::error(read_result1.status());
  }
  if (read_result1->is_error()) {
    fdf::error("Failed to read gpio {}: {}", idx,
               zx_status_get_string(read_result1->error_value()));
    return zx::error(read_result1->error_value());
  }
  current = read_result1.value()->value;

  fidl::Arena arena;
  auto config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                    .mode(current ? fuchsia_hardware_gpio::InterruptMode::kEdgeLow
                                  : fuchsia_hardware_gpio::InterruptMode::kEdgeHigh)
                    .Build();
  do {
    {
      fidl::WireResult result = gpio.client->ConfigureInterrupt(config);
      if (!result.ok()) {
        fdf::error("Failed to send ConfigureInterrupt request to gpio {}: {}", idx,
                   result.status_string());
        return zx::error(result.status());
      }
      if (result->is_error()) {
        fdf::error("Failed to set interrupt configuration of gpio {}: {}", idx,
                   zx_status_get_string(result->error_value()));
        return zx::error(result->error_value());
      }
    }

    old = current;
    fidl::WireResult read_result2 = gpio.client->Read();
    if (!read_result2.ok()) {
      fdf::error("Failed to send Read request to gpio {}: {}", idx, read_result2.status_string());
      return zx::error(read_result2.status());
    }
    if (read_result2->is_error()) {
      fdf::error("Failed to read gpio {}: {}", idx,
                 zx_status_get_string(read_result2->error_value()));
      return zx::error(read_result2->error_value());
    }
    current = read_result2.value()->value;
    fdf::trace("{} old gpio {} new gpio {}", idx, old, current);
    // If current switches after setup, we setup a new trigger for it (opposite edge).
  } while (current != old);
  return zx::ok(current);
}

zx_status_t ButtonsDevice::ConfigureInterrupt(size_t idx, uint64_t int_port) {
  fdf::debug("gpio {} port {}", idx, int_port);
  zx_status_t status;
  bool current = false;
  auto& gpio = gpios_[idx];

  const fidl::WireResult read_result = gpio.client->Read();
  if (!read_result.ok()) {
    fdf::error("Failed to send Read request to gpio {}: {}", idx, read_result.status_string());
    return read_result.status();
  }
  if (read_result->is_error()) {
    fdf::error("Failed to read gpio {}: {}", idx, zx_status_get_string(read_result->error_value()));
    return read_result->error_value();
  }
  current = read_result.value()->value;

  {
    const fidl::WireResult result = gpio.client->ReleaseInterrupt();
    if (!result.ok()) {
      fdf::error("Failed to send ReleaseInterrupt request to gpio {}: {}", idx,
                 result.status_string());
      return result.status();
    }
    if (result->is_error() && result->error_value() != ZX_ERR_NOT_FOUND) {
      fdf::error("Failed to release interrupt for gpio {}: {}", idx,
                 zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  fidl::Arena arena;
  // We setup a trigger for the opposite of the current GPIO value.
  auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                              .mode(current ? fuchsia_hardware_gpio::InterruptMode::kEdgeLow
                                            : fuchsia_hardware_gpio::InterruptMode::kEdgeHigh)
                              .Build();
  fidl::WireResult configure_result = gpio.client->ConfigureInterrupt(interrupt_config);
  if (!configure_result.ok()) {
    fdf::error("Failed to send ConfigureInterrupt request to gpio {}: {}", idx,
               configure_result.status_string());
    return configure_result.status();
  }
  if (configure_result->is_error()) {
    fdf::error("Failed to configure interrupt for gpio {}: {}", idx,
               zx_status_get_string(configure_result->error_value()));
    return configure_result->error_value();
  }

  fuchsia_hardware_gpio::InterruptOptions interrupt_options = {};
  const auto& flags = gpio.config.flags();
  if (!flags.has_value()) {
    fdf::error("Config for gpio {} missing flags", idx);
    return ZX_ERR_BAD_STATE;
  }

  fidl::WireResult interrupt_result = gpio.client->GetInterrupt(interrupt_options);
  if (!interrupt_result.ok()) {
    fdf::error("Failed to send GetInterrupt request to gpio {}: {}", idx,
               interrupt_result.status_string());
    return interrupt_result.status();
  }
  if (interrupt_result->is_error()) {
    fdf::error("Failed to get interrupt for gpio {}: {}", idx,
               zx_status_get_string(interrupt_result->error_value()));
    return interrupt_result->error_value();
  }
  gpio.irq = std::move(interrupt_result.value()->interrupt);

  status = gpios_[idx].irq.bind(port_, int_port, 0);
  if (status != ZX_OK) {
    fdf::error("zx_interrupt_bind failed {}", status);
    return status;
  }
  // To make sure polarity is correct in case it changed during configuration.
  auto reconfig_result = ReconfigurePolarity(idx, int_port);
  if (!reconfig_result.is_ok()) {
    return reconfig_result.error_value();
  }
  return ZX_OK;
}

ButtonsDevice::ButtonsDevice(async_dispatcher_t* dispatcher,
                             std::vector<fuchsia_buttons::GpioButtonConfig> buttons,
                             std::vector<Gpio> gpios,
                             fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag_client)
    : dispatcher_(dispatcher),
      buttons_(std::move(buttons)),
      gpios_(std::move(gpios)),
      wake_lease_(dispatcher, "buttons-driver-wake", std::move(sag_client), nullptr, true) {
  ZX_ASSERT(Init() == ZX_OK);
}

zx_status_t ButtonsDevice::Init() {
  zx_status_t status;
  fbl::AllocChecker ac;

  metrics_root_ = inspector_.GetRoot().CreateChild("hid-input-report-touch");
  average_latency_usecs_ = metrics_root_.CreateUint("average_latency_usecs", 0);
  max_latency_usecs_ = metrics_root_.CreateUint("max_latency_usecs", 0);
  total_report_count_ = metrics_root_.CreateUint("total_report_count", 0);
  last_event_timestamp_ = metrics_root_.CreateUint("last_event_timestamp", 0);

  status = zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port_);
  if (status != ZX_OK) {
    fdf::error("port_create failed {}", status);
    return status;
  }

  debounce_states_ = fbl::Array(new (&ac) debounce_state[buttons_.size()], buttons_.size());
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  for (auto& i : debounce_states_) {
    i.enqueued = false;
    zx::timer::create(0, ZX_CLOCK_MONOTONIC, &(i.timer));
    i.value = false;
  }

  zx::timer::create(0, ZX_CLOCK_MONOTONIC, &poll_timer_);

  // Check the metadata.
  for (size_t i = 0; i < buttons_.size(); ++i) {
    const fuchsia_buttons::GpioButtonConfig& button = buttons_[i];
    if (!button.gpio_a_index()) {
      fdf::error("Button {} missing gpio A index", i);
      return ZX_ERR_INTERNAL;
    }
    const uint8_t gpio_a_index = button.gpio_a_index().value();
    if (gpio_a_index >= gpios_.size()) {
      fdf::error("Invalid gpio A index {}", gpio_a_index);
      return ZX_ERR_INTERNAL;
    }
    if (!button.type().has_value()) {
      fdf::error("Button {} missing id", i);
      return ZX_ERR_INTERNAL;
    }
    const fuchsia_buttons::GpioButtonType& button_type = button.type().value();
    if (button_type.Which() == fuchsia_buttons::GpioButtonType::Tag::kMatrix) {
      const fuchsia_buttons::MatrixGpioButton& matrix = button_type.matrix().value();
      if (!matrix.gpio_b_index().has_value()) {
        fdf::error("Button {} missing gpio B index", i);
        return ZX_ERR_INTERNAL;
      }
      const uint8_t gpio_b_index = matrix.gpio_b_index().value();
      if (gpio_b_index >= gpios_.size()) {
        fdf::error("Invalid gpio B index {}", gpio_b_index);
        return ZX_ERR_INTERNAL;
      }
      const Gpio& gpio_b = gpios_[gpio_b_index];
      if (!gpio_b.config.type().has_value()) {
        fdf::error("Config for gpio {} missing period", gpio_b_index);
        return ZX_ERR_INTERNAL;
      }
      const fuchsia_buttons::GpioType& gpio_b_type = gpio_b.config.type().value();
      if (gpio_b_type.Which() != fuchsia_buttons::GpioType::Tag::kMatrixOutput) {
        fdf::error("Config for matrix gpio B has invalid type {}",
                   static_cast<uint32_t>(gpio_b_type.Which()));
        return ZX_ERR_INTERNAL;
      }
    }
    const Gpio& gpio_a = gpios_[gpio_a_index];
    if (!gpio_a.config.type().has_value()) {
      fdf::error("Config for gpio {} missing type", gpio_a_index);
      return ZX_ERR_INTERNAL;
    }
    const fuchsia_buttons::GpioType& gpio_a_type = gpio_a.config.type().value();
    switch (gpio_a_type.Which()) {
      case fuchsia_buttons::GpioType::Tag::kInterrupt:
        break;
      case fuchsia_buttons::GpioType::Tag::kPoll: {
        const fuchsia_buttons::PollGpio& poll = gpio_a_type.poll().value();
        if (!poll.period().has_value()) {
          fdf::error("Config for gpio {} missing period", gpio_a_index);
          return ZX_ERR_INTERNAL;
        }
        const zx::duration poll_period = zx::duration(poll.period().value());
        if (poll_period_ == zx::duration::infinite()) {
          poll_period_ = poll_period;
        }
        if (poll_period != poll_period_) {
          fdf::error("GPIOs must have the same poll period");
          return ZX_ERR_INTERNAL;
        }
        break;
      }
      default:
        fdf::error("Config for gpio {} has invalid type {}", gpio_a_index,
                   static_cast<uint32_t>(gpio_a_type.Which()));
        return ZX_ERR_INTERNAL;
    }
    if (!button.id().has_value()) {
      fdf::error("Button {} missing id", i);
      return ZX_ERR_INTERNAL;
    }
    const fuchsia_buttons::GpioButtonId button_id = button.id().value();
    if (button_id == fuchsia_buttons::GpioButtonId::kFdr) {
      fdf::info("FDR (up and down buttons) setup to GPIO {}", gpio_a_index);
    }
  }

  // Setup.
  for (size_t i = 0; i < gpios_.size(); ++i) {
    const Gpio& gpio = gpios_[i];
    if (!gpio.config.type().has_value()) {
      fdf::error("Config for gpio {} missing type", i);
      return ZX_ERR_BAD_STATE;
    }

    const fuchsia_buttons::GpioType& gpio_type = gpio.config.type().value();
    switch (gpio_type.Which()) {
      case fuchsia_buttons::GpioType::Tag::kMatrixOutput: {
        const fuchsia_buttons::MatrixOutputGpio& matrix = gpio_type.matrix_output().value();
        if (!matrix.output_value().has_value()) {
          fdf::error("Config for gpio {} missing output value", i);
          return ZX_ERR_BAD_STATE;
        }
        const fidl::WireResult result = gpio.client->SetBufferMode(
            matrix.output_value().value() ? fuchsia_hardware_gpio::BufferMode::kOutputHigh
                                          : fuchsia_hardware_gpio::BufferMode::kOutputLow);
        if (!result.ok()) {
          fdf::error("Failed to send SetBufferMode request to gpio {}: {}", i,
                     result.status_string());
          return result.status();
        }
        if (result->is_error()) {
          fdf::error("Failed to configure gpio {} to output: {}", i,
                     zx_status_get_string(result->error_value()));
          return ZX_ERR_NOT_SUPPORTED;
        }
        break;
      }
      case fuchsia_buttons::GpioType::Tag::kInterrupt: {
        const fidl::WireResult result =
            gpio.client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
        if (!result.ok()) {
          fdf::error("Failed to send SetBufferMode request to gpio {}: {}", i,
                     result.status_string());
          return result.status();
        }
        if (result->is_error()) {
          fdf::error("Failed to configure gpio {} to input: {}", i,
                     zx_status_get_string(result->error_value()));
          return ZX_ERR_NOT_SUPPORTED;
        }
        status = ConfigureInterrupt(i, kPortKeyInterruptStart + i);
        if (status != ZX_OK) {
          return status;
        }
        break;
      }
      case fuchsia_buttons::GpioType::Tag::kPoll: {
        const fidl::WireResult result =
            gpio.client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
        if (!result.ok()) {
          fdf::error("Failed to send SetBufferMode request to gpio {}: {}", i,
                     result.status_string());
          return result.status();
        }
        if (result->is_error()) {
          fdf::error("Failed to configure gpio {} to input: {}", i,
                     zx_status_get_string(result->error_value()));
          return ZX_ERR_NOT_SUPPORTED;
        }
        break;
      }
      default:
        fdf::error("Config for gpio {} has unknown type {}", i,
                   static_cast<uint32_t>(gpio_type.Which()));
        break;
    }
  }

  auto f = [](void* arg) -> int { return reinterpret_cast<ButtonsDevice*>(arg)->Thread(); };
  const int rc = thrd_create_with_name(&thread_, f, this, "buttons-thread");
  if (rc != thrd_success) {
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

void ButtonsDevice::ShutDown() {
  zx_port_packet packet = {kPortKeyShutDown, ZX_PKT_TYPE_USER, ZX_OK, {}};
  const zx_status_t status = port_.queue(&packet);
  ZX_ASSERT(status == ZX_OK);
  thread_started_.Wait();
  thrd_join(thread_, NULL);
  for (Gpio& gpio : gpios_) {
    if (gpio.irq.is_valid()) {
      std::move(gpio.irq).destroy();
    }
  }
}

}  // namespace buttons
