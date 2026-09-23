// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_TRB_FIFO_H_
#define SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_TRB_FIFO_H_

#include "src/devices/usb/drivers/dwc3/dwc3-fifo.h"
#include "src/devices/usb/drivers/dwc3/dwc3-types.h"

namespace dwc3 {

class TrbFifo : public Fifo<dwc3_trb_t> {
 public:
  zx::result<> Init(zx::bti& bti, bool cached) override {
    bool needs_init = !buffer_;
    auto result = Fifo::Init(bti, cached);
    if (result.is_error()) {
      fdf::error("Failed to init FIFO {}", result);
      return result.take_error();
    }

    if (needs_init) {
      // set up link TRB pointing back to the start of the fifo
      zx_paddr_t trb_phys = Fifo::GetPhys(first_);
      last_--;
      const zx_off_t offset = (last_ - first_) * sizeof(dwc3_trb_t);
      if (auto status =
              buffer_->ExecuteWriteOps(offset, sizeof(dwc3_trb_t),
                                       [&](uint8_t* ptr) {
                                         auto link_trb = reinterpret_cast<dwc3_trb_t*>(ptr);
                                         link_trb->ptr_low = (uint32_t)trb_phys;
                                         link_trb->ptr_high = (uint32_t)(trb_phys >> 32);
                                         link_trb->status = 0;
                                         link_trb->control = TRB_TRBCTL_LINK | TRB_HWO | TRB_CHN;
                                       });
          status.is_error()) {
        fdf::error("ExecuteWriteOps failed: {}", status);
        return status.take_error();
      }
    }
    return zx::ok();
  }

  zx::result<dwc3_trb_t> ReadOne() {
    const zx_off_t offset = (read_ - first_) * sizeof(dwc3_trb_t);
    if (auto status = buffer_->CacheFlushInvalidate(offset, sizeof(dwc3_trb_t));
        status.is_error()) {
      fdf::error("CacheFlushInvalidate failed: {}", status);
      return status.take_error();
    }
    return zx::ok(*read_);
  }

  dwc3_trb_t* current_read() { return read_; }

  dwc3_trb_t* AdvanceWrite() { return Fifo::Advance(write_); }
  void AdvanceRead() {
    if (read_ == write_) {
      fdf::error("Advancing read_ past write_. Invalid!");
      return;
    }
    Fifo::Advance(read_);
  }

  void Reset() {
    const size_t len = (last_ - first_) * sizeof(dwc3_trb_t);
    if (auto status = buffer_->ExecuteWriteOps(
            0, len,
            [&](uint8_t* ptr) {
              auto trbs = reinterpret_cast<dwc3_trb_t*>(ptr);
              for (size_t i = 0; i < static_cast<size_t>(last_ - first_); i++) {
                trbs[i].control = 0;
              }
            });
        status.is_error()) {
      fdf::error("ExecuteWriteOps failed: {}", status);
    }
    write_ = first_;
    read_ = write_;
  }
};

}  // namespace dwc3

#endif  // SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_TRB_FIFO_H_
