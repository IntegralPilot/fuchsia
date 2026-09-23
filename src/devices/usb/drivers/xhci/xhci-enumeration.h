// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_ENUMERATION_H_
#define SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_ENUMERATION_H_

#include <lib/fpromise/promise.h>
#include <zircon/types.h>

#include "src/devices/usb/drivers/xhci/xhci-hub.h"

// Helper functions to assist with setup and teardown of USB devices.

namespace usb_xhci {

class UsbXhci;

// Enumerates a device as specified in xHCI section 4.3 starting from step 4
// This method should be called once the physical port of a device has been
// initialized.
// Control TRBs must be run on the primary interrupter. Section 4.9.4.3: secondary interrupters
// cannot handle them..
fpromise::promise<void, zx_status_t> EnumerateDevice(UsbXhci* hci, uint8_t port,
                                                     std::optional<HubInfo> hub_info);

}  // namespace usb_xhci

#endif  // SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_ENUMERATION_H_
