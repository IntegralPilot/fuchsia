// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "buttons.h"

#include <fidl/fuchsia.buttons/cpp/fidl.h>
#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <cinttypes>

#include <fbl/alloc_checker.h>

namespace buttons {

zx::result<> Buttons::Start(fdf::DriverContext context) {
  config_ = context.take_config<buttons_config::Config>();
  zx::result pdev_client_end =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  zx::result metadata_result = pdev.GetFidlMetadata<fuchsia_buttons::GpioButtonsMetadata>();
  if (metadata_result.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata_result);
    return metadata_result.take_error();
  }
  fuchsia_buttons::GpioButtonsMetadata& metadata = metadata_result.value();

  if (!metadata.gpios().has_value()) {
    fdf::error("Metadata missing gpios");
    return zx::error(ZX_ERR_INTERNAL);
  }
  const std::span<const fuchsia_buttons::GpioConfig>& gpio_configs = metadata.gpios().value();
  std::vector<ButtonsDevice::Gpio> gpios;
  gpios.reserve(gpio_configs.size());

  if (!metadata.buttons().has_value()) {
    fdf::error("Metadata missing buttons");
    return zx::error(ZX_ERR_INTERNAL);
  }
  std::vector<fuchsia_buttons::GpioButtonConfig> buttons = std::move(metadata.buttons().value());

  for (size_t i = 0; i < gpio_configs.size(); ++i) {
    const char* name;
    const auto& button = buttons[i];
    if (!button.id().has_value()) {
      fdf::error("Button {} missing id", i);
      return zx::error(ZX_ERR_INTERNAL);
    }
    const fuchsia_buttons::GpioButtonId& button_id = button.id().value();
    switch (button_id) {
      case fuchsia_buttons::GpioButtonId::kVolumeUp:
        name = "volume-up";
        break;
      case fuchsia_buttons::GpioButtonId::kVolumeDown:
        name = "volume-down";
        break;
      case fuchsia_buttons::GpioButtonId::kFdr:
        name = "volume-both";
        break;
      case fuchsia_buttons::GpioButtonId::kMicMute:
      case fuchsia_buttons::GpioButtonId::kMicAndCamMute:
        name = "mic-mute";
        break;
      case fuchsia_buttons::GpioButtonId::kCamMute:
        name = "cam-mute";
        break;
      case fuchsia_buttons::GpioButtonId::kPower:
        name = "power";
        break;
      case fuchsia_buttons::GpioButtonId::kPlayPause:
        name = "play-pause";
        break;
      case fuchsia_buttons::GpioButtonId::kKeyA:
        name = "key-a";
        break;
      case fuchsia_buttons::GpioButtonId::kKeyM:
        name = "key-m";
        break;
      case fuchsia_buttons::GpioButtonId::kFunction:
        name = "function";
        break;
      default:
        fdf::error("Button {} has unknown id: {}", i, static_cast<uint32_t>(button_id));
        return zx::error(ZX_ERR_NOT_SUPPORTED);
    };
    zx::result gpio_client =
        context.incoming().Connect<fuchsia_hardware_gpio::Service::Device>(name);
    if (gpio_client.is_error() || !gpio_client->is_valid()) {
      fdf::error("Connect to GPIO {} failed: {}", name, gpio_client);
      return gpio_client.take_error();
    }
    gpios.emplace_back(ButtonsDevice::Gpio{
        .client{std::move(gpio_client.value())}, .irq{}, .config = gpio_configs[i]});
  }

  fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag_client;
  if (config_.suspend_enabled()) {
    auto sag_result = context.incoming().Connect<fuchsia_power_system::ActivityGovernor>();
    if (sag_result.is_ok() && sag_result->is_valid()) {
      sag_client = std::move(sag_result.value());
    } else {
      fdf::error(
          "Failed to connect to fuchsia.power.system.ActivityGovernor: {}; system may incorrectly enter suspend.",
          sag_result);
    }
  }

  device_ = std::make_unique<buttons::ButtonsDevice>(dispatcher(), std::move(buttons),
                                                     std::move(gpios), std::move(sag_client));

  auto result = outgoing()->component().AddUnmanagedProtocol<fuchsia_input_report::InputDevice>(
      input_report_bindings_.CreateHandler(device_.get(), dispatcher(),
                                           fidl::kIgnoreBindingClosure),
      kDeviceName);
  if (result.is_error()) {
    fdf::error("Failed to add input report service: {}", result);
    return result.take_error();
  }

  if (zx::result result = CreateDevfsNode(); result.is_error()) {
    fdf::error("Failed to export to devfs: {}", result);
    return result.take_error();
  }

  return zx::ok();
}

void Buttons::Stop(fdf::StopCompleter completer) {
  device_->ShutDown();
  completer(zx::ok());
}

zx::result<> Buttons::CreateDevfsNode() {
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args{
      {.connector = std::move(connector).value(), .class_name = "input-report"}};

  zx::result child = AddOwnedChild(kDeviceName, devfs_args);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child).value();

  return zx::ok();
}

}  // namespace buttons

FUCHSIA_DRIVER_EXPORT2(buttons::Buttons);
