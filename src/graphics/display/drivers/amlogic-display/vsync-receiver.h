// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VSYNC_RECEIVER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VSYNC_RECEIVER_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/fit/function.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <threads.h>
#include <zircon/syscalls/port.h>

#include <cstddef>
#include <memory>

#include <fbl/mutex.h>

namespace amlogic_display {

// Receives Vertical Sync (Vsync) interrupts triggered by the display engine
// indicating that the display engine finishes presenting a frame to the
// display device.
class VsyncReceiver {
 public:
  // Internal state size for the function called when a Vsync interrupt is
  // triggered.
  static constexpr size_t kOnVsyncTargetSize = 16;

  // The type of the function called when a Vsync interrupt is triggered.
  using VsyncHandler = fit::inline_function<void(zx::time_monotonic timestamp), kOnVsyncTargetSize>;

  // Factory method intended for production use.
  // Creates a VsyncReceiver that is receiving Vsync interrupts.
  //
  // `platform_device` must be valid.
  //
  // `on_vsync` is called when the display engine finishes presenting a frame
  // to the display device and triggers a Vsync interrupt. Must be non-null.
  static zx::result<std::unique_ptr<VsyncReceiver>> Create(
      fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
      VsyncHandler on_vsync);

  // Production code should prefer the factory method `Create()`.
  //
  // `irq_handler_dispatcher` must not be empty.
  // `vsync_irq` must be valid.
  // `on_vsync` must be non-null.
  explicit VsyncReceiver(zx::interrupt vsync_irq, VsyncHandler on_vsync,
                         fdf::SynchronizedDispatcher irq_handler_dispatcher);

  VsyncReceiver(const VsyncReceiver&) = delete;
  VsyncReceiver& operator=(const VsyncReceiver&) = delete;

  ~VsyncReceiver();

  // If `receiving` is true, starts receiving Vsync interrupts; otherwise, stops
  // receiving Vsync interrupts.
  //
  // This method is idempotent.
  zx::result<> SetReceivingState(bool receiving);

 private:
  // Posts a task to begin listening for Vsync interrupts.
  //
  // The VsyncReceiver must not be receiving (or scheduled to receive) Vsync
  // interrupts before this method is called.
  zx::result<> PostStart();

  // Posts a task to stop receiving Vsync interrupts.
  //
  // Unhandled Vysnc interrupts queued in the dispatcher will be canceled.
  //
  // The VsyncReceiver must be receiving (or scheduled to receive) Vsync
  // interrupts before this method is called.
  zx::result<> PostStop();

  void OnVsync(zx::time_monotonic timestamp);

  void InterruptHandler(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                        const zx_packet_interrupt_t* interrupt);

  const zx::interrupt vsync_irq_;

  const VsyncHandler on_vsync_;

  bool is_receiving_ = false;

  // The `irq_handler_dispatcher_` and `irq_handler_` are constant between
  // Init() and instance destruction. Only accessed on the threads used for
  // class initialization and destruction.
  fdf::SynchronizedDispatcher irq_handler_dispatcher_;
  async::IrqMethod<VsyncReceiver, &VsyncReceiver::InterruptHandler> irq_handler_{this};
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VSYNC_RECEIVER_H_
