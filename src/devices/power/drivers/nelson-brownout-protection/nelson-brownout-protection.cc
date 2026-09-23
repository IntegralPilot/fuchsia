// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "nelson-brownout-protection.h"

#include <fidl/fuchsia.hardware.power.sensor/cpp/wire.h>
#include <fuchsia/hardware/audio/cpp/banjo.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/trace/event.h>
#include <lib/zx/channel.h>
#include <zircon/threads.h>

#include <memory>

namespace {

constexpr zx::duration kVoltagePollInterval = zx::sec(5);
// AGL will be disabled once the voltage rises above this value.
constexpr float kVoltageUpwardThreshold = 11.5f;

}  // namespace

namespace brownout_protection {

zx_status_t CodecClientAgl::Init(fidl::ClientEnd<fuchsia_hardware_audio::Codec> codec_client_end) {
  fidl::WireSyncClient codec{std::move(codec_client_end)};

  zx::result signal_endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_audio_signalprocessing::SignalProcessing>();
  if (!signal_endpoints.is_ok()) {
    zxlogf(ERROR, "Failed to create signal processing endpoints: %s",
           signal_endpoints.status_string());
    return signal_endpoints.status_value();
  }
  auto signal_connect = codec->SignalProcessingConnect(std::move(signal_endpoints->server));
  if (!signal_connect.ok()) {
    zxlogf(ERROR, "Failed to call signal processing connect: %s", signal_connect.status_string());
    return signal_connect.status();
  }
  signal_processing_ = fidl::WireSyncClient(std::move(signal_endpoints->client));
  auto elements = signal_processing_->GetElements();
  if (!elements.ok()) {
    zxlogf(ERROR, "Failed to call signal processing get element: %s", elements.status_string());
    return elements.status();
  }
  for (auto& i : elements->value()->processing_elements) {
    if (i.has_id() && i.has_type() &&
        i.type() == fuchsia_hardware_audio_signalprocessing::ElementType::kAutomaticGainLimiter) {
      agl_id_.emplace(i.id());
      return ZX_OK;
    }
  }
  zxlogf(ERROR, "Failed find AGL element");
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t CodecClientAgl::SetAgl(bool enable) {
  if (!agl_id_.has_value()) {
    zxlogf(ERROR, "No AGL element available");
    return ZX_ERR_NOT_SUPPORTED;
  }
  fidl::Arena arena;
  auto state = fuchsia_hardware_audio_signalprocessing::wire::SettableElementState::Builder(arena);
  state.started(true).bypassed(!enable);
  auto set_state = signal_processing_->SetElementState(agl_id_.value(), state.Build());
  if (!set_state.ok()) {
    zxlogf(ERROR, "Failed to call signal processing set element state: %s",
           set_state.status_string());
    return set_state.status();
  }
  return ZX_OK;
}

zx_status_t NelsonBrownoutProtection::Create(void* ctx, zx_device_t* parent) {
  return NelsonBrownoutProtection::Create(ctx, parent, kVoltagePollInterval);
}

zx_status_t NelsonBrownoutProtection::Create(void* ctx, zx_device_t* parent,
                                             zx::duration voltage_poll_interval) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_audio::Codec>> codec_client_end =
      DdkConnectFragmentFidlProtocol<fuchsia_hardware_audio::CodecService::Codec>(parent, "codec");
  if (codec_client_end.is_error()) {
    zxlogf(ERROR, "No codec fragment: %s", zx_status_get_string(codec_client_end.status_value()));
    return codec_client_end.status_value();
  }

  zx::result client =
      DdkConnectFragmentFidlProtocol<fuchsia_hardware_power_sensor::Service::Device>(
          parent, "power-sensor");
  if (client.is_error()) {
    zxlogf(ERROR, "Failed to connect to FIDL fragment: %s", client.status_string());
    return client.status_value();
  }

  fidl::WireSyncClient power_sensor_client(std::move(client.value()));

  const char* kAlertGpioFragmentname = "alert-gpio";
  zx::result alert_gpio_result =
      ddk::Device<void>::DdkConnectFragmentFidlProtocol<fuchsia_hardware_gpio::Service::Device>(
          parent, kAlertGpioFragmentname);
  if (alert_gpio_result.is_error()) {
    zxlogf(ERROR, "Failed to get gpio protocol from fragment %s: %s", kAlertGpioFragmentname,
           alert_gpio_result.status_string());
    return alert_gpio_result.status_value();
  }
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> alert_gpio(
      std::move(alert_gpio_result.value()));

  {
    fidl::WireResult result = alert_gpio->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send SetBufferMode request: %s", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure gpio to input: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  fidl::Arena arena;
  auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                              .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeLow)
                              .Build();
  fidl::WireResult configure_result = alert_gpio->ConfigureInterrupt(interrupt_config);
  if (!configure_result.ok()) {
    zxlogf(ERROR, "Failed to send ConfigureInterrupt request to gpio: %s",
           configure_result.status_string());
    return configure_result.status();
  }
  if (configure_result->is_error()) {
    zxlogf(ERROR, "Failed to configure interrupt: %s",
           zx_status_get_string(configure_result->error_value()));
    return configure_result->error_value();
  }

  fidl::WireResult alert_interrupt = alert_gpio->GetInterrupt({});
  if (!alert_interrupt.ok()) {
    zxlogf(ERROR, "Failed to send GetInterrupt request: %s", alert_interrupt.status_string());
    return alert_interrupt.status();
  }
  if (alert_interrupt->is_error()) {
    zxlogf(ERROR, "Failed to get interrupt from gpio: %s",
           zx_status_get_string(alert_interrupt->error_value()));
    return alert_interrupt->error_value();
  }

  zx_status_t status;
  auto dev = std::make_unique<NelsonBrownoutProtection>(
      parent, std::move(power_sensor_client), alert_gpio.TakeClientEnd(),
      std::move(alert_interrupt.value()->interrupt), voltage_poll_interval);
  if ((status = dev->Init(std::move(*codec_client_end))) != ZX_OK) {
    return status;
  }

  if ((status = dev->DdkAdd("nelson-brownout-protection")) != ZX_OK) {
    zxlogf(ERROR, "DdkAdd failed: %s", zx_status_get_string(status));
    return status;
  }

  [[maybe_unused]] auto* _ = dev.release();
  return ZX_OK;
}

zx_status_t NelsonBrownoutProtection::Init(
    fidl::ClientEnd<fuchsia_hardware_audio::Codec> codec_client_end) {
  zx_status_t status = codec_.Init(std::move(codec_client_end));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to connect to codec driver: %s", zx_status_get_string(status));
    return status;
  }
  status = thrd_status_to_zx_status(thrd_create_with_name(
      &thread_,
      [](void* ctx) -> int { return reinterpret_cast<NelsonBrownoutProtection*>(ctx)->Thread(); },
      this, "Brownout protection thread"));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to start thread: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

int NelsonBrownoutProtection::Thread() {
  {
    // AGL should be enabled at most 4ms after the power sensor raises an interrupt. The capacity
    // was chosen through experimentation -- too low and page faults end up using most of the time.
    // This is especially noticeable with the codec driver.
    const char* role_name = "fuchsia.devices.power.drivers.nelson-brownout-protection";
    const zx_status_t status = device_set_profile_by_role(parent_, thrd_get_zx_handle(thread_),
                                                          role_name, strlen(role_name));
    if (status != ZX_OK) {
      zxlogf(WARNING, "Failed to set role: %s", zx_status_get_string(status));
    }
  }

  zx::time_boot timestamp = {};
  while (run_thread_ && alert_interrupt_.wait(&timestamp) == ZX_OK) {
    {
      TRACE_DURATION("brownout-protection", "Enable AGL", "timestamp", timestamp.get());
      zx_status_t status = codec_.SetAgl(true);
      if (status != ZX_OK) {
        zxlogf(WARNING, "Failed to enable AGL: %s", zx_status_get_string(status));
      }
    }

    while (run_thread_) {
      zx::nanosleep(zx::deadline_after(voltage_poll_interval_));
      const auto result = power_sensor_->GetVoltageVolts();
      if (result.ok() && result->value()->voltage >= kVoltageUpwardThreshold) {
        break;
      }
    }

    zx_status_t status = codec_.SetAgl(false);
    if (status != ZX_OK) {
      zxlogf(WARNING, "Failed to disable AGL: %s", zx_status_get_string(status));
    }
  }

  return thrd_success;
}

static constexpr zx_driver_ops_t nelson_brownout_protection_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = NelsonBrownoutProtection::Create;
  return ops;
}();

}  // namespace brownout_protection

ZIRCON_DRIVER(nelson_brownout_protection,
              brownout_protection::nelson_brownout_protection_driver_ops, "zircon", "0.1");
