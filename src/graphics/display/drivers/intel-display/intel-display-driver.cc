// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/intel-display-driver.h"

#include <fidl/fuchsia.boot/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pci/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.system.state/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/prepare_stop_completer.h>
#include <lib/driver/component/cpp/start_completer.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <cstdint>
#include <memory>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/intel/platform/gpucore/cpp/bind.h>
#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/intel-display/intel-display.h"

namespace intel_display {

namespace {

zx::result<fuchsia_system_state::SystemPowerState> GetSystemPowerState(fdf::Namespace& incoming) {
  zx::result<fidl::ClientEnd<fuchsia_system_state::SystemStateTransition>>
      sysmem_power_transition_result =
          incoming.Connect<fuchsia_system_state::SystemStateTransition>();
  if (sysmem_power_transition_result.is_error()) {
    fdf::error("Failed to connect to fuchsia.system.state/SystemStateTransition: {}",
               sysmem_power_transition_result);
    return sysmem_power_transition_result.take_error();
  }
  fidl::WireSyncClient system_power_transition(std::move(sysmem_power_transition_result).value());

  fidl::WireResult termination_state_result = system_power_transition->GetTerminationSystemState();
  if (!termination_state_result.ok()) {
    fdf::error("Failed to get termination state: {}", termination_state_result.status_string());
    return zx::error(termination_state_result.status());
  }
  return zx::ok(termination_state_result.value().state);
}

template <typename ResourceProtocol>
zx::result<zx::resource> GetKernelResource(fdf::Namespace& incoming, const char* resource_name) {
  zx::result resource_result = incoming.Connect<ResourceProtocol>();
  if (resource_result.is_error()) {
    fdf::error("Failed to connect to kernel {} resource: {}", resource_name, resource_result);
    return resource_result.take_error();
  }
  fidl::WireSyncClient<ResourceProtocol> resource =
      fidl::WireSyncClient(std::move(resource_result).value());
  fidl::WireResult<typename ResourceProtocol::Get> get_result = resource->Get();
  if (!get_result.ok()) {
    fdf::error("Failed to get kernel {} resource: {}", resource_name,
               get_result.FormatDescription());
    return zx::error(get_result.status());
  }
  return zx::ok(std::move(std::move(get_result).value()).resource);
}

zx::result<zbi_swfb_t> GetFramebufferInfo(fdf::Namespace& incoming) {
  zx::result client = incoming.Connect<fuchsia_boot::Items>();
  if (client.is_error()) {
    fdf::error("Failed to connect to fuchsia.boot/Items: {}", client);
    return client.take_error();
  }
  fidl::WireResult result = fidl::WireCall(*client)->Get2(ZBI_TYPE_FRAMEBUFFER, {});
  if (!result.ok()) {
    fdf::error("Failed to get framebuffer boot item: {}", result.status_string());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return zx::error(result->error_value());
  }
  fidl::VectorView items = result->value()->retrieved_items;
  if (items.size() == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  if (items[0].length < sizeof(zbi_swfb_t)) {
    return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
  }
  zbi_swfb_t framebuffer_info;
  zx_status_t status = items[0].payload.read(&framebuffer_info, 0, sizeof(framebuffer_info));
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(framebuffer_info);
}

}  // namespace

IntelDisplayDriver::IntelDisplayDriver() : fdf::DriverBase2("intel-display") {}

IntelDisplayDriver::~IntelDisplayDriver() = default;

zx::result<> IntelDisplayDriver::InitController() {
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> sysmem_result =
      incoming_->Connect<fuchsia_sysmem2::Allocator>();
  if (sysmem_result.is_error()) {
    fdf::error("Failed to connect to sysmem protocol: {}", sysmem_result);
    return sysmem_result.take_error();
  }
  fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem = std::move(sysmem_result).value();

  zx::result<fidl::ClientEnd<fuchsia_hardware_pci::Device>> pci_result =
      incoming_->Connect<fuchsia_hardware_pci::Service::Device>("pci");
  if (pci_result.is_error()) {
    fdf::error("Failed to connect to pci protocol: {}", pci_result);
    return pci_result.take_error();
  }
  fidl::ClientEnd<fuchsia_hardware_pci::Device> pci = std::move(pci_result).value();

  zx::result<zbi_swfb_t> framebuffer_info = GetFramebufferInfo(*incoming_);
  if (framebuffer_info.is_ok()) {
    framebuffer_info_ = framebuffer_info.value();
  }

  zx::result<zx::resource> mmio_resource_result =
      GetKernelResource<fuchsia_kernel::MmioResource>(*incoming_, "mmio");
  if (mmio_resource_result.is_error()) {
    return mmio_resource_result.take_error();
  }
  mmio_resource_ = std::move(mmio_resource_result).value();

  zx::result<zx::resource> ioport_resource_result =
      GetKernelResource<fuchsia_kernel::IoportResource>(*incoming_, "ioport");
  if (ioport_resource_result.is_error()) {
    return ioport_resource_result.take_error();
  }
  ioport_resource_ = std::move(ioport_resource_result).value();

  ControllerResources resources = {
      .mmio = mmio_resource_.borrow(),
      .ioport = ioport_resource_.borrow(),
  };

  zx::result<std::unique_ptr<Controller>> controller_result =
      Controller::Create(std::move(sysmem), std::move(pci), std::move(resources), framebuffer_info_,
                         &engine_events_, inspector_);
  if (controller_result.is_error()) {
    return controller_result.take_error();
  }
  controller_ = std::move(controller_result).value();

  fbl::AllocChecker alloc_checker;
  engine_fidl_adapter_ = fbl::make_unique_checked<display::DisplayEngineFidlAdapter>(
      &alloc_checker, controller_.get(), &engine_events_);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngineFidlAdapter");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok();
}

void IntelDisplayDriver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  component_inspector_.emplace(context.CreateInspector(this, inspector_));
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result<> init_controller_result = InitController();
  if (init_controller_result.is_error()) {
    completer(init_controller_result.take_error());
    return;
  }

