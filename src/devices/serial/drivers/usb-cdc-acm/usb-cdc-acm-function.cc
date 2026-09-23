// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>

#include <vector>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <fbl/mutex.h>
#include <usb-endpoint/usb-endpoint-client.h>
#include <usb/cdc.h>
#include <usb/descriptors.h>
#include <usb/hid.h>
#include <usb/peripheral.h>
#include <usb/usb.h>

namespace fake_usb_cdc_acm_function {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;
// Acts as a fake USB device for CDC-ACM serial tests. Stores a single write's worth of data and
// echos it back on the next read, unless the write is exactly a single '0' byte, in which case
// the next read will be an empty response.
class FakeUsbCdcAcmFunction;
constexpr int kBulkMaxPacket = 512;

class FakeUsbCdcAcmFunction
    : public fdf::DriverBase2,
      public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  explicit FakeUsbCdcAcmFunction() : fdf::DriverBase2("fake-usb-cdc-acm") {}

  zx::result<> Start(fdf::DriverContext context) override;

  // UsbFunctionInterface:
  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override;
  void SetConfigured(SetConfiguredRequest& request,
                     SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  void InComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void OutComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;

  usb::EndpointClient<FakeUsbCdcAcmFunction> bulk_in_ep_{
      usb::EndpointType::BULK, this, std::mem_fn(&FakeUsbCdcAcmFunction::InComplete)};
  usb::EndpointClient<FakeUsbCdcAcmFunction> bulk_out_ep_{
      usb::EndpointType::BULK, this, std::mem_fn(&FakeUsbCdcAcmFunction::OutComplete)};

  struct Descriptor {
    usb_interface_descriptor_t interface;
    usb_endpoint_descriptor_t bulk_in;
    usb_endpoint_descriptor_t bulk_out;
  } __PACKED descriptor_;

  uint8_t bulk_out_addr_ = 0;
  uint8_t bulk_in_addr_ = 0;

  fbl::Mutex mtx_;
  bool configured_ __TA_GUARDED(mtx_) = false;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      binding_;
};

void FakeUsbCdcAcmFunction::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  completer.Reply(
      zx::ok(fuchsia_hardware_usb_function::UsbFunctionInterfaceControlResponse().read({})));
}

void FakeUsbCdcAcmFunction::SetConfigured(SetConfiguredRequest& request,
                                          SetConfiguredCompleter::Sync& completer) {
  fbl::AutoLock lock(&mtx_);
  if (!request.configured()) {
    configured_ = false;
    completer.Reply(zx::ok());
    return;
  }
  if (configured_) {
    completer.Reply(zx::ok());
    return;
  }
  configured_ = true;

  fuchsia_hardware_usb_function::EndpointConfiguration config_in;
  fuchsia_hardware_usb_function::EndpointDescriptor desc_in;
  desc_in.bm_attributes(descriptor_.bulk_in.bm_attributes);
  desc_in.w_max_packet_size(descriptor_.bulk_in.w_max_packet_size);
  desc_in.b_interval(descriptor_.bulk_in.b_interval);
  config_in.descriptor(std::move(desc_in));

  function_->ConfigureEndpoint({bulk_in_addr_, std::move(config_in)});

  fuchsia_hardware_usb_function::EndpointConfiguration config_out;
  fuchsia_hardware_usb_function::EndpointDescriptor desc_out;
  desc_out.bm_attributes(descriptor_.bulk_out.bm_attributes);
  desc_out.w_max_packet_size(descriptor_.bulk_out.w_max_packet_size);
  desc_out.b_interval(descriptor_.bulk_out.b_interval);
  config_out.descriptor(std::move(desc_out));

  function_->ConfigureEndpoint({bulk_out_addr_, std::move(config_out)});

  auto req = bulk_out_ep_.GetRequest();
  if (req.has_value()) {
    std::vector<fuchsia_hardware_usb_request::Request> reqs;
    reqs.emplace_back(req->take_request());
    fit::result<fidl::OneWayError> result = bulk_out_ep_->QueueRequests({std::move(reqs)});
    if (result.is_error()) {
      fdf::error("QueueRequests failed: {}", result.error_value().FormatDescription());
    }
  }

  completer.Reply(zx::ok());
}

void FakeUsbCdcAcmFunction::SetInterface(SetInterfaceRequest& request,
                                         SetInterfaceCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeUsbCdcAcmFunction::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method: {}", metadata.method_ordinal);
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeUsbCdcAcmFunction::InComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  for (auto& c : completions) {
    bulk_in_ep_.PutRequest(usb::FidlRequest{std::move(c.request().value())});
  }
}

