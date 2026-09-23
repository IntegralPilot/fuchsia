// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-hid.h"

#include <endian.h>
#include <fuchsia/hardware/usb/c/banjo.h>
#include <fuchsia/hardware/usb/cpp/banjo.h>
#include <fuchsia/hardware/usb/descriptor/c/banjo.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/sync/completion.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <array>
#include <cstdint>
#include <string>
#include <string_view>
#include <thread>

#include <fbl/auto_lock.h>
#include <pretty/hexdump.h>
#include <usb/hid.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

namespace usb_hid {

namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace fhidbus = fuchsia_hardware_hidbus;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

#define to_usb_hid(d) containerof(d, usb_hid_device_t, hiddev)

// This driver binds on any USB device that exposes HID reports. It passes the
// reports to the HID driver by implementing the HidBus protocol.

namespace {

constexpr uint16_t kLanguageEnglishUnitedStates = 0x0409;

zx::result<std::string> GetUsbStringDescriptor(const ddk::UsbProtocolClient& usb,
                                               uint8_t descriptor_id) {
  if (descriptor_id == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  std::array<uint8_t, fuchsia_hardware_hidbus::wire::kMaxNameLength> bytes;
  size_t actual_size = 0;
  uint16_t actual_language_id = 0;
  zx_status_t status =
      usb.GetStringDescriptor(descriptor_id, kLanguageEnglishUnitedStates, &actual_language_id,
                              bytes.data(), bytes.size(), &actual_size);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  if (actual_size == 0) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  return zx::ok(std::string(reinterpret_cast<char*>(bytes.data()), actual_size));
}

}  // namespace

UsbHidbus::UsbHidbus() : fdf::DriverBase2(kDriverName) {}

UsbHidbus::~UsbHidbus() {
  if (unbind_thread_.joinable()) {
    unbind_thread_.join();
  }
}

void UsbHidbus::HandleBatchInterrupt(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
  for (auto& completion : completions) {
    HandleInterrupt(std::move(completion));
  }
}

void UsbHidbus::HandleInterrupt(fendpoint::Completion completion) {
  ZX_ASSERT(completion.request().has_value());
  ZX_ASSERT(completion.status().has_value());
  ZX_ASSERT(completion.transfer_size().has_value());

  usb::FidlRequest req(std::move(completion.request().value()));
  std::vector<uint8_t> buffer(*completion.transfer_size());
  auto actual = req.CopyFrom(0, buffer.data(), *completion.transfer_size(), ep_in_.GetMapped());
  ZX_ASSERT(actual.size() == 1);
  ZX_ASSERT(actual[0] == *completion.transfer_size());
  fdf::trace("usb-hid: callback request status {}", *completion.status());
  if (zxlog_level_enabled(TRACE)) {
    hexdump(buffer.data(), *completion.transfer_size());
  }

  bool requeue = true;
  switch (*completion.status()) {
    case ZX_ERR_IO_NOT_PRESENT:
      requeue = false;
      break;
    case ZX_OK:
      if (started_ && binding_) {
        fidl::Arena arena;
        auto report =
            fhidbus::wire::Report::Builder(arena)
                .buf(fidl::VectorView<uint8_t>::FromExternal(buffer.data(), buffer.size()))
                .timestamp(zx_clock_get_monotonic());
        if (completion.wake_lease().has_value()) {
          report.wake_lease(std::move(completion.wake_lease().value()));
        }
        auto result = fidl::WireSendEvent(*binding_)->OnReportReceived(report.Build());
        if (!result.ok()) {
          fdf::error("OnReportReceived failed: {}", result.error().FormatDescription());
        }
      }
      break;
    default:
      fdf::error("Not requeuing req: Unknown interrupt status {}", *completion.status());
      requeue = false;
      break;
  }

  if (requeue) {
    req.reset_buffers(ep_in_.GetMapped());
    req.CacheFlushInvalidate(ep_in_.GetMapped());
    std::vector<fuchsia_hardware_usb_request::Request> requests;
    requests.push_back(req.take_request());
    auto result = ep_in_->QueueRequests(std::move(requests));
    if (result.is_error()) {
      fdf::error("Failed to queue requests: {}", result.error_value().FormatDescription());
    }
  } else {
    ep_in_.PutRequest(std::move(req));
  }
}

void UsbHidbus::Query(QueryCompleter::Sync& completer) { completer.ReplySuccess(info_); }

void UsbHidbus::Start(
    fidl::WireServer<fuchsia_hardware_hidbus::Hidbus>::StartCompleter::Sync& completer) {
  if (started_) {
    fdf::error("Already started");
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }

  started_ = true;
  auto req = ep_in_.GetRequest();
  if (req.has_value()) {
    req->reset_buffers(ep_in_.GetMapped());
    req->CacheFlushInvalidate(ep_in_.GetMapped());
    std::vector<fuchsia_hardware_usb_request::Request> requests;
    requests.push_back(req->take_request());
    auto result = ep_in_->QueueRequests(std::move(requests));
    if (result.is_error()) {
      fdf::error("Failed to queue requests: {}", result.error_value().FormatDescription());
    }
  }
  completer.ReplySuccess();
}

void UsbHidbus::Stop(StopCompleter::Sync& completer) { StopHidbus(); }

void UsbHidbus::StopHidbus() {
  started_ = false;
  // TODO(tkilbourn) set flag to stop requeueing the interrupt request when we start using this
  // callback
  if (set_report_completer_.has_value()) {
    set_report_completer_->ReplyError(ZX_ERR_IO_NOT_PRESENT);
  }
}

zx_status_t UsbHidbus::UsbHidControlIn(uint8_t req_type, uint8_t request, uint16_t value,
                                       uint16_t index, void* data, size_t length,
                                       size_t* out_length) {
  zx_status_t status;
  status = usb_.ControlIn(req_type, request, value, index, ZX_TIME_INFINITE,
                          reinterpret_cast<uint8_t*>(data), length, out_length);
  if (status == ZX_ERR_IO_REFUSED || status == ZX_ERR_IO_INVALID) {
    status = usb_.ResetEndpoint(0);
  }
  return status;
}

zx_status_t UsbHidbus::UsbHidControlOut(uint8_t req_type, uint8_t request, uint16_t value,
                                        uint16_t index, const void* data, size_t length,
                                        size_t* out_length) {
  zx_status_t status;
  status = usb_.ControlOut(req_type, request, value, index, ZX_TIME_INFINITE,
                           reinterpret_cast<const uint8_t*>(data), length);
  if (status == ZX_ERR_IO_REFUSED || status == ZX_ERR_IO_INVALID) {
    status = usb_.ResetEndpoint(0);
  }
  return status;
}

void UsbHidbus::GetDescriptor(fhidbus::wire::HidbusGetDescriptorRequest* request,
                              GetDescriptorCompleter::Sync& completer) {
  int desc_idx = -1;
  for (int i = 0; i < hid_desc_->bNumDescriptors; i++) {
    if (hid_desc_->descriptors[i].bDescriptorType == static_cast<uint16_t>(request->desc_type)) {
      desc_idx = i;
      break;
    }
  }
  if (desc_idx < 0) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  size_t desc_len = hid_desc_->descriptors[desc_idx].wDescriptorLength;
  std::vector<uint8_t> desc;
  desc.resize(desc_len);
  zx_status_t status =
      UsbHidControlIn(USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE, USB_REQ_GET_DESCRIPTOR,
                      static_cast<uint16_t>(static_cast<uint16_t>(request->desc_type) << 8),
                      interface_, desc.data(), desc_len, nullptr);
  if (status < 0) {
    fdf::error("Failed to read report descriptor {:#02x}: {}",
               static_cast<uint16_t>(request->desc_type), zx_status_get_string(status));
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(desc.data(), desc.size()));
}

void UsbHidbus::GetReport(fhidbus::wire::HidbusGetReportRequest* request,
                          GetReportCompleter::Sync& completer) {
  std::vector<uint8_t> report;
  report.resize(request->len);
  size_t actual;
  auto status = UsbHidControlIn(
      USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_HID_GET_REPORT,
      static_cast<uint16_t>(static_cast<uint16_t>(request->rpt_type) << 8 | request->rpt_id),
      interface_, report.data(), report.size(), &actual);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  report.resize(actual);
  completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(report.data(), report.size()));
}

void UsbHidbus::SetReportComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& completion : completions) {
    ep_out_->PutRequest(usb::FidlRequest(std::move(*completion.request())));

    if (!set_report_completer_.has_value()) {
      // Shutting down. Probably has already replied.
      continue;
    }

    auto completer = std::move(*set_report_completer_);
    set_report_completer_.reset();
    if (*completion.status() == ZX_OK) {
      completer.ReplySuccess();
      continue;
    }
    completer.ReplyError(*completion.status());
  }
}

