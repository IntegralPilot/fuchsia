// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/aml-canvas/aml-canvas-driver.h"

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/compat/cpp/logging.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <cinttypes>
#include <cstdint>
#include <memory>

#include <bind/fuchsia/cpp/bind.h>
#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/aml-canvas/aml-canvas.h"
#include "src/graphics/display/drivers/aml-canvas/board-resources.h"

namespace aml_canvas {

AmlCanvasDriver::AmlCanvasDriver() : fdf::DriverBase2("aml-canvas") {}

void AmlCanvasDriver::Stop(fdf::StopCompleter completer) {
  fidl::OneWayStatus result = controller_->Remove();
  if (!result.ok()) {
    fdf::error("Failed to remove the Node: {}", result.status_string());
  }
  completer(zx::ok());
}

zx::result<std::unique_ptr<AmlCanvas>> AmlCanvasDriver::CreateAndServeCanvas(
    const fdf::Namespace& incoming, inspect::Inspector inspector) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_platform_device::Device>> pdev_result =
      incoming.Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_result.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_result);
    return pdev_result.take_error();
  }
  fidl::ClientEnd pdev(std::move(pdev_result).value());
  ZX_DEBUG_ASSERT(pdev.is_valid());

  zx::result<zx::bti> bti_result = GetBti(BtiResourceIndex::kCanvas, pdev);
  if (bti_result.is_error()) {
    fdf::error("Failed to get BTI from the platform device: {}", bti_result);
    return bti_result.take_error();
  }
  zx::bti bti = std::move(bti_result).value();

  zx::result<fdf::MmioBuffer> mmio_result = MapMmio(MmioResourceIndex::kDmc, pdev);
  if (mmio_result.is_error()) {
    fdf::error("Failed to map MMIO from the platform device: {}", mmio_result);
    return mmio_result.take_error();
  }
  fdf::MmioBuffer mmio = std::move(mmio_result).value();

  fbl::AllocChecker alloc_checker;
  auto canvas = fbl::make_unique_checked<AmlCanvas>(&alloc_checker, std::move(mmio), std::move(bti),
                                                    std::move(inspector));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate AmlCanvas");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status = canvas->ServeOutgoing(outgoing());
  if (status != ZX_OK) {
    fdf::error("Failed to serve to outgoing directory: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(std::move(canvas));
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AmlCanvasDriver::AddChildNode(
    compat::SyncInitializedDeviceServer* compat_server) {
  fidl::Arena arena;
  std::vector<fuchsia_driver_framework::wire::Offer> offers;
  if (compat_server != nullptr) {
    offers = compat_server->CreateOffers2(arena);
  }
  offers.push_back(
      fdf::MakeOffer2<fuchsia_hardware_amlogiccanvas::Service>(arena, component::kDefaultInstance));

  std::vector properties = {
      fdf::MakeProperty2(arena, bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
  };

  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, name())
                  .offers2(arena, std::move(offers))
                  .properties2(arena, std::move(properties))
                  .Build();

  zx::result<fidl::Endpoints<fuchsia_driver_framework::NodeController>>
      controller_endpoints_result =
          fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (controller_endpoints_result.is_error()) {
    fdf::error("Failed to create endpoints: {}", controller_endpoints_result);
    return controller_endpoints_result.take_error();
  }
  auto [controller_client, controller_server] = std::move(controller_endpoints_result).value();

  fidl::WireResult<fuchsia_driver_framework::Node::AddChild> add_child_result =
      fidl::WireCall(node())->AddChild(std::move(args), std::move(controller_server), {});
  if (!add_child_result.ok()) {
    fdf::error("Failed to call FIDL AddChild: {}", add_child_result.status_string());
    return zx::error(add_child_result.status());
  }
  if (add_child_result->is_error()) {
    fuchsia_driver_framework::NodeError error = add_child_result->error_value();
    fdf::error("Failed to AddChild: {}", static_cast<uint32_t>(error));
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(controller_client));
}

zx::result<> AmlCanvasDriver::Start(fdf::DriverContext context) {
  component_inspector_.emplace(context.CreateInspector(this, inspector_));
  std::shared_ptr<fdf::Namespace> incoming_ptr(context.take_incoming());
  zx::result<> compat_server_init_result =
      compat_server_.Initialize(incoming_ptr, outgoing(), context.node_name(), name());
  if (compat_server_init_result.is_error()) {
    return compat_server_init_result.take_error();
  }

  auto canvas_result = CreateAndServeCanvas(*incoming_ptr, inspector_);
  if (canvas_result.is_error()) {
    fdf::error("Failed to create AmlCanvas and set up service: {}", canvas_result);
    return canvas_result.take_error();
  }
  canvas_ = std::move(canvas_result).value();

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> controller_client_result =
      AddChildNode(&compat_server_);
  if (controller_client_result.is_error()) {
    fdf::error("Failed to add child node: {}", controller_client_result);
    return controller_client_result.take_error();
  }
  controller_ = fidl::WireSyncClient(std::move(controller_client_result).value());

  return zx::ok();
}

}  // namespace aml_canvas

FUCHSIA_DRIVER_EXPORT2(aml_canvas::AmlCanvasDriver);
