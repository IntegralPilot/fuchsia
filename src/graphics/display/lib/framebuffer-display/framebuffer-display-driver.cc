// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/framebuffer-display/framebuffer-display-driver.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <memory>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"
#include "src/graphics/display/lib/framebuffer-display/framebuffer-display.h"

namespace framebuffer_display {

FramebufferDisplayDriver::FramebufferDisplayDriver(std::string_view device_name)
    : fdf::DriverBase2(device_name) {}

FramebufferDisplayDriver::~FramebufferDisplayDriver() = default;

void FramebufferDisplayDriver::Stop(fdf::StopCompleter completer) {
  if (framebuffer_display_ != nullptr) {
    framebuffer_display_->Deinitialize();
  }
  completer(zx::ok());
}

zx::result<> FramebufferDisplayDriver::Start(fdf::DriverContext context) {
  incoming_ = context.take_incoming();
  zx::result<> configure_hardware_result = ConfigureHardware();
  if (configure_hardware_result.is_error()) {
    fdf::error("Failed to configure hardware: {}", configure_hardware_result);
    return configure_hardware_result.take_error();
  }

  zx::result<std::unique_ptr<FramebufferDisplay>> framebuffer_display_result =
      CreateAndInitializeFramebufferDisplay(*incoming_);
  if (framebuffer_display_result.is_error()) {
    fdf::error("Failed to create and initialize FramebufferDisplay: {}",
               framebuffer_display_result);
    return framebuffer_display_result.take_error();
  }
  framebuffer_display_ = std::move(framebuffer_display_result).value();

  zx::result<> add_fidl_service_node_result = InitializeFidlServiceNode();
  if (add_fidl_service_node_result.is_error()) {
    fdf::error("Failed to add FIDL service node: {}", add_fidl_service_node_result);
    return add_fidl_service_node_result.take_error();
  }

  return zx::ok();
}

zx::result<std::unique_ptr<FramebufferDisplay>>
FramebufferDisplayDriver::CreateAndInitializeFramebufferDisplay(const fdf::Namespace& incoming) {
  fbl::AllocChecker alloc_checker;
  engine_events_ = fbl::make_unique_checked<display::DisplayEngineEventsFidl>(&alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineEventsFidl");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<fdf::MmioBuffer> frame_buffer_mmio_result = GetFrameBufferMmioBuffer();
  if (frame_buffer_mmio_result.is_error()) {
    fdf::error("Failed to get frame buffer mmio buffer: {}", frame_buffer_mmio_result);
    return frame_buffer_mmio_result.take_error();
  }
  fdf::MmioBuffer frame_buffer_mmio = std::move(frame_buffer_mmio_result).value();

  zx::result<DisplayProperties> display_properties_result = GetDisplayProperties();
  if (display_properties_result.is_error()) {
    fdf::error("Failed to get display properties: {}", display_properties_result);
    return display_properties_result.take_error();
  }
  DisplayProperties display_properties = std::move(display_properties_result).value();

  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> sysmem_client_result =
      incoming.Connect<fuchsia_sysmem2::Allocator>();
  if (sysmem_client_result.is_error()) {
    fdf::error("Failed to get fuchsia.sysmem2.Allocator protocol: {}", sysmem_client_result);
    return sysmem_client_result.take_error();
  }
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_client(
      std::move(sysmem_client_result).value());

  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls,
                                          "framebuffer-display-dispatcher",
                                          [](fdf_dispatcher_t*) {});
  if (create_dispatcher_result.is_error()) {
    fdf::error("Failed to create framebuffer display dispatcher: {}", create_dispatcher_result);
    return create_dispatcher_result.take_error();
  }
  framebuffer_display_dispatcher_ = std::move(create_dispatcher_result).value();

  auto framebuffer_display = fbl::make_unique_checked<FramebufferDisplay>(
      &alloc_checker, engine_events_.get(), std::move(sysmem_client), std::move(frame_buffer_mmio),
      std::move(display_properties), framebuffer_display_dispatcher_.async_dispatcher());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for FramebufferDisplay");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  engine_fidl_adapter_ = fbl::make_unique_checked<display::DisplayEngineFidlAdapter>(
      &alloc_checker, framebuffer_display.get(), engine_events_.get());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineFidlAdapter");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> framebuffer_display_initialize_result = framebuffer_display->Initialize();
  if (framebuffer_display_initialize_result.is_error()) {
    fdf::error("Failed to initialize FramebufferDisplay: {}",
               framebuffer_display_initialize_result);
    return framebuffer_display_initialize_result.take_error();
  }

  return zx::ok(std::move(framebuffer_display));
}

zx::result<> FramebufferDisplayDriver::InitializeFidlServiceNode() {
  ZX_DEBUG_ASSERT(framebuffer_display_ != nullptr);
  ZX_DEBUG_ASSERT(engine_fidl_adapter_ != nullptr);

  // Serve `fuchsia.hardware.display.engine/Service` at the outgoing directory.
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

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>>
      node_controller_client_result =
          AddChild(name(), std::span<const fuchsia_driver_framework::NodeProperty>(), node_offers);
  if (node_controller_client_result.is_error()) {
    fdf::error("Failed to add child node: {}", node_controller_client_result);
    return node_controller_client_result.take_error();
  }
  node_controller_ = fidl::WireSyncClient(std::move(node_controller_client_result).value());

  return zx::ok();
}

}  // namespace framebuffer_display
