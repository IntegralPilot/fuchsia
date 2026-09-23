// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vsock_usb.h"

#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstdint>
#include <iterator>
#include <optional>
#include <variant>

#include <fbl/auto_lock.h>
#include <usb/request-cpp.h>

#include "fidl/fuchsia.hardware.vsockbridge/cpp/wire_types.h"
#include "lib/async/cpp/wait.h"
#include "lib/fidl/cpp/wire/channel.h"
#include "lib/fidl/cpp/wire/internal/transport.h"

namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace ffunction = fuchsia_hardware_usb_function;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

zx::result<> VsockUsb::Start(fdf::DriverContext context) {
  inspector_ = context.CreateInspector(this);
  auto client = context.incoming().Connect<ffunction::UsbFunctionService::Device>();
  if (client.is_error()) {
    FDF_SLOG(ERROR, "Failed to connect fidl protocol",
             KV("status", zx_status_get_string(client.error_value())));
    return zx::error(client.error_value());
  }
  function_.Bind(std::move(*client));

  zx::result bulk_out_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_out_endpoints.is_error()) {
    return bulk_out_endpoints.take_error();
  }
  zx::result bulk_in_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_in_endpoints.is_error()) {
    return bulk_in_endpoints.take_error();
  }

  std::vector<ffunction::EndpointResource> resources;
  resources.push_back(ffunction::EndpointResource(
      fdescriptor::EndpointDirection::kOut, std::move(bulk_out_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), kMaxPacketSize));

  resources.push_back(ffunction::EndpointResource(
      fdescriptor::EndpointDirection::kIn, std::move(bulk_in_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), kMaxPacketSize));

  fidl::Request<ffunction::UsbFunction::AllocResources> alloc_req;
  alloc_req.interface_count(1);
  alloc_req.endpoints(std::move(resources));
  alloc_req.strings({{"Vsock USB interface"}});

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    FDF_SLOG(ERROR, "Failed to allocate resources",
             KV("status", alloc_result.error_value().FormatDescription()));
    return zx::error(alloc_result.error_value().is_framework_error()
                         ? alloc_result.error_value().framework_error().status()
                         : alloc_result.error_value().domain_error());
  }

  auto& response = alloc_result.value();
  descriptors_.data_interface.b_interface_number = response.interface_nums()[0];
  descriptors_.out_ep.b_endpoint_address = response.endpoint_addrs()[0];
  descriptors_.in_ep.b_endpoint_address = response.endpoint_addrs()[1];
  descriptors_.data_interface.i_interface = response.string_indices()[0];
  FDF_LOG(DEBUG, "Out endpoint address %d", descriptors_.out_ep.b_endpoint_address);
  FDF_LOG(DEBUG, "In endpoint address %d", descriptors_.in_ep.b_endpoint_address);

  zx_status_t status = bulk_out_ep_.Init(std::move(bulk_out_endpoints->client), dispatcher());
  if (status != ZX_OK) {
    FDF_SLOG(ERROR, "Failed to init UsbEndpoint", KV("endpoint", "out"),
             KV("status", zx_status_get_string(status)));
    return zx::error(status);
  }

  status = bulk_in_ep_.Init(std::move(bulk_in_endpoints->client), dispatcher());
  if (status != ZX_OK) {
    FDF_SLOG(ERROR, "Failed to init UsbEndpoint", KV("endpoint", "in"),
             KV("status", zx_status_get_string(status)));
    return zx::error(status);
  }

  auto actual = bulk_in_ep_.AddRequests(kRequestPoolSize, kMtu,
                                        fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual != kRequestPoolSize) {
    FDF_SLOG(ERROR, "Could not allocate all requests for IN endpoint",
             KV("wanted", kRequestPoolSize), KV("got", actual));
  }
  actual = bulk_out_ep_.AddRequests(kRequestPoolSize, kMtu,
                                    fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual != kRequestPoolSize) {
    FDF_SLOG(ERROR, "Could not allocate all requests for OUT endpoint",
             KV("wanted", kRequestPoolSize), KV("got", actual));
  }

  zx::result iface_endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_usb_function::UsbFunctionInterface>();
  if (iface_endpoints.is_error()) {
    return iface_endpoints.take_error();
  }

  const uint8_t* desc_ptr = reinterpret_cast<const uint8_t*>(&descriptors_);
  std::vector<uint8_t> desc_vec(desc_ptr, desc_ptr + sizeof(descriptors_));

  auto config_result =
      function_->Configure({std::move(desc_vec), std::move(iface_endpoints->client)});
  if (config_result.is_error()) {
    FDF_SLOG(ERROR, "Failed to configure",
             KV("status", config_result.error_value().FormatDescription()));
    return zx::error(ZX_ERR_INTERNAL);
  }

  fidl::BindServer(dispatcher(), std::move(iface_endpoints->server), this);

  fuchsia_hardware_vsockbridge::UsbService::InstanceHandler handler({
      .device = fit::bind_member<&VsockUsb::FidlConnect>(this),
  });

  auto service_result =
      outgoing()->AddService<fuchsia_hardware_vsockbridge::UsbService>(std::move(handler));
  if (service_result.is_error()) {
    FDF_LOG(ERROR, "Failed to add service: %s", service_result.status_string());
    return service_result.take_error();
  }

  inspect_node_ = inspector().root().CreateChild("vsock-usb");
  online_property_ = inspect_node_.CreateBool("online", false);
  state_property_ = inspect_node_.CreateString("state", "Unconfigured");
  bulk_in_inspect_.Init(inspect_node_, "bulk_in");
  bulk_out_inspect_.Init(inspect_node_, "bulk_out");
  throughput_tracker_.emplace(dispatcher(), [this](zx::duration delta) {
    bulk_in_inspect_.MeasureThroughput(delta);
    bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
    bulk_out_inspect_.MeasureThroughput(delta);
    bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
  });
  throughput_tracker_->Start();

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
  zx::result child_result =
      AddChild("vsock-usb", properties,
               std::array{fdf::MakeOffer2<fuchsia_hardware_vsockbridge::UsbService>()});
  if (child_result.is_error()) {
    FDF_SLOG(ERROR, "Could not add child node");
    return child_result.take_error();
  }
  node_controller_.Bind(std::move(child_result.value()));

  return zx::ok();
}

