// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/drivers/usb-fastboot-function/usb_fastboot_function.h"

#include <lib/driver/compat/cpp/compat.h>
#include <lib/zircon-internal/align.h>

namespace usb_fastboot_function {

namespace ffunction = fuchsia_hardware_usb_function;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

namespace {
size_t CalculateRxHeaderLength(size_t data_size) {
  // Adjusts USB RX request length to bypass zero-length-packet. Upstream fastboot implementation
  // doesn't send zero-length packet when download completes. This causes host side driver to stall
  // if size of download data happens to be multiples of USB max packet size but not multiples of
  // bulk size. For example, suppose bulk request size is 2048, and USB max packet size is 512, and
  // host is sending the last 512/1024/1536 bytes during download, this last packet will not reach
  // this driver immediately. But if the host sends another 10 bytes of data, the driver will
  // receive a single packet of size (512/1024/1536 + 10) bytes. Thus we adjust the value of bulk
  // request based on the expected amount of data to receive. The size is required to be multiples
  // of kPacketSize. Thus we adjust it to be the smaller between kBulkRequestSize and the round
  // up value of `data_size' w.r.t kPacketSize. For example, if we are expecting exactly 512
  // bytes of data, the following will give 512 exactly.
  return std::min(kBulkRequestSize, ZX_ROUNDUP(data_size, kPacketSize));
}
}  // namespace

void UsbFastbootFunction::CleanUpTx(zx_status_t status, usb::FidlRequest req) {
  bulk_in_ep_.PutRequest(std::move(req));
  if (send_completer_.has_value()) {
    send_vmo_.Reset();
    if (status == ZX_OK) {
      send_completer_->ReplySuccess();
    } else {
      send_completer_->ReplyError(status);
    }
    send_completer_.reset();
  }
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
}

void UsbFastbootFunction::QueueTx() {
  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.reserve(kMaxRequestCount);

  while (queued_tx_size_ < total_to_send_) {
    auto req = bulk_in_ep_.GetRequest();
    if (!req) {
      break;
    }

    const size_t tx_size = std::min(kBulkRequestSize, total_to_send_ - queued_tx_size_);
    zx::result<std::vector<size_t>> actual =
        req->CachedCopyTo(0, static_cast<uint8_t*>(send_vmo_.start()) + queued_tx_size_, tx_size,
                          bulk_in_ep_.GetMapped());
    if (actual.is_error() || actual->size() != 1) {
      fdf::error("QueueTx: CachedCopyTo failed: {}", actual.status_string());
      CleanUpTx(actual.is_error() ? actual.status_value() : ZX_ERR_INTERNAL,
                std::move(req.value()));
      return;
    }
    (*req)->data()->at(0).size((*actual)[0]);

    requests.emplace_back(req->take_request());
    queued_tx_size_ += tx_size;
  }

  if (!requests.empty()) {
    auto result = bulk_in_ep_->QueueRequests(std::move(requests));
    if (result.is_error()) {
      fdf::error("Failed to QueueRequests: {}", result.error_value().FormatDescription());
      for (auto& req : requests) {
        bulk_in_ep_.PutRequest(usb::FidlRequest(std::move(req)));
      }
      if (send_completer_.has_value()) {
        send_completer_->ReplyError(ZX_ERR_INTERNAL);
        send_completer_.reset();
      }
      return;
    }
  }
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
}

void UsbFastbootFunction::TxBatchComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  for (auto& completion : completions) {
    TxComplete(std::move(completion));
  }
  if (send_completer_.has_value() && queued_tx_size_ < total_to_send_) {
    QueueTx();
  }
}

void UsbFastbootFunction::TxComplete(fuchsia_hardware_usb_endpoint::Completion completion) {
  usb::FidlRequest req{std::move(completion.request().value())};
  if (!send_completer_.has_value()) {
    bulk_in_ep_.PutRequest(std::move(req));
    return;
  }

  auto status = *completion.status();
  // Do not queue request on error.
  if (status != ZX_OK) {
    zxlogf(ERROR, "tx_completion error: %s", zx_status_get_string(status));
    bulk_in_inspect_.AddFailedTxBytes(req.length());
    CleanUpTx(status, std::move(req));
    return;
  }

  // If succeeds, update `sent_size_`, otherwise keep it the same to retry.
  sent_size_ += *completion.transfer_size();
  if (sent_size_ == total_to_send_) {
    bulk_in_inspect_.AddTxBytes(*completion.transfer_size());
    CleanUpTx(ZX_OK, std::move(req));
    return;
  }

  bulk_in_inspect_.AddTxBytes(*completion.transfer_size());
  bulk_in_ep_.PutRequest(std::move(req));
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
}

