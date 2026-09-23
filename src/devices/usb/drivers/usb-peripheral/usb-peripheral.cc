// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-peripheral/usb-peripheral.h"

#include <assert.h>
#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/span.h>
#include <lib/trace/event.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>
#include <zircon/errors.h>
#include <zircon/listnode.h>
#include <zircon/types.h>

#include <algorithm>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include <fbl/auto_lock.h>
#include <usb/cdc.h>
#include <usb/descriptors.h>
#include <usb/peripheral.h>
#include <usb/usb.h>

#include "src/devices/usb/drivers/usb-peripheral/config-parser.h"
#include "src/devices/usb/drivers/usb-peripheral/usb-function.h"

namespace usb_peripheral {

namespace fdci = fuchsia_hardware_usb_dci;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace ffunction = fuchsia_hardware_usb_function;
namespace fperipheral = fuchsia_hardware_usb_peripheral;
namespace fphy = fuchsia_hardware_usb_phy;

}  // namespace usb_peripheral

namespace fendpoint = fuchsia_hardware_usb_endpoint;

namespace std {

template <>
struct formatter<fendpoint::wire::EndpointInfo> : formatter<string_view> {
  auto format(const fendpoint::wire::EndpointInfo& info, format_context& ctx) const {
    string_view name = "unknown";
    switch (info.Which()) {
      case fendpoint::wire::EndpointInfo::Tag::kBulk:
        name = "bulk";
        break;
      case fendpoint::wire::EndpointInfo::Tag::kControl:
        name = "control";
        break;
      case fendpoint::wire::EndpointInfo::Tag::kIsochronous:
        name = "isochronous";
        break;
      case fendpoint::wire::EndpointInfo::Tag::kInterrupt:
        name = "interrupt";
        break;
      default:
        break;
    }
    return formatter<string_view>::format(name, ctx);
  }
};

}  // namespace std

namespace usb_peripheral {

bool UsbPeripheral::IsPeripheralStopping() const {
  fbl::AutoLock lock(&lock_);
  // Return true if the peripheral is undergoing teardown or the driver is stopping.
  return state_ == DeviceState::kStopping || stopping_driver_;
}

zx_status_t UsbPeripheral::UsbDciCancelAll(uint8_t ep_address) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", ep_address);
  fidl::Arena arena;
  auto result = dci_.buffer(arena)->CancelAll(ep_address);

