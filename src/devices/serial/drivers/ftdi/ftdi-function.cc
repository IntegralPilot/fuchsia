// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>

#include <memory>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <usb-endpoint/usb-endpoint-client.h>
#include <usb/hid.h>
#include <usb/peripheral.h>
#include <usb/request-cpp.h>
#include <usb/usb-request.h>

#define BULK_MAX_PACKET 512
#define FTDI_STATUS_SIZE 2

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fake_ftdi_function {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

class FakeFtdiFunction : public fdf::DriverBase2,
                         public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  explicit FakeFtdiFunction() : fdf::DriverBase2("ftdi-fake-usb") {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface>
  void Control(fuchsia_hardware_usb_function::UsbFunctionInterfaceControlRequest& request,
               ControlCompleter::Sync& completer) override;
  void SetConfigured(
      fuchsia_hardware_usb_function::UsbFunctionInterfaceSetConfiguredRequest& request,
      SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(fuchsia_hardware_usb_function::UsbFunctionInterfaceSetInterfaceRequest& request,
                    SetInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  void InComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void OutComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;

  struct fake_ftdi_descriptor_t {
    usb_interface_descriptor_t interface;
    usb_endpoint_descriptor_t bulk_in;
    usb_endpoint_descriptor_t bulk_out;
  } __PACKED descriptor_;

  size_t descriptor_size_ = 0;

  uint8_t bulk_out_addr_ = 0;
  uint8_t bulk_in_addr_ = 0;

  usb::EndpointClient<FakeFtdiFunction> bulk_in_ep_{usb::EndpointType::BULK, this,
                                                    std::mem_fn(&FakeFtdiFunction::InComplete)};
  usb::EndpointClient<FakeFtdiFunction> bulk_out_ep_{usb::EndpointType::BULK, this,
                                                     std::mem_fn(&FakeFtdiFunction::OutComplete)};

  bool configured_ = false;
  bool active_ = false;
};

void FakeFtdiFunction::InComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  for (auto& completion : completions) {
    if (completion.request()) {
      bulk_in_ep_.PutRequest(usb::FidlRequest(std::move(*completion.request())));
    }
  }
}

void FakeFtdiFunction::OutComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  for (auto& completion : completions) {
    if (completion.status() && *completion.status() != ZX_OK) {
      logger().log(fdf::ERROR, "OutComplete error: {}", zx_status_get_string(*completion.status()));
      continue;
    }

    if (!completion.transfer_size() || *completion.transfer_size() == 0) {
      continue;
    }

    size_t size = *completion.transfer_size();
    std::vector<uint8_t> data(size);

    usb::FidlRequest req(std::move(*completion.request()));
    req.CopyFrom(0, data.data(), size, bulk_out_ep_.GetMappedLocked());

    auto in_req = bulk_in_ep_.GetRequest();
    if (in_req) {
      in_req->CopyTo(FTDI_STATUS_SIZE, data.data(), size, bulk_in_ep_.GetMappedLocked());
      auto& d = (*in_req->operator->()->data())[0];
      d.size(size + FTDI_STATUS_SIZE);

      std::vector<fuchsia_hardware_usb_request::Request> in_reqs;
      in_reqs.push_back(in_req->take_request());
      fit::result<fidl::OneWayError> result = bulk_in_ep_->QueueRequests({std::move(in_reqs)});
      if (result.is_error()) {
        logger().log(fdf::ERROR, "QueueRequests IN failed: {}",
                     result.error_value().FormatDescription());
      }
    }

    // Re-queue OUT request
    auto& out_d = (*req.operator->()->data())[0];
    out_d.size(BULK_MAX_PACKET);

    std::vector<fuchsia_hardware_usb_request::Request> out_reqs;
    out_reqs.push_back(req.take_request());
    fit::result<fidl::OneWayError> result = bulk_out_ep_->QueueRequests({std::move(out_reqs)});
    if (result.is_error()) {
      logger().log(fdf::ERROR, "QueueRequests OUT failed: {}",
                   result.error_value().FormatDescription());
    }
  }
}

void FakeFtdiFunction::Control(
    fuchsia_hardware_usb_function::UsbFunctionInterfaceControlRequest& request,
    ControlCompleter::Sync& completer) {
  completer.Reply(zx::ok(fuchsia_hardware_usb_function::UsbFunctionInterfaceControlResponse()));
}

void FakeFtdiFunction::SetConfigured(
    fuchsia_hardware_usb_function::UsbFunctionInterfaceSetConfiguredRequest& request,
    SetConfiguredCompleter::Sync& completer) {
  if (!request.configured()) {
    configured_ = false;
    function_->DisableEndpoint({bulk_in_addr_});
    function_->DisableEndpoint({bulk_out_addr_});
    completer.Reply(zx::ok());
    return;
  }
  if (configured_) {
    completer.Reply(zx::ok());
    return;
  }
  configured_ = true;

  // Configure IN Endpoint
  fuchsia_hardware_usb_function::EndpointConfiguration ep_config_in;
  fuchsia_hardware_usb_function::EndpointDescriptor desc_in;
  desc_in.bm_attributes(descriptor_.bulk_in.bm_attributes);
  desc_in.w_max_packet_size(descriptor_.bulk_in.w_max_packet_size);
  desc_in.b_interval(descriptor_.bulk_in.b_interval);
  ep_config_in.descriptor(std::move(desc_in));

  fidl::Result result_in = function_->ConfigureEndpoint({bulk_in_addr_, std::move(ep_config_in)});
  if (result_in.is_error()) {
    logger().log(fdf::ERROR, "ConfigureEndpoint IN failed: {}",
                 result_in.error_value().FormatDescription());
  }

  // Configure OUT Endpoint
  fuchsia_hardware_usb_function::EndpointConfiguration ep_config_out;
  fuchsia_hardware_usb_function::EndpointDescriptor desc_out;
  desc_out.bm_attributes(descriptor_.bulk_out.bm_attributes);
  desc_out.w_max_packet_size(descriptor_.bulk_out.w_max_packet_size);
  desc_out.b_interval(descriptor_.bulk_out.b_interval);
  ep_config_out.descriptor(std::move(desc_out));

  fidl::Result result_out =
      function_->ConfigureEndpoint({bulk_out_addr_, std::move(ep_config_out)});
  if (result_out.is_error()) {
    logger().log(fdf::ERROR, "ConfigureEndpoint OUT failed: {}",
                 result_out.error_value().FormatDescription());
  }

  // Queue first read on OUT endpoint
  auto req = bulk_out_ep_.GetRequest();
  if (req) {
    std::vector<fuchsia_hardware_usb_request::Request> reqs;
    reqs.push_back(req->take_request());
    fit::result<fidl::OneWayError> result = bulk_out_ep_->QueueRequests({std::move(reqs)});
    if (result.is_error()) {
      logger().log(fdf::ERROR, "QueueRequests OUT failed: {}",
                   result.error_value().FormatDescription());
    }
  }
  completer.Reply(zx::ok());
}

void FakeFtdiFunction::SetInterface(
    fuchsia_hardware_usb_function::UsbFunctionInterfaceSetInterfaceRequest& request,
    SetInterfaceCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeFtdiFunction::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  logger().log(fdf::WARN, "Unknown method {}", metadata.method_ordinal);
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> FakeFtdiFunction::Start(fdf::DriverContext context) {
  descriptor_size_ = sizeof(descriptor_);
  descriptor_.interface = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = 0,
      .b_alternate_setting = 0,
      .b_num_endpoints = 2,
      .b_interface_class = 0xFF,
      .b_interface_sub_class = 0xFF,
      .b_interface_protocol = 0xFF,
      .i_interface = 0,
  };
  descriptor_.bulk_in = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = USB_ENDPOINT_IN,  // set later
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = htole16(BULK_MAX_PACKET),
      .b_interval = 0,
  };
  descriptor_.bulk_out = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = USB_ENDPOINT_OUT,  // set later
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = htole16(BULK_MAX_PACKET),
      .b_interval = 0,
  };

  active_ = true;

  zx::result client_end =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (client_end.is_error()) {
    logger().log(fdf::ERROR, "FakeFtdiFunction: Failed to connect FIDL protocol: {}", client_end);
    return client_end.take_error();
  }
  function_ = fidl::SyncClient(std::move(*client_end));

  // Allocate resources
  zx::result ep_in_channels = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (ep_in_channels.is_error())
    return zx::error(ep_in_channels.error_value());
  auto [client_in, server_in] = std::move(*ep_in_channels);

  zx::result ep_out_channels = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (ep_out_channels.is_error())
    return zx::error(ep_out_channels.error_value());
  auto [client_out, server_out] = std::move(*ep_out_channels);

  std::vector<fuchsia_hardware_usb_function::EndpointResource> endpoints;
  endpoints.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kIn, std::move(server_in),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), BULK_MAX_PACKET));
  endpoints.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kOut, std::move(server_out),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), BULK_MAX_PACKET));

  fuchsia_hardware_usb_function::UsbFunctionAllocResourcesRequest alloc_req;
  alloc_req.interface_count(1);
  alloc_req.endpoints(std::move(endpoints));

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));

  if (alloc_result.is_error()) {
    logger().log(fdf::ERROR, "FakeFtdiFunction: AllocResources failed: {}",
                 alloc_result.error_value().FormatDescription());
    return zx::error(alloc_result.error_value().is_framework_error()
                         ? alloc_result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  auto& response = alloc_result.value();
  descriptor_.interface.b_interface_number = response.interface_nums()[0];
  bulk_in_addr_ = response.endpoint_addrs()[0];
  bulk_out_addr_ = response.endpoint_addrs()[1];

  descriptor_.bulk_in.b_endpoint_address = bulk_in_addr_;
  descriptor_.bulk_out.b_endpoint_address = bulk_out_addr_;

  zx_status_t status = bulk_in_ep_.Init(std::move(client_in), dispatcher());
  if (status != ZX_OK) {
    return zx::error(status);
  }

  status = bulk_out_ep_.Init(std::move(client_out), dispatcher());
  if (status != ZX_OK) {
    return zx::error(status);
  }

  // Add requests to pool
  if (bulk_in_ep_.AddRequests(2, BULK_MAX_PACKET,
                              fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) != 2) {
    logger().log(fdf::ERROR, "Failed to allocate all IN requests");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (bulk_out_ep_.AddRequests(2, BULK_MAX_PACKET,
                               fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) != 2) {
    logger().log(fdf::ERROR, "Failed to allocate all OUT requests");
    return zx::error(ZX_ERR_INTERNAL);
  }

  // Configure
  std::vector<uint8_t> config_buf(sizeof(descriptor_));
  memcpy(config_buf.data(), &descriptor_, sizeof(descriptor_));

  zx::result endpoints_res =
      fidl::CreateEndpoints<fuchsia_hardware_usb_function::UsbFunctionInterface>();
  if (endpoints_res.is_error()) {
    logger().log(fdf::ERROR, "Failed to create endpoints");
    return zx::error(endpoints_res.error_value());
  }
  auto [iface_client, iface_server] = std::move(*endpoints_res);
  fidl::BindServer(dispatcher(), std::move(iface_server), this);

  fuchsia_hardware_usb_function::UsbFunctionConfigureRequest config_req;
  config_req.configuration(std::move(config_buf));
  config_req.iface(std::move(iface_client));

  fidl::Result configure_result = function_->Configure(std::move(config_req));
  if (configure_result.is_error()) {
    logger().log(fdf::ERROR, "FakeFtdiFunction: Configure failed: {}",
                 configure_result.error_value().FormatDescription());
    return zx::error(configure_result.error_value().is_framework_error()
                         ? configure_result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

void FakeFtdiFunction::Stop(fdf::StopCompleter completer) { completer(zx::ok()); }

}  // namespace fake_ftdi_function

FUCHSIA_DRIVER_EXPORT2(fake_ftdi_function::FakeFtdiFunction);