void UsbFastbootFunction::Send(::fuchsia_hardware_fastboot::wire::FastbootImplSendRequest* request,
                               SendCompleter::Sync& completer) {
  if (!configured_) {
    completer.ReplyError(ZX_ERR_UNAVAILABLE);
    return;
  }

  if (send_completer_.has_value()) {
    // A previous call to Send() is pending
    completer.ReplyError(ZX_ERR_UNAVAILABLE);
    return;
  }

  if (zx_status_t status = request->data.get_prop_content_size(&total_to_send_); status != ZX_OK) {
    total_to_send_ = 0;
    completer.ReplyError(status);
    return;
  }

  if (total_to_send_ == 0) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (zx_status_t status = send_vmo_.Map(std::move(request->data), 0, total_to_send_,
                                         ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
      status != ZX_OK) {
    zxlogf(ERROR, "Failed to map vmo %d", status);
    completer.ReplyError(status);
    return;
  }

  sent_size_ = 0;
  queued_tx_size_ = 0;
  send_completer_ = completer.ToAsync();
  QueueTx();
}

void UsbFastbootFunction::CleanUpRx(zx_status_t status, usb::FidlRequest req) {
  bulk_out_ep_.PutRequest(std::move(req));
  if (receive_completer_.has_value()) {
    if (status == ZX_OK) {
      receive_completer_->ReplySuccess(receive_vmo_.Release());
    } else {
      receive_vmo_.Reset();
      receive_completer_->ReplyError(status);
    }
    receive_completer_.reset();
  }
  bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
}

void UsbFastbootFunction::QueueRx() {
  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.reserve(kMaxRequestCount);
  while (queued_rx_size_ < requested_size_) {
    auto req = bulk_out_ep_.GetRequest();
    if (!req) {
      break;
    }

    ZX_ASSERT((*req)->data()->size() == 1);
    req->reset_buffers(bulk_out_ep_.GetMapped());
    const size_t rx_size = CalculateRxHeaderLength(requested_size_ - queued_rx_size_);
    (*req)->data()->at(0).size(rx_size);  // Each request has one buffer.
    if (auto status = req->CacheFlushInvalidate(bulk_out_ep_.GetMapped()); status != ZX_OK) {
      fdf::error("QueueRx: CacheFlushInvalidate failed: {}", zx_status_get_string(status));
      CleanUpRx(status, std::move(req.value()));
      return;
    }

    requests.emplace_back(req->take_request());
    queued_rx_size_ += rx_size;
  }

  if (!requests.empty()) {
    auto result = bulk_out_ep_->QueueRequests(std::move(requests));
    if (result.is_error()) {
      fdf::error("Failed to QueueRequests: {}", result.error_value().FormatDescription());
      for (auto& req : requests) {
        bulk_out_ep_.PutRequest(usb::FidlRequest(std::move(req)));
      }
      if (receive_completer_.has_value()) {
        receive_completer_->ReplyError(ZX_ERR_INTERNAL);
        receive_completer_.reset();
      }
      return;
    }
  }
  bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
}

void UsbFastbootFunction::RxBatchComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  for (auto& completion : completions) {
    RxComplete(std::move(completion));
  }
  if (receive_completer_.has_value() && queued_rx_size_ < requested_size_) {
    QueueRx();
  }
}

