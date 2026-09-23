// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file is the implementation of the common functions of Endpoint used by tests. All functions
// here return the equivalent of "not supported." Any functions that should be supported will be
// implemented in the test file itself.

#include "src/devices/usb/drivers/xhci/tests/test-env.h"

namespace usb_xhci {

Endpoint::Endpoint(UsbXhci* hci, uint32_t device_id, uint8_t address)
    : usb::EndpointServer(hci->bti(), address, usb::ScatterGatherSupport::kSupported), hci_(hci) {}

zx_status_t Endpoint::Init(EventRing* event_ring, fdf::MmioBuffer* mmio) {
  return transfer_ring_.Init(zx_system_get_page_size(), kFakeBti, event_ring, false, mmio, hci_);
}

void Endpoint::QueueRequests(QueueRequestsRequest& request,
                             QueueRequestsCompleter::Sync& completer) {}

void Endpoint::CancelAll(CancelAllCompleter::Sync& completer) {
  completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
}

void Endpoint::OnUnbound(fidl::UnbindInfo info,
                         fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) {}

}  // namespace usb_xhci