void VsockUsb::FidlConnect(fidl::ServerEnd<fuchsia_hardware_vsockbridge::Usb> request) {
  device_binding_group_.AddBinding(dispatcher_, std::move(request), this,
                                   fidl::kIgnoreBindingClosure);
}

void VsockUsb::Stop(fdf::StopCompleter completer) {
  Shutdown([completer = std::move(completer)]() mutable { completer(zx::ok()); });
}

void VsockUsb::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  auto setup = request.setup();
  uint16_t w_value = setup.w_value();
  uint16_t w_index = setup.w_index();
  uint16_t w_length = setup.w_length();

  FDF_LOG(
      DEBUG,
      "Control: bmRequestType=%02x bRequest=%02x wValue=%04x (%d) wIndex=%04x (%d) wLength=%04x (%d)",
      setup.bm_request_type(), setup.b_request(), w_value, w_value, w_index, w_index, w_length,
      w_length);

  if (setup.bm_request_type() == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT) &&
      setup.b_request() == USB_REQ_CLEAR_FEATURE && setup.w_value() == USB_ENDPOINT_HALT) {
    FDF_LOG(INFO, "clearing endpoint-halt");
    completer.Reply(zx::ok(std::vector<uint8_t>{}));
    return;
  }

  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

zx_status_t VsockUsb::ConfigureEndpoints() {
  // Reject configuration requests if the driver is in the process of shutting down.
  if (std::holds_alternative<ShuttingDown>(state_)) {
    return ZX_ERR_BAD_STATE;
  }

  if (endpoints_configured_) {
    FDF_LOG(DEBUG, "ConfigureEndpoints: endpoints already configured");
    return ZX_OK;
  }

  for (auto* ep_desc : {&descriptors_.in_ep, &descriptors_.out_ep}) {
    fuchsia_hardware_usb_function::EndpointConfiguration ep_config;
    fuchsia_hardware_usb_function::EndpointDescriptor desc;
    desc.b_interval(ep_desc->b_interval);
    desc.bm_attributes(ep_desc->bm_attributes);
    desc.w_max_packet_size(le16toh(ep_desc->w_max_packet_size));
    ep_config.descriptor(std::move(desc));
    fidl::Result result =
        function_->ConfigureEndpoint({ep_desc->b_endpoint_address, std::move(ep_config)});
    if (result.is_error()) {
      FDF_SLOG(ERROR, "ConfigureEndpoint failed",
               KV("status", result.error_value().FormatDescription()));
      return result.error_value().is_framework_error()
                 ? result.error_value().framework_error().status()
                 : result.error_value().domain_error();
    }
  }

  FDF_LOG(TRACE, "Setting state to Running");
  zx::socket socket;
  peer_socket_ = zx::socket();

  zx_status_t status = zx::socket::create(ZX_SOCKET_DATAGRAM, &socket, &peer_socket_.value());
  if (status != ZX_OK) {
    // There are two errors that can happen here: a kernel out of memory condition which the docs
    // say we shouldn't try to handle, and invalid arguments, which should be impossible.
    FDF_SLOG(FATAL, "Failed to create socket", KV("status", zx_status_get_string(status)));
  }
  state_ = Running(std::move(socket), this);
  SyncInspectState();
  HandleSocketAvailable();
  ProcessReadsFromSocket();

  endpoints_configured_ = true;

  std::vector<fuchsia_hardware_usb_request::Request> requests;
  while (auto req = bulk_out_ep_.GetRequest()) {
    req->reset_buffers(bulk_out_ep_.GetMapped());
    zx_status_t status = req->CacheFlushInvalidate(bulk_out_ep_.GetMapped());
    if (status != ZX_OK) {
      FDF_SLOG(ERROR, "Cache flush failed", KV("status", zx_status_get_string(status)));
    }

    requests.emplace_back(req->take_request());
  }
  FDF_SLOG(TRACE, "Queueing read requests", KV("count", requests.size()));
  auto result = bulk_out_ep_->QueueRequests(std::move(requests));
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to QueueRequests",
             KV("status", result.error_value().FormatDescription()));
    // Note: `requests` was moved into QueueRequests and consumed by the FIDL transport.
    // If QueueRequests fails, these requests cannot be reclaimed at this layer.
    return result.error_value().status();
  }

  return ZX_OK;
}

