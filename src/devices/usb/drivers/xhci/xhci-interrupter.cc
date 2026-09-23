// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/xhci-interrupter.h"

#include <lib/async/cpp/task.h>

#include "src/devices/usb/drivers/xhci/usb-xhci.h"

namespace usb_xhci {

zx_status_t Interrupter::Init(uint16_t interrupter, size_t page_size, fdf::MmioBuffer* buffer,
                              const RuntimeRegisterOffset& offset, uint32_t erst_max,
                              DoorbellOffset doorbell_offset, UsbXhci* hci, HCCPARAMS1 hcc_params_1,
                              uint64_t* dcbaa) {
  if (active_) {
    // Already active;
    return ZX_OK;
  }

  hci_ = hci;
  interrupter_ = interrupter;

  // Create our inspect node and start to add our properties.
  char name[64];
  snprintf(name, sizeof(name), "Interrupter %hu", interrupter_);
  inspect_root_ = hci_->inspect_root_node().CreateChild(name);
  total_irqs_ = inspect_root_.CreateUint("Total IRQs", 0);
  wake_events_ = inspect_root_.CreateChild("Wake Events");

  return event_ring_.Init(page_size, hci_->bti(), buffer, hci->Is32BitController(), erst_max,
                          ERSTSZ::Get(offset, interrupter_).ReadFrom(buffer),
                          ERDP::Get(offset, interrupter_).ReadFrom(buffer),
                          IMAN::Get(offset, interrupter_).FromValue(0), hci_->CapLength(),
                          HCSPARAMS1::Get().ReadFrom(buffer), hci_->GetCommandRing(),
                          doorbell_offset, hci, hcc_params_1, dcbaa, interrupter_, &inspect_root_);
}

zx_status_t Interrupter::Start(const RuntimeRegisterOffset& offset, fdf::MmioView mmio_view) {
  if (active_) {
    // Already active;
    return ZX_OK;
  }
  ERDP erdp = ERDP::Get(offset, interrupter_).ReadFrom(&mmio_view);
  if (!event_ring_.erdp_phys()) {
    return ZX_ERR_BAD_STATE;
  }
  erdp.set_reg_value(event_ring_.erdp_phys());
  erdp.WriteTo(&mmio_view);
  ERSTBA ba = ERSTBA::Get(offset, interrupter_).ReadFrom(&mmio_view);
  // This enables the interrupter
  ba.set_Pointer(event_ring_.erst()).WriteTo(&mmio_view);
  IMAN::Get(offset, interrupter_).FromValue(0).set_IE(1).WriteTo(&mmio_view);
  // TODO(https://fxbug.dev/42105800): See https://fxbug.dev/42105800.  Get rid of this.  For now we
  // need thread priorities so that realtime transactions use the completer which ends up getting
  // realtime latency guarantees.
  auto dispatcher_result = fdf::SynchronizedDispatcher::Create(
      {}, "xhci-interrupter", [&](fdf_dispatcher_t*) { irq_shutdown_completion_.Signal(); },
      "fuchsia.devices.usb.drivers.xhci.interrupter");
  if (dispatcher_result.is_error()) {
    fdf::error("Failed to create new dispatcher {}", dispatcher_result);
    return dispatcher_result.error_value();
  }
  dispatcher_ = std::move(*dispatcher_result);
  async::PostTask(dispatcher_.async_dispatcher(), [this]() { StartIrqThread(); });
  active_ = true;
  return ZX_OK;
}

fpromise::promise<void, zx_status_t> Interrupter::Timeout(zx::time deadline) {
  fpromise::bridge<void, zx_status_t> bridge;
  zx_status_t status = async::PostTaskForTime(
      dispatcher_.async_dispatcher(),
      [completer = std::move(bridge.completer), this]() mutable {
        completer.complete_ok();
        hci_->RunUntilIdle(interrupter_);
      },
      deadline);
  if (status != ZX_OK) {
    return fpromise::make_error_promise<zx_status_t>(status);
  }
  return bridge.consumer.promise().box();
}

zx_status_t Interrupter::StartIrqThread() {
  wake_lease_.emplace(dispatcher_.async_dispatcher(), "xhci-wake",
                      std::move(hci_->activity_governer()), &wake_events_);
  irq_handler_.set_object(irq_.get());
  irq_handler_.set_handler([&](async_dispatcher_t* dispatcher, async::Irq* irq, zx_status_t status,
                               const zx_packet_interrupt_t* interrupt) {
    if (!irq_.is_valid()) {
      return;
    }
    if (status != ZX_OK) {
      return;
    }

    // TODO(b/362759606): For now, release the lease after a timeout. In the future, this should be
    // passed up the USB stack in "baton-passing" manner.
    const zx::duration kLeaseTimeout = zx::msec(500);
    wake_lease_->HandleInterrupt(kLeaseTimeout);

    if (event_ring_.HandleIRQ() != ZX_OK) {
      fdf::error("Error handling IRQ. Exiting async loop.");
      return;
    }

    total_irqs_.Add(1);
    irq_.ack();
  });
  if (!interrupter_) {
    // Note: We need to run the ring 0 bringup after
    // initializing interrupts, since Qemu initialization
    // code assumes that interrupts are active and simulates
    // a port status changed event.
    if (event_ring_.Ring0Bringup()) {
      fdf::error("Failed to bring up ring 0");
      return ZX_ERR_INTERNAL;
    }
  }
  irq_handler_.Begin(dispatcher_.async_dispatcher());
  return ZX_OK;
}

}  // namespace usb_xhci
