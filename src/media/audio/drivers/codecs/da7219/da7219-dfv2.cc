// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/codecs/da7219/da7219-dfv2.h"

#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <fidl/fuchsia.hardware.acpi/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fit/defer.h>

namespace audio::da7219 {

zx::result<zx::interrupt> Driver::GetIrq(const fdf::Namespace& incoming) const {
  auto acpi_client = incoming.Connect<fuchsia_hardware_acpi::Service::Device>("acpi");
  if (!acpi_client.is_ok()) {
    return acpi_client.take_error();
  }
  auto irq = fidl::WireCall(acpi_client.value())->MapInterrupt(0);
  if (!irq.ok() || irq.value().is_error()) {
    DA7219_LOG(ERROR, "Could not get IRQ: %s",
               irq.ok() ? zx_status_get_string(irq.value().error_value())
                        : irq.FormatDescription().data());
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  return zx::ok(std::move(irq.value().value()->irq));
}

zx::result<> Driver::Start(fdf::DriverContext context) {
  auto incoming = context.take_incoming();
  zx::result i2c = incoming->Connect<fuchsia_hardware_i2c::Service::Device>("i2c000");
  if (!i2c.is_ok()) {
    DA7219_LOG(ERROR, "Could not get I2C client: %s", i2c.status_string());
    return i2c.take_error();
  }
  zx::result irq = GetIrq(*incoming);
  if (!irq.is_ok()) {
    DA7219_LOG(ERROR, "Could not get IRQ: %s", irq.status_string());
    return irq.take_error();
  }

  // There is a core class that implements the core logic and interaction with the hardware, and a
  // Server class that allows the creation of multiple instances (one for input and one for output).
  core_ = std::make_shared<Core>(std::move(i2c.value()), std::move(irq.value()), dispatcher());
  if (zx_status_t status = core_->Initialize(); status != ZX_OK) {
    DA7219_LOG(ERROR, "Could not initialize: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  if (zx::result output = Serve(std::string(name()).append("-output"), false); output.is_error()) {
    FDF_SLOG(ERROR, "Could not serve output server", KV("status", output.status_string()));
    return output.take_error();
  }

  if (zx::result input = Serve(std::string(name()).append("-input"), true); input.is_error()) {
    FDF_SLOG(ERROR, "Could not serve input server", KV("status", input.status_string()));
    return input.take_error();
  }

  FDF_SLOG(INFO, "Started");

  return zx::ok();
}

zx::result<> Driver::Serve(std::string_view name, bool is_input) {
  // Serve the fuchsia.hardware.audio/CodecConnector protocol to clients through the
  // fuchsia.hardware.audio/CodecConnectorService wrapper.
  fuchsia_hardware_audio::CodecConnectorService::InstanceHandler handler;

  // For a given instance of this driver only one client is supported at the time, however before
  // any client connects to the server the framework opens the corresponding devfs nodes creating
  // a first connection to CodecConnector.
  // Since the framework requires allowing and keeping this connection in addition to the connection
  // from a client, we allow multiple bindings of ServerConnector but share the instance.
  // ServerConnector limits the actual usage to one client at the time.
  if (is_input) {
    server_input_ = std::make_shared<ServerConnector>(core_, true);
  } else {
    server_output_ = std::make_shared<ServerConnector>(core_, false);
  }
  ServerConnector* connector = is_input ? server_input_.get() : server_output_.get();

  zx::result result =
      handler.add_codec_connector(fit::bind_member<&ServerConnector::BindConnector>(connector));
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  result = outgoing()->AddService<fuchsia_hardware_audio::CodecConnectorService>(std::move(handler),
                                                                                 name);
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to add service", KV("status", result.status_string()));
    return result.take_error();
  }
  if (zx::result result = connector->Serve(node()); result.is_error()) {
    FDF_SLOG(ERROR, "Failed to serve connector", KV("status", result.status_string()));
    return result.take_error();
  }

  return zx::ok();
}

}  // namespace audio::da7219

FUCHSIA_DRIVER_EXPORT2(audio::da7219::Driver);