void UsbHidbus::SetReport(fhidbus::wire::HidbusSetReportRequest* request,
                          SetReportCompleter::Sync& completer) {
  if (ep_out_.has_value()) {
    if (set_report_completer_.has_value()) {
      completer.ReplyError(ZX_ERR_SHOULD_WAIT);
      return;
    }

    auto req = ep_out_->GetRequest();
    if (!req.has_value()) {
      completer.ReplyError(ZX_ERR_SHOULD_WAIT);
      return;
    }
    auto actual = req->CopyTo(0, request->data.data(), request->data.size(), ep_out_->GetMapped());
    ZX_ASSERT(actual.size() == 1);
    if (request->data.size() != actual[0]) {
      completer.ReplyError(ZX_ERR_BUFFER_TOO_SMALL);
      return;
    }
    (*req)->data()->at(0).size(actual[0]);
    auto status = req->CacheFlush(ep_out_->GetMappedLocked());
    if (status != ZX_OK) {
      fdf::error("Failed to flush cache: {}", zx_status_get_string(status));
    }
    std::vector<fuchsia_hardware_usb_request::Request> requests;
    requests.push_back(req->take_request());
    set_report_completer_ = completer.ToAsync();
    auto result = (*ep_out_)->QueueRequests(std::move(requests));
    if (result.is_error()) {
      fdf::error("Failed to queue requests: {}", result.error_value().FormatDescription());
      set_report_completer_->ReplyError(result.error_value().status());
    }
    return;
  }
  auto status = UsbHidControlOut(
      USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_HID_SET_REPORT,
      (static_cast<uint16_t>(static_cast<uint16_t>(request->rpt_type) << 8 | request->rpt_id)),
      interface_, request->data.data(), request->data.size(), NULL);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess();
}

