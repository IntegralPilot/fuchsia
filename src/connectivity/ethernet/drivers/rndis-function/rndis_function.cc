// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/ethernet/drivers/rndis-function/rndis_function.h"

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <zircon/status.h>

#include <usb/request-cpp.h>

#include "src/connectivity/ethernet/lib/rndis/rndis.h"

namespace frequest = fuchsia_hardware_usb_request;
namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

constexpr uint32_t kArenaTag = 'RNDS';

std::optional<std::vector<uint8_t>> RndisFunction::QueryOid(uint32_t oid, void* input,
                                                            size_t length) {
  fdf::info("Query OID {}", oid);
  std::optional<std::vector<uint8_t>> response;
  switch (oid) {
    case OID_GEN_SUPPORTED_LIST: {
      static constexpr uint32_t supported[] = {
          // General OIDs.
          OID_GEN_SUPPORTED_LIST,
          OID_GEN_HARDWARE_STATUS,
          OID_GEN_MEDIA_SUPPORTED,
          OID_GEN_MEDIA_IN_USE,
          OID_GEN_MAXIMUM_FRAME_SIZE,
          OID_GEN_LINK_SPEED,
          OID_GEN_TRANSMIT_BLOCK_SIZE,
          OID_GEN_RECEIVE_BLOCK_SIZE,
          OID_GEN_VENDOR_ID,
          OID_GEN_VENDOR_DESCRIPTION,
          OID_GEN_VENDOR_DRIVER_VERSION,
          OID_GEN_CURRENT_PACKET_FILTER,
          OID_GEN_MAXIMUM_TOTAL_SIZE,
          OID_GEN_PHYSICAL_MEDIUM,
          OID_GEN_MEDIA_CONNECT_STATUS,

          // General statistic OIDs.
          OID_GEN_XMIT_OK,
          OID_GEN_RCV_OK,
          OID_GEN_XMIT_ERROR,
          OID_GEN_RCV_ERROR,
          OID_GEN_RCV_NO_BUFFER,

          // 802.3 OIDs.
          OID_802_3_PERMANENT_ADDRESS,
          OID_802_3_CURRENT_ADDRESS,
          OID_802_3_MULTICAST_LIST,
          OID_802_3_MAXIMUM_LIST_SIZE,
      };
      std::vector<uint8_t> buffer(sizeof(supported));
      memcpy(buffer.data(), &supported, sizeof(supported));
      response.emplace(buffer);
      break;
    }
    case OID_GEN_HARDWARE_STATUS: {
      uint32_t status = RNDIS_HW_STATUS_READY;
      std::vector<uint8_t> buffer(sizeof(status));
      memcpy(buffer.data(), &status, sizeof(status));
      response.emplace(buffer);
      break;
    }
    case OID_GEN_TRANSMIT_BLOCK_SIZE:
    case OID_GEN_RECEIVE_BLOCK_SIZE:
    case OID_GEN_MAXIMUM_FRAME_SIZE: {
      uint32_t frame_size = kMtu - sizeof(rndis_packet_header);
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&frame_size),
                               reinterpret_cast<uint8_t*>(&frame_size) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_LINK_SPEED: {
      static_assert(sizeof(link_speed_) == sizeof(uint32_t));
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&link_speed_),
                               reinterpret_cast<uint8_t*>(&link_speed_) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_VENDOR_ID: {
      static_assert(sizeof(kVendorId) == sizeof(uint32_t));
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<const uint8_t*>(&kVendorId),
                               reinterpret_cast<const uint8_t*>(&kVendorId) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_VENDOR_DESCRIPTION: {
      std::vector<uint8_t> buffer(sizeof(kVendorDescription));
      memcpy(buffer.data(), &kVendorDescription, sizeof(kVendorDescription));
      response.emplace(buffer);
      break;
    }
    case OID_GEN_VENDOR_DRIVER_VERSION: {
      static_assert(sizeof(kVendorDriverVersionMajor) == sizeof(uint16_t));
      static_assert(sizeof(kVendorDriverVersionMinor) == sizeof(uint16_t));
      uint32_t version = (kVendorDriverVersionMajor << 16) | kVendorDriverVersionMinor;
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&version),
                               reinterpret_cast<uint8_t*>(&version) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_MEDIA_CONNECT_STATUS: {
      uint32_t status = RNDIS_STATUS_MEDIA_CONNECT;
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&status),
                               reinterpret_cast<uint8_t*>(&status) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_MEDIA_SUPPORTED:
    case OID_GEN_MEDIA_IN_USE:
    case OID_GEN_PHYSICAL_MEDIUM: {
      uint32_t medium = RNDIS_MEDIUM_802_3;
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&medium),
                               reinterpret_cast<uint8_t*>(&medium) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_MAXIMUM_TOTAL_SIZE: {
      uint32_t total_size = RNDIS_MAX_DATA_SIZE;
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&total_size),
                               reinterpret_cast<uint8_t*>(&total_size) + sizeof(uint32_t)));
      break;
    }

    case OID_802_3_PERMANENT_ADDRESS:
    case OID_802_3_CURRENT_ADDRESS: {
      std::vector<uint8_t> buffer;
      buffer.insert(buffer.end(), mac_addr_.begin(), mac_addr_.end());
      // Make the host and device addresses different so packets are routed correctly.
      buffer[5] ^= 1;
      response.emplace(buffer);
      break;
    }
    case OID_802_3_MULTICAST_LIST: {
      static constexpr uint32_t list[] = {0xE0000000};
      std::vector<uint8_t> buffer(sizeof(list));
      memcpy(buffer.data(), &list, sizeof(list));
      response.emplace(buffer);
      break;
    }
    case OID_802_3_MAXIMUM_LIST_SIZE: {
      uint32_t list_size = 1;
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&list_size),
                               reinterpret_cast<uint8_t*>(&list_size) + sizeof(uint32_t)));
      break;
    }

    // These stats are from the perspective of the host, so transmit and receive are flipped.
    case OID_GEN_XMIT_OK: {
      static_assert(sizeof(receive_ok_) == sizeof(uint32_t));
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&receive_ok_),
                               reinterpret_cast<uint8_t*>(&receive_ok_) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_RCV_OK: {
      static_assert(sizeof(transmit_ok_) == sizeof(uint32_t));
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&transmit_ok_),
                               reinterpret_cast<uint8_t*>(&transmit_ok_) + sizeof(uint32_t)));

      break;
    }
    case OID_GEN_XMIT_ERROR: {
      static_assert(sizeof(receive_errors_) == sizeof(uint32_t));
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&receive_errors_),
                               reinterpret_cast<uint8_t*>(&receive_errors_) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_RCV_ERROR: {
      static_assert(sizeof(transmit_errors_) == sizeof(uint32_t));
      response.emplace(
          std::vector<uint8_t>(reinterpret_cast<uint8_t*>(&transmit_errors_),
                               reinterpret_cast<uint8_t*>(&transmit_errors_) + sizeof(uint32_t)));
      break;
    }
    case OID_GEN_RCV_NO_BUFFER: {
      static_assert(sizeof(transmit_no_buffer_) == sizeof(uint32_t));
      response.emplace(std::vector<uint8_t>(
          reinterpret_cast<uint8_t*>(&transmit_no_buffer_),
          reinterpret_cast<uint8_t*>(&transmit_no_buffer_) + sizeof(uint32_t)));
      break;
    }

    default:
      break;
  }
  if (!response.has_value()) {
    fdf::warn("Did not generate a response to OID query {}.", oid);
  }
  return response;
}