void UsbFastbootFunction::RxComplete(fuchsia_hardware_usb_endpoint::Completion completion) {
  usb::FidlRequest req{std::move(completion.request().value())};
  if (!receive_completer_.has_value()) {
    bulk_out_ep_.PutRequest(std::move(req));
    return;
  }

  zx_status_t status = *completion.status();

  if (status != ZX_OK) {
    zxlogf(ERROR, "rx_completion error: %s", zx_status_get_string(status));
    bulk_out_inspect_.AddFailedRxBytes(req.length());
    CleanUpRx(status, std::move(req));
    return;
  }

  // This should always be true because when we registered VMOs, we only registered one per
  // request.
  ZX_ASSERT(req->data()->size() == 1);
  auto addr = bulk_out_ep_.GetMappedAddr(req.request(), 0);
  if (!addr.has_value()) {
    zxlogf(ERROR, "Failed to get mapped");
    CleanUpRx(ZX_ERR_INTERNAL, std::move(req));
    return;
  }

  if (auto status = req.CacheFlushInvalidate(bulk_out_ep_.GetMapped()); status != ZX_OK) {
    zxlogf(ERROR, "RxComplete: CacheFlushInvalidate failed: %s", zx_status_get_string(status));
    CleanUpRx(status, std::move(req));
    return;
  }

  const uint8_t* data = reinterpret_cast<const uint8_t*>(*addr);
  bulk_out_inspect_.AddRxBytes(*completion.transfer_size());
  memcpy(static_cast<uint8_t*>(receive_vmo_.start()) + received_size_, data,
         *completion.transfer_size());
  received_size_ += *completion.transfer_size();
  if (received_size_ >= requested_size_) {
    zx_status_t status = receive_vmo_.vmo().set_prop_content_size(received_size_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to set content size %d", status);
    }
    CleanUpRx(status, std::move(req));
    return;
  }

  bulk_out_ep_.PutRequest(std::move(req));
  bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
}

void UsbFastbootFunction::Receive(
    ::fuchsia_hardware_fastboot::wire::FastbootImplReceiveRequest* request,
    ReceiveCompleter::Sync& completer) {
  if (!configured_) {
    completer.ReplyError(ZX_ERR_UNAVAILABLE);
    return;
  }

  if (receive_completer_.has_value()) {
    // A previous call to Receive() is pending
    completer.ReplyError(ZX_ERR_UNAVAILABLE);
    return;
  }

  received_size_ = 0;
  // Minimum set to 1 so that round up works correctly.
  requested_size_ = std::max(uint64_t{1}, request->requested);
  // Create vmo for receiving data. Roundup by `kBulkMaxPacketSize` since USB transmission is in
  // the unit of packet.
  zx_status_t status =
      receive_vmo_.CreateAndMap(ZX_ROUNDUP(requested_size_, kPacketSize), "usb fastboot receive");
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to create vmo %d.", status);
    completer.ReplyError(status);
    return;
  }

  queued_rx_size_ = 0;
  receive_completer_ = completer.ToAsync();
  QueueRx();
}

void UsbFastbootFunction::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

zx_status_t UsbFastbootFunction::ConfigureEndpoints(bool enable) {
  if (enable) {
    for (const auto* ep_desc : {&descriptors_.bulk_out_ep, &descriptors_.bulk_in_ep}) {
      fuchsia_hardware_usb_function::EndpointConfiguration ep_config;
      fuchsia_hardware_usb_function::EndpointDescriptor desc;
      desc.bm_attributes(ep_desc->bm_attributes);
      desc.w_max_packet_size(le16toh(ep_desc->w_max_packet_size));
      desc.b_interval(ep_desc->b_interval);
      ep_config.descriptor(std::move(desc));

      fidl::Result result =
          function_->ConfigureEndpoint({ep_desc->b_endpoint_address, std::move(ep_config)});
      if (result.is_error()) {
        fdf::error("ConfigureEndpoint failed: {}", result.error_value().FormatDescription());
        return result.error_value().is_framework_error()
                   ? result.error_value().framework_error().status()
                   : result.error_value().domain_error();
      }
    }
    configured_ = true;
  } else {
    for (const uint8_t ep_addr : {bulk_out_addr(), bulk_in_addr()}) {
      fidl::Result result = function_->DisableEndpoint({ep_addr});
      if (!result.is_ok()) {
        fdf::error("Failed to disable endpoint {}: {}", ep_addr,
                   result.error_value().FormatDescription());
        return result.error_value().is_framework_error()
                   ? result.error_value().framework_error().status()
                   : result.error_value().domain_error();
      }
    }
    configured_ = false;
  }

  return ZX_OK;
}