  zx::result<> init_display_node_result = InitDisplayNode();
  if (init_display_node_result.is_error()) {
    completer(init_display_node_result.take_error());
    return;
  }

  zx::result<> init_gpu_core_node_result = InitGpuCoreNode(context.node_name());
  if (init_gpu_core_node_result.is_error()) {
    completer(init_gpu_core_node_result.take_error());
    return;
  }

  controller_->Start(std::move(completer));
}

void IntelDisplayDriver::PrepareStopOnPowerOn(fdf::StopCompleter completer) {
  if (gpu_core_node_controller_.is_valid()) {
    fidl::OneWayStatus remove_gpu_result = gpu_core_node_controller_->Remove();
    if (!remove_gpu_result.ok()) {
      fdf::warn("Failed to remove gpu core node: {}", remove_gpu_result.status_string());
    }
  }

  if (display_node_controller_.is_valid()) {
    fidl::OneWayStatus remove_display_result = display_node_controller_->Remove();
    if (!remove_display_result.ok()) {
      fdf::warn("Failed to remove display node: {}", remove_display_result.status_string());
    }
  }

  if (controller_) {
    controller_->PrepareStopOnPowerOn(std::move(completer));
  } else {
    completer(zx::ok());
  }
}

void IntelDisplayDriver::PrepareStopOnPowerStateTransition(
    fuchsia_system_state::SystemPowerState power_state, fdf::StopCompleter completer) {
  if (controller_) {
    controller_->PrepareStopOnPowerStateTransition(power_state, std::move(completer));
  } else {
    completer(zx::ok());
  }
}

void IntelDisplayDriver::Stop(fdf::StopCompleter completer) {
  zx::result<fuchsia_system_state::SystemPowerState> system_power_state_result =
      GetSystemPowerState(*incoming_);
  if (system_power_state_result.is_error()) {
    fdf::warn("Failed to get system power state: {}, fallback to fully on",
              system_power_state_result);
  }

  fuchsia_system_state::SystemPowerState system_power_state =
      system_power_state_result.value_or(fuchsia_system_state::SystemPowerState::kFullyOn);

  if (system_power_state == fuchsia_system_state::SystemPowerState::kFullyOn) {
    PrepareStopOnPowerOn(std::move(completer));
    return;
  }
  PrepareStopOnPowerStateTransition(system_power_state, std::move(completer));
}

zx::result<ddk::AnyProtocol> IntelDisplayDriver::GetProtocol(uint32_t proto_id) {
  return controller_->GetProtocol(proto_id);
}

zx::result<> IntelDisplayDriver::InitDisplayNode() {
  ZX_DEBUG_ASSERT(!display_node_controller_.is_valid());

  // Serves the [`fuchsia.hardware.display.engine/Service`] service
  // over the compatibility server.
  fuchsia_hardware_display_engine::Service::InstanceHandler service_handler(
      {.engine = engine_fidl_adapter_->CreateHandler(*(driver_dispatcher()->get()))});
  zx::result<> add_service_result =
      outgoing()->AddService<fuchsia_hardware_display_engine::Service>(std::move(service_handler));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add service: {}", add_service_result);
    return add_service_result.take_error();
  }