zx_status_t RndisFunction::SetOid(uint32_t oid, const uint8_t* buffer, size_t length) {
  switch (oid) {
    case OID_GEN_CURRENT_PACKET_FILTER: {
      bool indicate_status = false;
      rndis_ready_ = true;
      if (netdevice_ifc_.is_valid()) {
        UpdatePortStatus();
        indicate_status = true;
      }

      std::vector<frequest::Request> requests;
      while (std::optional req = bulk_out_ep_.GetRequest()) {
        req.value().reset_buffers(bulk_out_ep_.GetMapped());
        requests.emplace_back(req.value().take_request());
      }
      fit::result<fidl::OneWayError> status = bulk_out_ep_->QueueRequests({std::move(requests)});
      if (!status.is_ok()) {
        fdf::error("Failed to queue requests: {}", status.error_value().FormatDescription());
      }

      if (indicate_status) {
        fdf::error("IndicateStatus from SetOid");
        IndicateConnectionStatus(true);
      } else {
        fdf::error("No IndicateStatus from SetOid");
      }
      return ZX_OK;
    }
    case OID_802_3_MULTICAST_LIST: {
      // Ignore
      fdf::warn("Host set multicast list (buffer len {}).", length);
      return ZX_OK;
    }
    default:
      fdf::warn("Unhandled OID: {}", oid);
      return ZX_ERR_NOT_SUPPORTED;
  }
}

std::vector<uint8_t> InvalidMessageResponse(const void* invalid_data, size_t size) {
  fdf::warn("Host sent an invalid message.");

  std::vector<uint8_t> buffer(sizeof(rndis_indicate_status) + sizeof(rndis_diagnostic_info) + size);

  rndis_indicate_status status{
      .msg_type = RNDIS_INDICATE_STATUS_MSG,
      .msg_length = static_cast<uint32_t>(buffer.size()),
      .status = RNDIS_STATUS_INVALID_DATA,
      .status_buffer_length = static_cast<uint32_t>(size),
      .status_buffer_offset = static_cast<uint32_t>(sizeof(rndis_indicate_status) -
                                                    offsetof(rndis_indicate_status, status)),
  };

  rndis_diagnostic_info info{
      .diagnostic_status = RNDIS_STATUS_INVALID_DATA,
      // TODO: This is supposed to an offset to the error in |invalid_data|.
      .error_offset = 0,
  };

  memcpy(buffer.data(), &status, sizeof(status));
  uintptr_t offset = sizeof(status);
  memcpy(buffer.data() + offset, &info, sizeof(info));
  offset += sizeof(info);
  memcpy(buffer.data() + offset, invalid_data, size);

  return buffer;
}

std::vector<uint8_t> InitResponse(uint32_t request_id, uint32_t status) {
  rndis_init_complete response{.msg_type = RNDIS_INITIALIZE_CMPLT,
                               .msg_length = sizeof(rndis_init_complete),
                               .request_id = request_id,
                               .status = status,
                               .major_version = RNDIS_MAJOR_VERSION,
                               .minor_version = RNDIS_MINOR_VERSION,
                               .device_flags = RNDIS_DF_CONNECTIONLESS,
                               .medium = RNDIS_MEDIUM_802_3,
                               .max_packets_per_xfer = 1,
                               .max_xfer_size = RNDIS_MAX_XFER_SIZE,
                               .packet_alignment = 0,
                               .reserved0 = 0,
                               .reserved1 = 0};

  std::vector<uint8_t> buffer(sizeof(rndis_init_complete));
  memcpy(buffer.data(), &response, sizeof(rndis_init_complete));
  return buffer;
}

std::vector<uint8_t> ResetResponse(uint32_t status) {
  rndis_reset_complete response{.msg_type = RNDIS_RESET_CMPLT,
                                .msg_length = sizeof(rndis_reset_complete),
                                .status = status,
                                .addressing_reset = 1};

  std::vector<uint8_t> buffer(sizeof(rndis_reset_complete));
  memcpy(buffer.data(), &response, sizeof(rndis_reset_complete));
  return buffer;
}

std::vector<uint8_t> QueryResponse(uint32_t request_id,
                                   const std::optional<std::vector<uint8_t>>& oid_response) {
  size_t buffer_size = sizeof(rndis_query_complete);
  if (oid_response.has_value()) {
    buffer_size += oid_response->size();
  }
  std::vector<uint8_t> buffer(buffer_size);

  rndis_query_complete response;
  response.msg_type = RNDIS_QUERY_CMPLT;
  response.msg_length = static_cast<uint32_t>(buffer.size());
  response.request_id = request_id;

  if (oid_response.has_value()) {
    response.status = RNDIS_STATUS_SUCCESS;
    response.info_buffer_offset =
        sizeof(rndis_query_complete) - offsetof(rndis_query_complete, request_id);
    response.info_buffer_length = static_cast<uint32_t>(oid_response->size());

    memcpy(buffer.data() + sizeof(rndis_query_complete), oid_response->data(),
           oid_response->size());
  } else {
    response.status = RNDIS_STATUS_NOT_SUPPORTED;
    response.info_buffer_offset = 0;
    response.info_buffer_length = 0;
  }

  memcpy(buffer.data(), &response, sizeof(rndis_query_complete));

  return buffer;
}

std::vector<uint8_t> SetResponse(uint32_t request_id, uint32_t status) {
  rndis_set_complete response{
      .msg_type = RNDIS_SET_CMPLT,
      .msg_length = static_cast<uint32_t>(sizeof(rndis_set_complete)),
      .request_id = request_id,
      .status = status,
  };

  std::vector<uint8_t> buffer(sizeof(rndis_set_complete));
  memcpy(buffer.data(), &response, sizeof(rndis_set_complete));
  return buffer;
}

std::vector<uint8_t> KeepaliveResponse(uint32_t request_id, uint32_t status) {
  rndis_header_complete response{
      .msg_type = RNDIS_KEEPALIVE_CMPLT,
      .msg_length = sizeof(rndis_header_complete),
      .request_id = request_id,
      .status = status,
  };

  std::vector<uint8_t> buffer(sizeof(rndis_header_complete));
  memcpy(buffer.data(), &response, sizeof(rndis_header_complete));
  return buffer;
}

