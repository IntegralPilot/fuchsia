// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.ax88179/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.request/cpp/fidl.h>
#include <fuchsia/hardware/ethernet/cpp/banjo.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>

#include <algorithm>
#include <memory>
#include <optional>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <fbl/mutex.h>
#include <usb-endpoint/usb-endpoint-client.h>
#include <usb/cdc.h>
#include <usb/request-fidl.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

namespace fake_usb_ax88179_function {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

constexpr int BULK_MAX_PACKET = 512;
constexpr size_t INTR_MAX_PACKET = 8;

// Acts as a fake USB device for asix-88179 tests. Currently only partially
// implemented for initialization order regression test.

class FakeUsbAx88179Function;

class FakeUsbAx88179Function
    : public fdf::DriverBase2,
      public fidl::WireServer<fuchsia_hardware_ax88179::Hooks>,
      public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  static constexpr std::string kDriverName = "FakeUsbAx88179Function";

  FakeUsbAx88179Function()
      : fdf::DriverBase2(kDriverName),
        connector_{fit::bind_member<&FakeUsbAx88179Function::DevfsConnect>(this)} {}

  zx::result<> Start(fdf::DriverContext context) override;

  // UsbFunctionInterface:
  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override;
  void SetConfigured(SetConfiguredRequest& request,
                     SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Hooks:
  void SetOnline(SetOnlineRequestView request, SetOnlineCompleter::Sync& completer) override;

 private:
  void DevfsConnect(fidl::ServerEnd<fuchsia_hardware_ax88179::Hooks> req);

  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;

  struct {
    usb_interface_descriptor_t interface;
    usb_endpoint_descriptor_t bulk_in;
    usb_endpoint_descriptor_t bulk_out;
    usb_endpoint_descriptor_t intr_ep;
  } __PACKED descriptor_;

  size_t descriptor_size_ = 0;
  uint8_t intr_addr_ = 0;

  void IntrComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completion);

  usb::EndpointClient<FakeUsbAx88179Function> intr_ep_{
      usb::EndpointType::INTERRUPT, this, std::mem_fn(&FakeUsbAx88179Function::IntrComplete)};

  fbl::Mutex mtx_;

  bool configured_ = false;

  std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_function::UsbFunctionInterface>>
      binding_;
  fidl::ServerBindingGroup<fuchsia_hardware_ax88179::Hooks> bindings_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> child_;
  driver_devfs::Connector<fuchsia_hardware_ax88179::Hooks> connector_;
};

void FakeUsbAx88179Function::SetOnline(SetOnlineRequestView request,
                                       SetOnlineCompleter::Sync& completer) {
  fbl::AutoLock lock(&mtx_);

  constexpr size_t kInterruptRequestSize = 8;
  uint8_t status[kInterruptRequestSize];
  memset(&status, 0, sizeof(status));
  status[2] = request->online;

  std::optional<usb::FidlRequest> req = intr_ep_.GetRequest();
  ZX_ASSERT(req.has_value());

  zx::result<std::vector<size_t>> actual =
      req->CachedCopyTo(0, status, sizeof(status), intr_ep_.GetMapped());
  ZX_ASSERT(actual.is_ok());
  ZX_ASSERT(actual->size() == 1);
  ZX_ASSERT((*actual)[0] == sizeof(status));

  std::vector<fuchsia_hardware_usb_request::Request> reqs;
  reqs.emplace_back(req->take_request());
  ZX_ASSERT(intr_ep_->QueueRequests({std::move(reqs)}).is_ok());

  completer.Reply(ZX_OK);
}

