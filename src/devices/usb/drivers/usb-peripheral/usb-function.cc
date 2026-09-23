// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-peripheral/usb-function.h"

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/metadata.h>
#include <lib/trace/event.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/usb/cpp/bind.h>
#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>

#include "src/devices/usb/drivers/usb-peripheral/usb-peripheral.h"

namespace usb_peripheral {

namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace ffunction = fuchsia_hardware_usb_function;

zx::result<> UsbFunction::AddChild(fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
                                   const std::shared_ptr<fdf::Namespace>& incoming,
                                   const std::shared_ptr<fdf::OutgoingDirectory>& outgoing) {
  TRACE_DURATION("usb-peripheral", __func__);
  outgoing_ = outgoing;
  inspect_.Init(peripheral_->inspect_node(), name_, static_cast<uint8_t>(index_));
  if (child_.is_valid()) {
    return zx::error(ZX_ERR_ALREADY_BOUND);
  }

  {
    zx::result result =
        compat_server_.Initialize(incoming, outgoing, std::string{UsbPeripheral::kChildNodeName},
                                  name_, compat::ForwardMetadata::None(), {});
    if (result.is_error()) {
      fdf::error("Failed to initialize compat server: {}", result);
      return result.take_error();
    }
  }

  auto& mac_address_metadata_server = mac_address_metadata_server_.emplace(name_);
  if (zx::result result =
          mac_address_metadata_server.ForwardAndServe(*outgoing, dispatcher_, incoming);
      result.is_error()) {
    fdf::error("Failed to forward mac address metadata: {}", result);
    return result.take_error();
  }

  auto& serial_number_metadata_server = serial_number_metadata_server_.emplace(name_);
  if (zx::result result =
          serial_number_metadata_server.ForwardAndServe(*outgoing, dispatcher_, incoming);
      result.is_error()) {
    fdf::error("Failed to forward serial number metadata: {}", result);
    return result.take_error();
  }

  bindings_.set_empty_set_handler([weak_this = weak_from_this()]() {
    std::shared_ptr self = weak_this.lock();
    if (!self) {
      return;
    }
    // We need to release all allocated resources when our channel is closed.
    // Which also means that we need to close our connection with the function
    // interface to make sure that resources are not used after they've been
    // released.
    if (self->function_intf_.is_valid()) {
      self->function_intf_.AsyncTeardown();
      self->function_intf_ = {};
    }

    self->peripheral_->ReleaseResources(self->index_);
  });

  zx::result result = outgoing->AddService<ffunction::UsbFunctionService>(
      ffunction::UsbFunctionService::InstanceHandler({
          .device = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
      }),
      name_);
  if (result.is_error()) {
    fdf::error("Failed to add usb-function service: {}", result);
    return result.take_error();
  }

  auto& desc = GetFunctionDescriptor();

  std::vector props = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_usb::BIND_PROTOCOL_FUNCTION),
      fdf::MakeProperty2(bind_fuchsia::USB_CLASS, static_cast<uint32_t>(desc.interface_class)),
      fdf::MakeProperty2(bind_fuchsia::USB_SUBCLASS,
                         static_cast<uint32_t>(desc.interface_subclass)),
      fdf::MakeProperty2(bind_fuchsia::USB_PROTOCOL,
                         static_cast<uint32_t>(desc.interface_protocol)),
      fdf::MakeProperty2(bind_fuchsia::USB_VID,
                         static_cast<uint32_t>(peripheral_->device_desc().id_vendor)),
      fdf::MakeProperty2(bind_fuchsia::USB_PID,
                         static_cast<uint32_t>(peripheral_->device_desc().id_product)),
  };

  std::vector offers = compat_server_.CreateOffers2();
  offers.push_back(fdf::MakeOffer2<ffunction::UsbFunctionService>(name_));
  if (std::optional offer = mac_address_metadata_server.CreateOffer(); offer.has_value()) {
    offers.push_back(std::move(offer.value()));
  }
  if (std::optional offer = serial_number_metadata_server.CreateOffer(); offer.has_value()) {
    offers.push_back(std::move(offer.value()));
  }

  auto bus_info = fuchsia_driver_framework::BusInfo{{
      .bus = fuchsia_driver_framework::BusType::kUsbPeripheral,
      .address =
          fuchsia_driver_framework::DeviceAddress::WithIntValue(static_cast<uint8_t>(index_)),
      .address_stability =
          fuchsia_driver_framework::DeviceAddressStability::kUnstableBetweenSoftwareUpdate,
  }};

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {std::string(name_)},
      .offers2 = std::move(offers),
      .bus_info = std::move(bus_info),
      .properties2 = std::move(props),
  }};

  auto [node_controller_client_end, node_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  fidl::Result add_child_result =
      fidl::Call(parent)->AddChild({std::move(args), std::move(node_controller_server_end), {}});

  if (add_child_result.is_error()) {
    fdf::error("Failed to add child: {}", add_child_result.error_value().FormatDescription());
    return zx::error(add_child_result.error_value().is_framework_error()
                         ? add_child_result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  child_.Bind(std::move(node_controller_client_end), dispatcher_,
              std::make_unique<NodeControllerEventHandler>(this));

  return zx::ok();
}

UsbFunction::~UsbFunction() {
  if (deconfigure_completer_.has_value()) {
    deconfigure_completer_->Reply(fit::ok());
    deconfigure_completer_.reset();
  }

  if (outgoing_ && !name_.empty()) {
    // We explicitly remove the child node and its associated services here to ensure
    // that they are scrubbed before the UsbFunction is destroyed. This prevents
    // resource name collisions if the driver re-initializes new functions without
    // a full driver restart (e.g. during a ClearFunctions/SetConfiguration cycle).
    // This also ensures teardown synchronization in the parent, keeping the system
    // clean by the time the parent's completion callbacks execute.
    fdf::debug("UsbFunction destructor: cleaning up child {}", name_);
    if (zx::result result = outgoing_->RemoveService<ffunction::UsbFunctionService>(name_);
        result.is_error()) {
      fdf::warn("Failed to remove usb-function service for {}: {}", name_, result);
    }
    if (mac_address_metadata_server_) {
      if (zx::result result = outgoing_->RemoveService(
              fuchsia_boot_metadata::MacAddressMetadata::kSerializableName, name_);
          result.is_error()) {
        fdf::warn("Failed to remove mac address metadata service for {}: {}", name_, result);
      }
    }
    if (serial_number_metadata_server_) {
      if (zx::result result = outgoing_->RemoveService(
              fuchsia_boot_metadata::SerialNumberMetadata::kSerializableName, name_);
          result.is_error()) {
        fdf::warn("Failed to remove serial number metadata service for {}: {}", name_, result);
      }
    }

    if (child_.is_valid()) {
      fidl::Status result = child_->Remove();
      if (!result.ok()) {
        fdf::error("Failed to send Remove request to child node {}: {}", name_,
                   result.FormatDescription());
      }
    }
  }
}

void UsbFunction::ConnectToEndpoint(ConnectToEndpointRequest& request,
                                    ConnectToEndpointCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  if (!peripheral_->ValidateEndpoint(index_, request.ep_addr())) {
    completer.Reply(fit::as_error(ZX_ERR_NOT_FOUND));
    return;
  }
  auto status = peripheral_->ConnectToEndpoint(request.ep_addr(), std::move(request.ep()));
  if (status != ZX_OK) {
    completer.Reply(fit::as_error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void UsbFunction::AllocResources(AllocResourcesRequest& request,
                                 AllocResourcesCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  if (function_intf_.is_valid()) {
    fdf::error("AllocResources failed: already configured");
    completer.Reply(fit::as_error(ZX_ERR_BAD_STATE));
    return;
  }

  zx::result result = peripheral_->AllocResources(index_, request.interface_count(),
                                                  request.endpoints(), request.strings());
  if (result.is_error()) {
    completer.Reply(fit::as_error(result.error_value()));
    return;
  }

  ffunction::UsbFunctionAllocResourcesResponse response;
  response.interface_nums(std::move(result->interface_nums));
  response.endpoint_addrs(std::move(result->endpoint_addrs));
  response.string_indices(std::move(result->string_indices));

  completer.Reply(fit::ok(std::move(response)));
}

void UsbFunction::EndpointSetStall(EndpointSetStallRequest& request,
                                   EndpointSetStallCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", request.endpoint_address());
  if (!function_intf_.is_valid()) {
    fdf::error("EndpointSetStall failed: not configured");
    completer.Reply(fit::as_error(ZX_ERR_BAD_STATE));
    return;
  }
  zx_status_t status = CommonEndpointSetStall(request.endpoint_address());
  if (status != ZX_OK) {
    completer.Reply(fit::as_error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void UsbFunction::EndpointClearStall(EndpointClearStallRequest& request,
                                     EndpointClearStallCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", request.endpoint_address());
  if (!function_intf_.is_valid()) {
    fdf::error("EndpointClearStall failed: not configured");
    completer.Reply(fit::as_error(ZX_ERR_BAD_STATE));
    return;
  }
  zx_status_t status = CommonEndpointClearStall(request.endpoint_address());
  if (status != ZX_OK) {
    completer.Reply(fit::as_error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void UsbFunction::ConfigureEndpoint(ConfigureEndpointRequest& request,
                                    ConfigureEndpointCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", request.endpoint_address());
  if (!function_intf_.is_valid()) {
    fdf::error("ConfigureEndpoint failed: not configured");
    completer.Reply(fit::as_error(ZX_ERR_BAD_STATE));
    return;
  }
  zx_status_t status =
      CommonEndpointConfigure(request.endpoint_address(), request.endpoint_configuration());
  if (status != ZX_OK) {
    completer.Reply(fit::as_error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void UsbFunction::DisableEndpoint(DisableEndpointRequest& request,
                                  DisableEndpointCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", request.endpoint_address());
  if (!function_intf_.is_valid()) {
    fdf::error("DisableEndpoint failed: not configured");
    completer.Reply(fit::as_error(ZX_ERR_BAD_STATE));
    return;
  }
  zx_status_t status = CommonEndpointDisable(request.endpoint_address());
  if (status != ZX_OK) {
    completer.Reply(fit::as_error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void UsbFunction::Configure(ConfigureRequest& request, ConfigureCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  if (peripheral_->IsPeripheralStopping()) {
    fdf::warn("Configure rejected: Peripheral is stopping or driver is tearing down.");
    completer.Reply(fit::as_error(ZX_ERR_BAD_STATE));
    return;
  }

  if (function_intf_.is_valid()) {
    fdf::error("Function interface already bound");
    completer.Reply(fit::as_error(ZX_ERR_ALREADY_BOUND));
    return;
  }

  size_t length = request.configuration().size();
  fbl::AllocChecker ac;
  auto* descriptors = new (&ac) uint8_t[length];
  if (!ac.check()) {
    fdf::error("UsbFunction::Configure failed due to no memory.");
    completer.Reply(fit::as_error(ZX_ERR_NO_MEMORY));
    return;
  }

  memcpy(descriptors, request.configuration().data(), length);
  num_interfaces_ = 0;

  zx::result validate_result = peripheral_->ValidateFunction(index_, descriptors, length);
  if (validate_result.is_error()) {
    fdf::error("UsbFunctionInterfaceClient::ValidateFunction() failed: {}", validate_result);
    delete[] descriptors;
    completer.Reply(fit::as_error(validate_result.error_value()));
    return;
  }
  num_interfaces_ = validate_result.value();

  SetDescriptors(descriptors, length);
  function_intf_.Bind(std::move(request.iface()), dispatcher_,
                      std::make_unique<FunctionEventHandler>(this));
  zx_status_t status = peripheral_->FunctionRegistered();

  if (status != ZX_OK) {
    completer.Reply(fit::as_error(status));
    fdf::error("FunctionRegistered failed: {}", zx_status_get_string(status));
    function_intf_ = {};
    ClearDescriptors();
    return;
  }

  completer.Reply(fit::ok());
}

void UsbFunction::Deconfigure(DeconfigureCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);

  if (deconfigure_completer_.has_value()) {
    completer.Reply(fit::as_error(ZX_ERR_UNAVAILABLE));
    return;
  }
  if (!function_intf_.is_valid()) {
    fdf::warn("Deconfigure called with no function interface bound");
    completer.Reply(fit::ok());
    return;
  }
  deconfigure_completer_.emplace(completer.ToAsync());
  function_intf_.AsyncTeardown();
}

void UsbFunction::FunctionEventHandler::on_fidl_error(fidl::UnbindInfo info) {
  switch (info.status()) {
    case ZX_ERR_PEER_CLOSED:
    case ZX_OK:
    case ZX_ERR_CANCELED:
      if (std::shared_ptr parent = parent_.lock()) {
        parent->CloseFunctionInterface();
      }
      break;
    default:
      fdf::error("Unexpected FIDL error on function interface: {}",
                 zx_status_get_string(info.status()));
      return;
  }
}

void UsbFunction::FunctionEventHandler::handle_unknown_event(
    fidl::UnknownEventMetadata<ffunction::UsbFunctionInterface> metadata) {
  fdf::error("Unknown event on function interface: {}", metadata.event_ordinal);
}

UsbFunction::FunctionEventHandler::~FunctionEventHandler() {
  if (std::shared_ptr parent = parent_.lock()) {
    parent->CloseFunctionInterface();
  }
}

void UsbFunction::CloseFunctionInterface() {
  function_intf_ = {};
  ClearDescriptors();
  inspect_.UpdateConfiguration(0, false);
  peripheral_->FunctionUnregistered();

  if (deconfigure_completer_.has_value()) {
    deconfigure_completer_->Reply(fit::ok());
    deconfigure_completer_.reset();
  }
}

void UsbFunction::RequestRemoval() {
  async::PostTask(dispatcher_, [this, self = shared_from_this()]() {
    if (!child_.is_valid()) {
      // If there is no child, we're already effectively cleared. Since child_ is invalid,
      // active_functions_count_ was either never incremented or already decremented.
      peripheral_->FunctionCleared(function_index(), config_generation());
      return;
    }

    fdf::debug("UsbFunction: Sending Remove() to node Controller for {}.", name_);
    fidl::Status result = child_->Remove();
    if (!result.ok()) {
      fdf::error("UsbFunction: Failed to send Remove request to child node {}: {}", name_,
                 result.FormatDescription());
    }
  });
}

void UsbFunction::OnNodeControllerUnbound(fidl::UnbindInfo info) {
  fdf::debug("UsbFunction: OnNodeControllerUnbound called for {}.", name_);
  if (info.is_peer_closed()) {
    fdf::info("UsbFunction: Node for {} unbound (peer closed).", name_);
  } else if (!info.is_user_initiated()) {
    fdf::error("UsbFunction: Node for {} unbound with error: {}", name_, info.FormatDescription());
  } else {
    fdf::info("UsbFunction: Node for {} unbound (user initiated).", name_);
  }
  child_ = {};
  CloseFunctionInterface();
  peripheral_->DecrementActiveFunctions(config_generation());
  peripheral_->FunctionCleared(function_index(), config_generation());
}

void UsbFunction::SetConfigured(bool configured, usb_speed_t speed,
                                fit::callback<void(zx_status_t)> completer) {
  TRACE_DURATION("usb-peripheral", __func__);

  bool unconfigure_first = false;
  if (last_configured_.has_value() && *last_configured_ == configured) {
    if (!configured) {
      // Nothing to do since it's already in the desired unconfigured state.
      completer(ZX_OK);
      return;
    }

    // From the USB 2.0 specification, section 9.1.1.5:
    //
    //    Before a USB device’s function may be used, the device must be
    //    configured. From the device’s perspective, configuration involves
    //    correctly processing a SetConfiguration() request with a non-zero
    //    configuration value. Configuring a device or changing an alternate
    //    setting causes all of the status and configuration values associated
    //    with endpoints in the affected interfaces to be set to their default
    //    values. This includes setting the data toggle of any endpoint using
    //    data toggles to the value DATA0.
    //
    // The easy way to get compliance for all function drivers is to flap them
    // before acknowledging the configuration change.
    fdf::info("SetConfigured called twice with configured = true; forcing configuration flap on {}",
              index_);
    unconfigure_first = true;
  }

  last_configured_ = configured;
  if (!function_intf_.is_valid()) {
    if (!configured) {
      // If the function interface is invalid (not yet registered or already unbound
      // during teardown/deconfigure), the function is de-facto unconfigured. Succeed
      // immediately with ZX_OK to prevent spurious ZX_ERR_BAD_STATE error logging and
      // allow unconfigure promises to complete cleanly during disconnect and teardown.
      completer(ZX_OK);
      return;
    }
    fdf::error("SetConfigured failed as the interface is invalid.");
    completer(ZX_ERR_BAD_STATE);
    return;
  }

  fdescriptor::wire::UsbSpeed fspeed = static_cast<fdescriptor::wire::UsbSpeed>(speed);
  auto send_set_configured = [configured, fspeed, weak_this = weak_from_this()](
                                 fit::callback<void(zx_status_t)> completer) {
    auto self = weak_this.lock();
    if (!self) {
      completer(!configured ? ZX_OK : ZX_ERR_CANCELED);
      return;
    }
    if (!self->function_intf_.is_valid()) {
      if (!configured) {
        completer(ZX_OK);
        return;
      }
      fdf::error("SetConfigured failed as the interface is invalid.");
      completer(ZX_ERR_BAD_STATE);
      return;
    }
    self->function_intf_->SetConfigured(configured, fspeed)
        .ThenExactlyOnce(
            [weak_this, configured, completer = std::move(completer)](
                fidl::WireUnownedResult<ffunction::UsbFunctionInterface::SetConfigured>&
                    result) mutable {
              if (!result.ok()) {
                // If we are deconfiguring, and the function interface was unbound or closed
                // (e.g. user initiated unbind or peer closed during disconnect/teardown),
                // the function is already de-facto unconfigured. Treat this as ZX_OK to prevent
                // spurious error logging and allow disconnect/teardown promises to complete
                // cleanly.
                if (!configured &&
                    (result.status() == ZX_ERR_CANCELED || result.status() == ZX_ERR_PEER_CLOSED)) {
                  completer(ZX_OK);
                  return;
                }
                fdf::error("UsbFunctionInterface.SetConfigured FIDL call failed: {}",
                           result.FormatDescription());
                completer(result.status());
                return;
              }
              if (result->is_error()) {
                if (!configured && (result->error_value() == ZX_ERR_BAD_STATE ||
                                    result->error_value() == ZX_ERR_NOT_CONNECTED)) {
                  completer(ZX_OK);
                  return;
                }
                fdf::error("UsbFunctionInterface.SetConfigured error: {}",
                           zx_status_get_string(result->error_value()));
                completer(result->error_value());
                return;
              }
              if (auto self = weak_this.lock()) {
                self->inspect_.UpdateConfiguration(self->configuration_ + 1, configured);
              }
              completer(ZX_OK);
            });
  };

  if (unconfigure_first) {
    function_intf_->SetConfigured(false, fspeed)
        .ThenExactlyOnce(
            [send_set_configured = std::move(send_set_configured),
             completer = std::move(completer)](
                fidl::WireUnownedResult<ffunction::UsbFunctionInterface::SetConfigured>&
                    result) mutable {
              if (!result.ok()) {
                if (result.status() == ZX_ERR_CANCELED || result.status() == ZX_ERR_PEER_CLOSED) {
                  send_set_configured(std::move(completer));
                  return;
                }
                fdf::error("UsbFunctionInterface.SetConfigured FIDL call failed on deconfigure: {}",
                           result.FormatDescription());
                completer(result.status());
                return;
              }
              if (result->is_error()) {
                if (result->error_value() == ZX_ERR_BAD_STATE ||
                    result->error_value() == ZX_ERR_NOT_CONNECTED) {
                  send_set_configured(std::move(completer));
                  return;
                }
                fdf::error("UsbFunctionInterface.SetConfigured error on deconfigure: {}",
                           zx_status_get_string(result->error_value()));
                completer(result->error_value());
                return;
              }
              send_set_configured(std::move(completer));
            });
  } else {
    send_set_configured(std::move(completer));
  }
}

void UsbFunction::SetInterface(uint8_t interface, uint8_t alt_setting,
                               fit::callback<void(zx_status_t)> completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  if (!function_intf_.is_valid()) {
    fdf::error("SetInterface failed as the interface is invalid.");
    completer(ZX_ERR_BAD_STATE);
    return;
  }

  function_intf_->SetInterface(interface, alt_setting)
      .ThenExactlyOnce([completer = std::move(completer)](
                           fidl::WireUnownedResult<ffunction::UsbFunctionInterface::SetInterface>&
                               result) mutable {
        if (!result.ok()) {
          fdf::error("UsbFunctionInterface.SetInterface FIDL call failed: {}",
                     result.FormatDescription());
          completer(result.status());
          return;
        }
        if (result->is_error()) {
          fdf::error("UsbFunctionInterface.SetInterface error: {}",
                     zx_status_get_string(result->error_value()));
          completer(result->error_value());
          return;
        }
        completer(ZX_OK);
      });
}

// TODO(https://fxbug.dev/493657863): This call should be async like
// SetConfigured and SetInterface once we can guarantee a single-dispatch of
// USB_RECIP_DEVICE requests to bound functions.
zx::result<std::vector<uint8_t>> UsbFunction::Control(const fdescriptor::wire::UsbSetup& setup,
                                                      cpp20::span<uint8_t> write_buffer) {
  TRACE_DURATION("usb-peripheral", __func__);
  if (!function_intf_.is_valid()) {
    fdf::error("Control failed as the interface is invalid.");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  fidl::VectorView<uint8_t> write_data =
      fidl::VectorView<uint8_t>::FromExternal(write_buffer.data(), write_buffer.size());
  size_t expected_read_size = le16toh(setup.w_length);

  auto result = function_intf_.sync()->Control(setup, write_data);
  if (!result.ok()) {
    fdf::error("UsbFunctionInterface.Control FIDL call failed: {}", result.FormatDescription());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("UsbFunctionInterface.Control error: {}",
               zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }

  ffunction::wire::UsbFunctionInterfaceControlResponse* response = result->value();
  size_t actual_read = response->read.size();
  if (actual_read > expected_read_size) {
    fdf::error("Control read too much data: {} > {}", actual_read, expected_read_size);
    return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
  }

  std::vector<uint8_t> read_data_vec(response->read.begin(), response->read.end());
  return zx::ok(std::move(read_data_vec));
}

zx_status_t UsbFunction::CommonEndpointSetStall(uint8_t ep_address) {
  if (!peripheral_->ValidateEndpoint(index_, ep_address)) {
    return ZX_ERR_NOT_FOUND;
  }

  fidl::Arena arena;
  auto result = peripheral_->dci().buffer(arena)->EndpointSetStall(ep_address);

  if (!result.ok()) {
    fdf::error("Failed to send EndpointSetStall request: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("EndpointSetStall failed: {}", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  peripheral_->dci_inspect().RecordEvent(std::format("endpoint 0x{:02x} stalled", ep_address));
  return ZX_OK;
}

zx_status_t UsbFunction::CommonEndpointClearStall(uint8_t ep_address) {
  if (!peripheral_->ValidateEndpoint(index_, ep_address)) {
    return ZX_ERR_NOT_FOUND;
  }

  fidl::Arena arena;
  auto result = peripheral_->dci().buffer(arena)->EndpointClearStall(ep_address);

  if (!result.ok()) {
    fdf::error("Failed to send EndpointClearStall request: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("EndpointClearStall failed: {}", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  peripheral_->dci_inspect().RecordEvent(
      std::format("endpoint 0x{:02x} stall cleared", ep_address));
  return ZX_OK;
}

zx_status_t UsbFunction::CommonEndpointConfigure(
    uint8_t ep_address, ffunction::EndpointConfiguration endpoint_configuration) {
  if (!endpoint_configuration.descriptor().has_value()) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!peripheral_->ValidateEndpoint(index_, ep_address)) {
    return ZX_ERR_NOT_FOUND;
  }

  fidl::Arena arena;
  fdescriptor::wire::UsbEndpointDescriptor fep_desc = {
      .b_endpoint_address = ep_address,
      .bm_attributes = endpoint_configuration.descriptor()->bm_attributes(),
      // TODO(https://fxbug.dev/497048374): FIDL types should use host
      // endianness.
      .w_max_packet_size = htole16(endpoint_configuration.descriptor()->w_max_packet_size()),
      .b_interval = endpoint_configuration.descriptor()->b_interval(),
  };
  fdescriptor::wire::UsbSsEpCompDescriptor fss_comp_desc;
  if (endpoint_configuration.super_speed_companion().has_value()) {
    fss_comp_desc = {
        .b_max_burst = endpoint_configuration.super_speed_companion()->b_max_burst(),
        .bm_attributes = endpoint_configuration.super_speed_companion()->bm_attributes(),
        // TODO(https://fxbug.dev/497048374): FIDL types should use host
        // endianness.
        .w_bytes_per_interval =
            htole16(endpoint_configuration.super_speed_companion()->w_bytes_per_interval()),
    };
  }
  auto result = peripheral_->dci().buffer(arena)->ConfigureEndpoint(fep_desc, fss_comp_desc);

  if (!result.ok()) {
    fdf::error("Failed to send ConfigureEndpoint request: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("Failed to configure endpoint: {}", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  peripheral_->dci_inspect().RecordEvent(std::format("endpoint 0x{:02x} configured", ep_address));
  return ZX_OK;
}

zx_status_t UsbFunction::CommonEndpointDisable(uint8_t ep_address) {
  if (!peripheral_->ValidateEndpoint(index_, ep_address)) {
    return ZX_ERR_NOT_FOUND;
  }

  fidl::Arena arena;
  auto result = peripheral_->dci().buffer(arena)->DisableEndpoint(ep_address);

  if (!result.ok()) {
    fdf::error("Failed to send DisableEndpoint request: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    // When the USB peripheral is disconnected or the DCI controller transitions
    // to an unpowered/stopped state, hardware endpoints are no longer present,
    // and the DCI driver returns ZX_ERR_IO_NOT_PRESENT. Because the endpoint
    // is already disabled/inaccessible in hardware, treating this as a success (ZX_OK)
    // allows teardown and disconnect cleanups to complete cleanly without logging
    // spurious errors, while recording the inspect event.
    if (result->error_value() != ZX_ERR_IO_NOT_PRESENT) {
      fdf::error("Failed to disable endpoint: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
  peripheral_->dci_inspect().RecordEvent(std::format("endpoint 0x{:02x} disabled", ep_address));
  return ZX_OK;
}

void UsbFunction::SetDescriptors(uint8_t* descriptors, size_t length) {
  descriptors_.reset(descriptors, length);
  inspect_.SetDescriptors(
      std::vector<uint8_t>(descriptors_.data(), descriptors_.data() + descriptors_.size()));
}

void UsbFunction::ClearDescriptors() {
  descriptors_.reset();
  inspect_.SetDescriptors({});
}

std::optional<std::vector<uint8_t>> UsbFunction::GetEndpointsForInterface(
    uint8_t interface_num, uint8_t alt_setting) const {
  if (descriptors_.size() < sizeof(usb_descriptor_header_t)) {
    return std::nullopt;
  }
  std::vector<uint8_t> endpoints;

  const auto* header = reinterpret_cast<const usb_descriptor_header_t*>(descriptors_.data());
  const auto* end =
      reinterpret_cast<const usb_descriptor_header_t*>(descriptors_.data() + descriptors_.size());

  std::optional<uint8_t> cur_interface;
  std::optional<uint8_t> cur_alt;

  while (reinterpret_cast<const uint8_t*>(header) + sizeof(usb_descriptor_header_t) <=
         reinterpret_cast<const uint8_t*>(end)) {
    if (header->b_length < sizeof(usb_descriptor_header_t) ||
        reinterpret_cast<const uint8_t*>(header) + header->b_length >
            reinterpret_cast<const uint8_t*>(end)) {
      break;
    }
    if (header->b_descriptor_type == USB_DT_INTERFACE) {
      if (header->b_length >= sizeof(usb_interface_descriptor_t)) {
        const auto* desc = reinterpret_cast<const usb_interface_descriptor_t*>(header);
        cur_interface = desc->b_interface_number;
        cur_alt = desc->b_alternate_setting;
      } else {
        cur_interface.reset();
        cur_alt.reset();
      }
    } else if (header->b_descriptor_type == USB_DT_ENDPOINT) {
      if (cur_interface == interface_num && cur_alt == alt_setting) {
        if (header->b_length >= sizeof(usb_endpoint_descriptor_t)) {
          const auto* desc = reinterpret_cast<const usb_endpoint_descriptor_t*>(header);
          endpoints.push_back(desc->b_endpoint_address);
        }
      }
    }
    header = reinterpret_cast<const usb_descriptor_header_t*>(
        reinterpret_cast<const uint8_t*>(header) + header->b_length);
  }

  return endpoints;
}

}  // namespace usb_peripheral