void UsbHidbus::GetIdle(fhidbus::wire::HidbusGetIdleRequest* request,
                        GetIdleCompleter::Sync& completer) {
  uint8_t duration;
  auto status = UsbHidControlIn(USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_HID_GET_IDLE,
                                request->rpt_id, interface_, &duration, sizeof(duration), NULL);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess(duration);
}

void UsbHidbus::SetIdle(fhidbus::wire::HidbusSetIdleRequest* request,
                        SetIdleCompleter::Sync& completer) {
  auto status = UsbHidControlOut(
      USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_HID_SET_IDLE,
      static_cast<uint16_t>((request->duration << 8) | request->rpt_id), interface_, NULL, 0, NULL);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess();
}

void UsbHidbus::GetProtocol(GetProtocolCompleter::Sync& completer) {
  uint8_t protocol;
  auto status =
      UsbHidControlIn(USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_HID_GET_PROTOCOL, 0,
                      interface_, &protocol, sizeof(protocol), NULL);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess(static_cast<fhidbus::wire::HidProtocol>(protocol));
}

void UsbHidbus::SetProtocol(fhidbus::wire::HidbusSetProtocolRequest* request,
                            SetProtocolCompleter::Sync& completer) {
  auto status =
      UsbHidControlOut(USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_HID_SET_PROTOCOL,
                       static_cast<uint8_t>(request->protocol), interface_, NULL, 0, NULL);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess();
}

void UsbHidbus::Stop(fdf::StopCompleter completer) {
  usb_desc_iter_release(&desc_iter_);
  unbind_thread_ = std::thread([this, completer = std::move(completer)]() mutable {
    ep_in_->CancelAll().Then([](fidl::Result<fendpoint::Endpoint::CancelAll>& result) {
      if (result.is_error()) {
        fdf::error("Failed to cancel all for in endpoint: {}",
                   result.error_value().FormatDescription().c_str());
      }
    });
    if (ep_out_.has_value()) {
      (*ep_out_)->CancelAll().Then([](fidl::Result<fendpoint::Endpoint::CancelAll>& result) {
        if (result.is_error()) {
          fdf::error("Failed to cancel all for out endpoint: {}",
                     result.error_value().FormatDescription().c_str());
        }
      });
    }
    completer(zx::ok());
  });
}