zx_status_t VsockUsb::UnconfigureEndpoints() {
  // Defense-in-depth: If the driver is shutting down, unconfiguration is already
  // in progress and endpoints are being torn down. Return ZX_OK without
  // clobbering the ShuttingDown state (which holds completion callbacks). While
  // SetConfigured() already checks for ShuttingDown before calling here, this
  // guard ensures direct or re-entrant invocations safely no-op during teardown.
  if (std::holds_alternative<ShuttingDown>(state_)) {
    return ZX_OK;
  }

  if (!endpoints_configured_) {
    FDF_LOG(DEBUG, "UnconfigureEndpoints: Endpoints already unconfigured");
    return ZX_OK;
  }

  endpoints_configured_ = false;

  FDF_LOG(TRACE, "UnconfigureEndpoints: Setting endpoint state to unconfigured");
  state_ = Unconfigured();
  SyncInspectState();

  CancelAllEndpoints();

  for (const uint8_t ep_addr : {BulkInAddress(), BulkOutAddress()}) {
    fidl::Result result = function_->DisableEndpoint({ep_addr});
    if (result.is_error()) {
      FDF_SLOG(ERROR, "DisableEndpoint failed", KV("endpoint", static_cast<uint32_t>(ep_addr)),
               KV("status", result.error_value().FormatDescription()));
      return result.error_value().is_framework_error()
                 ? result.error_value().framework_error().status()
                 : result.error_value().domain_error();
    }
  }
  return ZX_OK;
}

