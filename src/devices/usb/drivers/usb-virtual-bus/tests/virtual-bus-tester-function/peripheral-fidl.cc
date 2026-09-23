// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-virtual-bus/tests/virtual-bus-tester-function/peripheral-fidl.h"

#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace virtualbus {

namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace ffunction = fuchsia_hardware_usb_function;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

zx::result<> FidlTestFunction::SetFunctionInterface(bool connect) {
  if (connect) {
    if (binding_) {
      return zx::ok();
    }

    zx::result endpoints =
        fidl::CreateEndpoints<fuchsia_hardware_usb_function::UsbFunctionInterface>();
    if (endpoints.is_error()) {
      return endpoints.take_error();
    }

    binding_ = fidl::BindServer(dispatcher(), std::move(endpoints->server), this);

    std::vector<uint8_t> config_data(sizeof(descriptor_));
    memcpy(config_data.data(), &descriptor_, sizeof(descriptor_));

    fuchsia_hardware_usb_function::UsbFunctionConfigureRequest config_req;
    config_req.configuration(std::move(config_data));
    config_req.iface(std::move(endpoints->client));

    fidl::Result result = function_->Configure(std::move(config_req));
    if (result.is_error()) {
      fdf::error("Configure failed {}", result.error_value().FormatDescription().c_str());
      return zx::error(result.error_value().is_framework_error()
                           ? result.error_value().framework_error().status()
                           : ZX_ERR_INTERNAL);
    }
  } else {
    binding_.reset();
  }
  return zx::ok();
}

zx::result<> FidlTestFunction::Start(fdf::DriverContext context) {
  zx::result client =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (client.is_error()) {
    fdf::error("Failed to connect fidl protocol {}", client);
    return client.take_error();
  }
  function_ = fidl::SyncClient(std::move(*client));

  zx::result ep_out = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (ep_out.is_error()) {
    fdf::error("Failed to create endpoints {}", ep_out);
    return ep_out.take_error();
  }
  ep_out_client_ = std::move(ep_out->client);

  zx::result ep_in = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (ep_in.is_error()) {
    fdf::error("Failed to create endpoints {}", ep_in);
    return ep_in.take_error();
  }
  ep_in_client_ = std::move(ep_in->client);

  std::vector<ffunction::EndpointResource> endpoints;
  endpoints.push_back(
      ffunction::EndpointResource(fdescriptor::EndpointDirection::kOut, std::move(ep_out->server),
                                  fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), 512));

  endpoints.push_back(
      ffunction::EndpointResource(fdescriptor::EndpointDirection::kIn, std::move(ep_in->server),
                                  fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), 512));

  ffunction::UsbFunctionAllocResourcesRequest alloc_req;
  alloc_req.interface_count(1);
  alloc_req.endpoints(std::move(endpoints));

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    fdf::error("AllocResources failed {}", alloc_result.error_value().FormatDescription().c_str());
    return zx::error(alloc_result.error_value().is_framework_error()
                         ? alloc_result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  auto& resp = alloc_result.value();
  descriptor_.interface.b_interface_number = resp.interface_nums()[0];
  descriptor_.bulk_out.b_endpoint_address = resp.endpoint_addrs()[0];
  descriptor_.bulk_in.b_endpoint_address = resp.endpoint_addrs()[1];

  zx_status_t status = bulk_out_ep_.Init(std::move(ep_out_client_), dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to init UsbEndpoint {}", zx_status_get_string(status));
    return zx::error(status);
  }

  status = bulk_in_ep_.Init(std::move(ep_in_client_), dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to init UsbEndpoint {}", zx_status_get_string(status));
    return zx::error(status);
  }

  zx::result<> start_result = TestFunction::Start(std::move(context));
  if (start_result.is_error()) {
    fdf::error("Failed to start {}", start_result);
    return start_result.take_error();
  }

  zx::result<> connect_result = SetFunctionInterface(true);
  if (connect_result.is_error()) {
    fdf::error("Failed to set function interface {}", connect_result);
    return connect_result.take_error();
  }

  return zx::ok();
}

void FidlTestFunction::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  zx::result result = DoControl(request.setup(), request.write());
  if (result.is_error()) {
    completer.Reply(zx::error(result.error_value()));
    return;
  }
  fuchsia_hardware_usb_function::UsbFunctionInterfaceControlResponse resp;
  resp.read(std::move(*result));
  completer.Reply(zx::ok(std::move(resp)));
}

