// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTERRUPTS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTERRUPTS_H_

#include <fuchsia/hardware/intelgpucore/c/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/irq.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fit/function.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <threads.h>
#include <zircon/types.h>

#include <bitset>
#include <optional>

#include "src/graphics/display/drivers/intel-display/registers-ddi.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe.h"

namespace intel_display {

class Interrupts {
 public:
  // Callbacks that are invoked on various interrupt types.
  //
  // All interrupt callbacks are currently run on the same thread (the internal
  // thread dedicated to handling interrupt). However, implementations must be
  // thread-safe, and not rely on any assumptions around the threading model.
  using PipeVsyncCallback = fit::function<void(PipeId, zx::time_monotonic timestamp)>;
  using HotplugCallback = fit::function<void(DdiId ddi_id, bool long_pulse)>;

  Interrupts();
  ~Interrupts();

  // Copying and moving are not allowed.
  Interrupts(const Interrupts&) = delete;
  Interrupts& operator=(const Interrupts&) = delete;
  Interrupts(Interrupts&&) = delete;
  Interrupts& operator=(Interrupts&&) = delete;

  // Must be called exactly once.
  // Must be called from a driver-runtime managed dispatcher.
  //
  // `mmio_space` must be non-null and outlive the initialized `Interrupts`
  // instance.
  zx_status_t Init(PipeVsyncCallback pipe_vsync_callback, HotplugCallback hotplug_callback,
                   const ddk::Pci& pci, fdf::MmioBuffer* mmio_space, uint16_t device_id);
  void FinishInit();
  void Resume();
  void Destroy();

  // Enable or disable interrupt generation from `pipe`.
  //
  // This method enables and disables all the pipe-level interrupts that we are
  // prepared to handle.
  //
  // Transcoder VSync (vertical sync) interrupts trigger callbacks to the
  // PipeVsyncCallback provided to `Init()`. The callbacks are performed on the
  // internal thread dedicated to interrupt handling.
  void EnablePipeInterrupts(PipeId pipe_id, bool enable);

  // The GPU driver uses this to plug into the interrupt stream.
  //
  // On Tiger Lake, `gpu_callback` will be called during an interrupt
  // from the graphics hardware if the Graphics Primary Interrupt register
  // indicates there are GT interrupts pending.
  //
  // On Skylake and Kaby Lake, `gpu_callback` will be called during an interrupt
  // from the graphics hardware if the Display Interrupt Control register has
  // any bits in `gpu_interrupt_mask` set.
  zx_status_t SetGpuInterruptCallback(const intel_gpu_core_interrupt_t& gpu_interrupt_callback,
                                      uint32_t gpu_interrupt_mask);

 private:
  void EnableHotplugInterrupts();
  void HandlePipeInterrupt(PipeId pipe_id, zx::time_monotonic timestamp);

  void InterruptHandler(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                        const zx_packet_interrupt_t* interrupt);

  zx::result<> CancelInterruptHandler();

  PipeVsyncCallback pipe_vsync_callback_;
  HotplugCallback hotplug_callback_;
  fdf::MmioBuffer* mmio_space_ = nullptr;

  mtx_t lock_;

  // Initialized by `Init()`.
  zx::interrupt irq_;
  fuchsia_hardware_pci::InterruptMode irq_mode_;

  // The `irq_handler_dispatcher_` and `irq_handler_` are constant between
  // `Init()` and instance destruction. Only accessed on the threads used for
  // class initialization and destruction.
  fdf::SynchronizedDispatcher irq_handler_dispatcher_;
  libsync::Completion irq_handler_dispatcher_shutdown_completed_;
  async::IrqMethod<Interrupts, &Interrupts::InterruptHandler> irq_handler_{this};

  uint16_t device_id_;

  intel_gpu_core_interrupt_t gpu_interrupt_callback_ __TA_GUARDED(lock_) = {};
  uint32_t gpu_interrupt_mask_ __TA_GUARDED(lock_) = 0;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTERRUPTS_H_