  if (!result.ok()) {
    fdf::error("CancelAll failed (framework): {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zx_status_t status = result->error_value();
    fdf::error("CancelAll failed (DCI error): {}", zx_status_get_string(status));
    return status;
  }
  dci_inspect_.RecordEvent(std::format("endpoint 0x{:02x} cancelled all requests", ep_address));
  return ZX_OK;
}

zx_status_t UsbPeripheral::UsbDciEndpointSetStall(uint8_t ep_address) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", ep_address);
  {
    fbl::AutoLock _(&lock_);
    stalled_eps_.insert(ep_address);
  }
  fidl::Arena arena;
  auto result = dci_.buffer(arena)->EndpointSetStall(ep_address);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t UsbPeripheral::UsbDciEndpointClearStall(uint8_t ep_address) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", ep_address);
  {
    fbl::AutoLock _(&lock_);
    stalled_eps_.erase(ep_address);
  }
  fidl::Arena arena;
  auto result = dci_.buffer(arena)->EndpointClearStall(ep_address);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t UsbPeripheral::ConnectToEndpoint(uint8_t ep_address,
                                             fidl::ServerEnd<fendpoint::Endpoint> ep) {
  TRACE_DURATION("usb-peripheral", __func__, "ep_address", ep_address);

  auto result = dci_->ConnectToEndpoint(ep_address, std::move(ep));
  if (!result.ok()) {
    return ZX_ERR_INTERNAL;  // framework error.
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx::result<> UsbPeripheral::Start(fdf::DriverContext context) {
  TRACE_DURATION("usb-peripheral", __func__);
  inspector_ = context.CreateInspector(this);
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  executor_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher());
  usb_peripheral_node_ = inspector_->root().CreateChild("usb-peripheral");
  dci_inspect_.Init(usb_peripheral_node_, "dci_metrics");
  {
    fbl::AutoLock lock(&lock_);
    dci_inspect_.UpdateState(std::format("{}", state_));
    dci_inspect_.UpdateUsbMode(cur_usb_mode_);
  }

  zx::result dci_fidl = incoming_->Connect<fdci::UsbDciService::Device>();
  if (dci_fidl.is_error()) {
    fdf::error("Failed to connect dci fidl protocol: {}", dci_fidl);
  } else {
    // Try to set DciIntf over FIDL. We don't do this for Banjo because SetInterface is used to
    // indicate that all functions have been attached/un-attached. This is a separate method in
    // FIDL, so we set DciIntf here for FIDL.
    fidl::Arena arena;
    auto client_end = intf_srv_.AddBinding();
    auto result = fidl::WireCall(*dci_fidl)->SetInterface(std::move(client_end));
    // DCI FIDL is not available. Return OK because we could be using Banjo DCI instead.
    // In the future when we remove banjo. This should be an error.
    if (!result.ok()) {
      fdf::info("SetInterface failed (framework): {}", result.status_string());
    } else if (result->is_error()) {
      fdf::error("SetInterface failed (DCI error): {}",
                 zx_status_get_string(result->error_value()));
    } else {
      if (zx_status_t status = InitializeDci(std::move(*dci_fidl)); status != ZX_OK) {
        return zx::error(status);
      }
    }
  }

  if (!dci_.is_valid()) {
    fdf::error("No FIDL UsbDci protocol served by parent");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // Starting USB mode is determined from device metadata.
  // We read initial value and store it in dev->usb_mode, but do not actually
  // enable it until after all of our functions have bound.
  zx::result metadata = fdf_metadata::GetMetadataIfExists<fphy::Metadata>(incoming_);
  if (metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata);
    return metadata.take_error();
  }
  if (!metadata.value().has_value()) {
    fbl::AutoLock lock(&lock_);
    // Assume peripheral mode by default.
    parent_usb_mode_ = USB_MODE_PERIPHERAL;
  }

  // Create child.
  zx::result child = AddOwnedChild(kChildNodeName);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  // Advertise a service:
  fperipheral::Service::InstanceHandler handler({
      .device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
  zx::result add_result = outgoing()->AddService<fperipheral::Service>(std::move(handler));
  if (add_result.is_error()) {
    fdf::error("Failed to add service: {}", add_result);
    return add_result.take_error();
  }

  auto config = context.take_config<usb_peripheral_config::Config>();
  PeripheralConfigParser peripheral_config = {};
  zx_status_t status;
  if (!config.kboot_functions().empty()) {
    fdf::debug("-driver.usb.peripheral kboot overrides: {}", config.kboot_functions());
    status = peripheral_config.AddFunctions(config.kboot_functions() | std::views::split(','));
  } else {
    status = peripheral_config.AddFunctions(config.functions());
  }

  if (status != ZX_OK) {
    fdf::error("Failed to add usb functions from structured config: {}",
               zx_status_get_string(status));
    return zx::error(status);
  }

  device_desc_.id_vendor = peripheral_config.vid();
  device_desc_.id_product = peripheral_config.pid();

  status =
      AllocStringDesc(std::nullopt, peripheral_config.manufacturer(), &device_desc_.i_manufacturer);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate manufacturer string descriptor: {}",
               zx_status_get_string(status));
    return zx::error(status);
  }

  status = AllocStringDesc(std::nullopt, peripheral_config.product(), &device_desc_.i_product);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate product string descriptor: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  zx::result serial = GetSerialNumber();
  if (serial.is_error()) {
    fdf::error("Failed to get serial number: {}", serial);
    return serial.take_error();
  }
  status = AllocStringDesc(std::nullopt, std::move(serial.value()), &device_desc_.i_serial_number);
  if (status != ZX_OK) {
    fdf::error("Failed to add serial number descriptor: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  if (!peripheral_config.functions().empty()) {
    status = SetDefaultConfig(peripheral_config.functions());
    if (status != ZX_OK) {
      fdf::error("Failed to set default config: {}", zx_status_get_string(status));
      return zx::error(status);
    }
  } else {
    fdf::warn("No functions found in config");
  }

  usb_monitor_.Start();
  return zx::ok();
}

zx::result<std::string> UsbPeripheral::GetSerialNumber() {
  TRACE_DURATION("usb-peripheral", __func__);
  zx::result serial_number_result =
      fdf_metadata::GetMetadataIfExists<fuchsia_boot_metadata::SerialNumberMetadata>(incoming_);
  if (serial_number_result.is_error()) {
    fdf::error("Failed to get serial number metadata: {}", serial_number_result);
    return serial_number_result.take_error();
  }
  // Return serial number from metadata if present.
  if (serial_number_result.value().has_value()) {
    auto& metadata = serial_number_result.value().value();
    if (!metadata.serial_number().has_value()) {
      fdf::error("Serial number metadata missing serial_number field");
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok(std::move(metadata.serial_number().value()));
  }

  // Use MAC address as the next option.
  zx::result mac_address_result =
      fdf_metadata::GetMetadataIfExists<fuchsia_boot_metadata::MacAddressMetadata>(incoming_);
  if (mac_address_result.is_error()) {
    fdf::error("Failed to get MAC address metadata: {}", mac_address_result);
    return mac_address_result.take_error();
  }
  if (mac_address_result.value().has_value()) {
    const auto& metadata = mac_address_result.value().value();
    if (!metadata.mac_address().has_value()) {
      fdf::error("MAC address metadata missing mac_address field");
      return zx::error(ZX_ERR_INTERNAL);
    }
    const auto& octets = metadata.mac_address().value().octets();
    char buffer[13];
    snprintf(buffer, sizeof(buffer), "%02X%02X%02X%02X%02X%02X", octets[0], octets[1], octets[2],
             octets[3], octets[4], octets[5]);
    return zx::ok(std::string{buffer});
  }

  fdf::info("Serial number/MAC address not found. Using generic (non-unique) serial number.\n");

  return zx::ok(std::string{kDefaultSerialNumber});
}

zx_status_t UsbPeripheral::AllocStringDesc(std::optional<size_t> function_index, std::string desc,
                                           uint8_t* out_index) {
  fbl::AutoLock lock(&lock_);
  return AllocStringDescLocked(function_index, std::move(desc), out_index);
}

zx_status_t UsbPeripheral::AllocStringDescLocked(std::optional<size_t> function_index,
                                                 std::string desc, uint8_t* out_index) {
  TRACE_DURATION("usb-peripheral", __func__);
  // Try to find an empty slot first.
  for (size_t i = 0; i < strings_.size(); i++) {
    if (!strings_[i].allocated) {
      strings_[i] = {
          .text = std::move(desc),
          .function_index = function_index,
          .allocated = true,
      };
      *out_index = static_cast<uint8_t>(i + 1);
      return ZX_OK;
    }
  }

  if (strings_.size() >= kMaxStrings) {
    fdf::error("String descriptor limit reached");
    return ZX_ERR_NO_RESOURCES;
  }

  strings_.push_back({
      .text = std::move(desc),
      .function_index = function_index,
      .allocated = true,
  });
  *out_index = static_cast<uint8_t>(strings_.size());
  return ZX_OK;
}

zx::result<uint8_t> UsbPeripheral::ValidateFunction(size_t function_index, void* descriptors,
                                                    size_t length) {
  TRACE_DURATION("usb-peripheral", __func__, "function_index", function_index);
  fbl::AutoLock lock(&lock_);
  if (state_ != DeviceState::kWaitForFunctionBind || stopping_driver_) {
    fdf::error("ValidateFunction: invalid device state {} (stopping={})", state_, stopping_driver_);
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (length < sizeof(usb_descriptor_header_t)) {
    fdf::error("ValidateFunction: descriptor buffer too short (length {})", length);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  auto* first_header = static_cast<const usb_descriptor_header_t*>(descriptors);
  uint8_t num_interfaces = 0;
  if (first_header->b_descriptor_type == USB_DT_INTERFACE) {
    if (length < sizeof(usb_interface_descriptor_t) ||
        first_header->b_length != sizeof(usb_interface_descriptor_t)) {
      fdf::error("{}: interface descriptor is invalid", __func__);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  } else if (first_header->b_descriptor_type == USB_DT_INTERFACE_ASSOCIATION) {
    if (length < sizeof(usb_interface_assoc_descriptor_t) ||
        first_header->b_length != sizeof(usb_interface_assoc_descriptor_t)) {
      fdf::error("{}: interface association descriptor is invalid", __func__);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  } else {
    fdf::error("{}: first descriptor not an interface descriptor", __func__);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto* end =
      reinterpret_cast<const usb_descriptor_header_t*>(static_cast<uint8_t*>(descriptors) + length);
  auto* header = reinterpret_cast<const usb_descriptor_header_t*>(descriptors);

  while (reinterpret_cast<const uint8_t*>(header) + sizeof(usb_descriptor_header_t) <=
         reinterpret_cast<const uint8_t*>(end)) {
    if (header->b_length < sizeof(usb_descriptor_header_t) ||
        reinterpret_cast<const uint8_t*>(header) + header->b_length >
            reinterpret_cast<const uint8_t*>(end)) {
      fdf::error("{}: invalid or truncated descriptor length {}", __func__, header->b_length);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    if (header->b_descriptor_type == USB_DT_INTERFACE) {
      if (header->b_length < sizeof(usb_interface_descriptor_t)) {
        fdf::error("{}: interface descriptor too short", __func__);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      auto* desc = reinterpret_cast<const usb_interface_descriptor_t*>(header);
      auto& function = GetFunction(function_index);
      ZX_ASSERT(function.configuration() < configurations_.size());
      const auto& configuration = configurations_[function.configuration()];
      const auto& interface_map = configuration.interface_map;
      if (desc->b_interface_number >= std::size(interface_map)) {
        fdf::error("Interface number {} too large", desc->b_interface_number);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      auto mapped_index = interface_map[desc->b_interface_number];
      if (!mapped_index.has_value() || *mapped_index != function_index) {
        fdf::error("Function index mismatch at interface {}: expected {}, found {}",
                   desc->b_interface_number, function_index,
                   mapped_index.has_value() ? std::to_string(*mapped_index) : "nullopt");
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      if (desc->b_alternate_setting == 0) {
        if (num_interfaces == UINT8_MAX) {
          return zx::error(ZX_ERR_INVALID_ARGS);
        }
        num_interfaces++;
      }
    } else if (header->b_descriptor_type == USB_DT_ENDPOINT) {
      if (header->b_length < sizeof(usb_endpoint_descriptor_t)) {
        fdf::error("{}: endpoint descriptor too short", __func__);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      auto* desc = reinterpret_cast<const usb_endpoint_descriptor_t*>(header);
      auto index = EpAddressToIndex(desc->b_endpoint_address);
      if (index == 0 || index >= std::size(endpoint_map_) ||
          endpoint_map_[index] != function_index) {
        fdf::error("Bad endpoint address {:#x}", desc->b_endpoint_address);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
    }

    header = reinterpret_cast<const usb_descriptor_header_t*>(
        reinterpret_cast<const uint8_t*>(header) + header->b_length);
  }

  if (header != end) {
    fdf::error("{}: trailing bytes in descriptor list", __func__);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(num_interfaces);
}

bool UsbPeripheral::AllFunctionsRegistered() const {
  TRACE_DURATION("usb-peripheral", __func__);
  bool all_registered = true;
  std::stringstream registered_ss;
  std::stringstream pending_ss;
  bool first_registered = true;
  bool first_pending = true;

  if (functions_.empty()) {
    return false;
  }

  for (const auto& config : configurations_) {
    for (auto function_index : config.functions) {
      const auto& function = GetFunction(function_index);

      if (function.registered()) {
        if (!first_registered) {
          registered_ss << ", ";
        }
        registered_ss << function.name();
        first_registered = false;
      } else {
        if (!first_pending) {
          pending_ss << ", ";
        }
        pending_ss << function.name();
        first_pending = false;
        all_registered = false;
      }
    }
  }

  fdf::info("Functions registered: [{}], pending: [{}]",
            first_registered ? "none" : registered_ss.str(),
            first_pending ? "none" : pending_ss.str());
  return all_registered;
}

zx_status_t UsbPeripheral::FunctionRegistered() {
  TRACE_DURATION("usb-peripheral", __func__);

  DeviceState state = DeviceState::kNoConfiguration;
  bool stopping = false;
  {
    fbl::AutoLock lock(&lock_);
    state = state_;
    stopping = stopping_driver_;
  }

  if (state == DeviceState::kStopping || stopping) {
    fdf::info("FunctionRegistered: called while stopping or tearing down. Ignoring.");
    return ZX_OK;
  }

  if (state != DeviceState::kWaitForFunctionBind) {
    fdf::error("FunctionRegistered: unexpected state {}", state);
    return ZX_ERR_BAD_STATE;
  }

  return CheckAndStartController();
}

zx_status_t UsbPeripheral::CheckAndStartController() {
  {
    fbl::AutoLock lock(&lock_);
    if (state_ != DeviceState::kWaitForFunctionBind || stopping_driver_) {
      return ZX_OK;
    }
    if (functions_.empty() || !AllFunctionsRegistered()) {
      return ZX_OK;
    }

    fdf::info("All functions registered. Starting peripheral controller.");
    SetStateLocked(DeviceState::kStarting);

    size_t config_idx = 0;
    for (auto& config : configurations_) {
      std::vector<uint8_t> config_desc_bytes(sizeof(usb_configuration_descriptor_t));
      {
        auto* config_desc =
            reinterpret_cast<usb_configuration_descriptor_t*>(config_desc_bytes.data());

        config_desc->b_length = sizeof(*config_desc);
        config_desc->b_descriptor_type = USB_DT_CONFIG;
        config_desc->b_num_interfaces = 0;
        config_desc->b_configuration_value = static_cast<uint8_t>(1 + config_idx);
        config_desc->i_configuration = 0;
        config_desc->b_length = sizeof(*config_desc);
        config_desc->bm_attributes = USB_CONFIGURATION_SELF_POWERED | USB_CONFIGURATION_RESERVED_7;
        config_desc->b_max_power = 0;
      }

      for (auto function_index : config.functions) {
        auto& function = GetFunction(function_index);
        size_t descriptors_length;
        auto* descriptors = function.GetDescriptors(&descriptors_length);
        auto old_size = config_desc_bytes.size();
        config_desc_bytes.resize(old_size + descriptors_length);
        memcpy(config_desc_bytes.data() + old_size, descriptors, descriptors_length);
        reinterpret_cast<usb_configuration_descriptor_t*>(config_desc_bytes.data())
            ->b_num_interfaces += function.GetNumInterfaces();
      }
      reinterpret_cast<usb_configuration_descriptor_t*>(config_desc_bytes.data())->w_total_length =
          htole16(config_desc_bytes.size());
      config.config_desc = std::move(config_desc_bytes);
      config_idx++;
    }
  }

  zx_status_t status = StartController();
  if (status != ZX_OK) {
    fdf::error("Failed to start peripheral controller: {}", zx_status_get_string(status));
    fbl::AutoLock lock(&lock_);
    SetStateLocked(DeviceState::kWaitForFunctionBind);
    return status;
  }

  bool send_event = false;
  {
    fbl::AutoLock lock(&lock_);
    cur_usb_mode_ = USB_MODE_PERIPHERAL;
    // The host connection event can be received before we transition to kPeripheralReady.
    // This is not required once we move to a single dispatcher.
    if (state_ == DeviceState::kStarting) {
      SetStateLocked(DeviceState::kPeripheralReady);
    }
    send_event = listener_.is_valid();
  }

  if (send_event) {
    fdf::info("UsbPeripheral: Sending FunctionRegistered event");
    listener_->FunctionRegistered().Then(
        [](fidl::WireUnownedResult<fperipheral::Events::FunctionRegistered>& result) {
          if (!result.ok()) {
            fdf::error("Failed to send FunctionRegistered event: {}", result.status());
          }
        });
  }

  return ZX_OK;
}

zx_status_t UsbPeripheral::FunctionUnregistered() {
  TRACE_DURATION("usb-peripheral", __func__);
  bool do_stop = false;
  {
    fbl::AutoLock lock(&lock_);
    if (state_ == DeviceState::kStopping) {
      fdf::info("Function unregister called in state {}. Already stopping.", state_);
      return ZX_OK;
    }

    if (state_ == DeviceState::kPeripheralReady || state_ == DeviceState::kHostConnected) {
      fdf::info("Function unregister called in state {}. Dropping to kWaitForFunctionBind.",
                state_);
      do_stop = true;
    }
  }

  if (do_stop) {
    zx_status_t status = StopController();
    if (status == ZX_OK) {
      SetState(DeviceState::kWaitForFunctionBind);
    }
    return status;
  }

  return ZX_OK;
}

zx_status_t UsbPeripheral::StartController() {
  TRACE_DURATION("usb-peripheral", __func__);

  {
    fbl::AutoLock lock(&lock_);
    if (cur_usb_mode_ == USB_MODE_PERIPHERAL) {
      fdf::info("Controller already in mode USB_MODE_PERIPHERAL");
      return ZX_OK;
    }
    if (parent_usb_mode_ != USB_MODE_PERIPHERAL) {
      fdf::error("DCI device is not in peripheral mode, cannot start the controller");
      return ZX_ERR_BAD_STATE;
    }

    fdf::info("Starting controller: cur_mode={} new_mode=USB_MODE_PERIPHERAL (state={})",
              usb_mode_to_string(cur_usb_mode_), state_);
  }

  fidl::Arena arena;
  auto result = dci_.buffer(arena)->StartController();

  if (!result.ok()) {
    fdf::error("StartController failed (framework): {}", result.status_string());
    return ZX_ERR_INTERNAL;
  }
  if (result->is_error()) {
    fdf::error("StartController failed (DCI error): {}",
               zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  SetUsbMode(USB_MODE_PERIPHERAL);

  return ZX_OK;
}
zx_status_t UsbPeripheral::StopController() {
  TRACE_DURATION("usb-peripheral", __func__);

  {
    fbl::AutoLock lock(&lock_);
    if (cur_usb_mode_ == USB_MODE_NONE) {
      return ZX_OK;
    }
    fdf::info("Stopping controller: cur_mode={} (state={})", usb_mode_to_string(cur_usb_mode_),
              state_);
  }

  if (dci_.is_valid()) {
    fidl::Arena arena;
    auto result = dci_.buffer(arena)->StopController();

    if (!result.ok()) {
      fdf::error("StopController failed (framework): {}", result.status_string());
      return ZX_ERR_INTERNAL;
    }
    if (result->is_error()) {
      fdf::error("StopController failed (DCI error): {}",
                 zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
  // Note: Banjo DCI doesn't have a dedicated StopController call.
  // Transitioning cur_usb_mode_ to USB_MODE_NONE is sufficient here.

  SetUsbMode(USB_MODE_NONE);

  return ZX_OK;
}

void UsbPeripheral::SetStateLocked(DeviceState state) {
  fdf::info("UsbPeripheral: State transition {} -> {}", state_, state);
  state_ = state;
  dci_inspect_.UpdateState(std::format("{}", state));
}

zx_status_t UsbPeripheral::AllocInterfaceLocked(size_t function_index, uint8_t* out_intf_num) {
  TRACE_DURATION("usb-peripheral", __func__, "function_index", function_index);
  auto& function = GetFunction(function_index);
  ZX_ASSERT(function.configuration() < configurations_.size());
  auto& configuration = configurations_[function.configuration()];
  auto& interface_map = configuration.interface_map;
  for (size_t i = 0; i < std::size(interface_map); i++) {
    if (!interface_map[i].has_value()) {
      interface_map[i] = function_index;
      *out_intf_num = static_cast<uint8_t>(i);
      return ZX_OK;
    }
  }
  fdf::error("Exceeded maximum supported interfaces.");
  return ZX_ERR_NO_RESOURCES;
}

namespace {

fdescriptor::wire::EndpointType EndpointInfoToType(
    const fuchsia_hardware_usb_endpoint::wire::EndpointInfo& ep_info) {
  switch (ep_info.Which()) {
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kControl:
      return fdescriptor::wire::EndpointType::kControl;
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kIsochronous:
      return fdescriptor::wire::EndpointType::kIsochronous;
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kBulk:
      return fdescriptor::wire::EndpointType::kBulk;
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kInterrupt:
      return fdescriptor::wire::EndpointType::kInterrupt;
    default:
      return fdescriptor::wire::EndpointType::kBulk;
  }
}

const char* EndpointTypeToString(fdescriptor::wire::EndpointType type) {
  switch (type) {
    case fdescriptor::wire::EndpointType::kControl:
      return "control";
    case fdescriptor::wire::EndpointType::kIsochronous:
      return "isochronous";
    case fdescriptor::wire::EndpointType::kBulk:
      return "bulk";
    case fdescriptor::wire::EndpointType::kInterrupt:
      return "interrupt";
    default:
      return "unknown";
  }
}

const char* EndpointInfoToString(const fuchsia_hardware_usb_endpoint::wire::EndpointInfo& ep_info) {
  switch (ep_info.Which()) {
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kControl:
      return "control";
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kIsochronous:
      return "isochronous";
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kBulk:
      return "bulk";
    case fuchsia_hardware_usb_endpoint::wire::EndpointInfo::Tag::kInterrupt:
      return "interrupt";
    default:
      return "unknown";
  }
}

}  // namespace

zx_status_t UsbPeripheral::AllocEndpointLocked(
    size_t function_index, fdescriptor::EndpointDirection direction,
    fuchsia_hardware_usb_endpoint::wire::EndpointInfo ep_info, uint32_t max_packet_size,
    uint8_t* out_address) {
  std::string type_str = EndpointInfoToString(ep_info);
  TRACE_DURATION("usb-peripheral", __func__, "function_index", function_index, "direction",
                 fidl::ToUnderlying(direction), "type", type_str, "max_packet_size",
                 max_packet_size);

  if (!has_dci_hardware_info_) {
    return AllocEndpointLegacyLocked(function_index, direction, out_address);
  }

  if (supports_dynamic_ep_sizing_) {
    return AllocEndpointDynamicLocked(function_index, direction, ep_info, max_packet_size,
                                      out_address);
  }

  return AllocEndpointBestFitLocked(function_index, direction, ep_info, max_packet_size,
                                    out_address);
}

zx_status_t UsbPeripheral::AllocEndpointDynamicLocked(
    size_t function_index, fdescriptor::EndpointDirection direction,
    fuchsia_hardware_usb_endpoint::wire::EndpointInfo ep_info, uint32_t max_packet_size,
    uint8_t* out_address) {
  fidl::Arena arena;
  auto alloc_req = fuchsia_hardware_usb_dci::wire::EndpointAllocationRequest::Builder(arena)
                       .direction(direction)
                       .endpoint_type(EndpointInfoToType(ep_info))
                       .max_packet_size(static_cast<uint16_t>(max_packet_size))
                       .Build();

  auto result = dci_->AllocEndpoint(alloc_req);
  if (!result.ok()) {
    fdf::error("AllocEndpoint failed (framework): {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("AllocEndpoint failed (DCI error): {}", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  uint8_t ep_addr = result->value()->ep_address;

  auto cleanup = fit::defer([this, ep_addr]() {
    auto free_result = dci_->FreeEndpoint(ep_addr);
    if (!free_result.ok()) {
      fdf::warn("FreeEndpoint failed during validation cleanup (framework): {}",
                free_result.status_string());
    } else if (free_result->is_error()) {
      fdf::warn("FreeEndpoint failed during validation cleanup (DCI error): {}",
                zx_status_get_string(free_result->error_value()));
    }
  });

  uint8_t index = EpAddressToIndex(ep_addr);
  if (index == 0 || index == 16 || index >= std::size(endpoint_map_)) {
    fdf::error("DCI returned invalid endpoint address {:#x}", ep_addr);
    return ZX_ERR_BAD_STATE;
  }
  if (endpoint_map_[index].has_value()) {
    fdf::error("DCI returned already allocated endpoint address {:#x}", ep_addr);
    return ZX_ERR_ALREADY_BOUND;
  }
  endpoint_map_[index] = function_index;
  *out_address = ep_addr;

  cleanup.cancel();
  return ZX_OK;
}

zx_status_t UsbPeripheral::AllocEndpointBestFitLocked(
    size_t function_index, fdescriptor::EndpointDirection direction,
    fuchsia_hardware_usb_endpoint::wire::EndpointInfo ep_info, uint32_t max_packet_size,
    uint8_t* out_address) {
  std::optional<size_t> best_index;
  uint32_t best_fit_size = UINT32_MAX;
  fdescriptor::wire::EndpointType req_type = EndpointInfoToType(ep_info);

  for (size_t i = 0; i < dci_endpoints_.size(); i++) {
    const auto& ep = dci_endpoints_[i];

    // Check direction.
    bool ep_is_in = (ep.ep_address & 0x80) != 0;
    bool req_is_in = (direction == fdescriptor::EndpointDirection::kIn);
    if (ep_is_in != req_is_in) {
      continue;
    }

    // Check if already allocated.
    uint8_t ep_index = EpAddressToIndex(ep.ep_address);
    if (endpoint_map_[ep_index].has_value()) {
      continue;
    }

    // Check type support and size limit.
    std::optional<uint32_t> ep_max_limit;
    for (const auto& st : ep.supported_types) {
      if (st.endpoint_type == req_type) {
        ep_max_limit = st.max_packet_size_limit;
        break;
      }
    }
    if (!ep_max_limit.has_value() || *ep_max_limit < max_packet_size) {
      continue;
    }

    // Check if it is a better fit.
    if (*ep_max_limit < best_fit_size) {
      best_fit_size = *ep_max_limit;
      best_index = i;
    }
  }

  if (!best_index.has_value()) {
    std::string type_str = EndpointInfoToString(ep_info);
    fdf::error("No suitable endpoint found for type {}, size {}", type_str, max_packet_size);
    return ZX_ERR_NO_RESOURCES;
  }

  const auto& ep = dci_endpoints_[*best_index];
  uint8_t ep_index = EpAddressToIndex(ep.ep_address);
  endpoint_map_[ep_index] = function_index;
  *out_address = ep.ep_address;
  return ZX_OK;
}

zx_status_t UsbPeripheral::AllocEndpointLegacyLocked(size_t function_index,
                                                     fdescriptor::EndpointDirection direction,
                                                     uint8_t* out_address) {
  uint8_t start, end;

  if (direction == fdescriptor::EndpointDirection::kOut) {
    start = kOutEpStart;
    end = kOutEpEnd;
  } else if (direction == fdescriptor::EndpointDirection::kIn) {
    start = kInEpStart;
    end = kInEpEnd;
  } else {
    fdf::error("Invalid direction.");
    return ZX_ERR_INVALID_ARGS;
  }

  for (uint8_t endpoint_index = start; endpoint_index <= end; endpoint_index++) {
    if (!endpoint_map_[endpoint_index].has_value()) {
      endpoint_map_[endpoint_index] = function_index;
      *out_address = EpIndexToAddress(endpoint_index);
      return ZX_OK;
    }
  }

  fdf::error("Exceeded maximum supported endpoints.");
  return ZX_ERR_NO_RESOURCES;
}

void UsbPeripheral::FreeEndpointInternalLocked(uint8_t index, std::string_view context) {
  if (index >= std::size(endpoint_map_)) {
    return;
  }
  // Physical endpoint 0 (index 0 for OUT, index 16 for IN) is the default
  // control endpoint and is managed by the controller, not dynamically allocated.
  if (index == 0 || index == 16) {
    return;
  }
  if (!supports_dynamic_ep_sizing_) {
    return;
  }
  uint8_t ep_addr = EpIndexToAddress(index);
  auto result = dci_->FreeEndpoint(ep_addr);
  if (!result.ok()) {
    fdf::error("FreeEndpoint {:#x} failed in {} (framework): {}", ep_addr, context,
               result.status_string());
  } else if (result->is_error()) {
    fdf::error("FreeEndpoint {:#x} failed in {} (DCI error): {}", ep_addr, context,
               zx_status_get_string(result->error_value()));
  }
}

zx::result<UsbPeripheral::ResourceAllocations> UsbPeripheral::AllocResources(
    size_t function_index, uint8_t interface_count,
    std::span<fuchsia_hardware_usb_function::EndpointResource> endpoints,
    std::span<std::string> strings) {
  TRACE_DURATION("usb-peripheral", __func__, "function_index", function_index);

  ResourceAllocations allocations;
  std::vector<uint8_t> allocated_strings;
  zx_status_t status = ZX_OK;

  auto cleanup = fit::defer([this, function_index, &allocations, &allocated_strings]() {
    fbl::AutoLock lock(&lock_);
    auto& function = GetFunction(function_index);
    ZX_ASSERT(function.configuration() < configurations_.size());
    auto& configuration = configurations_[function.configuration()];
    for (uint8_t intf : allocations.interface_nums) {
      if (intf < std::size(configuration.interface_map) &&
          configuration.interface_map[intf] == function_index) {
        configuration.interface_map[intf].reset();
      }
    }
    for (uint8_t str_idx : allocated_strings) {
      strings_[str_idx - 1].allocated = false;
      strings_[str_idx - 1].text.clear();
      strings_[str_idx - 1].function_index.reset();
    }
    for (uint8_t ep_addr : allocations.endpoint_addrs) {
      FreeEndpointInternalLocked(EpAddressToIndex(ep_addr), "rollback");
      uint8_t index = EpAddressToIndex(ep_addr);
      if (index < std::size(endpoint_map_) && endpoint_map_[index] == function_index) {
        endpoint_map_[index].reset();
      }
    }
  });

  fbl::AutoLock lock(&lock_);

  // Allocate interfaces
  for (uint8_t i = 0; i < interface_count; i++) {
    uint8_t intf_num;
    status = AllocInterfaceLocked(function_index, &intf_num);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    allocations.interface_nums.push_back(intf_num);
  }

  // Allocate strings
  for (const std::string& str : strings) {
    uint8_t str_idx;
    status = AllocStringDescLocked(function_index, str, &str_idx);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    allocations.string_indices.push_back(str_idx);
    allocated_strings.push_back(str_idx);
  }

  // Allocate endpoints
  for (const auto& ep : endpoints) {
    uint8_t ep_addr;
    fidl::Arena arena;
    auto wire_ep_info = fidl::ToWire(arena, ep.ep_info());
    status = AllocEndpointLocked(function_index, ep.direction(), wire_ep_info, ep.max_packet_size(),
                                 &ep_addr);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    allocations.endpoint_addrs.push_back(ep_addr);
  }

  // If all allocations succeeded, connect endpoints.
  for (size_t i = 0; i < endpoints.size(); i++) {
    if (!endpoints[i].endpoint().is_valid()) {
      continue;
    }
    status = ConnectToEndpoint(allocations.endpoint_addrs[i], std::move(endpoints[i].endpoint()));
    if (status != ZX_OK) {
      return zx::error(status);
    }
  }

  cleanup.cancel();
  return zx::ok(std::move(allocations));
}

zx_status_t UsbPeripheral::InitializeDci(
    fidl::ClientEnd<fuchsia_hardware_usb_dci::UsbDci> dci_client_end) {
  dci_ = fidl::WireSyncClient<fuchsia_hardware_usb_dci::UsbDci>(std::move(dci_client_end));

  auto result = dci_->GetHardwareInfo();
  if (!result.ok()) {
    if (result.status() == ZX_ERR_NOT_SUPPORTED) {
      fdf::info("DCI driver does not support GetHardwareInfo, falling back to legacy mode");
      return ZX_OK;
    }
    fdf::error("GetHardwareInfo failed (framework): {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    if (result->error_value() == ZX_ERR_NOT_SUPPORTED) {
      fdf::info("DCI driver does not support GetHardwareInfo, falling back to legacy mode");
      return ZX_OK;
    }
    fdf::error("GetHardwareInfo failed (DCI error): {}",
               zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  const auto& info = result->value()->info;
  has_dci_hardware_info_ = true;

  hardware_info_node_ = usb_peripheral_node_.CreateChild("hardware_info");
  if (info.has_supports_dynamic_ep_sizing()) {
    supports_dynamic_ep_sizing_ = info.supports_dynamic_ep_sizing();
    supports_dynamic_ep_sizing_property_ =
        hardware_info_node_.CreateBool("supports_dynamic_ep_sizing", supports_dynamic_ep_sizing_);
  }

  if (!info.has_endpoints() || info.endpoints().empty()) {
    return ZX_OK;
  }

  for (const auto& ep : info.endpoints()) {
    DciEndpointInfo local_ep;
    if (!ep.has_ep_address()) {
      fdf::error("DCI endpoint info missing address, ignoring");
      continue;
    }
    uint8_t ep_addr = ep.ep_address();
    uint8_t ep_index = EpAddressToIndex(ep_addr);
    if (ep_index == 0 || ep_index == 16 || ep_index >= USB_MAX_EPS) {
      fdf::error("DCI reported invalid endpoint address {:#x}, ignoring", ep_addr);
      continue;
    }
    local_ep.ep_address = ep_addr;
    uint32_t max_packet_limit_overall = 0;
    std::string types_str = "";
    if (ep.has_supported_types()) {
      for (const auto& type_info : ep.supported_types()) {
        if (!type_info.has_endpoint_type() || !type_info.has_max_packet_size_limit()) {
          continue;
        }
        DciSupportedEndpointInfo st{
            .endpoint_type = type_info.endpoint_type(),
            .max_packet_size_limit = type_info.max_packet_size_limit(),
            .min_lead_time = type_info.has_min_lead_time() ? type_info.min_lead_time() : 0,
        };
        local_ep.supported_types.push_back(st);
        if (st.max_packet_size_limit > max_packet_limit_overall) {
          max_packet_limit_overall = st.max_packet_size_limit;
        }
        if (!types_str.empty()) {
          types_str += ", ";
        }
        types_str += EndpointTypeToString(st.endpoint_type);
      }
    }
    dci_endpoints_.push_back(local_ep);

    // Add to inspect
    std::string ep_name = std::format("ep_{:#04x}", ep_addr);
    auto ep_node = hardware_info_node_.CreateChild(ep_name);
    auto max_limit_prop = ep_node.CreateUint("max_packet_size_limit", max_packet_limit_overall);
    auto types_prop = ep_node.CreateString("supported_types", types_str);

    inspect_endpoints_.push_back(InspectEndpoint{
        .node = std::move(ep_node),
        .max_packet_size_limit = std::move(max_limit_prop),
        .supported_types = std::move(types_prop),
    });
  }

  return ZX_OK;
}

bool UsbPeripheral::ValidateEndpoint(size_t function_index, uint8_t ep_address) const {
  uint8_t index = EpAddressToIndex(ep_address);
  return index != 0 && index < std::size(endpoint_map_) && endpoint_map_[index] == function_index;
}

zx_status_t UsbPeripheral::GetDescriptor(uint8_t request_type, uint16_t value, uint16_t index,
                                         void* buffer, size_t length, size_t* out_actual) {
  TRACE_DURATION("usb-peripheral", __func__, "request_type", request_type, "value", value, "index",
                 index);
  uint8_t type = request_type & USB_TYPE_MASK;

  if (type != USB_TYPE_STANDARD) {
    fdf::debug("Unsupported request type: {}", request_type);
    return ZX_ERR_NOT_SUPPORTED;
  }

  fbl::AutoLock lock(&lock_);

  auto desc_type = static_cast<uint8_t>(value >> 8);
  if (desc_type == USB_DT_DEVICE && index == 0) {
    if (device_desc_.b_length == 0) {
      fdf::error("Device descriptor not set");
      return ZX_ERR_INTERNAL;
    }
    length = std::min(length, sizeof(device_desc_));
    memcpy(buffer, &device_desc_, length);
    *out_actual = length;
    return ZX_OK;
  } else if ((desc_type == USB_DT_CONFIG || desc_type == USB_DT_OTHER_SPEED_CONFIG) && index == 0) {
    index = value & 0xff;
    if (index >= configurations_.size()) {
      fdf::error("Invalid configuration index: {}", index);
      return ZX_ERR_INVALID_ARGS;
    }
    auto& config_desc = configurations_[index].config_desc;
    if (config_desc.size() == 0) {
      fdf::error("Configuration descriptor not set");
      return ZX_ERR_INTERNAL;
    }
    auto desc_length = config_desc.size();
    length = std::min(length, desc_length);
    memcpy(buffer, config_desc.data(), length);
    if (desc_type == USB_DT_OTHER_SPEED_CONFIG && length >= 2) {
      static_cast<uint8_t*>(buffer)[1] = USB_DT_OTHER_SPEED_CONFIG;
    }
    *out_actual = length;
    return ZX_OK;
  } else if (desc_type == USB_DT_STRING) {
    uint8_t desc[255];
    auto* header = reinterpret_cast<usb_descriptor_header_t*>(desc);
    header->b_descriptor_type = USB_DT_STRING;

    auto string_index = static_cast<uint8_t>(value & 0xFF);
    if (string_index == 0) {
      // special case - return language list
      header->b_length = 4;
      desc[2] = 0x09;  // language ID
      desc[3] = 0x04;
    } else {
      // String indices are 1-based.
      string_index--;
      if (string_index >= strings_.size()) {
        fdf::error("UsbPeripheral::GetDescriptor: Invalid string index: {} strings_.size()={}",
                   string_index, strings_.size());
        return ZX_ERR_INVALID_ARGS;
      }
      const char* string = strings_[string_index].text.c_str();
      unsigned index = 2;

      // convert ASCII to UTF16
      if (string) {
        while (*string && index < sizeof(desc) - 2) {
          desc[index++] = *string++;
          desc[index++] = 0;
        }
      }
      header->b_length = static_cast<uint8_t>(index);
    }

    length = std::min<size_t>(header->b_length, length);
    memcpy(buffer, desc, length);
    *out_actual = length;
    return ZX_OK;
  } else if (desc_type == USB_DT_DEVICE_QUALIFIER) {
    if (device_desc_.b_length == 0) {
      fdf::error("Device descriptor not set");
      return ZX_ERR_INTERNAL;
    }
    usb_device_qualifier_descriptor_t qualifier = {};
    // The first 8 bytes of usb_device_descriptor_t and usb_device_qualifier_descriptor_t match.
    // Bytes 9 and 10 (b_num_configurations and b_reserved) are set explicitly below.
    memcpy(&qualifier, &device_desc_, 8);
    qualifier.b_length = sizeof(usb_device_qualifier_descriptor_t);
    qualifier.b_descriptor_type = USB_DT_DEVICE_QUALIFIER;
    // TODO(b/459580056): Replace the following WAR with a correct solution.
    qualifier.b_num_configurations = device_desc_.b_num_configurations;
    qualifier.b_reserved = 0;

    length = std::min(length, sizeof(usb_device_qualifier_descriptor_t));
    memcpy(buffer, &qualifier, length);
    *out_actual = length;
    return ZX_OK;
  } else if (desc_type == USB_DT_BOS && (value & 0xFF) == 0 && index == 0) {
    // USB 2.0 devices (bcdUSB < USB_2_0_1) without BOS capabilities must stall GET_DESCRIPTOR(BOS).
    if (le16toh(device_desc_.bcd_usb) < USB_2_0_1) {
      fdf::debug("BOS descriptor unsupported for bcd_usb < {:#x}: {:#x}", USB_2_0_1,
                 le16toh(device_desc_.bcd_usb));
      return ZX_ERR_NOT_SUPPORTED;
    }
    usb_bos_descriptor_t bos{
        .b_length = sizeof(usb_bos_descriptor_t),
        .b_descriptor_type = USB_DT_BOS,
        .w_total_length = htole16(sizeof(usb_bos_descriptor_t)),
        .b_num_device_caps = 0,  // No device capabilities.
    };
    length = std::min(length, sizeof(usb_bos_descriptor_t));
    memcpy(buffer, &bos, length);
    *out_actual = length;
    return ZX_OK;
  }

  fdf::debug("Unsupported value: {:#x} index: {}", value, index);
  return ZX_ERR_NOT_SUPPORTED;
}

// This is called by the DCI driver (via a SETUP request) when the USB host selects a configuration
// (e.g. SET_CONFIGURATION).
void UsbPeripheral::SetConfiguration(uint8_t configuration,
                                     fit::callback<void(zx_status_t)> completer) {
  TRACE_DURATION("usb-peripheral", __func__, "configuration", configuration);
  if (configuration > configurations_.size()) {
    fdf::error("Invalid configuration value: {} (max {})", configuration, configurations_.size());
    completer(ZX_ERR_INVALID_ARGS);
    return;
  }
  bool configured = configuration > 0;
  // TODO(b/355271738): Logs added to debug b/355271738. Remove when fixed.
  fdf::info("Configuration {}", configuration);

  std::vector<std::shared_ptr<UsbFunction>> functions_to_configure;

  fit::callback<void(zx_status_t)> canceled_completer;
  bool deferred = false;
  {
    fbl::AutoLock lock(&lock_);
    if (state_ == DeviceState::kStopping || clearing_functions_ || stopping_driver_) {
      fdf::warn("SetConfiguration({}) rejected: peripheral is stopping or clearing functions.",
                configuration);
      completer(ZX_ERR_BAD_STATE);
      return;
    }
    if (in_flight_disconnect_unconfigures_ > 0) {
      fdf::info("SetConfiguration({}) deferred: disconnect unconfigure in flight.", configuration);
      if (pending_set_configuration_.has_value()) {
        canceled_completer = std::move(pending_set_configuration_->completer);
      }
      pending_set_configuration_ = PendingSetConfiguration{
          .configuration = configuration,
          .completer = std::move(completer),
      };
      deferred = true;
    } else {
      // Call SetConfigured for all functions, waiting for the completion in
      // parallel for all of them.
      //
      // If any function fails to configure, we report the first error we see
      // back to the caller via `completer`.
      for (const auto& config : configurations_) {
        for (auto function_index : config.functions) {
          if (function_index >= functions_.size()) {
            fdf::error("Function index {} out of bounds (size {})", function_index,
                       functions_.size());
            completer(ZX_ERR_INVALID_ARGS);
            return;
          }
          functions_to_configure.push_back(functions_[function_index]);
        }
      }
    }
  }

  if (canceled_completer) {
    canceled_completer(ZX_ERR_CANCELED);
  }
  if (deferred) {
    return;
  }

  // Call SetConfigured for all functions in parallel and outside the lock.
  std::vector<fpromise::promise<void, zx_status_t>> promises;
  for (auto& function : functions_to_configure) {
    fpromise::bridge<void, zx_status_t> bridge;
    bool config_match = (function->configuration() == (configuration - 1));
    function->SetConfigured(
        config_match, speed_,
        [completer = std::move(bridge.completer), configured](zx_status_t status) mutable {
          if (status == ZX_OK || !configured) {
            // Ignore errors when unconfiguring.
            completer.complete_ok();
          } else {
            completer.complete_error(status);
          }
        });
    promises.push_back(bridge.consumer.promise_or(fpromise::error(ZX_ERR_CANCELED)));
  }

  auto join_task =
      fpromise::join_promise_vector(std::move(promises))
          .then([this, configuration, completer = std::move(completer)](
                    fpromise::result<std::vector<fpromise::result<void, zx_status_t>>>&
                        results) mutable {
            zx_status_t final_status = ZX_OK;
            if (results.is_ok()) {
              for (auto& res : results.value()) {
                if (res.is_error()) {
                  final_status = res.error();
                  fdf::error("Failed to set interface: {}", zx_status_get_string(final_status));
                  break;
                }
              }
            } else {
              final_status = ZX_ERR_CANCELED;
            }

            if (final_status == ZX_OK) {
              std::vector<uint8_t> eps_to_clear;
              {
                fbl::AutoLock lock(&lock_);
                configuration_ = configuration;

                // USB 2.0 § 9.4.7: SetConfiguration resets the halt status and data toggle of each
                // endpoint to not halted. Collect both active function endpoints and previously
                // stalled endpoints to ensure hardware state is cleared.
                for (uint8_t ep : stalled_eps_) {
                  eps_to_clear.push_back(ep);
                }
                stalled_eps_.clear();

                for (size_t ep_idx = 1; ep_idx < std::size(endpoint_map_); ++ep_idx) {
                  if (endpoint_map_[ep_idx].has_value()) {
                    uint8_t ep_addr = EpIndexToAddress(static_cast<uint8_t>(ep_idx));
                    if (std::find(eps_to_clear.begin(), eps_to_clear.end(), ep_addr) ==
                        eps_to_clear.end()) {
                      eps_to_clear.push_back(ep_addr);
                    }
                  }
                }

                // USB 2.0 § 9.4.7: SetConfiguration also selects default alternate setting 0 for
                // each interface.
                if (configuration_ > 0 && configuration_ <= configurations_.size()) {
                  std::fill(std::begin(configurations_[configuration_ - 1].alternate_setting),
                            std::end(configurations_[configuration_ - 1].alternate_setting), 0);
                }
              }
              for (uint8_t ep_addr : eps_to_clear) {
                UsbDciEndpointClearStall(ep_addr);
              }
            }
            completer(final_status);
          });

  executor_->schedule_task(std::move(join_task).wrap_with(scope_));
}

void UsbPeripheral::SetInterface(uint8_t interface, uint8_t alt_setting,
                                 fit::callback<void(zx_status_t)> completer) {
  TRACE_DURATION("usb-peripheral", __func__, "interface", interface, "alt_setting", alt_setting);

  std::shared_ptr<UsbFunction> function;
  uint8_t target_config = 0;
  {
    fbl::AutoLock lock(&lock_);
    if (stopping_driver_ || state_ == DeviceState::kStopping) {
      fdf::warn("SetInterface called while peripheral is stopping (state={})", state_);
      completer(ZX_ERR_BAD_STATE);
      return;
    }
    if (configuration_ == 0) {
      fdf::error("SetInterface called before device is configured");
      completer(ZX_ERR_BAD_STATE);
      return;
    }
    if (configuration_ > configurations_.size()) {
      fdf::error("SetInterface: invalid configuration_ {}", configuration_);
      completer(ZX_ERR_BAD_STATE);
      return;
    }
    target_config = configuration_;
    const auto& configuration = configurations_[configuration_ - 1];
    if (interface >= std::size(configuration.interface_map)) {
      fdf::error("Invalid interface index: {}", interface);
      completer(ZX_ERR_OUT_OF_RANGE);
      return;
    }

    auto function_index = configuration.interface_map[interface];
    if (function_index.has_value()) {
      if (function_index.value() >= functions_.size()) {
        fdf::error("SetInterface: function_index {} out of bounds", function_index.value());
        completer(ZX_ERR_BAD_STATE);
        return;
      }
      function = functions_[function_index.value()];
    }
  }

  if (function) {
    auto func_idx = function->function_index();
    fpromise::bridge<zx_status_t, void> bridge;
    function->SetInterface(interface, alt_setting,
                           [completer = std::move(bridge.completer)](zx_status_t status) mutable {
                             completer.complete_ok(status);
                           });

    auto task =
        bridge.consumer.promise_or(fpromise::ok(ZX_ERR_CANCELED))
            .then([this, function, interface, alt_setting, target_config, func_idx,
                   completer =
                       std::move(completer)](fpromise::result<zx_status_t, void>& result) mutable {
              zx_status_t status = result.is_ok() ? result.value() : ZX_ERR_CANCELED;
              if (status == ZX_OK) {
                // USB 2.0 § 9.4.5: A SetInterface request for an interface resets the halt
                // status and data toggle sequence to 0 for each endpoint associated with that
                // interface. Query descriptor endpoints outside lock_ to avoid holding lock_
                // during descriptor traversal.
                std::optional<std::vector<uint8_t>> iface_eps =
                    function->GetEndpointsForInterface(interface, alt_setting);

                std::vector<uint8_t> eps_to_clear;
                {
                  fbl::AutoLock lock(&lock_);
                  if (configuration_ == target_config && configuration_ > 0 &&
                      configuration_ <= configurations_.size()) {
                    configurations_[configuration_ - 1].alternate_setting[interface] = alt_setting;

                    if (iface_eps.has_value()) {
                      // Descriptors are present. Unconditionally clear stalls and reset data
                      // toggles for all endpoints associated with this specific interface and
                      // alternate setting.
                      eps_to_clear = std::move(*iface_eps);
                    } else {
                      // Fallback ONLY if descriptors are genuinely missing (e.g. mock test
                      // functions). Start from ep_idx = 1 since Endpoint 0 (control pipe) is never
                      // assigned to functions.
                      for (size_t ep_idx = 1; ep_idx < std::size(endpoint_map_); ++ep_idx) {
                        if (endpoint_map_[ep_idx] == func_idx) {
                          uint8_t ep_addr = EpIndexToAddress(static_cast<uint8_t>(ep_idx));
                          eps_to_clear.push_back(ep_addr);
                        }
                      }
                    }
                  }
                }
                // Note: UsbDciEndpointClearStall explicitly acquires lock_ and removes each
                // ep_addr from stalled_eps_, maintaining driver protocol state tracking as well as
                // hardware state.
                for (uint8_t ep_addr : eps_to_clear) {
                  UsbDciEndpointClearStall(ep_addr);
                }
              }
              completer(status);
            });

    executor_->schedule_task(std::move(task).wrap_with(scope_));
    return;
  }

  fdf::error("Function does not exist");
  completer(ZX_ERR_NOT_SUPPORTED);
}

zx::result<size_t> UsbPeripheral::AddFunction(UsbConfiguration& config, FunctionDescriptor desc) {
  TRACE_DURATION("usb-peripheral", __func__);
  fbl::AutoLock lock(&lock_);
  ZX_ASSERT(state_ == DeviceState::kNoConfiguration);

  auto function_index = functions_.size();
  auto* func_ptr =
      new UsbFunction(function_index, this, desc, config.index, config_generation_, dispatcher());
  auto deleter = [dispatcher = dispatcher()](UsbFunction* ptr) {
    fdf_dispatcher_t* current = fdf_dispatcher_get_current_dispatcher();
    bool on_dispatcher = (current && fdf_dispatcher_get_async_dispatcher(current) == dispatcher) ||
                         (!current && async_get_default_dispatcher() == dispatcher);
    if (on_dispatcher) {
      delete ptr;
    } else {
      auto task_ptr = std::unique_ptr<UsbFunction>(ptr);
      if (zx_status_t status = async::PostTask(
              dispatcher, [task_ptr = std::move(task_ptr)]() mutable { task_ptr.reset(); });
          status != ZX_OK) {
        // If PostTask fails, the callback closure is dropped and destroyed, which deletes the
        // captured task_ptr on the current thread as fallback.
      }
    }
  };
  std::shared_ptr<UsbFunction> function(func_ptr, std::move(deleter));
  functions_.emplace_back(std::move(function));

  config.functions.push_back(function_index);
  return zx::ok(function_index);
}

void UsbPeripheral::ClearFunctions(std::optional<fit::callback<void()>> callback) {
  TRACE_DURATION("usb-peripheral", __func__);
  fdf::debug("{}", __func__);

  std::vector<std::shared_ptr<UsbFunction>> to_teardown;
  bool already_stopping = false;
  fit::callback<void(zx_status_t)> canceled_completer;
  {
    fbl::AutoLock lock(&lock_);
    stalled_eps_.clear();
    if (state_ == DeviceState::kStopping) {
      fdf::info("Already in process of clearing the functions (state=kStopping)");
      already_stopping = true;
    } else {
      fdf::info("UsbPeripheral::ClearFunctions: starting teardown (state from {} to kStopping)",
                state_);
      SetStateLocked(DeviceState::kStopping);
      clearing_functions_ = true;
      if (pending_set_configuration_.has_value()) {
        canceled_completer = std::move(pending_set_configuration_->completer);
        pending_set_configuration_.reset();
      }
      // Prepare for function removal.
      for (auto& function : functions_) {
        if (function) {
          to_teardown.push_back(function);
          fdf::info("UsbPeripheral::ClearFunctions: tearing down function {}",
                    function->function_index());
        }
      }
      configuration_ = 0;
    }
  }

  if (canceled_completer) {
    canceled_completer(ZX_ERR_CANCELED);
  }

  if (already_stopping) {
    if (callback) {
      fbl::AutoLock lock(&lock_);
      on_all_functions_cleared_.push_back(UnlockedCallback(std::move(*callback), lock_));
    }
    CheckAllFunctionsCleared();
    return;
  }

  if (to_teardown.empty()) {
    fdf::info("No functions to teardown. Stopping controller immediately.");
    CompleteFunctionsTeardown(std::move(to_teardown), std::move(callback));
    return;
  }

  // Cooperative teardown: query each function to complete its logical teardown asynchronously.
  std::vector<fpromise::promise<void, zx_status_t>> promises;
  for (auto& function : to_teardown) {
    fpromise::bridge<void, zx_status_t> bridge;
    function->SetConfigured(
        false, USB_SPEED_UNDEFINED,
        [completer = std::move(bridge.completer),
         name = function->name()](zx_status_t status) mutable {
          if (status != ZX_OK) {
            fdf::error("SetConfigured(false) during teardown failed for function {}: {}", name,
                       zx_status_get_string(status));
          }
          completer.complete_ok();
        });
    promises.push_back(bridge.consumer.promise_or(fpromise::error(ZX_ERR_CANCELED)));
  }

  auto join_task = fpromise::join_promise_vector(std::move(promises))
                       .then([this, to_teardown = std::move(to_teardown), cb = std::move(callback)](
                                 fpromise::result<std::vector<fpromise::result<void, zx_status_t>>>&
                                     results) mutable {
                         fdf::info("All functions logical teardown complete. Stopping controller.");
                         CompleteFunctionsTeardown(std::move(to_teardown), std::move(cb));
                       });

  executor_->schedule_task(std::move(join_task).wrap_with(scope_));
}

void UsbPeripheral::CompleteFunctionsTeardown(std::vector<std::shared_ptr<UsbFunction>> to_teardown,
                                              std::optional<fit::callback<void()>> callback) {
  zx_status_t status = StopController();
  if (status != ZX_OK) {
    fdf::error("Failed to stop controller during teardown: {}", zx_status_get_string(status));
  }

  std::vector<UsbConfiguration> old_configurations;
  std::vector<StringDescriptor> old_strings;
  std::vector<uint8_t> endpoints_to_cancel;
  fit::callback<void(zx_status_t)> canceled_completer;
  {
    fbl::AutoLock lock(&lock_);
    if (pending_set_configuration_.has_value()) {
      canceled_completer = std::move(pending_set_configuration_->completer);
      pending_set_configuration_.reset();
    }
    old_configurations = std::move(configurations_);
    configurations_.clear();
    for (size_t i = 0; i < std::size(endpoint_map_); i++) {
      if (endpoint_map_[i].has_value() && i != 0 && i != 16) {
        endpoints_to_cancel.push_back(EpIndexToAddress(static_cast<uint8_t>(i)));
      }
      endpoint_map_[i].reset();
    }
    old_strings = std::move(strings_);
    strings_.clear();
  }

  if (canceled_completer) {
    canceled_completer(ZX_ERR_CANCELED);
  }

  // Cancel requests on all endpoints that were allocated to functions.
  // EP0 is managed directly by the controller driver and is reset in StopController().
  // We avoid sweeping unallocated endpoints because controller drivers like dwc3 return
  // ZX_ERR_INVALID_ARGS when CancelAll is called on endpoints that are not allocated.
  for (uint8_t ep_address : endpoints_to_cancel) {
    UsbDciCancelAll(ep_address);
  }

  // Register callback before requesting removal to ensure that even if
  // RequestRemoval() runs synchronously, callback is invoked during teardown.
  if (callback) {
    fbl::AutoLock lock(&lock_);
    on_all_functions_cleared_.push_back(UnlockedCallback(std::move(*callback), lock_));
  }

  // Request removal (unlocked).
  for (auto& function : to_teardown) {
    fdf::info("UsbPeripheral: Requesting removal for function index {}",
              function->function_index());
    function->RequestRemoval();
  }

  // If no functions remain (e.g. to_teardown was empty or all functions were already unbound),
  // trigger completion immediately.
  {
    fbl::AutoLock lock(&lock_);
    clearing_functions_ = false;
    if (!functions_.empty()) {
      return;
    }
  }
  CheckAllFunctionsCleared();
}

void UsbPeripheral::CheckAllFunctionsCleared() {
  std::vector<UnlockedCallback> callbacks;
  bool send_event = false;
  {
    fbl::AutoLock lock(&lock_);
    if (!functions_.empty() || clearing_functions_) {
      return;
    }
    config_generation_++;  // Increment configuration generation fence.
    if (!stopping_driver_) {
      // If the device was put in kWaitForFunctionBind due to an unsolicited function unbind,
      // it normally remains in kWaitForFunctionBind waiting for functions to re-bind.
      // However, if ClearFunctions() was explicitly invoked (indicated by pending completion
      // callbacks in on_all_functions_cleared_), the caller intended to reset the configuration,
      // so we must transition to kNoConfiguration to allow subsequent SetConfiguration calls.
      if (state_ != DeviceState::kWaitForFunctionBind || !on_all_functions_cleared_.empty()) {
        SetStateLocked(DeviceState::kNoConfiguration);
      }
    }
    if (listener_.is_valid()) {
      send_event = true;
    }
    callbacks = std::move(on_all_functions_cleared_);
    on_all_functions_cleared_.clear();
  }

  if (send_event) {
    fdf::info("UsbPeripheral: Sending FunctionsCleared event");
    if (fidl::Status status = listener_->FunctionsCleared(); !status.ok()) {
      fdf::error("Failed to send FunctionsCleared request: {}", status.status_string());
    }
  }

  for (auto& callback : callbacks) {
    callback();
  }
}

void UsbPeripheral::FunctionCleared(size_t function_index, uint64_t config_generation) {
  bool do_stop = false;
  std::shared_ptr<UsbFunction> deferred_destroy;

  {
    fbl::AutoLock lock(&lock_);

    fdf::info("UsbPeripheral: FunctionCleared called for index {} (generation {}).", function_index,
              config_generation);

    if (config_generation < config_generation_) {
      fdf::info(
          "UsbPeripheral: Discarding stale unbind callback from old config generation {} (current {})",
          config_generation, config_generation_);
      return;
    }

    if (state_ != DeviceState::kStopping) {
      if (state_ == DeviceState::kPeripheralReady || state_ == DeviceState::kHostConnected) {
        fdf::info(
            "UsbPeripheral: Function {} removed! Taking peripheral offline"
            " (state {} -> kWaitForFunctionBind)",
            function_index, state_);
        do_stop = true;
      }
    }

    auto it = std::find_if(functions_.begin(), functions_.end(),
                           [function_index](const std::shared_ptr<UsbFunction>& func) {
                             return func->function_index() == function_index;
                           });
    if (it != functions_.end()) {
      fdf::info("UsbPeripheral: Removing function object for index {} from active list.",
                function_index);
      deferred_destroy = std::move(*it);
      functions_.erase(it);
      ReleaseResourcesLocked(function_index);
    }
  }

  if (do_stop) {
    if (zx_status_t status = StopController(); status != ZX_OK) {
      fdf::error("Failed to stop controller: {}", zx_status_get_string(status));
    }
    fbl::AutoLock lock(&lock_);
    SetStateLocked(DeviceState::kWaitForFunctionBind);
  }

  CheckAllFunctionsCleared();
}

zx_status_t UsbPeripheral::AddFunctionDevices() {
  TRACE_DURATION("usb-peripheral", __func__);
  fdf::debug("{}", __func__);
  for (const auto& configuration : configurations_) {
    for (auto function_index : configuration.functions) {
      auto& function = GetFunction(function_index);
      zx::result result = function.AddChild(child_.node_, incoming_, outgoing());
      if (result.is_ok()) {
        IncrementActiveFunctionsLocked();
      } else if (result.status_value() != ZX_ERR_ALREADY_BOUND) {
        fdf::error("Failed to add child {}: {}; Continuing on to next.", function.name(), result);
      }
    }
  }

  return ZX_OK;
}

void UsbPeripheral::CommonControl(const fdescriptor::wire::UsbSetup& setup,
                                  cpp20::span<uint8_t> write_buffer,
                                  fit::callback<void(zx::result<std::vector<uint8_t>>)> completer) {
  uint8_t request_type = setup.bm_request_type;
  uint8_t direction = request_type & USB_DIR_MASK;
  uint8_t request = setup.b_request;
  uint16_t value = le16toh(setup.w_value);
  uint16_t index = le16toh(setup.w_index);
  uint16_t length = le16toh(setup.w_length);

  TRACE_DURATION("usb-peripheral", __func__, "request_type", request_type, "value", value, "index",
                 index);

  if (direction == USB_DIR_OUT && length > write_buffer.size()) {
    fdf::warn("CommonControl: write buffer too small (length: {}, buffer size: {})", length,
              write_buffer.size());
    completer(zx::error(ZX_ERR_BUFFER_TOO_SMALL));
    return;
  }
  if (write_buffer.size() > 0 && write_buffer.data() == nullptr) {
    fdf::error("CommonControl: write buffer data is null but size is {}", write_buffer.size());
    completer(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  fdf::debug("usb_dev_control type={:#02X}, req={}, value={}, index={}, length={}", request_type,
             request, value, index, length);

  switch (request_type & USB_RECIP_MASK) {
    case USB_RECIP_DEVICE: {
      // handle standard device requests
      if ((request_type & (USB_DIR_MASK | USB_TYPE_MASK)) == (USB_DIR_IN | USB_TYPE_STANDARD) &&
          request == USB_REQ_GET_DESCRIPTOR) {
        std::vector<uint8_t> read_data_vec(length);
        size_t out_read_actual = 0;
        zx_status_t status = GetDescriptor(request_type, value, index, read_data_vec.data(), length,
                                           &out_read_actual);
        if (status == ZX_OK) {
          read_data_vec.resize(out_read_actual);
          completer(zx::ok(std::move(read_data_vec)));
        } else {
          completer(zx::error(status));
        }
        return;
      }
      if (request_type == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE) &&
          request == USB_REQ_SET_CONFIGURATION && length == 0) {
        SetConfiguration(static_cast<uint8_t>(value),
                         [completer = std::move(completer)](zx_status_t status) mutable {
                           if (status == ZX_OK) {
                             completer(zx::ok(std::vector<uint8_t>()));
                           } else {
                             completer(zx::error(status));
                           }
                         });
        return;
      }
      if (request_type == (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE) &&
          request == USB_REQ_GET_CONFIGURATION && length > 0) {
        uint8_t cur_config = 0;
        {
          fbl::AutoLock lock(&lock_);
          cur_config = configuration_;
        }
        completer(zx::ok(std::vector<uint8_t>{cur_config}));
        return;
      }
      // Per USB 2.0 Spec Section 9.4.5 / USB 3.0 Spec Section 9.4.5, GET_STATUS to USB_RECIP_DEVICE
      // returns a 16-bit status word containing two feature flags:
      //   Bit 0: Self Powered (1 = self-powered, 0 = bus-powered)
      //   Bit 1: Remote Wakeup (1 = remote wakeup enabled, 0 = disabled)
      // All other bits are reserved and must be zero.
      // TODO(https://fxbug.dev/533013195): Add an API to get the current config (power and remote
      // wakeup) from the system or active peripheral configuration, and update these status flags
      // dynamically rather than returning static feature bits.
      if (request_type == (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE) &&
          request == USB_REQ_GET_STATUS && length == 2) {
        std::vector<uint8_t> read_data_vec(length, 0);
        read_data_vec[0] = 1 << USB_DEVICE_SELF_POWERED;
        completer(zx::ok(std::move(read_data_vec)));
        return;
      }
      // Delegate to one of the function drivers.
      // USB_RECIP_DEVICE should only be used when there is a single active interface.
      // But just to be conservative, try all the available interfaces.
      std::vector<std::shared_ptr<UsbFunction>> funcs_to_call;
      zx_status_t dev_error = ZX_OK;
      {
        fbl::AutoLock lock(&lock_);
        if (configuration_ == 0) {
          dev_error = ZX_ERR_BAD_STATE;
        } else if (configuration_ > configurations_.size()) {
          fdf::error("CommonControl: invalid configuration_ {}", configuration_);
          dev_error = ZX_ERR_BAD_STATE;
        } else {
          const auto& configuration = configurations_[configuration_ - 1];
          const auto& interface_map = configuration.interface_map;

          for (auto function_index : interface_map) {
            if (function_index.has_value()) {
              if (function_index.value() < functions_.size()) {
                funcs_to_call.push_back(functions_[function_index.value()]);
              }
            }
          }
        }
      }
      if (dev_error != ZX_OK) {
        completer(zx::error(dev_error));
        return;
      }

      for (auto& function : funcs_to_call) {
        auto result = function->Control(setup, write_buffer);
        if (result.is_ok()) {
          completer(std::move(result));
          return;
        }
      }

      // Exhausted all interfaces, no one handled it.
      fdf::debug(
          "CommonControl: USB_RECIP_DEVICE request {:#02X} (req: {:#02X}) not handled by any function",
          request_type, request);
      completer(zx::error(ZX_ERR_NOT_SUPPORTED));
      return;
    }
    case USB_RECIP_INTERFACE: {
      uint8_t interface_num = static_cast<uint8_t>(index & 0xFF);
      if (request_type == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE) &&
          request == USB_REQ_SET_INTERFACE && length == 0) {
        SetInterface(interface_num, static_cast<uint8_t>(value),
                     [completer = std::move(completer)](zx_status_t status) mutable {
                       if (status == ZX_OK) {
                         completer(zx::ok(std::vector<uint8_t>()));
                       } else {
                         completer(zx::error(status));
                       }
                     });
        return;
      }

      std::shared_ptr<UsbFunction> function;
      uint8_t alt_setting = 0;
      zx_status_t error_status = ZX_OK;
      {
        fbl::AutoLock lock(&lock_);
        if (configuration_ == 0) {
          fdf::error("Control request received for interface before configuration");
          error_status = ZX_ERR_BAD_STATE;
        } else if (configuration_ > configurations_.size()) {
          fdf::error("CommonControl: invalid configuration_ {}", configuration_);
          error_status = ZX_ERR_BAD_STATE;
        } else {
          const auto& configuration = configurations_[configuration_ - 1];
          const auto& interface_map = configuration.interface_map;
          if (interface_num >= std::size(interface_map) ||
              !interface_map[interface_num].has_value()) {
            fdf::warn(
                "CommonControl: USB_RECIP_INTERFACE index {} (interface {}) out of range or unassigned (max {})",
                index, interface_num, std::size(interface_map));
            error_status = ZX_ERR_OUT_OF_RANGE;
          } else {
            alt_setting = configuration.alternate_setting[interface_num];

            auto function_index = interface_map[interface_num];
            if (function_index.has_value()) {
              if (function_index.value() < functions_.size()) {
                function = functions_[function_index.value()];
              }
            }
          }
        }
      }
      if (error_status != ZX_OK) {
        completer(zx::error(error_status));
        return;
      }

      if (request_type == (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE) &&
          request == USB_REQ_GET_STATUS && length == 2) {
        std::vector<uint8_t> read_data_vec(length, 0);
        completer(zx::ok(std::move(read_data_vec)));
        return;
      }
      if (request_type == (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE) &&
          request == USB_REQ_GET_INTERFACE && length == 1) {
        completer(zx::ok(std::vector<uint8_t>{alt_setting}));
        return;
      }

      if (function) {
        completer(function->Control(setup, write_buffer));
        return;
      }
      break;
    }
    case USB_RECIP_ENDPOINT: {
      uint8_t ep_addr = static_cast<uint8_t>(index);
      bool is_ep0 = (ep_addr & USB_ENDPOINT_NUM_MASK) == 0;
      if (!is_ep0) {
        bool unconfigured = false;
        {
          fbl::AutoLock lock(&lock_);
          unconfigured = (configuration_ == 0);
        }
        if (unconfigured) {
          completer(zx::error(ZX_ERR_BAD_STATE));
          return;
        }
      }
      std::shared_ptr<UsbFunction> function;
      if (!is_ep0) {
        uint8_t ep_index = EpAddressToIndex(ep_addr);
        if (ep_index >= std::size(endpoint_map_)) {
          fdf::warn(
              "CommonControl: USB_RECIP_ENDPOINT ep index {} out of range (max {}) (raw index: {})",
              ep_index, std::size(endpoint_map_), index);
          completer(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
        zx_status_t ep_error = ZX_OK;
        {
          fbl::AutoLock lock(&lock_);
          auto function_index = endpoint_map_[ep_index];
          if (!function_index.has_value()) {
            fdf::warn("CommonControl: USB_RECIP_ENDPOINT ep index {} not allocated (raw index: {})",
                      ep_index, index);
            ep_error = ZX_ERR_NOT_FOUND;
          } else if (function_index.value() < functions_.size()) {
            function = functions_[function_index.value()];
          }
        }
        if (ep_error != ZX_OK) {
          completer(zx::error(ep_error));
          return;
        }
      }

      if (request_type == (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT) &&
          request == USB_REQ_GET_STATUS && length == 2) {
        uint16_t status = 0;
        if (!is_ep0) {
          fbl::AutoLock _(&lock_);
          if (stalled_eps_.contains(ep_addr)) {
            status = 1;
          }
        }
        std::vector<uint8_t> read_data_vec = {static_cast<uint8_t>(status & 0xFF),
                                              static_cast<uint8_t>(status >> 8)};
        completer(zx::ok(std::move(read_data_vec)));
        return;
      }
      if (request_type == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT) &&
          request == USB_REQ_SET_FEATURE && value == USB_ENDPOINT_HALT && length == 0) {
        if (!is_ep0) {
          UsbDciCancelAll(ep_addr);
          UsbDciEndpointSetStall(ep_addr);
        }
        completer(zx::ok(std::vector<uint8_t>()));
        return;
      }
      if (request_type == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT) &&
          request == USB_REQ_CLEAR_FEATURE && value == USB_ENDPOINT_HALT && length == 0) {
        if (!is_ep0) {
          UsbDciEndpointClearStall(ep_addr);
        }
        completer(zx::ok(std::vector<uint8_t>()));
        return;
      }
      // delegate to the function driver for the endpoint
      if (function) {
        completer(function->Control(setup, write_buffer));
        return;
      }
      break;
    }
    default:
      break;
  }

  fdf::debug(
      "CommonControl: Unhandled request (type: {:#02X}, req: {:#02X}, value: {:#04X}, index: {:#04X})",
      request_type, request, value, index);
  completer(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void UsbPeripheral::OnHostConnectionChanged(bool connected) {
  TRACE_DURATION("usb-peripheral", __func__);

  std::vector<std::shared_ptr<UsbFunction>> functions_to_unconfigure;
  fit::callback<void(zx_status_t)> canceled_completer;
  {
    fbl::AutoLock lock(&lock_);
    fdf::info("OnHostConnectionChanged: current_state={} connected={}", state_, connected);
    dci_inspect_.UpdateConnectionStatus(connected, speed_);

    connected_ = connected;

    if (connected) {
      // We also allow transition from kStarting because the controller might report
      // connection before we fully transition to kPeripheralReady in CheckAndStartController. This
      // is not required once we move to a single dispatcher.
      if (state_ == DeviceState::kPeripheralReady || state_ == DeviceState::kStarting) {
        SetStateLocked(DeviceState::kHostConnected);
      } else if (state_ == DeviceState::kHostConnected && in_flight_disconnect_unconfigures_ > 0) {
        fdf::info(
            "Host reconnected while disconnect unconfigure is in flight; remaining connected.");
      } else {
        fdf::info("Host connected event ignored in state {}", state_);
      }
      return;
    }

    // Disconnect event.
    if (pending_set_configuration_.has_value()) {
      canceled_completer = std::move(pending_set_configuration_->completer);
      pending_set_configuration_.reset();
    }
    bool ignore_disconnect = false;
    if (stopping_driver_ || state_ == DeviceState::kStopping) {
      fdf::info("Host disconnected event ignored: driver is stopping or tearing down (state={}).",
                state_);
      ignore_disconnect = true;
    } else if (state_ == DeviceState::kNoConfiguration) {
      fdf::info("Host disconnected event ignored in state {}", state_);
      ignore_disconnect = true;
    }

    if (!ignore_disconnect) {
      for (auto& function : functions_) {
        if (function) {
          functions_to_unconfigure.push_back(function);
        }
      }
      configuration_ = 0;

      if (functions_to_unconfigure.empty()) {
        if (state_ == DeviceState::kHostConnected || state_ == DeviceState::kPeripheralReady) {
          fdf::info(
              "No functions to unconfigure on disconnect. Transitioning to kPeripheralReady.");
          SetStateLocked(DeviceState::kPeripheralReady);
        }
      } else {
        in_flight_disconnect_unconfigures_++;
      }
    }
  }

  if (canceled_completer) {
    canceled_completer(ZX_ERR_CANCELED);
  }

  if (functions_to_unconfigure.empty()) {
    return;
  }

  // When a host disconnects, we must NOT transition the state to kPeripheralReady immediately.
  // If we do, parent regulator or PHY drivers are permitted to cut the controller power rails
  // and gate internal bus and PHY clocks.
  // To prevent unconfigure/endpoint-disable calls from executing on disabled hardware, we keep
  // the driver state in kHostConnected while function unconfigurations are executing.
  // This guarantees a powered, live core for a clean teardown. Once all unconfigure promises
  // resolve, we cleanly transition to kPeripheralReady under the lock.
  //
  // We call SetConfigured(false) for all functions in parallel and outside the lock
  // to avoid recursive lock deadlocks if Banjo fallback callbacks are synchronous.
  std::vector<fpromise::promise<void, zx_status_t>> promises;
  for (auto& function : functions_to_unconfigure) {
    fpromise::bridge<void, zx_status_t> bridge;
    function->SetConfigured(false, USB_SPEED_UNDEFINED,
                            [completer = std::move(bridge.completer),
                             name = function->name()](zx_status_t status) mutable {
                              if (status != ZX_OK) {
                                fdf::error(
                                    "SetConfigured(false) on disconnect failed for function {}: {}",
                                    name, zx_status_get_string(status));
                              }
                              completer.complete_ok();
                            });
    promises.push_back(bridge.consumer.promise_or(fpromise::error(ZX_ERR_CANCELED)));
  }

  auto join_task =
      fpromise::join_promise_vector(std::move(promises))
          .then([this, functions = std::move(functions_to_unconfigure)](
                    fpromise::result<std::vector<fpromise::result<void, zx_status_t>>>& results) {
            std::optional<PendingSetConfiguration> pending_set_config;
            fit::callback<void(zx_status_t)> canceled_task_completer;
            {
              fbl::AutoLock lock(&lock_);
              ZX_ASSERT(in_flight_disconnect_unconfigures_ > 0);
              in_flight_disconnect_unconfigures_--;
              if (in_flight_disconnect_unconfigures_ > 0) {
                fdf::info(
                    "Disconnect unconfigure task completed, but {} unconfigure task(s) are "
                    "still in-flight; maintaining state.",
                    in_flight_disconnect_unconfigures_);
                return;
              }

              // Verify the driver isn't actively tearing down the whole topology.
              if (state_ == DeviceState::kHostConnected ||
                  state_ == DeviceState::kPeripheralReady) {
                if (!connected_) {
                  fdf::info(
                      "All functions unconfigured on disconnect. Transitioning to kPeripheralReady.");
                  SetStateLocked(DeviceState::kPeripheralReady);
                } else {
                  fdf::info("Host reconnected during async teardown. Remaining in kHostConnected.");
                  SetStateLocked(DeviceState::kHostConnected);
                }
              }

              if (pending_set_configuration_.has_value()) {
                if (state_ == DeviceState::kHostConnected && connected_) {
                  pending_set_config = std::move(pending_set_configuration_);
                } else {
                  canceled_task_completer = std::move(pending_set_configuration_->completer);
                }
                pending_set_configuration_.reset();
              }
            }

            if (canceled_task_completer) {
              canceled_task_completer(ZX_ERR_CANCELED);
            }

            if (pending_set_config.has_value()) {
              fdf::info("Executing deferred SetConfiguration({}) after disconnect teardown.",
                        pending_set_config->configuration);
              SetConfiguration(pending_set_config->configuration,
                               std::move(pending_set_config->completer));
            }
          });

  executor_->schedule_task(std::move(join_task).wrap_with(scope_));
}

// This is called by management components to define the initial configuration of the
// USB peripheral device.
void UsbPeripheral::SetConfiguration(SetConfigurationRequestView request,
                                     SetConfigurationCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);

  {
    fbl::AutoLock _(&lock_);
    if (state_ == DeviceState::kStopping || stopping_driver_) {
      fdf::error("Cannot set configuration while peripheral is stopping (state={}, stopping={})",
                 state_, stopping_driver_);
      completer.ReplyError(ZX_ERR_BAD_STATE);
      return;
    }
    if (state_ != DeviceState::kNoConfiguration) {
      fdf::error("Cannot set configuration while functions are bound (state={})", state_);
      completer.ReplyError(ZX_ERR_ALREADY_BOUND);
      return;
    }
  }

  ZX_ASSERT(!request->config_descriptors.empty());
  zx_status_t status = SetDeviceDescriptor(request->device_desc);
  if (status != ZX_OK) {
    fdf::error("Failed to set device descriptor: {}", status);
    completer.ReplyError(status);
    return;
  }

  uint8_t index = 0;
  for (auto& func_descs : request->config_descriptors) {
    auto& descriptor = configurations_.emplace_back(index);
    if (func_descs.size() == 0) {
      fdf::error("Cannot set configuration with no functions");
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }

    for (auto func_desc : func_descs) {
      auto result = AddFunction(descriptor, func_desc);
      if (result.is_error()) {
        fdf::error("Failed to add function: {}", result);
      }
    }
    index++;
  }

  {
    fbl::AutoLock _(&lock_);
    if (zx_status_t status = AddFunctionDevices(); status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    SetStateLocked(DeviceState::kWaitForFunctionBind);
  }

  completer.ReplySuccess();

  // This can trigger a synchronous fidl call to the listener (which can be the same client who
  // invoked SetConfiguration). So we do this after the reply.
  if (zx_status_t status = CheckAndStartController(); status != ZX_OK) {
    fdf::error("CheckAndStartController failed: {}", zx_status_get_string(status));
  }
}

zx_status_t UsbPeripheral::SetDeviceDescriptor(DeviceDescriptor desc) {
  TRACE_DURATION("usb-peripheral", __func__);

  if (desc.b_num_configurations == 0) {
    fdf::error("bNumConfigurations must be non-zero");
    return ZX_ERR_INVALID_ARGS;
  } else {
    device_desc_.b_length = sizeof(usb_device_descriptor_t);
    device_desc_.b_descriptor_type = USB_DT_DEVICE;
    device_desc_.bcd_usb = desc.bcd_usb;
    device_desc_.b_device_class = desc.b_device_class;
    device_desc_.b_device_sub_class = desc.b_device_sub_class;
    device_desc_.b_device_protocol = desc.b_device_protocol;
    device_desc_.b_max_packet_size0 = desc.b_max_packet_size0;
    device_desc_.id_vendor = desc.id_vendor;
    device_desc_.id_product = desc.id_product;
    device_desc_.bcd_device = desc.bcd_device;
    zx_status_t status = AllocStringDesc(
        std::nullopt, std::string(desc.manufacturer.data(), desc.manufacturer.size()),
        &device_desc_.i_manufacturer);
    if (status != ZX_OK) {
      return status;
    }
    status = AllocStringDesc(std::nullopt, std::string(desc.product.data(), desc.product.size()),
                             &device_desc_.i_product);
    if (status != ZX_OK) {
      return status;
    }
    status = AllocStringDesc(std::nullopt, std::string(desc.serial.data(), desc.serial.size()),
                             &device_desc_.i_serial_number);
    if (status != ZX_OK) {
      return status;
    }
    device_desc_.b_num_configurations = desc.b_num_configurations;
    return ZX_OK;
  }
}

void UsbPeripheral::ClearFunctions(ClearFunctionsCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);

  fdf::debug("{}", __func__);
  auto on_complete = [completer = completer.ToAsync()]() mutable { completer.Reply(); };
  ClearFunctions(std::move(on_complete));
}

void UsbPeripheral::GetConfiguration(GetConfigurationCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  fbl::AutoLock lock(&lock_);

  if (configurations_.empty()) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  fidl::Arena arena;

  fperipheral::wire::DeviceDescriptor dev_desc{
      .bcd_usb = device_desc_.bcd_usb,
      .b_device_class = device_desc_.b_device_class,
      .b_device_sub_class = device_desc_.b_device_sub_class,
      .b_device_protocol = device_desc_.b_device_protocol,
      .b_max_packet_size0 = device_desc_.b_max_packet_size0,
      .id_vendor = device_desc_.id_vendor,
      .id_product = device_desc_.id_product,
      .bcd_device = device_desc_.bcd_device,
      .manufacturer = fidl::StringView(arena, ""),
      .product = fidl::StringView(arena, ""),
      .serial = fidl::StringView(arena, ""),
      .b_num_configurations = device_desc_.b_num_configurations,
  };

  if (device_desc_.i_manufacturer != 0 && device_desc_.i_manufacturer <= strings_.size() &&
      strings_[device_desc_.i_manufacturer - 1].allocated) {
    dev_desc.manufacturer = fidl::StringView(arena, strings_[device_desc_.i_manufacturer - 1].text);
  }
  if (device_desc_.i_product != 0 && device_desc_.i_product <= strings_.size() &&
      strings_[device_desc_.i_product - 1].allocated) {
    dev_desc.product = fidl::StringView(arena, strings_[device_desc_.i_product - 1].text);
  }
  if (device_desc_.i_serial_number != 0 && device_desc_.i_serial_number <= strings_.size() &&
      strings_[device_desc_.i_serial_number - 1].allocated) {
    dev_desc.serial = fidl::StringView(arena, strings_[device_desc_.i_serial_number - 1].text);
  }

  fidl::VectorView<fidl::VectorView<fperipheral::wire::FunctionDescriptor>> configs(
      arena, configurations_.size());

  size_t config_idx = 0;
  for (const auto& config : configurations_) {
    fidl::VectorView<fperipheral::wire::FunctionDescriptor> funcs(arena, config.functions.size());
    size_t func_idx = 0;
    for (size_t function_index : config.functions) {
      if (function_index < functions_.size() && functions_[function_index] != nullptr) {
        const auto& desc = functions_[function_index]->GetFunctionDescriptor();
        funcs[func_idx].interface_class = desc.interface_class;
        funcs[func_idx].interface_subclass = desc.interface_subclass;
        funcs[func_idx].interface_protocol = desc.interface_protocol;
        func_idx++;
      }
    }
    funcs.set_size(func_idx);
    configs[config_idx++] = funcs;
  }
  configs.set_size(config_idx);

  completer.ReplySuccess(dev_desc, configs);
}

void UsbPeripheral::SetStateChangeListener(SetStateChangeListenerRequestView request,
                                           SetStateChangeListenerCompleter::Sync& completer) {
  TRACE_DURATION("usb-peripheral", __func__);
  fbl::AutoLock lock(&lock_);
  listener_ =
      fidl::WireSharedClient<fperipheral::Events>(std::move(request->listener), dispatcher());
}

void UsbPeripheral::handle_unknown_method(fidl::UnknownMethodMetadata<fperipheral::Device> metadata,
                                          fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("UsbPeripheral: unknown method ordinal={}", metadata.method_ordinal);
}

void UsbPeripheral::Stop(fdf::StopCompleter completer) {
  TRACE_DURATION("usb-peripheral", __func__);

  fdf::info("UsbPeripheral::Stop: started");

  fit::callback<void()> on_complete;
  bool call_clear = false;
  bool wait_clear = false;
  {
    fbl::AutoLock lock(&lock_);
    stopping_driver_ = true;

    on_complete = [completer = std::move(completer)]() mutable {
      fdf::info("UsbPeripheral::Stop: Functions cleared, replying to completer");
      completer(zx::ok());
    };

    switch (state_) {
      case DeviceState::kWaitForFunctionBind:
        [[fallthrough]];
      case DeviceState::kStarting:
        [[fallthrough]];
      case DeviceState::kPeripheralReady:
        [[fallthrough]];
      case DeviceState::kHostConnected:
        call_clear = true;
        break;
      case DeviceState::kStopping:
        wait_clear = true;
        break;
      case DeviceState::kNoConfiguration:
        break;
    }
  }

  if (call_clear) {
    fdf::info("UsbPeripheral::Stop: proceeding to clear functions.");
    ClearFunctions(std::move(on_complete));
  } else if (wait_clear) {
    fdf::info("UsbPeripheral::Stop: already stopping, waiting for functions to clear.");
    WaitForFunctionsCleared(std::move(on_complete));
  } else {
    on_complete();
  }
}

zx_status_t UsbPeripheral::SetDefaultConfig(std::vector<FunctionDescriptor>& functions) {
  TRACE_DURATION("usb-peripheral", __func__);

  {
    fbl::AutoLock _(&lock_);
    if (state_ == DeviceState::kStopping || stopping_driver_) {
      return ZX_ERR_BAD_STATE;
    }
    if (state_ != DeviceState::kNoConfiguration) {
      return ZX_ERR_ALREADY_BOUND;
    }
  }

  auto& descriptor = configurations_.emplace_back(static_cast<uint8_t>(0));
  device_desc_.b_length = sizeof(usb_device_descriptor_t),
  device_desc_.b_descriptor_type = USB_DT_DEVICE;
  device_desc_.bcd_usb = htole16(USB_2_0);
  device_desc_.b_device_class = 0;
  device_desc_.b_device_sub_class = 0;
  device_desc_.b_device_protocol = 0;
  device_desc_.b_max_packet_size0 = 64;
  device_desc_.bcd_device = htole16(0x0100);
  device_desc_.b_num_configurations = 1;

  for (auto function : functions) {
    auto result = AddFunction(descriptor, function);
    if (result.is_error()) {
      fdf::error("Failed to add function: ({}:{}:{}) status: {}", function.interface_class,
                 function.interface_subclass, function.interface_protocol, result);
      return result.status_value();
    }
  }

  {
    fbl::AutoLock _(&lock_);
    if (zx_status_t status = AddFunctionDevices(); status != ZX_OK) {
      return status;
    }
    if (!functions_.empty()) {
      SetStateLocked(DeviceState::kWaitForFunctionBind);
    }
  }
  if (zx_status_t status = CheckAndStartController(); status != ZX_OK) {
    fdf::error("CheckAndStartController failed: {}", zx_status_get_string(status));
  }
  return ZX_OK;
}

UsbFunction& UsbPeripheral::GetFunction(size_t index) {
  TRACE_DURATION("usb-peripheral", __func__, "index", index);

  ZX_ASSERT_MSG(index < functions_.size(), "Function %lu does not exist (functions_.size() = %lu)",
                index, functions_.size());
  auto& function = functions_[index];
  ZX_ASSERT(function != nullptr);
  return *function;
}

const UsbFunction& UsbPeripheral::GetFunction(size_t index) const {
  TRACE_DURATION("usb-peripheral", __func__);

  ZX_ASSERT_MSG(index < functions_.size(), "Function %lu does not exist", index);
  const auto& function = functions_[index];
  ZX_ASSERT(function != nullptr);
  return *function;
}

void UsbPeripheral::ReleaseResources(size_t function_index) {
  fbl::AutoLock lock(&lock_);
  ReleaseResourcesLocked(function_index);
}

void UsbPeripheral::ReleaseResourcesLocked(size_t function_index) {
  TRACE_DURATION("usb-peripheral", __func__, "function_index", function_index);

  // Clear entries in interface_map for all configurations.
  for (auto& config : configurations_) {
    for (auto& intf : config.interface_map) {
      if (intf == function_index) {
        intf.reset();
      }
    }
  }

  // Clear entries in endpoint_map_.
  for (size_t i = 0; i < std::size(endpoint_map_); i++) {
    if (endpoint_map_[i] == function_index) {
      FreeEndpointInternalLocked(static_cast<uint8_t>(i), "release");
      endpoint_map_[i].reset();
    }
  }

  // Clear entries in strings_.
  for (auto& str : strings_) {
    if (str.function_index == function_index) {
      str.text.clear();
      str.function_index.reset();
      str.allocated = false;
    }
  }

  // If there are trailing empty strings, we can truncate the vector.
  while (!strings_.empty() && !strings_.back().allocated) {
    strings_.pop_back();
  }
}

UsbPeripheral::ResourceAllocations UsbPeripheral::GetResourceAllocations(size_t function_index) {
  fbl::AutoLock lock(&lock_);
  ResourceAllocations allocations;

  for (const auto& config : configurations_) {
    for (size_t i = 0; i < std::size(config.interface_map); i++) {
      if (config.interface_map[i] == function_index) {
        allocations.interface_nums.push_back(static_cast<uint8_t>(i));
      }
    }
  }

  for (size_t i = 0; i < std::size(endpoint_map_); i++) {
    if (endpoint_map_[i] == function_index) {
      allocations.endpoint_addrs.push_back(EpIndexToAddress(static_cast<uint8_t>(i)));
    }
  }

  for (size_t i = 0; i < strings_.size(); i++) {
    if (strings_[i].function_index == function_index) {
      allocations.string_indices.push_back(static_cast<uint8_t>(i + 1));
    }
  }

  return allocations;
}

void UsbPeripheral::WaitForFunctionsCleared(fit::callback<void()> callback) {
  fbl::AutoLock lock(&lock_);
  if (functions_.empty() && !clearing_functions_) {
    lock.release();
    callback();
    return;
  }
  on_all_functions_cleared_.push_back(UnlockedCallback(std::move(callback), lock_));
}

}  // namespace usb_peripheral
FUCHSIA_DRIVER_EXPORT2(usb_peripheral::UsbPeripheral);
