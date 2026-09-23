// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_INTERRUPTER_H_
#define SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_INTERRUPTER_H_

#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/task.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>

#include "src/devices/usb/drivers/xhci/xhci-event-ring.h"

namespace usb_xhci {

struct Inspect;  // fwd decl

// An interrupter that manages an event ring, and handles interrupts.
class Interrupter {
 public:
  Interrupter() = default;

  ~Interrupter() {
    if (dispatcher_.get()) {
      zx_status_t cancel_status;
      libsync::Completion cancel_completion;
      zx_status_t post_task_status = async::PostTask(dispatcher_.async_dispatcher(), [&]() {
        cancel_status = irq_handler_.Cancel();
        cancel_completion.Signal();
      });

      if (post_task_status != ZX_OK) {
        fdf::error("Failed to post the irq handler cancel task: {}",
                   zx_status_get_string(post_task_status));
      } else {
        cancel_completion.Wait();
        if (cancel_status != ZX_OK) {
          fdf::error("Failed to cancel the irq handler: {}", zx_status_get_string(cancel_status));
        }
      }

      dispatcher_.ShutdownAsync();
      irq_shutdown_completion_.Wait();
    }
  }

  zx_status_t Init(uint16_t interrupter, size_t page_size, fdf::MmioBuffer* buffer,
                   const RuntimeRegisterOffset& offset, uint32_t erst_max,
                   DoorbellOffset doorbell_offset, UsbXhci* hci, HCCPARAMS1 hcc_params_1,
                   uint64_t* dcbaa);

  zx_status_t Start(const RuntimeRegisterOffset& offset, fdf::MmioView interrupter_regs);

  void Stop() {
    if (!active_) {
      // Already inactive;
      return;
    }
    active_ = false;
  }

  EventRing& ring() { return event_ring_; }
  bool active() { return active_; }

  // Returns a pointer to the IRQ
  // owned by this interrupter
  zx::interrupt& GetIrq() { return irq_; }

  fpromise::promise<void, zx_status_t> Timeout(zx::time deadline);

 private:
  zx_status_t StartIrqThread();

  fdf::SynchronizedDispatcher dispatcher_;

  std::atomic_bool active_ = false;
  uint16_t interrupter_;
  zx::interrupt irq_;
  EventRing event_ring_;
  async::Irq irq_handler_;
  libsync::Completion irq_shutdown_completion_;

  std::optional<fdf_power::TimeoutWakeLease> wake_lease_;

  // published inspect data
  inspect::Node inspect_root_;
  inspect::UintProperty total_irqs_;

  inspect::Node wake_events_;
  inspect::UintProperty total_wake_events_;
  inspect::BoolProperty wake_lease_held_;
  inspect::UintProperty wake_lease_last_acquired_timestamp_;
  inspect::UintProperty wake_lease_last_refreshed_timestamp_;

  // Reference to the xHCI core. Since Interrupter is a part of the
  // UsbXhci (always instantiated as a class member), this reference
  // will always be valid for the lifetime of the Interrupter.
  UsbXhci* hci_ = nullptr;
};

}  // namespace usb_xhci

#endif  // SRC_DEVICES_USB_DRIVERS_XHCI_XHCI_INTERRUPTER_H_