void FakeUsbCdcAcmFunction::OutComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  uint8_t buffer[kBulkMaxPacket];
  std::vector<fuchsia_hardware_usb_request::Request> return_reqs;
  for (auto& c : completions) {
    usb::FidlRequest req(std::move(c.request().value()));
    uint64_t size = c.transfer_size().value();
    FDF_ASSERT(size <= kBulkMaxPacket);
    auto put_req = fit::defer([&] { return_reqs.emplace_back(req.take_request()); });
    if (size == 0) {
      continue;
    }
    std::optional req_out = bulk_in_ep_.GetRequest();
    if (!req_out.has_value()) {
      fdf::error("No IN request available");
      continue;
    }

    req.CopyFrom(0, buffer, size, bulk_out_ep_.GetMapped());

    // Queue up the exact same read data, unless the read was a single '0', in which case queue an
    // empty response.
    if (size == 1 && buffer[0] == '0') {
      size = 0;
    }
    std::vector<size_t> copied = req_out->CopyTo(0, buffer, size, bulk_in_ep_.GetMapped());
    for (size_t i = 0; i < copied.size(); i++) {
      req_out.value()->data()->at(i).size(copied[i]);
    }

    std::vector<fuchsia_hardware_usb_request::Request> reqs;
    reqs.emplace_back(req_out->take_request());
    fit::result<fidl::OneWayError> result = bulk_in_ep_->QueueRequests({std::move(reqs)});
    if (result.is_error()) {
      fdf::error("QueueRequests IN failed: {}", result.error_value().FormatDescription());
    }
  }

  if (!return_reqs.empty()) {
    fit::result<fidl::OneWayError> result = bulk_out_ep_->QueueRequests(std::move(return_reqs));
    if (result.is_error()) {
      fdf::error("QueueRequests OUT failed: {}", result.error_value().FormatDescription());
    }
  }
}

zx::result<> FakeUsbCdcAcmFunction::Start(fdf::DriverContext context) {
  fbl::AutoLock lock(&mtx_);

  auto svc =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (svc.is_error()) {
    return svc.take_error();
  }
  function_ = fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction>(std::move(*svc));

  descriptor_.interface = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = 0,
      .b_alternate_setting = 0,
      .b_num_endpoints = 2,
      .b_interface_class = USB_CLASS_COMM,
      .b_interface_sub_class = USB_CDC_SUBCLASS_ABSTRACT,
      .b_interface_protocol = 1,
      .i_interface = 0,
  };
  descriptor_.bulk_in = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = USB_ENDPOINT_IN,  // set later
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = htole16(kBulkMaxPacket),
      .b_interval = 0,
  };
  descriptor_.bulk_out = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = USB_ENDPOINT_OUT,  // set later
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = htole16(kBulkMaxPacket),
      .b_interval = 0,
  };

  std::vector<fuchsia_hardware_usb_function::EndpointResource> ep_res;
  zx::result bulk_in_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_in_endpoints.is_error()) {
    return bulk_in_endpoints.take_error();
  }
  zx::result bulk_out_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_out_endpoints.is_error()) {
    return bulk_out_endpoints.take_error();
  }

  ep_res.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kIn, std::move(bulk_in_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), kBulkMaxPacket));

  ep_res.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kOut,
      std::move(bulk_out_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), kBulkMaxPacket));

  fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources> alloc_req;
  alloc_req.interface_count(1);
  alloc_req.endpoints(std::move(ep_res));

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    fdf::error("failed to allocate resources: {}", alloc_result.error_value().FormatDescription());
    return zx::error(ZX_ERR_INTERNAL);
  }

  descriptor_.interface.b_interface_number = alloc_result->interface_nums()[0];
  bulk_in_addr_ = alloc_result->endpoint_addrs()[0];
  bulk_out_addr_ = alloc_result->endpoint_addrs()[1];

  descriptor_.bulk_in.b_endpoint_address = bulk_in_addr_;
  descriptor_.bulk_out.b_endpoint_address = bulk_out_addr_;

  zx_status_t status = bulk_in_ep_.Init(std::move(bulk_in_endpoints->client), dispatcher());
  if (status != ZX_OK)
    return zx::error(status);

  status = bulk_out_ep_.Init(std::move(bulk_out_endpoints->client), dispatcher());
  if (status != ZX_OK)
    return zx::error(status);

  if (bulk_in_ep_.AddRequests(2, kBulkMaxPacket,
                              fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) != 2) {
    fdf::error("failed to allocate IN requests");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (bulk_out_ep_.AddRequests(2, kBulkMaxPacket,
                               fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) != 2) {
    fdf::error("failed to allocate OUT requests");
    return zx::error(ZX_ERR_INTERNAL);
  }

  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_hardware_usb_function::UsbFunctionInterface>::Create();
  auto bind_result = fidl::BindServer(dispatcher(), std::move(server_end), this);
  binding_ = std::move(bind_result);

  std::vector<uint8_t> descriptors_buffer(sizeof(descriptor_));
  memcpy(descriptors_buffer.data(), &descriptor_, sizeof(descriptor_));

  fidl::Request<fuchsia_hardware_usb_function::UsbFunction::Configure> config_req;
  config_req.configuration(descriptors_buffer);
  config_req.iface(std::move(client_end));

  fidl::Result config_result = function_->Configure(std::move(config_req));
  if (config_result.is_error()) {
    fdf::error("failed to configure: {}", config_result.error_value().FormatDescription());
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

FUCHSIA_DRIVER_EXPORT2(FakeUsbCdcAcmFunction);

}  // namespace fake_usb_cdc_acm_function
