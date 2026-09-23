// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-trip.h"

#include <fidl/fuchsia.driver.framework/cpp/wire_types.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.trippoint/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/wire_messaging_declarations.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <memory>
#include <optional>

#include <src/devices/temperature/drivers/aml-trip/aml-trip-device.h>

#include "lib/driver/compat/cpp/metadata.h"
#include "src/devices/temperature/drivers/aml-trip/util.h"

namespace temperature {

zx::result<> AmlTrip::Start(fdf::DriverContext context) {
  fidl::Arena arena;
  std::optional<TemperatureCelsius> critical_temperature = std::nullopt;

  zx::result pdev_client =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client.is_error() || !pdev_client->is_valid()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client.value())};

  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_trippoint::TripDeviceMetadata>();
  if (metadata.is_error()) {
    if (metadata.status_value() != ZX_ERR_NOT_FOUND) {
      fdf::error("Failed to get trip sensor metadata: {}", metadata);
      return zx::error(metadata.status_value());
    }
  } else {
    critical_temperature = metadata->critical_temp_celsius();
  }

  // Stash a name for this device to be returned by `GetSensorName`
  zx::result device_info_result = pdev.GetDeviceInfo();
  if (device_info_result.is_error()) {
    fdf::error("Failed to get device info: {}", device_info_result);
    return device_info_result.take_error();
  }
  fdf::PDev::DeviceInfo device_info = std::move(device_info_result.value());

  std::string name = device_info.name;

  zx::result sensor_mmio = pdev.MapMmio(kSensorMmioIndex);
  if (sensor_mmio.is_error()) {
    fdf::error("Failed to map sensor mmio: {}", sensor_mmio);
    return sensor_mmio.take_error();
  }

  zx::result trim_mmio = pdev.MapMmio(kTrimMmioIndex);
  if (trim_mmio.is_error()) {
    fdf::error("Failed to map trim mmio: {}", trim_mmio);
    return trim_mmio.take_error();
  }

  const uint32_t trim_info = trim_mmio->Read32(0);

  zx::result irq = pdev.GetInterrupt(0);
  if (irq.is_error()) {
    fdf::error("Failed to get sensor interrupt: {}", irq);
    return irq.take_error();
  }

  device_ = std::make_unique<AmlTripDevice>(dispatcher(), trim_info, name,
                                            std::move(sensor_mmio.value()), std::move(irq.value()));
  device_->Init();

  if (critical_temperature) {
    fdf::info("Configuring critical temperature for '{}' at {:.2f}C", name.c_str(),
              *critical_temperature);
    device_->SetRebootTemperatureCelsius(*critical_temperature);
  }

  auto result = outgoing()->AddService<fuchsia_hardware_temperature::Service>(
      fuchsia_hardware_temperature::Service::InstanceHandler({
          .device = temperature_bindings_.CreateHandler(
              device_.get(), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              fidl::kIgnoreBindingClosure),
          .trippoint = trippoint_bindings_.CreateHandler(
              device_.get(), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              fidl::kIgnoreBindingClosure),
      }),
      kChildNodeName);
  if (result.is_error()) {
    fdf::error("Failed to add service {}", result);
    return result.take_error();
  }

  zx::result child = AddOwnedChild(kChildNodeName);
  if (child.is_error()) {
    fdf::error("Failed to add owned child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  fdf::info("Started Amlogic Trip Point Driver");

  return zx::ok();
}

void AmlTrip::Stop(fdf::StopCompleter completer) {
  device_->Shutdown();
  completer(zx::ok());
}

}  // namespace temperature

FUCHSIA_DRIVER_EXPORT2(temperature::AmlTrip);
