// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "two-endpoint-hid-function.h"

#include <assert.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>

#include <memory>
#include <utility>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <usb/peripheral.h>
#include <usb/usb-request.h>

constexpr int BULK_MAX_PACKET = 512;

namespace two_endpoint_hid_function {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

namespace ffdf = fuchsia_driver_framework;
namespace fhidbus = fuchsia_hardware_hidbus;

static const uint8_t boot_mouse_r_desc[50] = {
    0x05, 0x01,  // Usage Page (Generic Desktop Ctrls)
    0x09, 0x02,  // Usage (Mouse)
    0xA1, 0x01,  // Collection (Application)
    0x09, 0x01,  //   Usage (Pointer)
    0xA1, 0x00,  //   Collection (Physical)
    0x05, 0x09,  //     Usage Page (Button)
    0x19, 0x01,  //     Usage Minimum (0x01)
    0x29, 0x03,  //     Usage Maximum (0x03)
    0x15, 0x00,  //     Logical Minimum (0)
    0x25, 0x01,  //     Logical Maximum (1)
    0x95, 0x03,  //     Report Count (3)
    0x75, 0x01,  //     Report Size (1)
    0x81, 0x02,  //     Input (Data,Var,Abs,No Wrap,Linear,No Null Position)
    0x95, 0x01,  //     Report Count (1)
    0x75, 0x05,  //     Report Size (5)
    0x81, 0x03,  //     Input (Const,Var,Abs,No Wrap,Linear,No Null Position)
    0x05, 0x01,  //     Usage Page (Generic Desktop Ctrls)
    0x09, 0x30,  //     Usage (X)
    0x09, 0x31,  //     Usage (Y)
    0x15, 0x81,  //     Logical Minimum (-127)
    0x25, 0x7F,  //     Logical Maximum (127)
    0x75, 0x08,  //     Report Size (8)
    0x95, 0x02,  //     Report Count (2)
    0x81, 0x06,  //     Input (Data,Var,Rel,No Wrap,Linear,No Null Position)
    0xC0,        //   End Collection
    0xC0,        // End Collection
};

FakeUsbHidFunction::FakeUsbHidFunction() : fdf::DriverBase2(kDriverName) {}

void FakeUsbHidFunction::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  const auto& setup = request.setup();
  const std::vector<uint8_t>& write = request.write();

  if (setup.bm_request_type() == (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE)) {
    if (setup.b_request() == USB_REQ_GET_DESCRIPTOR) {
      completer.Reply(zx::ok(report_desc_));
      return;
    }
  }
  if (setup.bm_request_type() == (USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE)) {
    if (setup.b_request() == USB_HID_GET_REPORT) {
      completer.Reply(zx::ok(report_));
      return;
    }
    if (setup.b_request() == USB_HID_GET_PROTOCOL) {
      std::vector<uint8_t> data(sizeof(hid_protocol_));
      memcpy(data.data(), &hid_protocol_, sizeof(hid_protocol_));
      completer.Reply(zx::ok(data));
      return;
    }
  }
  if (setup.b_request() == USB_HID_SET_REPORT) {
    report_ = write;
    completer.Reply(zx::ok(std::vector<uint8_t>{}));
    return;
  }
  if (setup.b_request() == USB_HID_SET_PROTOCOL) {
    hid_protocol_ = static_cast<fhidbus::wire::HidProtocol>(setup.w_value());
    completer.Reply(zx::ok(std::vector<uint8_t>{}));
    return;
  }
  completer.Reply(zx::error(ZX_ERR_IO_REFUSED));
}