zx::result<> FakeUsbAx88179Function::Start(fdf::DriverContext context) {
  fbl::AutoLock lock(&mtx_);

  descriptor_size_ = sizeof(descriptor_);
  descriptor_.interface = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = 0,
      .b_alternate_setting = 0,
      .b_num_endpoints = 3,
      .b_interface_class = USB_CLASS_COMM,
      .b_interface_sub_class = USB_CDC_SUBCLASS_ETHERNET,
      .b_interface_protocol = 1,
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
  descriptor_.intr_ep = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = 0,  // set later
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
      .w_max_packet_size = htole16(INTR_MAX_PACKET),
      .b_interval = 8,
  };

  zx::result func =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (func.is_error()) {
    fdf::error("Failed to connect to usb endpoint service: {}", func);
    return func.take_error();
  }
  function_.Bind(std::move(*func));

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }

  std::vector<fuchsia_hardware_usb_function::EndpointResource> resources;
  resources.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kIn, std::move(endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithInterrupt({}), INTR_MAX_PACKET));

  zx::result bulk_in_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_in_endpoints.is_error()) {
    return bulk_in_endpoints.take_error();
  }
  resources.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kIn, std::move(bulk_in_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), BULK_MAX_PACKET));

  zx::result bulk_out_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_out_endpoints.is_error()) {
    return bulk_out_endpoints.take_error();
  }
  resources.push_back(fuchsia_hardware_usb_function::EndpointResource(
      fuchsia_hardware_usb_descriptor::EndpointDirection::kOut,
      std::move(bulk_out_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), BULK_MAX_PACKET));

  fidl::Request<fuchsia_hardware_usb_function::UsbFunction::AllocResources> alloc_req;
  alloc_req.interface_count(1);
  alloc_req.endpoints(std::move(resources));

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    fdf::error("AllocResources failed: {}", alloc_result.error_value().FormatDescription());
    return zx::error(alloc_result.error_value().is_framework_error()
                         ? alloc_result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  auto& response = alloc_result.value();
  descriptor_.interface.b_interface_number = response.interface_nums()[0];
  descriptor_.intr_ep.b_endpoint_address = response.endpoint_addrs()[0];
  descriptor_.bulk_in.b_endpoint_address = response.endpoint_addrs()[1];
  descriptor_.bulk_out.b_endpoint_address = response.endpoint_addrs()[2];

  intr_addr_ = descriptor_.intr_ep.b_endpoint_address;

  zx_status_t status = intr_ep_.Init(std::move(endpoints->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("Could not init usb endpoint client: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  size_t actual =
      intr_ep_.AddRequests(1, INTR_MAX_PACKET, fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual != 1) {
    fdf::error("Could not allocate endpoint request");
    return zx::error(ZX_ERR_INTERNAL);
  }

  fuchsia_hardware_ax88179::Service::InstanceHandler handler({
      .hooks = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
  zx::result serve = outgoing()->AddService<fuchsia_hardware_ax88179::Service>(std::move(handler));
  if (serve.is_error()) {
    fdf::error("Failed to serve Hooks service: {}", serve);
    return serve.take_error();
  }

  zx::result connector = connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("connector_.Bind(): {}", connector);
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args{};
  devfs_args.connector(std::move(*connector));
  devfs_args.class_name("test-asix-function");

  std::vector<fuchsia_driver_framework::NodeProperty> props{};
  std::vector offers{fdf::MakeOffer2<fuchsia_hardware_ax88179::Service>()};

  zx::result child = AddChild(name(), devfs_args, props, offers);
  if (child.is_error()) {
    fdf::error("AddChild: {}", child);
    return child.take_error();
  }
  child_.Bind(std::move(*child));

  zx::result iface_endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_usb_function::UsbFunctionInterface>();
  if (iface_endpoints.is_error()) {
    return iface_endpoints.take_error();
  }
  binding_ = fidl::BindServer(this->dispatcher(), std::move(iface_endpoints->server), this);

  std::vector<uint8_t> descriptors_buffer(descriptor_size_);
  memcpy(descriptors_buffer.data(), &descriptor_, descriptor_size_);

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
void FakeUsbAx88179Function::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_ax88179::Hooks> req) {
  bindings_.AddBinding(dispatcher(), std::move(req), this, fidl::kIgnoreBindingClosure);
}

void FakeUsbAx88179Function::Control(ControlRequest& request, ControlCompleter::Sync& completer) {
  completer.Reply(zx::ok(std::vector<uint8_t>{}));
}

void FakeUsbAx88179Function::SetConfigured(SetConfiguredRequest& request,
                                           SetConfiguredCompleter::Sync& completer) {
  fbl::AutoLock lock(&mtx_);

  if (request.configured()) {
    if (configured_) {
      completer.Reply(zx::ok());
      return;
    }
    fuchsia_hardware_usb_function::EndpointConfiguration ep_config;
    fuchsia_hardware_usb_function::EndpointDescriptor desc;
    desc.bm_attributes(descriptor_.intr_ep.bm_attributes);
    desc.w_max_packet_size(le16toh(descriptor_.intr_ep.w_max_packet_size));
    desc.b_interval(descriptor_.intr_ep.b_interval);
    ep_config.descriptor(std::move(desc));

    fidl::Result result = function_->ConfigureEndpoint(
        {descriptor_.intr_ep.b_endpoint_address, std::move(ep_config)});
    if (result.is_error()) {
      fdf::error("usb-ax88179-function: ConfigureEndpoint failed: {}",
                 result.error_value().FormatDescription());
      completer.Reply(zx::error(result.error_value().is_framework_error()
                                    ? result.error_value().framework_error().status()
                                    : ZX_ERR_INTERNAL));
      return;
    }
    configured_ = true;
  } else {
    fidl::Result result = function_->DisableEndpoint({descriptor_.intr_ep.b_endpoint_address});
    if (result.is_error()) {
      fdf::error("usb-ax88179-function: DisableEndpoint failed: {}",
                 result.error_value().FormatDescription());
      completer.Reply(zx::error(result.error_value().is_framework_error()
                                    ? result.error_value().framework_error().status()
                                    : ZX_ERR_INTERNAL));
      return;
    }
    configured_ = false;
  }
  completer.Reply(zx::ok());
}

void FakeUsbAx88179Function::SetInterface(SetInterfaceRequest& request,
                                          SetInterfaceCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeUsbAx88179Function::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method %ld", metadata.method_ordinal);
}

void FakeUsbAx88179Function::IntrComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completion) {
  for (auto& c : completion) {
    usb::FidlRequest req{std::move(c.request().value())};
    intr_ep_.PutRequest(std::move(req));
  }
}

}  // namespace fake_usb_ax88179_function

FUCHSIA_DRIVER_EXPORT2(fake_usb_ax88179_function::FakeUsbAx88179Function);
