// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/display-device-driver.h"

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <cstdint>
#include <memory>

#include <fbl/alloc_checker.h>

#include "fidl/fuchsia.driver.framework/cpp/natural_types.h"
#include "src/graphics/display/drivers/amlogic-display/display-engine.h"
#include "src/graphics/display/drivers/amlogic-display/structured_config.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"

namespace amlogic_display {

DisplayDeviceDriver::DisplayDeviceDriver() : fdf::DriverBase2("amlogic-display") {}

zx::result<> DisplayDeviceDriver::Start(fdf::DriverContext context) {
  auto config = context.take_config<structured_config::Config>();
  std::shared_ptr<fdf::Namespace> incoming_ptr(context.take_incoming());

  fbl::AllocChecker alloc_checker;
  engine_events_ = fbl::make_unique_checked<display::DisplayEngineEventsFidl>(&alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineEventsFidl");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<std::unique_ptr<DisplayEngine>> create_display_engine_result =
      DisplayEngine::Create(incoming_ptr, engine_events_.get(), config);
  if (create_display_engine_result.is_error()) {
    fdf::error("Failed to create DisplayEngine: {}", create_display_engine_result);
    return create_display_engine_result.take_error();
  }
  display_engine_ = std::move(create_display_engine_result).value();

  engine_fidl_adapter_ = fbl::make_unique_checked<display::DisplayEngineFidlAdapter>(
      &alloc_checker, display_engine_.get(), engine_events_.get());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineFidlAdapter");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  inspect::Node config_node = display_engine_->inspector().GetRoot().CreateChild("config");
  config.RecordInspect(&config_node);

  fuchsia_hardware_display_engine::Service::InstanceHandler service_handler(
      {.engine = engine_fidl_adapter_->CreateHandler(*(driver_dispatcher()->get()))});
  zx::result<> add_service_result =
      outgoing()->AddService<fuchsia_hardware_display_engine::Service>(std::move(service_handler));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add service: {}", add_service_result);
    return add_service_result.take_error();
  }

  const std::vector<fuchsia_driver_framework::Offer> node_offers = {
      fdf::MakeOffer2<fuchsia_hardware_display_engine::Service>(),
  };
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> controller_client_result =
      AddChild(name(), std::span<const fuchsia_driver_framework::NodeProperty>(), node_offers);
  if (controller_client_result.is_error()) {
    fdf::error("Failed to add child node: {}", controller_client_result);
    return controller_client_result.take_error();
  }
  controller_ = fidl::WireSyncClient(std::move(controller_client_result).value());

  return zx::ok();
}

}  // namespace amlogic_display

FUCHSIA_DRIVER_EXPORT2(amlogic_display::DisplayDeviceDriver);