void UsbFastbootFunction::SetConfigured(SetConfiguredRequest& request,
                                        SetConfiguredCompleter::Sync& completer) {
  bool configured = request.configured();
  fuchsia_hardware_usb_descriptor::UsbSpeed speed = request.speed();
  fdf::info("configured? - {}  speed - {}.", configured, static_cast<uint32_t>(speed));

  completer.Reply(zx::make_result(ConfigureEndpoints(configured)));
}

void UsbFastbootFunction::SetInterface(SetInterfaceRequest& request,
                                       SetInterfaceCompleter::Sync& completer) {
  uint8_t interface = request.interface();
  uint8_t alt_setting = request.alt_setting();
  fdf::info("interface - {}  alt_setting - {}.", interface, alt_setting);
  if (interface != descriptors_.fastboot_intf.b_interface_number || alt_setting != 0) {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }
  completer.Reply(zx::ok());
}

void UsbFastbootFunction::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method {}", metadata.method_ordinal);
}

zx::result<> UsbFastbootFunction::Start(fdf::DriverContext context) {
  inspector_ = context.CreateInspector(this);
  auto client =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (client.is_error()) {
    fdf::error("Failed to connect to UsbFunctionService: {}", client.status_string());
    return client.take_error();
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
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), kPacketSize));
  resources.push_back(ffunction::EndpointResource(
      fdescriptor::EndpointDirection::kIn, std::move(bulk_in_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), kPacketSize));

  fidl::Request<ffunction::UsbFunction::AllocResources> alloc_req;
  alloc_req.interface_count(2);
  alloc_req.endpoints(std::move(resources));

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    fdf::error("AllocResources failed: {}", alloc_result.error_value().FormatDescription());
    return zx::error(alloc_result.error_value().is_framework_error()
                         ? alloc_result.error_value().framework_error().status()
                         : alloc_result.error_value().domain_error());
  }

  auto& response = alloc_result.value();
  descriptors_.fastboot_intf.b_interface_number = response.interface_nums()[0];
  descriptors_.placehodler_intf.b_interface_number = response.interface_nums()[1];

  descriptors_.bulk_out_ep.b_endpoint_address = response.endpoint_addrs()[0];
  descriptors_.bulk_in_ep.b_endpoint_address = response.endpoint_addrs()[1];

  zx_status_t status = bulk_out_ep_.Init(std::move(bulk_out_endpoints->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("bulk_out_ep_.Init failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  status = bulk_in_ep_.Init(std::move(bulk_in_endpoints->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("bulk_in_ep_.Init failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  if (bulk_out_ep_.AddRequests(kMaxRequestCount, kBulkRequestSize,
                               fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) !=
      kMaxRequestCount) {
    fdf::error("Failed to allocate RX requests");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (bulk_in_ep_.AddRequests(kMaxRequestCount, kBulkRequestSize,
                              fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) !=
      kMaxRequestCount) {
    fdf::error("Failed to allocate TX requests");
    return zx::error(ZX_ERR_INTERNAL);
  }

  auto serve_result = outgoing()->AddService<fuchsia_hardware_fastboot::Service>(
      fuchsia_hardware_fastboot::Service::InstanceHandler({
          .fastboot = bindings_.CreateHandler(
              this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
      }));
  if (serve_result.is_error()) {
    fdf::error("Failed to add Device service {}", serve_result.status_string());
    return serve_result.take_error();
  }

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
    return zx::error(ZX_ERR_INTERNAL);
  }

  inspect_node_ = inspector().root().CreateChild("usb-fastboot");
  bulk_in_inspect_.Init(inspect_node_, "bulk_in");
  bulk_out_inspect_.Init(inspect_node_, "bulk_out");
  throughput_tracker_.emplace(dispatcher(), [this](zx::duration delta) {
    bulk_in_inspect_.MeasureThroughput(delta);
    bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
    bulk_out_inspect_.MeasureThroughput(delta);
    bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
  });
  throughput_tracker_->Start();

  return zx::ok();
}

void UsbFastbootFunction::Stop(fdf::StopCompleter completer) {
  if (throughput_tracker_) {
    throughput_tracker_->Stop();
  }
  completer(zx::ok());
}

}  // namespace usb_fastboot_function

FUCHSIA_DRIVER_EXPORT2(usb_fastboot_function::UsbFastbootFunction);
