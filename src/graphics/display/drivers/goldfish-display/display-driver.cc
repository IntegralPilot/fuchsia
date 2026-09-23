// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/goldfish-display/display-driver.h"

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/status.h>

#include <memory>
#include <string_view>
#include <vector>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/goldfish-display/display-engine.h"
#include "src/graphics/display/drivers/goldfish-display/render_control.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"

namespace goldfish {
namespace {

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> CreateAndInitializeSysmemAllocator(
    fdf::Namespace* incoming) {
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> connect_sysmem_protocol_result =
      incoming->Connect<fuchsia_sysmem2::Allocator>();
  if (connect_sysmem_protocol_result.is_error()) {
    fdf::error("Failed to connect to the sysmem Allocator FIDL protocol: {}",
               connect_sysmem_protocol_result);
    return connect_sysmem_protocol_result.take_error();
  }
  fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_allocator =
      std::move(connect_sysmem_protocol_result).value();

  const zx_koid_t pid = GetKoid(zx_process_self());
  static constexpr std::string_view kDebugName = "goldfish-display";
  fidl::Arena arena;
  fidl::OneWayStatus set_debug_status =
      fidl::WireCall(sysmem_allocator)
          ->SetDebugClientInfo(
              fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
                  .name(kDebugName)
                  .id(pid)
                  .Build());
  if (!set_debug_status.ok()) {
    fdf::error("Failed to set sysmem allocator debug info: {}", set_debug_status.status_string());
    return zx::error(set_debug_status.status());
  }

  return zx::ok(std::move(sysmem_allocator));
}

zx::result<std::unique_ptr<RenderControl>> CreateAndInitializeRenderControl(
    fdf::Namespace* incoming) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_goldfish_pipe::Bus>>
      render_control_connect_pipe_service_result =
          incoming->Connect<fuchsia_hardware_goldfish_pipe::Service::Device>();
  if (render_control_connect_pipe_service_result.is_error()) {
    fdf::error("Failed to connect to the goldfish pipe FIDL service: {}",
               render_control_connect_pipe_service_result);
    return render_control_connect_pipe_service_result.take_error();
  }
  fidl::ClientEnd<fuchsia_hardware_goldfish_pipe::Bus> render_control_pipe =
      std::move(render_control_connect_pipe_service_result).value();

  fbl::AllocChecker alloc_checker;
  auto render_control = fbl::make_unique_checked<RenderControl>(&alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for RenderControl");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status =
      render_control->InitRcPipe(fidl::WireSyncClient(std::move(render_control_pipe)));
  if (status != ZX_OK) {
    fdf::error("Failed to initialize RenderControl: {}", status);
    return zx::error(status);
  }

  return zx::ok(std::move(render_control));
}

}  // namespace

DisplayDriver::DisplayDriver() : fdf::DriverBase2("goldfish-display") {}

DisplayDriver::~DisplayDriver() = default;

zx::result<> DisplayDriver::Start(fdf::DriverContext context) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_goldfish::ControlDevice>>
      connect_control_service_result =
          context.incoming().Connect<fuchsia_hardware_goldfish::ControlService::Device>();
  if (connect_control_service_result.is_error()) {
    fdf::error("Failed to connect to the goldfish Control FIDL service: {}",
               connect_control_service_result);
    return connect_control_service_result.take_error();
  }
  fidl::ClientEnd<fuchsia_hardware_goldfish::ControlDevice> control =
      std::move(connect_control_service_result).value();

  zx::result<fidl::ClientEnd<fuchsia_hardware_goldfish_pipe::Bus>> connect_pipe_service_result =
      context.incoming().Connect<fuchsia_hardware_goldfish_pipe::Service::Device>();
  if (connect_pipe_service_result.is_error()) {
    fdf::error("Failed to connect to the goldfish pipe FIDL service: {}",
               connect_pipe_service_result);
    return connect_pipe_service_result.take_error();
  }
  fidl::ClientEnd<fuchsia_hardware_goldfish_pipe::Bus> pipe =
      std::move(connect_pipe_service_result).value();

  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> create_sysmem_allocator_result =
      CreateAndInitializeSysmemAllocator(&context.incoming());
  if (create_sysmem_allocator_result.is_error()) {
    fdf::error("Failed to create and initialize sysmem allocator: {}",
               create_sysmem_allocator_result);
    return create_sysmem_allocator_result.take_error();
  }
  fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_allocator =
      std::move(create_sysmem_allocator_result).value();

  zx::result<std::unique_ptr<RenderControl>> create_render_control_result =
      CreateAndInitializeRenderControl(&context.incoming());
  if (create_render_control_result.is_error()) {
    fdf::error("Failed to create and initialize RenderControl: {}", create_render_control_result);
    return create_render_control_result.take_error();
  }
  std::unique_ptr<RenderControl> render_control = std::move(create_render_control_result).value();

  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(fdf::SynchronizedDispatcher::Options{},
                                          "display-event-dispatcher",
                                          /*shutdown_handler=*/[](fdf_dispatcher_t*) {});
  if (create_dispatcher_result.is_error()) {
    fdf::error("Failed to create display event dispatcher: {}", create_dispatcher_result);
    return create_dispatcher_result.take_error();
  }
  display_event_dispatcher_ = std::move(create_dispatcher_result).value();

  fbl::AllocChecker alloc_checker;
  engine_events_ = fbl::make_unique_checked<display::DisplayEngineEventsFidl>(&alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineEventsFidl");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  display_engine_ = fbl::make_unique_checked<DisplayEngine>(
      &alloc_checker, std::move(control), std::move(pipe), std::move(sysmem_allocator),
      std::move(render_control), display_event_dispatcher_.async_dispatcher(),
      engine_events_.get());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngine");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> init_result = display_engine_->Initialize();
  if (init_result.is_error()) {
    fdf::error("Failed to initialize DisplayEngine: {}", init_result);
    return init_result.take_error();
  }

  engine_fidl_adapter_ = fbl::make_unique_checked<display::DisplayEngineFidlAdapter>(
      &alloc_checker, display_engine_.get(), engine_events_.get());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineFidlAdapter");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  fuchsia_hardware_display_engine::Service::InstanceHandler service_handler(
      {.engine = engine_fidl_adapter_->CreateHandler(*(driver_dispatcher()->get()))});
  zx::result<> add_service_result =
      outgoing()->AddService<fuchsia_hardware_display_engine::Service>(std::move(service_handler));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add service: {}", add_service_result);
    return add_service_result.take_error();
  }

  const std::vector<fuchsia_driver_framework::Offer> node_offers = {
      fdf::MakeOffer2<fuchsia_hardware_display_engine::Service>()};
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> controller_client_result =
      AddChild(name(), std::span<const fuchsia_driver_framework::NodeProperty>(), node_offers);
  if (controller_client_result.is_error()) {
    fdf::error("Failed to add child node: {}", controller_client_result);
    return controller_client_result.take_error();
  }
  controller_ = fidl::WireSyncClient(std::move(controller_client_result).value());

  return zx::ok();
}

}  // namespace goldfish

FUCHSIA_DRIVER_EXPORT2(goldfish::DisplayDriver);