void VsockUsb::CancelAllEndpoints() {
  if (bulk_out_ep_.client().is_valid()) {
    bulk_out_ep_->CancelAll().Then([](fidl::Result<fendpoint::Endpoint::CancelAll>& result) {
      if (result.is_error()) {
        FDF_LOG(WARNING, "Failed to cancel all for bulk out endpoint: %s",
                result.error_value().FormatDescription().c_str());
      }
    });
  }
  if (bulk_in_ep_.client().is_valid()) {
    bulk_in_ep_->CancelAll().Then([](fidl::Result<fendpoint::Endpoint::CancelAll>& result) {
      if (result.is_error()) {
        FDF_LOG(WARNING, "Failed to cancel all for bulk in endpoint: %s",
                result.error_value().FormatDescription().c_str());
      }
    });
  }
}

void VsockUsb::SetConfigured(SetConfiguredRequest& request,
                             SetConfiguredCompleter::Sync& completer) {
  bool configured = request.configured();
  if (std::holds_alternative<ShuttingDown>(state_)) {
    // We're already shutting down, so we can't configure the endpoints, no-op
    // succeed.
    fdf::warn("Received configured = {}, but we're shutting down", configured);
    completer.Reply(zx::ok());
    return;
  }

  fuchsia_hardware_usb_descriptor::UsbSpeed speed = request.speed();
  FDF_LOG(TRACE, "SetConfigured(%d, %d)", configured, static_cast<uint32_t>(speed));
  zx_status_t status = configured ? ConfigureEndpoints() : UnconfigureEndpoints();
  completer.Reply(zx::make_result(status));
}

void VsockUsb::SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) {
  if (!endpoints_configured_) {
    FDF_LOG(WARNING, "SetInterface called while endpoints are not configured");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (request.interface() != descriptors_.data_interface.b_interface_number ||
      request.alt_setting() != descriptors_.data_interface.b_alternate_setting) {
    FDF_LOG(WARNING,
            "SetInterface unexpected parameters: got (interface=%u, alt=%u), expected (%u, %u)",
            request.interface(), request.alt_setting(),
            descriptors_.data_interface.b_interface_number,
            descriptors_.data_interface.b_alternate_setting);
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }
  completer.Reply(zx::ok());
}

void VsockUsb::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FDF_LOG(ERROR, "Unknown method ordinal: %lu", metadata.method_ordinal);
}

std::optional<usb::FidlRequest> VsockUsb::PrepareTx() {
  if (!Online()) {
    return std::nullopt;
  }

  auto request = bulk_in_ep_.GetRequest();
  if (!request) {
    FDF_SLOG(DEBUG, "No available TX requests");
    return std::nullopt;
  }
  request->clear_buffers();

  return request;
}

void VsockUsb::HandleSocketReadable(async_dispatcher_t*, async::WaitBase*, zx_status_t status,
                                    const zx_packet_signal_t*) {
  FDF_LOG(TRACE, "HandleSocketReadable(..., %d, ...)", status);
  if (status != ZX_OK) {
    if (status != ZX_ERR_CANCELED) {
      FDF_SLOG(WARNING, "Unexpected error waiting on socket",
               KV("status", zx_status_get_string(status)));
    }

    return;
  }

  auto request = PrepareTx();

  if (!request) {
    return;
  }

  // This should always be true because when we registered VMOs, we only registered one per
  // request.
  ZX_ASSERT((*request)->data()->size() == 1);

  std::optional<zx_vaddr_t> addr = bulk_in_ep_.GetMappedAddr(request->request(), 0);

  if (!addr.has_value()) {
    FDF_LOG(ERROR, "Failed to map request");
    ZX_ASSERT(!bulk_in_ep_.RequestsFull());
    bulk_in_ep_.PutRequest(std::move(*request));
    bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
    return;
  }

  size_t actual;

  const size_t old_index = state_.index();
  std::visit(
      [this, &addr, &actual, &status](auto&& state) {
        state_ = std::forward<decltype(state)>(state).SendData(reinterpret_cast<uint8_t*>(*addr),
                                                               kMtu, &actual, &status);
      },
      std::move(state_));
  if (state_.index() != old_index) {
    SyncInspectState();
  }

  if (status == ZX_OK) {
    (*request)->data()->at(0).size(actual);
    (*request)->short_(actual < kMtu);
    status = request->CacheFlush(bulk_in_ep_.GetMapped());
    if (status != ZX_OK) {
      FDF_SLOG(ERROR, "Cache flush failed", KV("status", zx_status_get_string(status)));
      ZX_ASSERT(!bulk_in_ep_.RequestsFull());
      bulk_in_ep_.PutRequest(std::move(*request));
    } else {
      std::vector<fuchsia_hardware_usb_request::Request> requests;
      requests.emplace_back(request->take_request());
      FDF_LOG(DEBUG, "Queuing write request (data)");
      auto result = bulk_in_ep_->QueueRequests(std::move(requests));
      if (result.is_error()) {
        FDF_SLOG(ERROR, "Failed to QueueRequests",
                 KV("status", result.error_value().FormatDescription()));
        // Note: `requests` was moved into QueueRequests and consumed by the FIDL transport.
      }
    }
  } else {
    FDF_LOG(WARNING, "SendData failed, returning request to pool");
    ZX_ASSERT(!bulk_in_ep_.RequestsFull());
    bulk_in_ep_.PutRequest(std::move(*request));
  }

  std::visit(
      [this](auto& state) {
        if (state.ReadsWaiting()) {
          ProcessReadsFromSocket();
        }
      },
      state_);
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
}