  static constexpr std::string_view kDisplayChildNodeName = "intel-display-controller";
  const std::vector<fuchsia_driver_framework::Offer> node_offers = {
      fdf::MakeOffer2<fuchsia_hardware_display_engine::Service>(),
  };
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

zx::result<> IntelDisplayDriver::InitGpuCoreNode(const std::optional<std::string>& node_name) {
  ZX_DEBUG_ASSERT(!gpu_core_node_controller_.is_valid());

  // Serves the [`fuchsia.hardware.intelgpucore/IntelGpuCore`] protocol
  // over the compatibility server.
  zx::result<ddk::AnyProtocol> protocol_result =
      controller_->GetProtocol(ZX_PROTOCOL_INTEL_GPU_CORE);
  ZX_DEBUG_ASSERT(protocol_result.is_ok());
  ddk::AnyProtocol protocol = std::move(protocol_result).value();

  gpu_banjo_server_ = compat::BanjoServer(ZX_PROTOCOL_INTEL_GPU_CORE, protocol.ctx, protocol.ops);
  compat::DeviceServer::BanjoConfig banjo_config;
  banjo_config.callbacks[ZX_PROTOCOL_INTEL_GPU_CORE] = gpu_banjo_server_->callback();

  static constexpr std::string_view kGpuCoreChildNodeName = "intel-gpu-core";
  zx::result<> compat_server_init_result =
      gpu_compat_server_.Initialize(incoming_, outgoing(), node_name, kGpuCoreChildNodeName,
                                    /*forward_metadata=*/compat::ForwardMetadata::None(),
                                    /*banjo_config=*/std::move(banjo_config));
  if (compat_server_init_result.is_error()) {
    fdf::error("Failed to initialize the compatibility server: {}", compat_server_init_result);
    return compat_server_init_result.take_error();
  }

  const std::vector<fuchsia_driver_framework::NodeProperty2> node_properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL,
                         bind_fuchsia_intel_platform_gpucore::BIND_PROTOCOL_DEVICE),
  };
  const std::vector<fuchsia_driver_framework::Offer> node_offers =
      gpu_compat_server_.CreateOffers2();
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>>
      gpu_core_node_controller_client_result =
          AddChild(kGpuCoreChildNodeName, node_properties, node_offers);
  if (gpu_core_node_controller_client_result.is_error()) {
    fdf::error("Failed to add child node: {}", gpu_core_node_controller_client_result);
    return gpu_core_node_controller_client_result.take_error();
  }

  gpu_core_node_controller_ =
      fidl::WireSyncClient(std::move(gpu_core_node_controller_client_result).value());
  return zx::ok();
}

}  // namespace intel_display

FUCHSIA_DRIVER_EXPORT2(intel_display::IntelDisplayDriver);