zx_status_t RndisFunction::HandleCommand(const void* buffer, size_t size) {
  if (size < sizeof(rndis_header)) {
    control_responses_.push(InvalidMessageResponse(buffer, size));
    Notify();
    return ZX_OK;
  }

  auto header = static_cast<const rndis_header*>(buffer);
  std::optional<std::vector<uint8_t>> response;

  switch (header->msg_type) {
    case RNDIS_INITIALIZE_MSG: {
      if (size < sizeof(rndis_init)) {
        response.emplace(InvalidMessageResponse(buffer, size));
        break;
      }

      auto init = static_cast<const rndis_init*>(buffer);
      if (init->major_version != RNDIS_MAJOR_VERSION) {
        fdf::warn("Invalid RNDIS major version. Expected {}, got {}.", RNDIS_MAJOR_VERSION,
                  init->major_version);
        response.emplace(InitResponse(init->request_id, RNDIS_STATUS_NOT_SUPPORTED));
      } else if (init->minor_version != RNDIS_MINOR_VERSION) {
        fdf::warn("Invalid RNDIS minor version. Expected {}, got {}.", RNDIS_MINOR_VERSION,
                  init->minor_version);
        response.emplace(InitResponse(init->request_id, RNDIS_STATUS_NOT_SUPPORTED));
      }

      response.emplace(InitResponse(init->request_id, RNDIS_STATUS_SUCCESS));
      break;
    }
    case RNDIS_QUERY_MSG: {
      if (size < sizeof(rndis_query)) {
        response.emplace(InvalidMessageResponse(buffer, size));
        break;
      }

      auto query = static_cast<const rndis_query*>(buffer);
      auto oid_response = QueryOid(query->oid, nullptr, 0);
      response.emplace(QueryResponse(query->request_id, oid_response));
      break;
    }
    case RNDIS_SET_MSG: {
      if (size < sizeof(rndis_set)) {
        response.emplace(InvalidMessageResponse(buffer, size));
        break;
      }

      auto set = static_cast<const rndis_set*>(buffer);
      if (set->info_buffer_length > RNDIS_SET_INFO_BUFFER_LENGTH) {
        response.emplace(SetResponse(set->request_id, RNDIS_STATUS_INVALID_DATA));
        break;
      }

      size_t offset = offsetof(rndis_set, request_id) + set->info_buffer_offset;
      if (offset + set->info_buffer_length > size) {
        response.emplace(SetResponse(set->request_id, RNDIS_STATUS_INVALID_DATA));
        break;
      }

      zx_status_t status = SetOid(set->oid, reinterpret_cast<const uint8_t*>(buffer) + offset,
                                  set->info_buffer_length);

      uint32_t rndis_status = RNDIS_STATUS_SUCCESS;
      if (status == ZX_ERR_NOT_SUPPORTED) {
        rndis_status = RNDIS_STATUS_NOT_SUPPORTED;
      } else if (status != ZX_OK) {
        rndis_status = RNDIS_STATUS_FAILURE;
      }
      response.emplace(SetResponse(set->request_id, rndis_status));
      break;
    }
    case RNDIS_KEEPALIVE_MSG:
      response.emplace(KeepaliveResponse(header->request_id, RNDIS_STATUS_SUCCESS));
      break;
    case RNDIS_HALT_MSG: {
      zx_status_t status = Halt();
      if (status != ZX_OK) {
        fdf::warn("Failed to handle HALT command: {}", zx_status_get_string(status));
      }
      break;
    }
    case RNDIS_RESET_MSG:
      Reset();
      response.emplace(ResetResponse(RNDIS_STATUS_SUCCESS));
      break;
    case RNDIS_PACKET_MSG:
      // The should only send packets on the data channel.
      // TODO: How should we respond to this?
      fdf::warn("Host sent a data packet on the control channel.");
      break;
    default:
      fdf::warn("Host sent an unrecognised message: {}.", header->msg_type);
      response.emplace(InvalidMessageResponse(buffer, size));
      break;
  }

  if (!response.has_value()) {
    return ZX_OK;
  }
  control_responses_.push(std::move(response.value()));
  Notify();
  return ZX_OK;
}

zx::result<std::vector<uint8_t>> ErrorResponse(size_t size) {
  if (size < 1) {
    return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
  }
  // From
  // https://docs.microsoft.com/en-au/windows-hardware/drivers/network/control-channel-characteristics:
  // If for some reason the device receives a GET_ENCAPSULATED_RESPONSE and is unable to respond
  // with a valid data on the Control endpoint, then it should return a one-byte packet set to
  // 0x00, rather than stalling the Control endpoint.
  return zx::ok(std::vector<uint8_t>{0x00});
}

zx::result<std::vector<uint8_t>> RndisFunction::HandleResponse(size_t size) {
  if (control_responses_.empty()) {
    fdf::warn("Host tried to read a control response when none was available.");
    return ErrorResponse(size);
  }

  auto& packet = control_responses_.front();
  if (size < packet.size()) {
    fdf::warn(
        "Buffer too small to read a control response. Packet size is {} but the buffer is {}.",
        packet.size(), size);
    return ErrorResponse(size);
  }

  std::vector<uint8_t> response = std::move(packet);
  control_responses_.pop();
  return zx::ok(std::move(response));
}

zx_status_t RndisFunction::Halt() {
  Reset();

  for (auto ep_info : GetEndpoints()) {
    fidl::Result result = function_->DisableEndpoint({ep_info.address});
    if (result.is_error()) {
      fdf::error("Failed to disable {} endpoint: {}", ep_info.name,
                 result.error_value().FormatDescription());
      return result.error_value().is_framework_error()
                 ? result.error_value().framework_error().status()
                 : ZX_ERR_INTERNAL;
    }
  }
  return ZX_OK;
}

void RndisFunction::Reset() {
  CancelAllRequests();
  while (!control_responses_.empty()) {
    control_responses_.pop();
  }

  rndis_ready_ = false;
  link_speed_ = 0;
  UpdatePortStatus();
}