VsockUsb::State VsockUsb::Running::SendData(uint8_t* data, size_t len, size_t* actual,
                                            zx_status_t* status) && {
  *status = socket_.read(0, data, len, actual);

  if (*status != ZX_OK && *status != ZX_ERR_SHOULD_WAIT) {
    if (*status != ZX_ERR_PEER_CLOSED) {
      FDF_SLOG(ERROR, "Failed to read from socket", KV("status", zx_status_get_string(*status)));
    }
    FDF_LOG(INFO, "Client socket closed, returning to ready state");
    return Unconfigured();
  }

  return std::move(*this);
}

void VsockUsb::HandleSocketWritable(async_dispatcher_t*, async::WaitBase*, zx_status_t status,
                                    const zx_packet_signal_t*) {
  FDF_LOG(TRACE, "HandleSocketWritable(..., %d, ...)", status);

  if (status != ZX_OK) {
    if (status != ZX_ERR_CANCELED) {
      FDF_SLOG(WARNING, "Unexpected error waiting on socket",
               KV("status", zx_status_get_string(status)));
    }

    return;
  }

  const size_t old_index = state_.index();
  std::visit([this](auto&& state) { state_ = std::forward<decltype(state)>(state).Writable(); },
             std::move(state_));
  if (state_.index() != old_index) {
    SyncInspectState();
  }
  std::visit(
      [this](auto& state) {
        if (state.WritesWaiting()) {
          ProcessWritesToSocket();
        }
      },
      state_);
}

VsockUsb::State VsockUsb::Running::Writable() && {
  while (!socket_out_queue_.empty()) {
    const auto& packet = socket_out_queue_.front();
    size_t actual;
    zx_status_t status = socket_.write(0, packet.data(), packet.size(), &actual);

    if (status == ZX_OK) {
      socket_out_queue_.pop();
    } else if (status == ZX_ERR_SHOULD_WAIT) {
      break;
    } else {
      if (status != ZX_ERR_PEER_CLOSED) {
        FDF_SLOG(ERROR, "Failed to write to socket", KV("status", zx_status_get_string(status)));
      }
      FDF_LOG(INFO, "Client socket closed, returning to ready state");
      return Unconfigured();
    }
  }

  return std::move(*this);
}

void VsockUsb::SetCallback(fuchsia_hardware_vsockbridge::wire::UsbSetCallbackRequest* request,
                           SetCallbackCompleter::Sync& completer) {
  FDF_LOG(TRACE, "SetCallback");
  uint64_t id = ++callback_id_;
  callback_ = Callback(fidl::WireSharedClient(std::move(request->callback), dispatcher_,
                                              fidl::ObserveTeardown([this, id]() {
                                                if (callback_id_ == id) {
                                                  callback_ = std::nullopt;
                                                }
                                              })));
  HandleSocketAvailable();

  completer.Reply();
}

void VsockUsb::HandleSocketAvailable() {
  if (!callback_) {
    FDF_LOG(TRACE, "No callback set, deferring socket callback");
    return;
  }

  if (!peer_socket_) {
    FDF_LOG(TRACE, "No peer socket created yet, deferring socket callback");
    return;
  }

  FDF_LOG(TRACE, "Callback set and peer socket available, sending socket to callback");
  (*callback_)(std::move(*peer_socket_));
  peer_socket_ = std::nullopt;
}

