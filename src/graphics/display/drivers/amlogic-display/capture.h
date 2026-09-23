// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CAPTURE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CAPTURE_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>

#include <memory>

#include <fbl/mutex.h>

namespace amlogic_display {

// Manages the display capture (VDIN) hardware.
class Capture {
 public:
  // Internal state size for the function called when a capture completes.
  static constexpr size_t kOnCaptureCompleteTargetSize = 16;

  // The type of the function called when a capture completes.
  using OnCaptureCompleteHandler = fit::inline_function<void(), kOnCaptureCompleteTargetSize>;

  // Factory method intended for production use.
  //
  // `platform_device` must be valid.
  //
  // `on_state_change` is called when the display engine finishes writing a
  // captured image to DRAM.
  static zx::result<std::unique_ptr<Capture>> Create(
      fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
      OnCaptureCompleteHandler on_capture_complete);

  explicit Capture(zx::interrupt capture_finished_interrupt,
                   OnCaptureCompleteHandler on_capture_complete,
                   fdf::SynchronizedDispatcher irq_handler_dispatcher);

  Capture(const Capture&) = delete;
  Capture& operator=(const Capture&) = delete;

  ~Capture();

  // Initialization work that is not suitable for the constructor.
  //
  // Called by Create().
  zx::result<> Init();

 private:
  void OnCaptureComplete();

  void InterruptHandler(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                        const zx_packet_interrupt_t* interrupt);

  const zx::interrupt capture_finished_irq_;

  // Guaranteed to have a target.
  const OnCaptureCompleteHandler on_capture_complete_;

  // The `irq_handler_dispatcher_` and the `irq_handler_` are constant between
  // Init() and instance destruction. Only accessed on the threads used for
  // class initialization and destruction.
  fdf::SynchronizedDispatcher irq_handler_dispatcher_;
  async::IrqMethod<Capture, &Capture::InterruptHandler> irq_handler_{this};
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CAPTURE_H_