void FakeUsbHidFunction::SetConfigured(SetConfiguredRequest& request,
                                       SetConfiguredCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeUsbHidFunction::SetInterface(SetInterfaceRequest& request,
                                      SetInterfaceCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeUsbHidFunction::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method: {}", metadata.method_ordinal);
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> FakeUsbHidFunction::Start(fdf::DriverContext context) {
  zx::result client_end =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (client_end.is_error()) {
    fdf::error("could not connect to UsbFunctionService: {}", client_end);
    return client_end.take_error();
  }
  function_.Bind(std::move(*client_end));

  report_desc_.resize(sizeof(boot_mouse_r_desc));
  memcpy(report_desc_.data(), &boot_mouse_r_desc, sizeof(boot_mouse_r_desc));
  report_.resize(3);

  descriptor_size_ = sizeof(fake_usb_hid_descriptor_t) + sizeof(usb_hid_descriptor_entry_t);
  descriptor_.reset(static_cast<fake_usb_hid_descriptor_t*>(calloc(1, descriptor_size_)));
  descriptor_->interface = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = 0,
      .b_alternate_setting = 0,
      .b_num_endpoints = 1,
      .b_interface_class = USB_CLASS_HID,
      .b_interface_sub_class = USB_HID_SUBCLASS_BOOT,
      .b_interface_protocol = USB_HID_PROTOCOL_MOUSE,
      .i_interface = 0,
  };
  descriptor_->interrupt_in = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = USB_ENDPOINT_IN,  // set later
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
      .w_max_packet_size = htole16(BULK_MAX_PACKET),
      .b_interval = 8,
  };
  descriptor_->interrupt_out = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = USB_ENDPOINT_OUT,  // set later
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
      .w_max_packet_size = htole16(BULK_MAX_PACKET),
      .b_interval = 8,
  };
  descriptor_->hid_descriptor = {
      .bLength = sizeof(usb_hid_descriptor_t) + sizeof(usb_hid_descriptor_entry_t),
      .bDescriptorType = USB_DT_HID,
      .bcdHID = 0,
      .bCountryCode = 0,
      .bNumDescriptors = 1,
  };
  descriptor_->hid_descriptor.descriptors[0] = {
      .bDescriptorType = 0x22,  // HID TYPE REPORT
      .wDescriptorLength = static_cast<uint16_t>(report_desc_.size()),
  };

  zx::result endpoints_res = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (endpoints_res.is_error()) {
    fdf::error("CreateEndpoints failed: {}", endpoints_res);
    return endpoints_res.take_error();
  }
  zx::result endpoints_out_res = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (endpoints_out_res.is_error()) {
    fdf::error("CreateEndpoints failed: {}", endpoints_out_res);
    return endpoints_out_res.take_error();
  }

  std::vector<fuchsia_hardware_usb_function::EndpointResource> endpoints;
  endpoints.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kIn, std::move(endpoints_res->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithInterrupt({}), BULK_MAX_PACKET));
  endpoints.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kOut,
      std::move(endpoints_out_res->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithInterrupt({}), BULK_MAX_PACKET));

  fuchsia_hardware_usb_function::UsbFunctionAllocResourcesRequest alloc_req;
  alloc_req.interface_count() = 1;
  alloc_req.endpoints() = std::move(endpoints);

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    fdf::error("AllocResources failed: {}", alloc_result.error_value().FormatDescription());
    return zx::error(ZX_ERR_INTERNAL);
  }

  descriptor_->interface.b_interface_number = alloc_result->interface_nums()[0];
  if (alloc_result->endpoint_addrs().size() > 1) {
    descriptor_->interrupt_in.b_endpoint_address = alloc_result->endpoint_addrs()[0];
    descriptor_->interrupt_out.b_endpoint_address = alloc_result->endpoint_addrs()[1];
  }

  endpoints_.push_back(std::move(endpoints_res->client));

  out_ep_.Init(std::move(endpoints_out_res->client), dispatcher());

  std::vector<uint8_t> desc(descriptor_size_);
  memcpy(desc.data(), descriptor_.get(), descriptor_size_);

  zx::result iface_endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_usb_function::UsbFunctionInterface>();
  if (iface_endpoints.is_error()) {
    fdf::error("CreateEndpoints failed: {}", iface_endpoints);
    return iface_endpoints.take_error();
  }

  fuchsia_hardware_usb_function::UsbFunctionConfigureRequest config_req;
  config_req.configuration() = std::move(desc);
  config_req.iface() = std::move(iface_endpoints->client);

  fidl::Result config_result = function_->Configure(std::move(config_req));
  if (config_result.is_error()) {
    fdf::error("Configure failed: {}", config_result.error_value().FormatDescription());
    return zx::error(ZX_ERR_INTERNAL);
  }

  active_ = true;

  out_ep_.AddRequests(1, BULK_MAX_PACKET, fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);

  std::optional req = out_ep_.GetRequest();
  FDF_ASSERT(req.has_value());

  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.push_back(req->take_request());
  fuchsia_hardware_usb_endpoint::EndpointQueueRequestsRequest queue_req;
  queue_req.req() = std::move(requests);
  fit::result<fidl::OneWayError> queue_result = out_ep_->QueueRequests(std::move(queue_req));
  if (queue_result.is_error()) {
    fdf::error("QueueRequests failed: {}", queue_result.error_value().FormatDescription());
  }

  fidl::BindServer(dispatcher(), std::move(iface_endpoints->server), this);

  fuchsia_driver_framework::DevfsAddArgs devfs_args{};
  std::vector<ffdf::NodeProperty> props{};
  std::vector<ffdf::Offer> offers{};

  zx::result result = AddChild(name(), devfs_args, props, offers);
  if (result.is_error()) {
    fdf::error("AddChild(): {}", result);
    return result.take_error();
  }

  return zx::ok();
}

void FakeUsbHidFunction::UsbEndpointOutCallback(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  for (auto& completion : completions) {
    if (*completion.status() == ZX_OK) {
      usb::FidlRequest wrapped_req(std::move(*completion.request()));
      report_.resize(*completion.transfer_size());
      zx::result<std::vector<size_t>> res = wrapped_req.CachedCopyFrom(
          0, report_.data(), *completion.transfer_size(), out_ep_.GetMapped());
      ZX_ASSERT(res.is_ok());
      out_ep_.PutRequest(std::move(wrapped_req));
    } else {
      fdf::error("request status: {}", zx_status_get_string(*completion.status()));
      active_ = false;
      if (completion.request().has_value()) {
        out_ep_.PutRequest(usb::FidlRequest(std::move(*completion.request())));
      }
    }
  }

  if (active_) {
    std::optional req = out_ep_.GetRequest();
    if (req) {
      std::vector<fuchsia_hardware_usb_request::Request> requests;
      requests.push_back(req->take_request());
      fuchsia_hardware_usb_endpoint::EndpointQueueRequestsRequest queue_req;
      queue_req.req() = std::move(requests);
      auto queue_result = out_ep_->QueueRequests(std::move(queue_req));
      if (queue_result.is_error()) {
        fdf::error("QueueRequests failed: {}", queue_result.error_value().status());
      }
    }
  }
}

}  // namespace two_endpoint_hid_function

FUCHSIA_DRIVER_EXPORT2(two_endpoint_hid_function::FakeUsbHidFunction);
