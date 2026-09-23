// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_EVENT_FIFO_H_
#define SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_EVENT_FIFO_H_

#include "src/devices/usb/drivers/dwc3/dwc3-fifo.h"

namespace dwc3 {

class EventFifo : public Fifo<uint32_t> {
 public:
  std::vector<uint32_t> Read(size_t count) { return Fifo::Read(read_, count); }
  void Advance(size_t count) { Fifo::Advance(read_, count); }
  zx_paddr_t GetPhys() const { return Fifo::GetPhys(first_); }
};

}  // namespace dwc3

#endif  // SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_EVENT_FIFO_H_
