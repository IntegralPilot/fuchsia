// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/gpu-device-driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/component/cpp/start_completer.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/virtio/driver_utils.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstring>
#include <memory>
#include <utility>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/display-engine.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"

namespace virtio_display {

zx::result<> GpuDeviceDriver::InitResources(const fdf::Namespace& incoming) {
  fbl::AllocChecker alloc_checker;
  engine_events_ = fbl::make_unique_checked<display::DisplayEngineEventsFidl>(&alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineEventsFidl");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result sysmem_client_result = incoming.Connect<fuchsia_sysmem2::Allocator>();
  if (sysmem_client_result.is_error()) {
    fdf::error("Failed to get sysmem protocol: {}", sysmem_client_result);
    return sysmem_client_result.take_error();
  }
  fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client =
      std::move(sysmem_client_result).value();

  zx::result<fidl::ClientEnd<fuchsia_hardware_pci::Device>> pci_client_result =
      incoming.Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_result.is_error()) {
    fdf::error("Failed to get pci client: {}", pci_client_result);
    return pci_client_result.take_error();
  }

  zx::result<std::pair<zx::bti, std::unique_ptr<virtio::Backend>>> bti_and_backend_result =
      virtio::GetBtiAndBackend(std::move(pci_client_result).value());
  if (!bti_and_backend_result.is_ok()) {
    fdf::error("GetBtiAndBackend failed: {}", bti_and_backend_result);
    return bti_and_backend_result.take_error();
  }
  auto [bti, backend] = std::move(bti_and_backend_result).value();

  zx::result<std::unique_ptr<DisplayEngine>> display_engine_result = DisplayEngine::Create(
      std::move(sysmem_client), std::move(bti), std::move(backend), engine_events_.get());
  if (display_engine_result.is_error()) {
    // DisplayEngine::Create() logs on error.
    return display_engine_result.take_error();
  }
  display_engine_ = std::move(display_engine_result).value();

  engine_fidl_adapter_ = fbl::make_unique_checked<display::DisplayEngineFidlAdapter>(
      &alloc_checker, display_engine_.get(), engine_events_.get());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineFidlAdapter");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  gpu_control_server_ = fbl::make_unique_checked<GpuControlServer>(
      &alloc_checker, this, display_engine_->pci_device().GetCapabilitySetLimit());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for GpuControlServer");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok();
}

zx::result<> GpuDeviceDriver::InitDisplayNode() {
  // Serve `fuchsia.hardware.display.engine/Service` at the outgoing directory.
  fuchsia_hardware_display_engine::Service::InstanceHandler service_handler(
      {.engine = engine_fidl_adapter_->CreateHandler(*driver_dispatcher()->get())});
  zx::result<> add_service_result =
      outgoing()->AddService<fuchsia_hardware_display_engine::Service>(std::move(service_handler));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add service: {}", add_service_result);
    return add_service_result.take_error();
  }

  const std::vector<fuchsia_driver_framework::Offer> node_offers = {
      fdf::MakeOffer2<fuchsia_hardware_display_engine::Service>()};

  static constexpr std::string_view kDisplayChildNodeName = "virtio-gpu-display";
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>>
      display_node_controller_client_result =
          AddChild(kDisplayChildNodeName, std::span<const fuchsia_driver_framework::NodeProperty>(),
                   node_offers);
  if (display_node_controller_client_result.is_error()) {
    fdf::error("Failed to add child node: {}", display_node_controller_client_result);
    return display_node_controller_client_result.take_error();
  }
  display_node_controller_ =
      fidl::WireSyncClient(std::move(display_node_controller_client_result).value());

  return zx::ok();
}

zx::result<> GpuDeviceDriver::InitGpuControlNode() {
  async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  zx::result<> add_service_result = outgoing()->AddService<fuchsia_gpu_virtio::Service>(
      gpu_control_server_->GetInstanceHandler(dispatcher),
      /*instance=*/component::kDefaultInstance);
  if (add_service_result.is_error()) {
    fdf::error("Failed to add fuchsia.gpu.virtio service to the outgoing directory: {}",
               add_service_result);
    return add_service_result.take_error();
  }

  static constexpr std::string_view kGpuControlChildNodeName = "virtio-gpu-control";

  const fuchsia_driver_framework::Offer node_offers[] = {
      fdf::MakeOffer2<fuchsia_gpu_virtio::Service>(component::kDefaultInstance),
  };
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>>
      gpu_control_node_controller_client_result = AddChild(
          kGpuControlChildNodeName, std::span<const fuchsia_driver_framework::NodeProperty>(),
          std::span<const fuchsia_driver_framework::Offer>(node_offers, 1));
  if (gpu_control_node_controller_client_result.is_error()) {
    fdf::error("Failed to add child node: {}", gpu_control_node_controller_client_result);
    return gpu_control_node_controller_client_result.take_error();
  }
  gpu_control_node_controller_ =
      fidl::WireSyncClient(std::move(gpu_control_node_controller_client_result).value());

  return zx::ok();
}

GpuDeviceDriver::GpuDeviceDriver() : fdf::DriverBase2("virtio-gpu-display") {}

GpuDeviceDriver::~GpuDeviceDriver() {}

void GpuDeviceDriver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  zx::result<> init_resources_result = InitResources(context.incoming());
  if (init_resources_result.is_error()) {
    fdf::error("Failed to initialize the resources: {}", init_resources_result);
    completer(init_resources_result.take_error());
    return;
  }

  zx::result<> init_display_node_result = InitDisplayNode();
  if (init_display_node_result.is_error()) {
    fdf::error("Failed to initialize the display child node: {}", init_display_node_result);
    completer(init_display_node_result.take_error());
    return;
  }

  zx::result<> init_gpu_control_node_result = InitGpuControlNode();
  if (init_gpu_control_node_result.is_error()) {
    fdf::error("Failed to initialize the gpu control child node: {}", init_gpu_control_node_result);
    completer(init_gpu_control_node_result.take_error());
    return;
  }

  start_thread_ = std::thread([this, completer = std::move(completer)]() mutable {
    zx_status_t status = display_engine_->Start();
    completer(zx::make_result(status));
  });
}

void GpuDeviceDriver::Stop(fdf::StopCompleter completer) {
  if (start_thread_.joinable()) {
    start_thread_.join();
  }
  completer(zx::ok());
}

void GpuDeviceDriver::SendHardwareCommand(std::span<uint8_t> request,
                                          std::function<void(std::span<uint8_t>)> callback) {
  display_engine_->pci_device().ExchangeControlqVariableLengthRequestResponse(std::move(request),
                                                                              std::move(callback));
}

}  // namespace virtio_display

FUCHSIA_DRIVER_EXPORT2(virtio_display::GpuDeviceDriver);
