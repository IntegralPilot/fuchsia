// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/xhci-enumeration.h"

#include <endian.h>
#include <fuchsia/hardware/usb/descriptor/c/banjo.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <optional>

#include <fbl/ref_ptr.h>

#include "src/devices/usb/drivers/xhci/registers.h"
#include "src/devices/usb/drivers/xhci/usb-xhci.h"
#include "src/devices/usb/drivers/xhci/xhci-context.h"

namespace usb_xhci {

fpromise::promise<usb_device_descriptor_t, zx_status_t> GetDeviceDescriptor(UsbXhci* hci,
                                                                            uint8_t slot_id,
                                                                            uint16_t length) {
  std::optional<OwnedRequest> request_wrapper;
  OwnedRequest::Alloc(&request_wrapper, length, 0, hci->UsbHciGetRequestSize());
  usb_request_t* request = request_wrapper->request();
  request->header.device_id = slot_id - 1;
  request->header.ep_address = 0;
  request->setup.bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE;
  request->setup.w_value = USB_DT_DEVICE << 8;
  request->setup.w_index = 0;
  request->setup.b_request = USB_REQ_GET_DESCRIPTOR;
  request->setup.w_length = length;
  request->direct = true;
  return hci->UsbHciRequestQueue(std::move(*request_wrapper))
      .then([=](fpromise::result<OwnedRequest, void>& result)
                -> fpromise::result<usb_device_descriptor_t, zx_status_t> {
        auto& request = result.value();
        zx_status_t status = request.request()->response.status;
        if (status != ZX_OK) {
          fdf::error("GetDeviceDescriptor request failed {}", zx_status_get_string(status));
          return fpromise::error(status);
        }
        usb_device_descriptor_t descriptor;
        ssize_t actual = request.CopyFrom(&descriptor, length, 0);
        if (actual != length) {
          fdf::error("GetDeviceDescriptor request expected {} bytes, got {}", length, actual);
          return fpromise::error(ZX_ERR_IO);
        }
        if (descriptor.b_descriptor_type != USB_DT_DEVICE) {
          fdf::error("GetDeviceDescriptor got bad descriptor type: {}",
                     descriptor.b_descriptor_type);
          return fpromise::error(ZX_ERR_IO);
        }
        return fpromise::ok(descriptor);
      })
      .box();
}

fpromise::promise<uint8_t, zx_status_t> GetMaxPacketSize(UsbXhci* hci, uint8_t slot_id) {
  // Read the first 8 bytes of the descriptor only, to get the max packet size.
  return GetDeviceDescriptor(hci, slot_id, 8)
      .and_then(
          [](const usb_device_descriptor_t& descriptor) -> fpromise::result<uint8_t, zx_status_t> {
            return fpromise::ok(descriptor.b_max_packet_size0);
          })
      .box();
}

TRBPromise UpdateMaxPacketSize(UsbXhci* hci, uint8_t slot_id) {
  return GetMaxPacketSize(hci, slot_id)
      .and_then([=](const uint8_t& packet_size) {
        return hci->SetMaxPacketSizeCommand(slot_id, packet_size);
      })
      .box();
}

struct AsyncState : public fbl::RefCounted<AsyncState> {
  // The current slot that is being enumerated
  std::optional<uint8_t> slot;
};

// Retries enumeration if we get a USB transaction error.
// See section 4.3 of revision 1.2 of the xHCI specification for details
fpromise::promise<void, zx_status_t> RetryEnumeration(UsbXhci* hci, uint8_t port,
                                                      std::optional<HubInfo> hub_info,
                                                      const fbl::RefPtr<AsyncState>& state) {
  // Take the current slot out of state, so the error handler won't try to disable the slot a second
  // time.
  uint8_t old_slot = *state->slot;
  state->slot = std::nullopt;

  // Disabling the slot is required due to https://fxbug.dev/42118061
  return hci->DisableSlotCommand(old_slot)
      .and_then([=]() -> TRBPromise {
        // DisableSlotCommand will never return an error in the TRB.
        // Failure to disable a slot is considered a fatal error, and will result in
        // ZX_ERR_BAD_STATE being returned.
        return hci->EnableSlotCommand();
      })
      .and_then([=](TRB*& result) -> TRBPromise {
        auto completion_event = static_cast<CommandCompletionEvent*>(result);
        if (completion_event->CompletionCode() != CommandCompletionEvent::Success) {
          fdf::error("Enable slot command failed: {}", completion_event->CompletionCode());
          return fpromise::make_error_promise(ZX_ERR_IO);
        }
        // After successfully obtaining a device slot, issue an Address Device command and enable
        // its default control endpoint.
        state->slot = static_cast<uint8_t>(completion_event->SlotID());
        hci->SetDeviceInformation(*state->slot, port, hub_info);

        // For the second attempt, BSR should be set to true. Section 4.3.4.
        return hci->AddressDeviceCommand(*state->slot, port, hub_info, /*bsr=*/true);
      })
      .and_then([=](TRB*& result) -> TRBPromise {
        // If we fail during the retry, give up.
        auto completion_event = static_cast<CommandCompletionEvent*>(result);
        if (completion_event->CompletionCode() != CommandCompletionEvent::Success) {
          fdf::error("Address device command failed: {}", completion_event->CompletionCode());
          return fpromise::make_error_promise(ZX_ERR_IO);
        }
        return UpdateMaxPacketSize(hci, *state->slot);
      })
      .and_then([=](TRB*& result) -> TRBPromise {
        auto completion_event = static_cast<CommandCompletionEvent*>(result);
        if (completion_event->CompletionCode() != CommandCompletionEvent::Success) {
          fdf::error("Update max packat size failed: {}", completion_event->CompletionCode());
          return fpromise::make_ok_promise(result);
        }
        // Issue a SET_ADDRESS request to the device
        return hci->AddressDeviceCommand(*state->slot, port);
      })
      .and_then([=](TRB*& result) -> fpromise::result<void, zx_status_t> {
        auto completion_event = static_cast<CommandCompletionEvent*>(result);
        if (completion_event->CompletionCode() != CommandCompletionEvent::Success) {
          fdf::error("Address device command failed: {}", completion_event->CompletionCode());
          return fpromise::error(ZX_ERR_IO);
        }
        return fpromise::ok();
      })
      .box();
}

fpromise::promise<void, zx_status_t> EnumerateDevice(UsbXhci* hci, uint8_t port,
                                                     std::optional<HubInfo> hub_info) {
  auto state = fbl::MakeRefCounted<AsyncState>();

  // Obtain a Device Slot for the newly attached device
  return hci->EnableSlotCommand()
      .and_then([=](TRB*& result) -> TRBPromise {
        auto completion_event = static_cast<CommandCompletionEvent*>(result);
        if (completion_event->CompletionCode() != CommandCompletionEvent::Success) {
          fdf::error("Enable slot command failed: {}", completion_event->CompletionCode());
          return fpromise::make_error_promise(ZX_ERR_IO);
        }
        // After successfully obtaining a device slot,
        // issue an Address Device command and enable its default control endpoint.
        state->slot = static_cast<uint8_t>(completion_event->SlotID());
        hci->SetDeviceInformation(*state->slot, port, hub_info);

        // For the first attempt, BSR should be set to false. Section 4.3.4.
        return hci->AddressDeviceCommand(*state->slot, port, hub_info, /*bsr=*/false);
      })
      .and_then([=](TRB*& result) -> fpromise::promise<void, zx_status_t> {
        // Check for errors and retry if the device refuses the SET_ADDRESS command
        auto completion_event = static_cast<CommandCompletionEvent*>(result);
        if (completion_event->CompletionCode() == CommandCompletionEvent::UsbTransactionError) {
          fdf::error("Address device command failed with transaction error.");
          if (!hci->IsDeviceConnected(*state->slot)) {
            return fpromise::make_error_promise<zx_status_t>(ZX_ERR_IO);
          }
          fdf::info("Retrying enumeration.");
          return RetryEnumeration(hci, port, hub_info, state);
        }
        if (completion_event->CompletionCode() != CommandCompletionEvent::Success) {
          fdf::error("Address device failed error: {}", completion_event->CompletionCode());
          return fpromise::make_error_promise<zx_status_t>(ZX_ERR_IO);
        }
        return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
      })
      .and_then([=]() -> fpromise::promise<void, zx_status_t> {
        auto speed = hci->GetDeviceSpeed(*state->slot);
        if (!speed.has_value()) {
          fdf::error("Device speed is invalid.");
          return fpromise::make_error_promise<>(ZX_ERR_BAD_STATE);
        }
        if (speed.value() != USB_SPEED_SUPER) {
          // See USB 2.0 specification (revision 2.0) section 9.2.6
          return hci->Timeout(kPrimaryInterrupter, zx::deadline_after(zx::msec(10)))
              .discard_value();
        }
        return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
      })
      .and_then([=]() -> fpromise::promise<uint8_t, zx_status_t> {
        // For full-speed devices, system software should read the first 8 bytes
        // of the device descriptor to determine the max packet size of the default control
        // endpoint. Additionally, certain devices may require the controller to read this value
        // before fetching the full descriptor; so we always read the max packet size in order to
        // prevent later enumeration failures.
        return GetMaxPacketSize(hci, *state->slot);
      })
      .and_then([=](uint8_t& max_packet_size) -> fpromise::promise<void, zx_status_t> {
        // Set the max packet size if the device is a full speed device.
        auto speed = hci->GetDeviceSpeed(*state->slot);
        if (!speed.has_value()) {
          fdf::error("Device speed is invalid.");
          return fpromise::make_error_promise<zx_status_t>(ZX_ERR_BAD_STATE);
        }
        if (speed.value() != USB_SPEED_FULL) {
          return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
        }
        return hci->SetMaxPacketSizeCommand(*state->slot, max_packet_size)
            .and_then([](TRB*& result) -> fpromise::promise<void, zx_status_t> {
              // Discard TRB result.
              return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
            });
      })
      .and_then([=]() -> fpromise::promise<usb_device_descriptor_t, zx_status_t> {
        // Now read the whole descriptor.
        return GetDeviceDescriptor(hci, *state->slot, sizeof(usb_device_descriptor_t));
      })
      .and_then([=](usb_device_descriptor_t& descriptor) -> fpromise::result<void, zx_status_t> {
        hci->CreateDeviceInspectNode(*state->slot, le16toh(descriptor.id_vendor),
                                     le16toh(descriptor.id_product));

        // Online the device, making it visible to the DDK (enumeration has completed)
        auto speed = hci->GetDeviceSpeed(*state->slot);
        if (!speed.has_value()) {
          fdf::error("Device speed is invalid.");
          return fpromise::error(ZX_ERR_BAD_STATE);
        }
        zx_status_t status = hci->DeviceOnline(*state->slot, port, speed.value());
        if (status != ZX_OK) {
          fdf::error("DeviceOnline failed: {}", zx_status_get_string(status));
          return fpromise::error(status);
        }
        fdf::debug("Enumeration successful");
        return fpromise::ok();
      })
      .or_else([=](const zx_status_t& status) -> fpromise::result<void, zx_status_t> {
        // Clean up the slot if we didn't successfully enumerate.
        fdf::error("Enumeration failed: {}", zx_status_get_string(status));
        if (state->slot.has_value()) {
          hci->ScheduleTask(kPrimaryInterrupter, hci->DisableSlotCommand(*state->slot));
        }
        return fpromise::error(status);
      })
      .box();
}

}  // namespace usb_xhci