void VsockUsb::Callback::operator()(zx::socket socket) {
  if (!fidl_.is_valid()) {
    return;
  }

  fidl_->NewLink(std::move(socket))
      .Then([](fidl::WireUnownedResult<fuchsia_hardware_vsockbridge::Callback::NewLink>& result) {
        if (!result.ok()) {
          auto res = result.FormatDescription();
          FDF_SLOG(ERROR, "Failed to share socket with component", KV("status", res));
        }
      });
}

VsockUsb::State VsockUsb::Unconfigured::ReceiveData(uint8_t*, size_t len,
                                                    std::optional<zx::socket>*,
                                                    VsockUsb* owner) && {
  FDF_SLOG(WARNING, "Dropped incoming data (device not configured)", KV("bytes", len));
  return std::move(*this);
}

VsockUsb::State VsockUsb::ShuttingDown::ReceiveData(uint8_t*, size_t len,
                                                    std::optional<zx::socket>*,
                                                    VsockUsb* owner) && {
  FDF_SLOG(WARNING, "Dropped incoming data (device shutting down)", KV("bytes", len));
  return std::move(*this);
}

VsockUsb::State VsockUsb::Running::ReceiveData(uint8_t* data, size_t len,
                                               std::optional<zx::socket>* peer_socket,
                                               VsockUsb* owner) && {
  FDF_LOG(TRACE, "Running::ReceiveData(%zu)", len);

  if (len == 0) {
    return std::move(*this);
  }

  if (socket_out_queue_.empty()) {
    size_t actual;
    zx_status_t status = socket_.write(0, data, len, &actual);

    if (status == ZX_OK) {
      return std::move(*this);
    }

    if (status != ZX_ERR_SHOULD_WAIT) {
      if (status != ZX_ERR_PEER_CLOSED) {
        FDF_SLOG(ERROR, "Failed to write to socket", KV("status", zx_status_get_string(status)));
      }
      FDF_LOG(INFO, "Client socket closed, returning to ready state");
      return Unconfigured();
    }
  }

  bool was_empty = socket_out_queue_.empty();
  socket_out_queue_.emplace(data, data + len);
  if (was_empty) {
    owner->ProcessWritesToSocket();
  }

  return std::move(*this);
}

void VsockUsb::ReadBatchComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& c : completions) {
    ReadComplete(std::move(c));
  }
}