void RndisFunction::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  auto& setup = request.setup();
  auto& write_buffer = request.write();

  uint8_t bm_request_type = setup.bm_request_type();
  uint8_t b_request = setup.b_request();

  if (bm_request_type == (USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE) &&
      b_request == USB_CDC_SEND_ENCAPSULATED_COMMAND) {
    zx_status_t status = HandleCommand(write_buffer.data(), write_buffer.size());
    if (status != ZX_OK) {
      fdf::error("Error handling command: {}", zx_status_get_string(status));
      completer.Reply(zx::error(status));
      return;
    }
    completer.Reply(zx::ok(std::vector<uint8_t>{}));
    return;
  }
  if (bm_request_type == (USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE) &&
      b_request == USB_CDC_GET_ENCAPSULATED_RESPONSE) {
    completer.Reply(HandleResponse(request.setup().w_length()));
    return;
  }

  fdf::warn("Unrecognised control interface transfer: bm_request_type {} b_request {}",
            bm_request_type, b_request);
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void RndisFunction::SetConfigured(SetConfiguredRequest& request,
                                  SetConfiguredCompleter::Sync& completer) {
  if (!request.configured()) {
    completer.Reply(zx::make_result(Halt()));
    return;
  }

  auto config_ep = [&](const usb_endpoint_descriptor_t& desc) -> zx_status_t {
    fuchsia_hardware_usb_function::EndpointConfiguration ep_config;
    fuchsia_hardware_usb_function::EndpointDescriptor ep_desc;
    ep_desc.bm_attributes(desc.bm_attributes);
    ep_desc.w_max_packet_size(le16toh(desc.w_max_packet_size));
    ep_desc.b_interval(desc.b_interval);
    ep_config.descriptor(std::move(ep_desc));

    fidl::Result result =
        function_->ConfigureEndpoint({desc.b_endpoint_address, std::move(ep_config)});
    if (result.is_error()) {
      fdf::error("Failed to configure endpoint: {}", result.error_value().FormatDescription());
      return result.error_value().is_framework_error()
                 ? result.error_value().framework_error().status()
                 : ZX_ERR_INTERNAL;
    }
    return ZX_OK;
  };

  zx_status_t status = config_ep(descriptors_.notification_ep);
  if (status != ZX_OK) {
    fdf::error("Failed to configure control endpoint: {}", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  status = config_ep(descriptors_.in_ep);
  if (status != ZX_OK) {
    fdf::error("Failed to configure bulk in endpoint: {}", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }
  status = config_ep(descriptors_.out_ep);
  if (status != ZX_OK) {
    fdf::error("Failed to configure bulk out endpoint: {}", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  switch (request.speed()) {
    case fuchsia_hardware_usb_descriptor::UsbSpeed::kLow:
      link_speed_ = 15'000;
      break;
    case fuchsia_hardware_usb_descriptor::UsbSpeed::kFull:
      link_speed_ = 120'000;
      break;
    case fuchsia_hardware_usb_descriptor::UsbSpeed::kHigh:
      link_speed_ = 4'800'000;
      break;
    case fuchsia_hardware_usb_descriptor::UsbSpeed::kSuper:
      link_speed_ = 50'000'000;
      break;
    default:
      link_speed_ = 0;
      break;
  }
  completer.Reply(zx::ok());
}

void RndisFunction::SetInterface(SetInterfaceRequest& request,
                                 SetInterfaceCompleter::Sync& completer) {
  uint8_t interface = request.interface();
  uint8_t alt_setting = request.alt_setting();

  if (interface == descriptors_.communication_interface.b_interface_number ||
      interface == descriptors_.data_interface.b_interface_number) {
    if (alt_setting != 0) {
      completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
      return;
    }
    completer.Reply(zx::ok());
    return;
  }
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void RndisFunction::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method %ld", metadata.method_ordinal);
}

void RndisFunction::Init(InitRequestView request, fdf::Arena& arena,
                         InitCompleter::Sync& completer) {
  fdf::info("RndisFunction::Init called");
  netdevice_ifc_.Bind(std::move(request->iface), driver_dispatcher()->get());

  auto [client, server] = fdf::Endpoints<fnetdev::NetworkPort>::Create();
  fdf::BindServer(driver_dispatcher()->get(), std::move(server), this);

  netdevice_ifc_.buffer(arena)
      ->AddPort(kPortId, std::move(client))
      .ThenExactlyOnce(
          [completer = completer.ToAsync()](
              fdf::WireUnownedResult<fnetdev::NetworkDeviceIfc::AddPort>& result) mutable {
            fdf::Arena arena(kArenaTag);
            if (!result.ok()) {
              fdf::error("AddPort failed: {}", result.FormatDescription());
              completer.buffer(arena).Reply(result.status());
              return;
            }
            if (result->status != ZX_OK) {
              fdf::error("AddPort returned error: {}", zx_status_get_string(result->status));
            }
            completer.buffer(arena).Reply(result->status);
          });
}

void RndisFunction::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  IndicateConnectionStatus(true);
  completer.buffer(arena).Reply(ZX_OK);
}

void RndisFunction::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  DiscardPendingTxBuffers(ZX_ERR_CANCELED);
  ReturnPendingRxSpace();
  IndicateConnectionStatus(false);
  completer.buffer(arena).Reply();
}

void RndisFunction::GetInfo(
    fdf::Arena& arena,
    fdf::WireServer<fnetdev::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) {
  fnetdev::wire::DeviceImplInfo info = fnetdev::wire::DeviceImplInfo::Builder(arena)
                                           .tx_depth(kRequestPoolSize)
                                           .rx_depth(kRequestPoolSize)
                                           .rx_threshold(kRequestPoolSize / 2)
                                           .max_buffer_parts(1)
                                           .max_buffer_length(RNDIS_MAX_XFER_SIZE)
                                           .buffer_alignment(1)
                                           .min_rx_buffer_length(kMtu)
                                           .min_tx_buffer_length(0)
                                           .Build();

  completer.buffer(arena).Reply(info);
}

void RndisFunction::QueueTx(QueueTxRequestView request, fdf::Arena& arena,
                            QueueTxCompleter::Sync& completer) {
  std::array<fnetdev::wire::TxResult, kRequestPoolSize> results;
  auto results_iter = results.begin();
  std::array<frequest::wire::Request, kRequestPoolSize> reqs;
  auto reqs_iter = reqs.begin();

  bool offline = shutting_down_ || !Online();

  for (const auto& buffer : request->buffers) {
    if (offline) {
      *results_iter++ = {.id = buffer.id, .status = ZX_ERR_BAD_STATE};
      transmit_errors_++;
      continue;
    }
    if (buffer.data.size() != 1) {
      *results_iter++ = {.id = buffer.id, .status = ZX_ERR_INVALID_ARGS};
      transmit_errors_++;
      continue;
    }
    const auto& region = buffer.data[0];

    std::optional<usb::FidlRequest> tx_req = bulk_in_ep_.GetRequest();
    if (!tx_req.has_value()) {
      *results_iter++ = {.id = buffer.id, .status = ZX_ERR_NO_RESOURCES};
      transmit_no_buffer_++;
      continue;
    }
    auto return_request = fit::defer([&]() { bulk_in_ep_.PutRequest(std::move(tx_req.value())); });

    auto* stored_vmo = vmo_store_.GetVmo(region.vmo);
    if (!stored_vmo) {
      *results_iter++ = {.id = buffer.id, .status = ZX_ERR_INVALID_ARGS};
      transmit_errors_++;
      continue;
    }
    std::span<uint8_t> data = stored_vmo->data();
    if (region.length > data.size() || region.offset > data.size() - region.length) {
      *results_iter++ = {.id = buffer.id, .status = ZX_ERR_INVALID_ARGS};
      transmit_errors_++;
      continue;
    }

    rndis_packet_header header{};
    header.msg_type = RNDIS_PACKET_MSG;
    header.msg_length = static_cast<uint32_t>(sizeof(header) + region.length);
    header.data_offset = sizeof(header) - offsetof(rndis_packet_header, data_offset);
    header.data_length = static_cast<uint32_t>(region.length);

    tx_req->clear_buffers();
    size_t offset = 0;
    std::vector<size_t> copied =
        tx_req->CopyTo(0, &header, sizeof(header), bulk_in_ep_.GetMapped());
    for (size_t i = 0; i < copied.size(); i++) {
      tx_req.value()->data()->at(i).size(copied[i]);
      offset += copied[i];
    }
    std::vector<size_t> copied_payload =
        tx_req->CopyTo(offset, data.data() + region.offset, region.length, bulk_in_ep_.GetMapped());
    for (size_t i = 0; i < copied_payload.size(); i++) {
      auto& data_buf = tx_req.value()->data()->at(i);
      data_buf.size(data_buf.size().value() + copied_payload[i]);
    }
    if (zx_status_t status = tx_req->CacheFlush(bulk_in_ep_.GetMapped()); status != ZX_OK) {
      fdf::warn("failed to copy data and flush cache: {}", zx_status_get_string(status));
      *results_iter++ = {.id = buffer.id, .status = status};
      transmit_errors_++;
      continue;
    }

    return_request.cancel();
    tx_completion_queue_.push(buffer.id);
    *reqs_iter++ = fidl::ToWire(arena, tx_req->take_request());
    transmit_ok_++;
  }

  if (results_iter != results.begin()) {
    fidl::OneWayStatus status = netdevice_ifc_.buffer(arena)->CompleteTx(
        fidl::VectorView<fnetdev::wire::TxResult>::FromExternal(
            results.data(), std::distance(results.begin(), results_iter)));
    if (!status.ok()) {
      fdf::error("failed to complete tx: {}", status.FormatDescription());
    }
  }

  if (reqs_iter != reqs.begin()) {
    fidl::OneWayStatus queue_status = bulk_in_ep_.client().wire()->QueueRequests(
        fidl::VectorView<frequest::wire::Request>::FromExternal(
            reqs.data(), std::distance(reqs.begin(), reqs_iter)));

    if (!queue_status.ok()) {
      fdf::error("failed to queue tx requests: {}", queue_status.FormatDescription());
      for (auto it = reqs.begin(); it != reqs_iter; it++) {
        bulk_in_ep_.PutRequest(usb::FidlRequest(fidl::ToNatural(*it)));
      }
    }
  }
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
}

void RndisFunction::QueueRxSpace(QueueRxSpaceRequestView request, fdf::Arena& arena,
                                 QueueRxSpaceCompleter::Sync& completer) {
  for (const auto& buffer : request->buffers) {
    rx_space_buffers_.push(buffer);
  }
  FDF_ASSERT_MSG(rx_space_buffers_.size() <= kRequestPoolSize, "rx space buffers too large",
                 rx_space_buffers_.size());

  if (rx_completion_queue_.empty()) {
    return;
  }
  // Take over all pending completions and process them. We'll re-queue if
  // not enough space available.
  ProcessRxCompletions(std::move(rx_completion_queue_));
}

void RndisFunction::PrepareVmo(PrepareVmoRequestView request, fdf::Arena& arena,
                               PrepareVmoCompleter::Sync& completer) {
  zx_status_t status = vmo_store_.RegisterWithKey(request->id, std::move(request->vmo));
  completer.buffer(arena).Reply(status);
}

void RndisFunction::ReleaseVmo(ReleaseVmoRequestView request, fdf::Arena& arena,
                               ReleaseVmoCompleter::Sync& completer) {
  zx::result result = vmo_store_.Unregister(request->id);
  if (result.is_error()) {
    fdf::error("failed to unregister vmo {}: {}", request->id, result.status_string());
  }
  completer.buffer(arena).Reply();
}

void RndisFunction::NotifyComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& completion : completions) {
    notification_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
  }
  notification_inspect_.UpdateTxQueue(notification_ep_.GetInFlightCount());
  ContinueStop();
}

void RndisFunction::TxComplete(std::vector<fendpoint::Completion> completions) {
  std::array<fnetdev::wire::TxResult, kRequestPoolSize> results;
  auto results_iter = results.begin();

  for (auto& completion : completions) {
    zx_status_t status = *completion.status();
    usb::FidlRequest req(std::move(completion.request().value()));
    size_t size = req.length();
    if (status == ZX_OK) {
      bulk_in_inspect_.AddTxBytes(completion.transfer_size().value_or(0));
    } else {
      bulk_in_inspect_.AddFailedTxBytes(size);
    }
    bulk_in_ep_.PutRequest(std::move(req));
    if (tx_completion_queue_.empty()) {
      fdf::error("received tx completion without pending tx");
      continue;
    }
    uint32_t id = tx_completion_queue_.front();
    tx_completion_queue_.pop();

    *results_iter++ = {.id = id, .status = *completion.status()};
  }

  if (results_iter != results.begin()) {
    fdf::Arena arena(kArenaTag);
    fidl::OneWayStatus status = netdevice_ifc_.buffer(arena)->CompleteTx(
        fidl::VectorView<fnetdev::wire::TxResult>::FromExternal(
            results.data(), std::distance(results.begin(), results_iter)));
    if (!status.ok()) {
      fdf::error("failed to complete tx: {}", status.FormatDescription());
    }
  }
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
  ContinueStop();
}

void RndisFunction::RxComplete(std::vector<fendpoint::Completion> completions) {
  if (shutting_down_) {
    for (auto& completion : completions) {
      bulk_out_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
    }
    ContinueStop();
    return;
  }

  ProcessRxCompletions(std::move(completions));
}

void RndisFunction::ProcessRxCompletions(std::vector<fendpoint::Completion> completions) {
  fdf::Arena arena(kArenaTag);

  std::array<frequest::wire::Request, kRequestPoolSize> reqs;
  auto reqs_iter = reqs.begin();

  std::array<fnetdev::wire::RxBuffer, kRequestPoolSize> rx_buffers;
  auto rx_buffers_iter = rx_buffers.begin();

  std::array<fnetdev::wire::RxBufferPart, kRequestPoolSize> rx_buffers_parts;
  auto rx_buffers_parts_iter = rx_buffers_parts.begin();

  auto reset_and_enqueue = [&](usb::FidlRequest req) {
    req.reset_buffers(bulk_out_ep_.GetMapped());
    *reqs_iter++ = fidl::ToWire(arena, req.take_request());
  };

  for (auto& completion : completions) {
    zx_status_t status = *completion.status();
    if (status == ZX_ERR_IO_NOT_PRESENT) {
      bulk_out_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
      continue;
    }

    if (status != ZX_OK) {
      fdf::error("rx_completion: {}", zx_status_get_string(status));
      usb::FidlRequest req(std::move(completion.request().value()));
      bulk_out_inspect_.AddFailedRxBytes(req.length());
      reset_and_enqueue(std::move(req));
      continue;
    }

    if (rx_space_buffers_.empty()) {
      rx_completion_queue_.push_back(std::move(completion));
      continue;
    }

    bulk_out_inspect_.AddRxBytes(completion.transfer_size().value_or(0));

    usb::FidlRequest req(std::move(completion.request().value()));

    rndis_packet_header header;
    zx::result<std::vector<size_t>> copied =
        req.CachedCopyFrom(0, &header, sizeof(header), bulk_out_ep_.GetMapped());
    if (copied.is_error()) {
      reset_and_enqueue(std::move(req));
      fdf::error("failed to invalidate cache or retrieve header from request: {}",
                 copied.status_string());
      continue;
    }
    size_t header_copied = std::accumulate(copied->begin(), copied->end(), 0);
    if (header_copied < sizeof(header)) {
      reset_and_enqueue(std::move(req));
      fdf::error("failed to retrieve header from request: {} bytes copied, want {}", header_copied,
                 sizeof(header));
      continue;
    }

    if (header.msg_type != RNDIS_PACKET_MSG) {
      reset_and_enqueue(std::move(req));
      fdf::warn("unrecognized message type: {}", header.msg_type);
      continue;
    }

    fnetdev::wire::RxSpaceBuffer space = rx_space_buffers_.front();
    auto* stored_vmo = vmo_store_.GetVmo(space.region.vmo);
    if (!stored_vmo) {
      reset_and_enqueue(std::move(req));
      continue;
    }

    uint32_t data_offset = header.data_offset + offsetof(rndis_packet_header, data_offset);
    uint32_t data_length = header.data_length;

    if (data_length > space.region.length) {
      reset_and_enqueue(std::move(req));
      continue;
    }

    req.CopyFrom(data_offset,
                 reinterpret_cast<void*>(stored_vmo->data().data() + space.region.offset),
                 data_length, bulk_out_ep_.GetMapped());

    *rx_buffers_parts_iter = fnetdev::wire::RxBufferPart{
        .id = space.id,
        .offset = 0,
        .length = header.data_length,
    };
    *rx_buffers_iter++ = {
        .meta =
            {
                .port = kPortId,
                .frame_type = fuchsia_hardware_network::FrameType::kEthernet,
            },
        .data =
            fidl::VectorView<fnetdev::wire::RxBufferPart>::FromExternal(&*rx_buffers_parts_iter, 1),
    };

    rx_buffers_parts_iter++;
    rx_space_buffers_.pop();

    reset_and_enqueue(std::move(req));
  }

  if (rx_buffers_iter != rx_buffers.begin()) {
    fidl::OneWayStatus status = netdevice_ifc_.buffer(arena)->CompleteRx(
        fidl::VectorView<fnetdev::wire::RxBuffer>::FromExternal(
            rx_buffers.data(), std::distance(rx_buffers.begin(), rx_buffers_iter)));
    if (!status.ok()) {
      fdf::error("failed to complete rx: {}", status.FormatDescription());
    }
    receive_ok_ += static_cast<uint32_t>(std::distance(rx_buffers.begin(), rx_buffers_iter));
  }

  if (reqs_iter != reqs.begin()) {
    fidl::OneWayStatus queue_status = bulk_out_ep_.client().wire()->QueueRequests(
        fidl::VectorView<frequest::wire::Request>::FromExternal(
            reqs.data(), std::distance(reqs.begin(), reqs_iter)));
    if (!queue_status.ok()) {
      fdf::error("failed to queue rx requests: {}", queue_status.FormatDescription());
    }
  }
  bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
}

void RndisFunction::Notify() {
  std::optional<usb::FidlRequest> req = notification_ep_.GetRequest();
  if (!req) {
    fdf::error("No notify request available");
    return;
  }

  rndis_notification notification{
      .notification = htole32(1),
      .reserved = 0,
  };

  zx::result<std::vector<size_t>> copied =
      req->CachedCopyTo(0, &notification, sizeof(notification), notification_ep_.GetMapped());
  if (copied.is_error()) {
    fdf::error("Failed to copy notify request: {}", copied.status_string());
    notification_ep_.PutRequest(std::move(req.value()));
    return;
  }
  for (size_t i = 0; i < copied->size(); i++) {
    req.value()->data()->at(i).size((*copied)[i]);
  }

  fdf::Arena arena(kArenaTag);
  frequest::wire::Request req_wire = fidl::ToWire(arena, req->take_request());

  fidl::OneWayStatus queue_status = notification_ep_.client().wire()->QueueRequests(
      fidl::VectorView<frequest::wire::Request>::FromExternal(&req_wire, 1));

  if (!queue_status.ok()) {
    fdf::error("failed to queue notify requests: {}", queue_status.FormatDescription());
    notification_ep_.PutRequest(usb::FidlRequest(fidl::ToNatural(req_wire)));
  }
  notification_inspect_.UpdateTxQueue(notification_ep_.GetInFlightCount());
}

void RndisFunction::IndicateConnectionStatus(bool connected) {
  if (!rndis_ready_) {
    return;
  }

  rndis_indicate_status status;
  status.msg_type = RNDIS_INDICATE_STATUS_MSG;
  status.msg_length = static_cast<uint32_t>(sizeof(rndis_indicate_status));
  if (connected) {
    status.status = RNDIS_STATUS_MEDIA_CONNECT;
  } else {
    status.status = RNDIS_STATUS_MEDIA_DISCONNECT;
  }
  status.status_buffer_length = 0;
  status.status_buffer_offset = 0;

  std::vector<uint8_t> buffer(sizeof(rndis_indicate_status));
  memcpy(buffer.data(), &status, sizeof(rndis_indicate_status));

  control_responses_.push(std::move(buffer));
  Notify();
}

zx::result<> RndisFunction::Start(fdf::DriverContext context) {
  inspector_ = context.CreateInspector(this);
  zx::result func =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (func.is_error()) {
    fdf::error("Failed to connect to UsbFunctionService: {}", func);
    return func.take_error();
  }
  function_.Bind(std::move(*func));

  if (zx_status_t status = vmo_store_.Reserve(fuchsia_hardware_network::wire::kMaxDataVmos);
      status != ZX_OK) {
    fdf::error("failed to initialize vmo store: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  zx::result intr_ep_res = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (intr_ep_res.is_error()) {
    return intr_ep_res.take_error();
  }

  zx::result in_ep_res = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (in_ep_res.is_error()) {
    return in_ep_res.take_error();
  }

  zx::result out_ep_res = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (out_ep_res.is_error()) {
    return out_ep_res.take_error();
  }

  std::vector<fuchsia_hardware_usb_function::EndpointResource> resources;
  resources.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fdescriptor::EndpointDirection::kIn, std::move(intr_ep_res->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithInterrupt({}), kNotificationMaxPacketSize));

  resources.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fdescriptor::EndpointDirection::kIn, std::move(in_ep_res->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), 512));

  resources.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fdescriptor::EndpointDirection::kOut, std::move(out_ep_res->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), 512));

  fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources> alloc_req;
  alloc_req.interface_count(2);
  alloc_req.endpoints(std::move(resources));
  alloc_req.strings({
      "RNDIS Communications Control",
      "RNDIS Ethernet Data",
      "RNDIS",
  });

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    fdf::error("AllocResources failed: {}", alloc_result.error_value().FormatDescription());
    return zx::error(alloc_result.error_value().is_framework_error()
                         ? alloc_result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  auto& response = alloc_result.value();
  uint8_t comm_intf_num = response.interface_nums()[0];
  uint8_t data_intf_num = response.interface_nums()[1];

  uint8_t notification_addr = response.endpoint_addrs()[0];
  uint8_t bulk_in_addr = response.endpoint_addrs()[1];
  uint8_t bulk_out_addr = response.endpoint_addrs()[2];

  // Initialize Descriptors
  descriptors_.assoc = usb_interface_assoc_descriptor_t{
      .b_length = sizeof(usb_interface_assoc_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE_ASSOCIATION,
      .b_first_interface = comm_intf_num,
      .b_interface_count = 2,
      .b_function_class = USB_CLASS_WIRELESS,
      .b_function_sub_class = USB_SUBCLASS_WIRELESS_MISC,
      .b_function_protocol = USB_PROTOCOL_WIRELESS_MISC_RNDIS,
      .i_function = response.string_indices()[2],
  };
  descriptors_.communication_interface = usb_interface_descriptor_t{
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = comm_intf_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 1,
      .b_interface_class = USB_CLASS_WIRELESS,
      .b_interface_sub_class = USB_SUBCLASS_WIRELESS_MISC,
      .b_interface_protocol = USB_PROTOCOL_WIRELESS_MISC_RNDIS,
      .i_interface = response.string_indices()[0],
  };
  descriptors_.cdc_header = usb_cs_header_interface_descriptor_t{
      .bLength = sizeof(usb_cs_header_interface_descriptor_t),
      .bDescriptorType = USB_DT_CS_INTERFACE,
      .bDescriptorSubType = USB_CDC_DST_HEADER,
      .bcdCDC = htole16(0x0110),
  };
  descriptors_.call_mgmt = usb_cs_call_mgmt_interface_descriptor_t{
      .bLength = sizeof(usb_cs_call_mgmt_interface_descriptor_t),
      .bDescriptorType = USB_DT_CS_INTERFACE,
      .bDescriptorSubType = USB_CDC_DST_CALL_MGMT,
      .bmCapabilities = 0x00,
      .bDataInterface = data_intf_num,
  };
  descriptors_.acm = usb_cs_abstract_ctrl_mgmt_interface_descriptor_t{
      .bLength = sizeof(usb_cs_abstract_ctrl_mgmt_interface_descriptor_t),
      .bDescriptorType = USB_DT_CS_INTERFACE,
      .bDescriptorSubType = USB_CDC_DST_ABSTRACT_CTRL_MGMT,
      .bmCapabilities = 0,
  };
  descriptors_.cdc_union = usb_cs_union_interface_descriptor_1_t{
      .bLength = sizeof(usb_cs_union_interface_descriptor_1_t),
      .bDescriptorType = USB_DT_CS_INTERFACE,
      .bDescriptorSubType = USB_CDC_DST_UNION,
      .bControlInterface = comm_intf_num,
      .bSubordinateInterface = data_intf_num,
  };
  descriptors_.notification_ep = usb_endpoint_descriptor_t{
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = notification_addr,
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
      .w_max_packet_size = htole16(kNotificationMaxPacketSize),
      .b_interval = 1,
  };
  descriptors_.data_interface = usb_interface_descriptor_t{
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = data_intf_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 2,
      .b_interface_class = USB_CLASS_CDC,
      .b_interface_sub_class = 0,
      .b_interface_protocol = 0,
      .i_interface = response.string_indices()[1],
  };
  descriptors_.in_ep = usb_endpoint_descriptor_t{
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = bulk_in_addr,
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = htole16(512),
      .b_interval = 0,
  };
  descriptors_.out_ep = usb_endpoint_descriptor_t{
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = bulk_out_addr,
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = htole16(512),
      .b_interval = 0,
  };

  // Get MAC address
  zx::result metadata_result =
      fdf_metadata::GetMetadataIfExists<fuchsia_boot_metadata::MacAddressMetadata>(
          context.incoming());
  if (metadata_result.is_error()) {
    fdf::error("Failed to get MAC address metadata: {}", metadata_result);
    return metadata_result.take_error();
  }
  if (metadata_result.value().has_value()) {
    const auto& metadata = metadata_result.value().value();
    if (!metadata.mac_address().has_value()) {
      fdf::error("MAC address metadata missing mac_address field");
      return zx::error(ZX_ERR_INTERNAL);
    }
    mac_addr_ = metadata.mac_address().value().octets();
  } else {
    fdf::info("Generating random address: Ethernet MAC metadata not found");
    zx_cprng_draw(mac_addr_.data(), mac_addr_.size());
    mac_addr_[0] = 0x02;
  }
  fdf::info("MAC address: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", mac_addr_[0], mac_addr_[1],
            mac_addr_[2], mac_addr_[3], mac_addr_[4], mac_addr_[5]);

  // Init endpoint clients
  zx_status_t status = notification_ep_.Init(std::move(intr_ep_res->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("Could not init intr endpoint client: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  if (notification_ep_.AddRequests(kRequestPoolSize, kNotificationMaxPacketSize,
                                   fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) !=
      kRequestPoolSize) {
    fdf::error("Failed to allocate intr requests");
    return zx::error(ZX_ERR_INTERNAL);
  }

  status = bulk_in_ep_.Init(std::move(in_ep_res->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("Could not init in endpoint client: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  if (bulk_in_ep_.AddRequests(kRequestPoolSize, RNDIS_MAX_XFER_SIZE,
                              fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) !=
      kRequestPoolSize) {
    fdf::error("Failed to allocate in requests");
    return zx::error(ZX_ERR_INTERNAL);
  }

  status = bulk_out_ep_.Init(std::move(out_ep_res->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("Could not init out endpoint client: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  if (bulk_out_ep_.AddRequests(kRequestPoolSize, RNDIS_MAX_XFER_SIZE,
                               fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) !=
      kRequestPoolSize) {
    fdf::error("Failed to allocate out requests");
    return zx::error(ZX_ERR_INTERNAL);
  }

  auto protocol = [this](fdf::ServerEnd<fnetdev::NetworkDeviceImpl> server_end) mutable {
    fdf::BindServer(driver_dispatcher()->get(), std::move(server_end), this);
  };
  fnetdev::Service::InstanceHandler handler({.network_device_impl = std::move(protocol)});

  if (zx::result status = outgoing()->AddService<fnetdev::Service>(std::move(handler));
      status.is_error()) {
    fdf::error("Failed to add service: {}", status);
    return status.take_error();
  }

  inspect_node_ = inspector().root().CreateChild(name());
  inspect_node_.RecordString("mac_address", std::format("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                                                        mac_addr_[0], mac_addr_[1], mac_addr_[2],
                                                        mac_addr_[3], mac_addr_[4], mac_addr_[5]));

  bulk_in_inspect_.Init(inspect_node_, "bulk_in");
  bulk_out_inspect_.Init(inspect_node_, "bulk_out");
  notification_inspect_.Init(inspect_node_, "notification");

  throughput_tracker_.emplace(
      dispatcher(),
      [this](zx::duration delta) {
        bulk_in_inspect_.MeasureThroughput(delta);
        bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());

        bulk_out_inspect_.MeasureThroughput(delta);
        bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());

        notification_inspect_.MeasureThroughput(delta);
        notification_inspect_.UpdateTxQueue(notification_ep_.GetInFlightCount());
      },
      zx::sec(1));
  throughput_tracker_->Start();

  std::vector<fuchsia_driver_framework::Offer> offers;
  offers.push_back(fdf::MakeOffer2<fnetdev::Service>());

  zx::result child =
      AddChild(kChildNodeName, std::vector<fuchsia_driver_framework::NodeProperty2>{}, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  zx::result iface_endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_usb_function::UsbFunctionInterface>();
  if (iface_endpoints.is_error()) {
    return iface_endpoints.take_error();
  }
  fidl::BindServer(dispatcher(), std::move(iface_endpoints->server), this);

  std::vector<uint8_t> descriptors_buffer(sizeof(descriptors_));
  memcpy(descriptors_buffer.data(), &descriptors_, sizeof(descriptors_));

  fidl::Request<fuchsia_hardware_usb_function::UsbFunction::Configure> config_req;
  config_req.configuration(std::move(descriptors_buffer));
  config_req.iface(std::move(iface_endpoints->client));

  fidl::Result config_res = function_->Configure(std::move(config_req));
  if (config_res.is_error()) {
    fdf::error("Configure failed: {}", config_res.error_value().FormatDescription());
    return zx::error(config_res.error_value().is_framework_error()
                         ? config_res.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

void RndisFunction::GetInfo(
    fdf::Arena& arena, fdf::WireServer<fnetdev::NetworkPort>::GetInfoCompleter::Sync& completer) {
  static constexpr fuchsia_hardware_network::wire::FrameType kRxTypes[] = {
      fuchsia_hardware_network::wire::FrameType::kEthernet};
  static constexpr fuchsia_hardware_network::wire::FrameTypeSupport kTxTypes[] = {{
      .type = fuchsia_hardware_network::wire::FrameType::kEthernet,
      .features = fuchsia_hardware_network::wire::kFrameFeaturesRaw,
  }};

  fuchsia_hardware_network::wire::PortBaseInfo info =
      fuchsia_hardware_network::wire::PortBaseInfo::Builder(arena)
          .port_class(fuchsia_hardware_network::wire::PortClass::kEthernet)
          .rx_types(fidl::VectorView<fuchsia_hardware_network::wire::FrameType>::FromExternal(
              const_cast<fuchsia_hardware_network::wire::FrameType*>(kRxTypes), 1))
          .tx_types(
              fidl::VectorView<fuchsia_hardware_network::wire::FrameTypeSupport>::FromExternal(
                  const_cast<fuchsia_hardware_network::wire::FrameTypeSupport*>(kTxTypes), 1))
          .Build();

  completer.buffer(arena).Reply(info);
}

void RndisFunction::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  completer.buffer(arena).Reply(fidl::ToWire(arena, ReadStatus()));
}

void RndisFunction::SetActive(SetActiveRequestView request, fdf::Arena& arena,
                              SetActiveCompleter::Sync& completer) {
  FDF_ASSERT(!completer.is_reply_needed());
}

void RndisFunction::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  auto [client, server] = fdf::Endpoints<fnetdev::MacAddr>::Create();
  fdf::BindServer(fdf::Dispatcher::GetCurrent()->get(), std::move(server), this);
  completer.buffer(arena).Reply(std::move(client));
}

void RndisFunction::Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) {}

void RndisFunction::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  fuchsia_net::wire::MacAddress mac;
  static_assert(sizeof(mac.octets) == sizeof(mac_addr_), "MAC address size mismatch");
  memcpy(mac.octets.data(), mac_addr_.data(), sizeof(mac_addr_));
  completer.buffer(arena).Reply(mac);
}

void RndisFunction::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  fnetdev::wire::Features features =
      fnetdev::wire::Features::Builder(arena)
          .multicast_filter_count(0)
          .supported_modes(fnetdev::wire::SupportedMacFilterMode::kPromiscuous)
          .Build();
  completer.buffer(arena).Reply(features);
}

void RndisFunction::SetMode(SetModeRequestView request, fdf::Arena& arena,
                            SetModeCompleter::Sync& completer) {
  completer.buffer(arena).Reply();
}

fuchsia_hardware_network::PortStatus RndisFunction::ReadStatus() const {
  fuchsia_hardware_network::PortStatus status;
  status.mtu(kMtu);
  fuchsia_hardware_network::StatusFlags flags;
  if (Online()) {
    flags |= fuchsia_hardware_network::StatusFlags::kOnline;
  }
  status.flags(flags);
  return status;
}

void RndisFunction::UpdatePortStatus() {
  if (!netdevice_ifc_.is_valid()) {
    return;
  }
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus status =
      netdevice_ifc_.buffer(arena)->PortStatusChanged(kPortId, fidl::ToWire(arena, ReadStatus()));
  if (!status.ok()) {
    fdf::error("Failed to update port status: {}", status.FormatDescription());
  }
}

void RndisFunction::ContinueStop() {
  if (!shutting_down_ || !stop_completer_.has_value()) {
    return;
  }

  for (auto& ep_info : GetEndpoints()) {
    if (!ep_info.ep.RequestsFull()) {
      fdf::info("waiting for {} requests to complete", ep_info.name);
      return;
    }
    ep_info.ep.Close();
  }

  auto completer = std::move(stop_completer_.value());
  stop_completer_.reset();
  completer(zx::ok());
}

void RndisFunction::Stop(fdf::StopCompleter completer) {
  if (throughput_tracker_) {
    throughput_tracker_->Stop();
  }
  shutting_down_ = true;
  stop_completer_.emplace(std::move(completer));

  for (auto& c : rx_completion_queue_) {
    bulk_out_ep_.PutRequest(usb::FidlRequest(std::move(c.request().value())));
  }
  rx_completion_queue_.clear();

  DiscardPendingTxBuffers(ZX_ERR_CANCELED);
  ReturnPendingRxSpace();
  CancelAllRequests();
  ContinueStop();
}

void RndisFunction::CancelAllRequests() {
  for (auto& ep_info : GetEndpoints()) {
    // No need to cancel, nothing in flight.
    if (ep_info.ep.RequestsFull()) {
      continue;
    }
    fidl::WireResult result = ep_info.ep.client().wire_sync()->CancelAll();
    if (!result.ok()) {
      fdf::error("Failed to cancel {} requests: {}", ep_info.name,
                 result.error().FormatDescription());
    } else if (!result.value().is_ok()) {
      fdf::error("Failed to cancel {} requests: {}", ep_info.name,
                 zx_status_get_string(result.value().error_value()));
    }
  }
}

void RndisFunction::DiscardPendingTxBuffers(zx_status_t status) {
  std::array<fnetdev::wire::TxResult, kRequestPoolSize> results;
  auto results_iter = results.begin();
  while (!tx_completion_queue_.empty()) {
    uint32_t id = tx_completion_queue_.front();
    *results_iter++ = {.id = id, .status = status};
    tx_completion_queue_.pop();
  }
  if (results_iter == results.begin() || !netdevice_ifc_.is_valid()) {
    return;
  }
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus fidl_status = netdevice_ifc_.buffer(arena)->CompleteTx(
      fidl::VectorView<fnetdev::wire::TxResult>::FromExternal(
          results.data(), std::distance(results.begin(), results_iter)));
  if (!fidl_status.ok()) {
    fdf::error("Failed to complete tx: {}", fidl_status.FormatDescription());
  }
}

void RndisFunction::ReturnPendingRxSpace() {
  fdf::Arena arena(kArenaTag);

  std::array<fnetdev::wire::RxBuffer, kRequestPoolSize> rx_buffers;
  auto rx_buffers_iter = rx_buffers.begin();

  std::array<fnetdev::wire::RxBufferPart, kRequestPoolSize> rx_buffers_parts;
  auto rx_buffers_parts_iter = rx_buffers_parts.begin();

  while (!rx_space_buffers_.empty()) {
    *rx_buffers_parts_iter = {
        .id = rx_space_buffers_.front().id,
        .offset = 0,
        .length = 0,
    };
    rx_space_buffers_.pop();
    *rx_buffers_iter++ = {
        .meta =
            {
                .port = kPortId,
                .frame_type = fuchsia_hardware_network::FrameType::kEthernet,
            },
        .data =
            fidl::VectorView<fnetdev::wire::RxBufferPart>::FromExternal(&*rx_buffers_parts_iter, 1),
    };
    rx_buffers_parts_iter++;
  }

  if (rx_buffers_iter == rx_buffers.begin() || !netdevice_ifc_.is_valid()) {
    return;
  }

  fidl::OneWayStatus fidl_status = netdevice_ifc_.buffer(arena)->CompleteRx(
      fidl::VectorView<fnetdev::wire::RxBuffer>::FromExternal(
          rx_buffers.data(), std::distance(rx_buffers.begin(), rx_buffers_iter)));
  if (!fidl_status.ok()) {
    fdf::error("Failed to complete rx: {}", fidl_status.error());
  }
}

FUCHSIA_DRIVER_EXPORT2(RndisFunction);