void FidlTestFunction::SetConfigured(SetConfiguredRequest& request,
                                     SetConfiguredCompleter::Sync& completer) {
  if (request.configured()) {
    if (configured_) {
      completer.Reply(zx::ok());
      return;
    }
    configured_ = true;
    fuchsia_hardware_usb_function::EndpointConfiguration config;
    fuchsia_hardware_usb_function::EndpointDescriptor desc;
    desc.bm_attributes(descriptor_.bulk_out.bm_attributes);
    desc.w_max_packet_size(descriptor_.bulk_out.w_max_packet_size);
    desc.b_interval(descriptor_.bulk_out.b_interval);
    config.descriptor(std::move(desc));

    fidl::Result result =
        function_->ConfigureEndpoint({descriptor_.bulk_out.b_endpoint_address, std::move(config)});
    if (result.is_error()) {
      fdf::error("ConfigureEndpoint failed {}", result.error_value().FormatDescription().c_str());
      completer.Reply(zx::error(result.error_value().is_framework_error()
                                    ? result.error_value().framework_error().status()
                                    : ZX_ERR_INTERNAL));
      return;
    }

    config = {};
    desc = {};
    desc.bm_attributes(descriptor_.bulk_in.bm_attributes);
    desc.w_max_packet_size(descriptor_.bulk_in.w_max_packet_size);
    desc.b_interval(descriptor_.bulk_in.b_interval);
    config.descriptor(std::move(desc));

    result =
        function_->ConfigureEndpoint({descriptor_.bulk_in.b_endpoint_address, std::move(config)});
    if (result.is_error()) {
      fdf::error("ConfigureEndpoint failed {}", result.error_value().FormatDescription().c_str());
      completer.Reply(zx::error(result.error_value().is_framework_error()
                                    ? result.error_value().framework_error().status()
                                    : ZX_ERR_INTERNAL));
      return;
    }

  } else {
    configured_ = false;
  }
  completer.Reply(zx::ok());
}

void FidlTestFunction::SetInterface(SetInterfaceRequest& request,
                                    SetInterfaceCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FidlTestFunction::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method {}", metadata.method_ordinal);
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void FidlTestFunction::QueueOut() {
  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.emplace_back(usb::FidlRequest(usb::EndpointType::BULK)
                            .add_data(std::vector<uint8_t>(kMaxPacketSize), kMaxPacketSize)
                            .take_request());
  auto result = bulk_out_ep_->QueueRequests(std::move(requests));
  if (result.is_error()) {
    fdf::error("Failed to QueueRequests {}", result.error_value().FormatDescription().c_str());
    expect_out_->Reply(zx::error(ZX_ERR_INTERNAL));
    expect_out_.reset();
    return;
  }
}

void FidlTestFunction::QueueIn(std::vector<uint8_t> data) {
  size_t size = data.size();
  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.emplace_back(
      usb::FidlRequest(usb::EndpointType::BULK).add_data(std::move(data), size).take_request());
  auto result = bulk_in_ep_->QueueRequests(std::move(requests));
  if (result.is_error()) {
    fdf::error("Failed to QueueRequests {}", result.error_value().FormatDescription().c_str());
    expect_in_->Reply(zx::error(ZX_ERR_INTERNAL));
    expect_in_.reset();
    return;
  }
}

void FidlTestFunction::OutComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& completion : completions) {
    if (!expect_out_) {
      return;
    }

    if (*completion.status() != ZX_OK) {
      expect_out_->Reply(zx::error(*completion.status()));
      expect_out_.reset();
      continue;
    }

    auto req = usb::FidlRequest(std::move(completion.request().value()));
    std::vector<uint8_t> data = std::move((*req->data())[0].buffer()->data().value());
    data.resize(*completion.transfer_size());
    expect_out_->Reply(zx::ok(std::move(data)));
    expect_out_.reset();
  }
}

void FidlTestFunction::InComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& completion : completions) {
    if (!expect_in_) {
      return;
    }

    *completion.status() == ZX_OK ? expect_in_->Reply(zx::ok(*completion.transfer_size()))
                                  : expect_in_->Reply(zx::error(*completion.status()));
    expect_in_.reset();
  }
}

}  // namespace virtualbus

FUCHSIA_DRIVER_EXPORT2(virtualbus::FidlTestFunction);