void VsockUsb::ReadComplete(fendpoint::Completion completion) {
  FDF_LOG(TRACE, "ReadComplete (status: %d, size: %zu)", *completion.status(),
          *completion.transfer_size());

  auto request = usb::FidlRequest(std::move(completion.request().value()));
  if (*completion.status() == ZX_ERR_IO_NOT_PRESENT) {
    FDF_LOG(
        INFO,
        "Device disconnected from host or requires reconfiguration. Unconfiguring endpoints and returning request to pool");
    ZX_ASSERT(!bulk_out_ep_.RequestsFull());
    bulk_out_ep_.PutRequest(std::move(request));
    bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
    if (std::holds_alternative<ShuttingDown>(state_)) {
      if (!HasPendingRequests()) {
        ShutdownComplete();
      }
    } else {
      state_ = Unconfigured();
      SyncInspectState();
    }
    return;
  }

  if (*completion.status() == ZX_OK) {
    if (zx_status_t status = request.CacheFlushInvalidate(bulk_out_ep_.GetMapped());
        status != ZX_OK) {
      FDF_SLOG(ERROR, "Cache flush invalidate failed", KV("status", zx_status_get_string(status)));
    } else {
      // This should always be true because when we registered VMOs, we only registered one per
      // request.
      ZX_ASSERT(request->data()->size() == 1);
      auto addr = bulk_out_ep_.GetMappedAddr(request.request(), 0);
      if (!addr.has_value()) {
        FDF_SLOG(ERROR, "Failed to map RX data");
      } else {
        uint8_t* data = reinterpret_cast<uint8_t*>(*addr);
        size_t data_length = *completion.transfer_size();
        bulk_out_inspect_.AddRxBytes(data_length);

        const size_t old_index = state_.index();
        std::visit(
            [this, data, data_length](auto&& state) {
              state_ = std::forward<decltype(state)>(state).ReceiveData(data, data_length,
                                                                        &peer_socket_, this);
            },
            std::move(state_));
        if (state_.index() != old_index) {
          SyncInspectState();
        }
      }
    }
  } else if (*completion.status() != ZX_ERR_CANCELED) {
    FDF_SLOG(ERROR, "Read failed", KV("status", zx_status_get_string(*completion.status())));
    bulk_out_inspect_.AddFailedRxBytes(request.length());
  }

  if (Online()) {
    request.reset_buffers(bulk_out_ep_.GetMapped());

    // Only extract the underlying FIDL request when we are actively online and
    // re-queueing to the endpoint. If the driver is offline or shutting down,
    // `request` must remain intact so it can be returned to `bulk_out_ep_` in
    // the else branch below.
    std::vector<fuchsia_hardware_usb_request::Request> requests;
    requests.emplace_back(request.take_request());
    FDF_LOG(TRACE, "Re-queuing read request");
    auto result = bulk_out_ep_->QueueRequests(std::move(requests));
    if (result.is_error()) {
      FDF_SLOG(ERROR, "Failed to QueueRequests",
               KV("status", result.error_value().FormatDescription()));
      // Note: `requests` was moved into QueueRequests and consumed by the FIDL transport.
    }
  } else {
    ZX_ASSERT(!bulk_out_ep_.RequestsFull());
    bulk_out_ep_.PutRequest(std::move(request));
    if (std::holds_alternative<ShuttingDown>(state_)) {
      FDF_LOG(DEBUG, "Shutting down from ReadComplete");
      bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
      if (!HasPendingRequests()) {
        ShutdownComplete();
      }
      return;
    }
    FDF_LOG(DEBUG, "ReadComplete while unconnected");
  }
  bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
}

void VsockUsb::WriteBatchComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& c : completions) {
    WriteComplete(std::move(c));
  }
}

void VsockUsb::WriteComplete(fendpoint::Completion completion) {
  FDF_LOG(TRACE, "WriteComplete");
  zx_status_t status = *completion.status();
  auto request = usb::FidlRequest(std::move(completion.request().value()));
  size_t size = request.length();
  if (status == ZX_OK) {
    bulk_in_inspect_.AddTxBytes(completion.transfer_size().value_or(0));
  } else if (status != ZX_ERR_CANCELED) {
    bulk_in_inspect_.AddFailedTxBytes(size);
  }
  ZX_ASSERT(!bulk_in_ep_.RequestsFull());
  bulk_in_ep_.PutRequest(std::move(request));
  if (std::holds_alternative<ShuttingDown>(state_)) {
    FDF_LOG(DEBUG, "Shutting down from WriteComplete");
    bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
    if (!HasPendingRequests()) {
      ShutdownComplete();
    }
    return;
  }

  FDF_LOG(DEBUG, "Write completed");
  ProcessReadsFromSocket();
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
}

void VsockUsb::Shutdown(fit::function<void()> callback) {
  if (auto* state = std::get_if<ShuttingDown>(&state_)) {
    state->AddCallback(std::move(callback));
    return;
  }

  if (throughput_tracker_) {
    throughput_tracker_->Stop();
  }
  // Transition to ShuttingDown before canceling endpoints so any completions delivered
  // as a result of CancelAll observe the ShuttingDown state and drain into the pool.
  state_ = ShuttingDown(std::move(callback));
  SyncInspectState();
  CancelAllEndpoints();

  if (!HasPendingRequests()) {
    ShutdownComplete();
  }
}

void VsockUsb::ShutdownComplete() {
  if (auto state = std::get_if<ShuttingDown>(&state_)) {
    bulk_in_ep_.Close();
    bulk_out_ep_.Close();
    state->FinishWithCallback();
  } else {
    FDF_SLOG(ERROR, "ShutdownComplete called outside of shutdown path");
  }
}

FUCHSIA_DRIVER_EXPORT2(VsockUsb);
