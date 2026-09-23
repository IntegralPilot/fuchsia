// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-bus/tests/common.h"

namespace usb_bus {

const char16_t* kStringDescriptors[][2] = {{u"Fuchsia", u"Fucsia"}, {u"Device", u"Dispositivo"}};

FakeHci::FakeHci(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
  proto_.ops = &usb_hci_protocol_ops_;
  proto_.ctx = this;
}

zx_status_t FakeHci::UsbHciResetEndpoint(uint32_t device_id, uint8_t ep_address) {
  if (device_id == kDeviceId) {
    reset_endpoint_ = ep_address;
  }
  return ZX_OK;
}

zx_status_t FakeHci::UsbHciResetDevice(uint32_t hub_address, uint32_t device_id) {
  if (device_id == kDeviceId) {
    device_reset_ = true;
  }
  return ZX_OK;
}

zx_status_t FakeHci::UsbHciCancelAll(uint32_t device_id, uint8_t ep_address) {
  auto requests = pending_requests();
  requests.CompleteAll(ZX_ERR_CANCELED, 0);
  return ZX_OK;
}

void FakeHci::UsbHciRequestQueue(usb_request_t* usb_request_,
                                 const usb_request_complete_callback_t* complete_cb_) {
  usb::BorrowedRequest<void> request(usb_request_, *complete_cb_, sizeof(usb_request_t));
  if (should_return_empty_) {
    request.Complete(ZX_OK, 0);
    return;
  }
  if ((request.request()->header.ep_address == 0) && !custom_control_) {
    if ((request.request()->setup.bm_request_type ==
         (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE)) &&
        (request.request()->setup.b_request == USB_REQ_GET_DESCRIPTOR)) {
      uint8_t type = static_cast<uint8_t>(request.request()->setup.w_value >> 8);
      uint8_t index = static_cast<uint8_t>(request.request()->setup.w_value);
      switch (type) {
        case USB_DT_DEVICE: {
          usb_device_descriptor_t* descriptor;
          request.Mmap(reinterpret_cast<void**>(&descriptor));
          descriptor->b_num_configurations = 2;
          descriptor->id_vendor = kVendorId;
          descriptor->id_product = kProductId;
          descriptor->b_device_class = kDeviceClass;
          descriptor->b_device_sub_class = kDeviceSubclass;
          descriptor->b_device_protocol = kDeviceProtocol;
          request.Complete(ZX_OK, sizeof(*descriptor));
        }
          return;
        case USB_DT_CONFIG: {
          usb_configuration_descriptor_t* descriptor;
          request.Mmap(reinterpret_cast<void**>(&descriptor));
          descriptor->w_total_length = sizeof(*descriptor);
          descriptor->b_configuration_value = static_cast<uint8_t>(index + 1);
          request.Complete(ZX_OK, sizeof(*descriptor));
        }
          return;
        case USB_DT_STRING: {
          if (index == 0) {
            // Fetch language table
            usb_langid_desc_t* languages;
            request.Mmap(reinterpret_cast<void**>(&languages));
            languages->b_length = 2 + (2 * 2);
            languages->w_lang_ids[0] = MakeConstant<uint16_t, 2>("EN");
            languages->w_lang_ids[1] = MakeConstant<uint16_t, 2>("ES");
            request.Complete(ZX_OK, languages->b_length);
            return;
          }
          index--;
          uint16_t lang = request.request()->setup.w_index;
          switch (lang) {
            case MakeConstant<uint16_t, 2>("EN"):
              lang = 0;
              break;
            case MakeConstant<uint16_t, 2>("ES"):
              lang = 1;
              break;
          }
          if ((index < 2) && (lang < 2)) {
            usb_string_desc_t* descriptor;
            request.Mmap(reinterpret_cast<void**>(&descriptor));
            descriptor->b_length = static_cast<uint8_t>(
                2 + (2 * std::char_traits<char16_t>::length(kStringDescriptors[index][lang])));
            memcpy(descriptor->code_points, kStringDescriptors[index][lang],
                   descriptor->b_length - 2);
            request.Complete(ZX_OK, descriptor->b_length);
            return;
          }
        }
      }
    }
    if ((request.request()->setup.bm_request_type ==
         (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE)) &&
        (request.request()->setup.b_request == USB_REQ_SET_CONFIGURATION)) {
      selected_configuration_ = static_cast<uint8_t>(request.request()->setup.w_value);
      request.Complete(ZX_OK, 0);
      return;
    }
    request.Complete(ZX_ERR_INVALID_ARGS, 0);
    return;
  }
  pending_requests_.push(std::move(request));
}

zx_status_t FakeHci::UsbHciEnableEndpoint(uint32_t device_id,
                                          const usb_endpoint_descriptor_t* ep_desc,
                                          const usb_ss_ep_comp_descriptor_t* ss_com_desc,
                                          bool enable) {
  if (!enable_endpoint_hook_) {
    return ZX_ERR_BAD_STATE;
  }
  return enable_endpoint_hook_(device_id, ep_desc, ss_com_desc, enable);
}

void FakeHci::SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) {
  hci_interface_client_ = fidl::WireSharedClient<fuchsia_hardware_usb_hci::UsbHciInterface>(
      std::move(request.interface()), dispatcher_);
  completer.Reply(zx::ok());
}

void FakeHci::ConnectToEndpoint(ConnectToEndpointRequest& request,
                                ConnectToEndpointCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::GetMaxDeviceCount(GetMaxDeviceCountCompleter::Sync& completer) {
  completer.Reply(UsbHciGetMaxDeviceCount());
}

void FakeHci::EnableEndpoint(EnableEndpointRequest& request,
                             EnableEndpointCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::GetCurrentFrame(GetCurrentFrameCompleter::Sync& completer) {
  completer.Reply(UsbHciGetCurrentFrame());
}

void FakeHci::ConfigureHub(ConfigureHubRequest& request, ConfigureHubCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::HubDeviceAdded(HubDeviceAddedRequest& request,
                             HubDeviceAddedCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::HubDeviceRemoved(HubDeviceRemovedRequest& request,
                               HubDeviceRemovedCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::HubDeviceReset(HubDeviceResetRequest& request,
                             HubDeviceResetCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::ResetEndpoint(ResetEndpointRequest& request,
                            ResetEndpointCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::ResetDevice(ResetDeviceRequest& request, ResetDeviceCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeHci::GetMaxTransferSize(GetMaxTransferSizeRequest& request,
                                 GetMaxTransferSizeCompleter::Sync& completer) {
  completer.Reply(zx::ok(UsbHciGetMaxTransferSize(request.device_id(), request.ep_address())));
}

}  // namespace usb_bus