void UsbHidbus::FindDescriptors(usb::Interface interface, const usb_hid_descriptor_t** hid_desc,
                                const usb_endpoint_descriptor_t** endptin,
                                const usb_endpoint_descriptor_t** endptout) {
  for (const auto& descriptor : interface.GetDescriptorList()) {
    if (descriptor.b_descriptor_type == USB_DT_HID) {
      *hid_desc = reinterpret_cast<const usb_hid_descriptor_t*>(&descriptor);
    } else if (descriptor.b_descriptor_type == USB_DT_ENDPOINT) {
      auto endpt_desc = reinterpret_cast<const usb_endpoint_descriptor_t*>(&descriptor);
      if (usb_ep_type(endpt_desc) == fdescriptor::EndpointType::kInterrupt) {
        switch (usb_ep_direction(endpt_desc)) {
          case USB_ENDPOINT_IN:
            *endptin = endpt_desc;
            break;
          case USB_ENDPOINT_OUT:
            *endptout = endpt_desc;
            break;
          default:
            break;
        }
      }
    }
  }
}

zx::result<> UsbHidbus::Start(fdf::DriverContext context) {
  auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result usb_banjo = compat::ConnectBanjo<ddk::UsbProtocolClient>(incoming_ptr);
  if (usb_banjo.is_error()) {
    fdf::error("Failed to connect to usb banjo: {}", usb_banjo);
    return usb_banjo.take_error();
  }
  usb_ = usb_banjo.value();

  zx::result usb_fidl = incoming_ptr->Connect<fuchsia_hardware_usb::UsbService::Device>();
  if (usb_fidl.is_error()) {
    fdf::error("Failed to connect to usb fidl: {}", usb_fidl.is_error());
    return usb_fidl.take_error();
  }

  dispatcher_loop_.StartThread("usb-hid-dispatcher-loop");

  zx_status_t status;

  usb_device_descriptor_t device_desc;
  usb_.GetDeviceDescriptor(&device_desc);
  auto info_builder = fhidbus::wire::HidInfo::Builder(arena_);
  info_builder.vendor_id(le16toh(device_desc.id_vendor))
      .product_id(le16toh(device_desc.id_product))
      .version(0);

  zx::result<std::string> manufacturer_result =
      GetUsbStringDescriptor(usb_, device_desc.i_manufacturer);
  if (manufacturer_result.is_ok()) {
    info_builder.manufacturer_name(arena_, manufacturer_result.value());
  } else {
    zxlogf(WARNING, "Failed to get manufacturer name: %s",
           zx_status_get_string(manufacturer_result.status_value()));
  }

  zx::result<std::string> product_result = GetUsbStringDescriptor(usb_, device_desc.i_product);
  if (product_result.is_ok()) {
    info_builder.product_name(arena_, product_result.value());
  } else {
    zxlogf(WARNING, "Failed to get product name: %s",
           zx_status_get_string(product_result.status_value()));
  }

  zx::result<std::string> serial_result = GetUsbStringDescriptor(usb_, device_desc.i_serial_number);
  if (serial_result.is_ok()) {
    info_builder.serial_number(arena_, serial_result.value());
  } else {
    zxlogf(WARNING, "Failed to get serial number: %s",
           zx_status_get_string(serial_result.status_value()));
  }

  parent_req_size_ = usb_.GetRequestSize();
  status = usb::InterfaceList::Create(usb_, true, &usb_interface_list_);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  const usb_hid_descriptor_t* hid_desc = NULL;
  const usb_endpoint_descriptor_t* endptin = NULL;
  const usb_endpoint_descriptor_t* endptout = NULL;
  auto interface = *usb_interface_list_->begin();

  FindDescriptors(interface, &hid_desc, &endptin, &endptout);
  if (!hid_desc) {
    status = ZX_ERR_NOT_SUPPORTED;
    return zx::error(status);
  }
  if (!endptin) {
    status = ZX_ERR_NOT_SUPPORTED;
    return zx::error(status);
  }
  hid_desc_ = hid_desc;
  status =
      ep_in_.Init(endptin->b_endpoint_address, usb_fidl.value(), dispatcher_loop_.dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to init IN ep: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  // Calculation according to 9.6.6 of USB2.0 Spec for interrupt endpoints
  switch (auto speed = usb_.GetSpeed()) {
    case USB_SPEED_LOW:
    case USB_SPEED_FULL:
      if (endptin->b_interval > 255 || endptin->b_interval < 1) {
        fdf::error("bInterval for LOW/FULL Speed EPs must be between 1 and 255. bInterval = {}",
                   endptin->b_interval);
        return zx::error(ZX_ERR_OUT_OF_RANGE);
      }
      info_builder.polling_rate(zx::msec(endptin->b_interval).to_usecs());
      break;
    case USB_SPEED_HIGH:
      if (endptin->b_interval > 16 || endptin->b_interval < 1) {
        fdf::error("bInterval for HIGH Speed EPs must be between 1 and 16. bInterval = {}",
                   endptin->b_interval);
        return zx::error(ZX_ERR_OUT_OF_RANGE);
      }
      info_builder.polling_rate((uint64_t{1} << (endptin->b_interval - 1)) *
                                zx::usec(125).to_usecs());
      break;
    default:
      fdf::error("Unrecognized USB Speed {}", speed);
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  if (endptout) {
    ep_out_.emplace(usb::EndpointType::INTERRUPT, this, std::mem_fn(&UsbHidbus::SetReportComplete));
    status = ep_out_->Init(endptout->b_endpoint_address, usb_fidl.value(),
                           dispatcher_loop_.dispatcher());
    if (status != ZX_OK) {
      fdf::error("Failed to init IN ep {}", status);
      return zx::error(status);
    }
    auto actual = ep_out_->AddRequests(1, usb_ep_max_packet(endptout),
                                       fuchsia_hardware_usb_request::Buffer::Tag::kData);
    if (actual == 0) {
      fdf::error("Could not add any requests!");
      return zx::error(ZX_ERR_INTERNAL);
    }
    if (actual != 1) {
      fdf::warn("Wanted {} request, got {} requests", 1, actual);
    }
  }

  interface_ = interface.descriptor()->b_interface_number;
  info_builder.dev_num(interface_);
  if (interface.descriptor()->b_interface_protocol == USB_HID_PROTOCOL_KBD) {
    info_builder.boot_protocol(fhidbus::wire::HidBootProtocol::kKbd);
  } else if (interface.descriptor()->b_interface_protocol == USB_HID_PROTOCOL_MOUSE) {
    info_builder.boot_protocol(fhidbus::wire::HidBootProtocol::kPointer);
  } else {
    info_builder.boot_protocol(fhidbus::wire::HidBootProtocol::kNone);
  }
  info_ = info_builder.Build();

  auto actual = ep_in_.AddRequests(1, usb_ep_max_packet(endptin),
                                   fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual == 0) {
    fdf::error("Could not add any requests!");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (actual != 1) {
    fdf::warn("Wanted {} request, got {} requests", 1, actual);
  }

  auto result = outgoing()->AddService<fhidbus::Service>(fhidbus::Service::InstanceHandler({
      .device =
          [this](fidl::ServerEnd<fhidbus::Hidbus> server_end) {
            if (binding_) {
              server_end.Close(ZX_ERR_ALREADY_BOUND);
              return;
            }
            binding_.emplace(dispatcher(), std::move(server_end), this,
                             [this](fidl::UnbindInfo info) {
                               StopHidbus();
                               binding_.reset();
                             });
          },
  }));
  if (result.is_error()) {
    fdf::error("Failed to add Hidbus protocol: {}", result);
    return result.take_error();
  }

  std::vector offers = {fdf::MakeOffer2<fhidbus::Service>()};
  std::vector properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, static_cast<uint32_t>(ZX_PROTOCOL_HIDBUS))};
  zx::result child = AddChild(kChildNodeName, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

}  // namespace usb_hid

FUCHSIA_DRIVER_EXPORT2(usb_hid::UsbHidbus);
