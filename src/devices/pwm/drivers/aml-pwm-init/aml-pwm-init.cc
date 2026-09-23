// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-pwm-init.h"

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <unistd.h>

#include <cstring>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/pwm/cpp/bind.h>
#include <fbl/alloc_checker.h>

namespace pwm_init {

const char* kWifiClkFragName = "wifi-32k768-clk";
const char* kDeviceName = "aml-pwm-init";

zx::result<> PwmInitDriver::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx_status_t status;
  zx::result init_result = compat_server_.Initialize(incoming, outgoing(), context.node_name(),
                                                     kDeviceName, compat::ForwardMetadata::All());
  if (init_result.is_error()) {
    fdf::error("Failed to initialize compat server, st = {}", init_result);
    return init_result.take_error();
  }

  zx::result clock_result =
      incoming->Connect<fuchsia_hardware_clock::Service::Clock>(kWifiClkFragName);
  if (clock_result.is_error()) {
    fdf::error("Failed to initialize Clock Client, st = {}", clock_result);
    return clock_result.take_error();
  }

  zx::result client_end = incoming->Connect<fuchsia_hardware_pwm::Service::Pwm>("pwm");
  if (client_end.is_error()) {
    fdf::error("Failed to initialize PWM Client, st = {}", client_end);
    return client_end.take_error();
  }
  fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> pwm(std::move(client_end.value()));

  const char* kBtGpioFragmentName = "gpio-bt";
  zx::result bt_gpio =
      incoming->Connect<fuchsia_hardware_gpio::Service::Device>(kBtGpioFragmentName);
  if (bt_gpio.is_error()) {
    fdf::error("Failed to get gpio FIDL protocol from fragment {}: {}", kBtGpioFragmentName,
               bt_gpio);
    return bt_gpio.take_error();
  }

  initer_ = std::make_unique<PwmInitDevice>(std::move(clock_result.value()), std::move(pwm),
                                            std::move(bt_gpio.value()));

  if ((status = initer_->Init()) != ZX_OK) {
    fdf::error("could not initialize PWM for bluetooth and SDIO. st = {}",
               zx_status_get_string(status));
    return zx::error(status);
  }

  auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM),
  };

  auto result = AddChild(name(), properties, compat_server_.CreateOffers2());
  if (result.is_error()) {
    fdf::error("Failed to add child: {}", result.status_string());
    return result.take_error();
  }

  controller_.Bind(std::move(result.value()));
  return zx::ok();
}

zx_status_t PwmInitDevice::Init() {
  // Enable PWM_CLK_* for WIFI 32K768
  // This connection is optional, so if it does not connect, don't return an error.
  // In DFv2 Connect will succeed even if the fragment is not there, so we first learn of the
  // failed connection when sending the Enable command.
  {
    fidl::WireResult result = wifi_32k768_clk_->Enable();
    if (!result.ok()) {
      fdf::warn("Failed to send Enable request to clock for wifi_32k768: {}",
                result.status_string());
    } else {  // only check the returned result if we actually got a valid response.
      if (result->is_error()) {
        fdf::warn("Failed to enable clock for wifi_32k768: {}",
                  zx_status_get_string(result->error_value()));
        return result->error_value();
      }
    }
  }

  auto result = pwm_->Enable();
  if (!result.ok()) {
    fdf::error("Could not enable PWM: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("Could not enable PWM: {}", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  aml_pwm::mode_config two_timer;
  memset(&two_timer, 0, sizeof(two_timer));
  two_timer.mode = aml_pwm::Mode::kTwoTimer;
  two_timer.two_timer.period_ns2 = 30052;
  two_timer.two_timer.duty_cycle2 = 50.0;
  two_timer.two_timer.timer1 = 0x0a;
  two_timer.two_timer.timer2 = 0x0a;
  fuchsia_hardware_pwm::wire::PwmConfig init_cfg = {
      .polarity = false,
      .period_ns = 30053,
      .duty_cycle = static_cast<float>(49.931787176),
      .mode_config = fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&two_timer),
                                                             sizeof(two_timer)),
  };
  auto set_config_result = pwm_->SetConfig(init_cfg);
  if (!set_config_result.ok()) {
    fdf::error("Could not initialize PWM: {}", set_config_result.status_string());
    return set_config_result.status();
  }
  if (set_config_result->is_error()) {
    fdf::error("Could not initialize PWM: {}",
               zx_status_get_string(set_config_result->error_value()));
    return set_config_result->error_value();
  }

  // set GPIO to reset Bluetooth module
  fidl::WireResult config_result =
      bt_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputLow);
  if (!config_result.ok()) {
    fdf::error("Failed to send SetBufferMode request to bt gpio: {}",
               config_result.status_string());
    return config_result.status();
  }
  if (config_result->is_error()) {
    fdf::error("Failed to configure bt gpio to output: {}",
               zx_status_get_string(config_result->error_value()));
    return config_result->error_value();
  }
  usleep(10 * 1000);
  fidl::WireResult write_result =
      bt_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh);
  if (!write_result.ok()) {
    fdf::error("Failed to send SetBufferMode request to bt gpio: {}", write_result.status_string());
    return write_result.status();
  }
  if (write_result->is_error()) {
    fdf::error("Failed to write to bt gpio: {}", zx_status_get_string(write_result->error_value()));
    return write_result->error_value();
  }
  usleep(100 * 1000);

  return ZX_OK;
}

}  // namespace pwm_init

FUCHSIA_DRIVER_EXPORT2(pwm_init::PwmInitDriver);
