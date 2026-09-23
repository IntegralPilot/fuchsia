// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/capture.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/threads.h>

#include <memory>
#include <utility>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/board-resources.h"

namespace amlogic_display {

// static
zx::result<std::unique_ptr<Capture>> Capture::Create(
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
    OnCaptureCompleteHandler on_capture_complete) {
  ZX_DEBUG_ASSERT(platform_device.is_valid());

  zx::result<zx::interrupt> capture_interrupt_result =
      GetInterrupt(kInterruptNameVdin1WriteDone, platform_device);
  if (capture_interrupt_result.is_error()) {
    return capture_interrupt_result.take_error();
  }

  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls,
                                          "capture-interrupt-thread",
                                          /*shutdown_handler=*/[](fdf_dispatcher_t*) {},
                                          /*scheduler_role=*/{});
  if (create_dispatcher_result.is_error()) {
    fdf::error("Failed to create capture interrupt handler dispatcher: {}",
               create_dispatcher_result);
    return create_dispatcher_result.take_error();
  }
  fdf::SynchronizedDispatcher dispatcher = std::move(create_dispatcher_result).value();

  fbl::AllocChecker alloc_checker;
  auto capture =
      fbl::make_unique_checked<Capture>(&alloc_checker, std::move(capture_interrupt_result).value(),
                                        std::move(on_capture_complete), std::move(dispatcher));
  if (!alloc_checker.check()) {
    fdf::error("Out of memory while allocating Capture");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> init_result = capture->Init();
  if (init_result.is_error()) {
    fdf::error("Failed to initalize Capture: {}", init_result);
    return init_result.take_error();
  }

  return zx::ok(std::move(capture));
}

Capture::Capture(zx::interrupt capture_finished_interrupt,
                 OnCaptureCompleteHandler on_capture_complete,
                 fdf::SynchronizedDispatcher irq_handler_dispatcher)
    : capture_finished_irq_(std::move(capture_finished_interrupt)),
      on_capture_complete_(std::move(on_capture_complete)),
      irq_handler_dispatcher_(std::move(irq_handler_dispatcher)) {
  irq_handler_.set_object(capture_finished_irq_.get());
}

Capture::~Capture() {
  // In order to shut down the interrupt handler and join the thread, the
  // interrupt must be destroyed first.
  if (capture_finished_irq_.is_valid()) {
    zx_status_t status = capture_finished_irq_.destroy();
    if (status != ZX_OK) {
      fdf::error("Capture done interrupt destroy failed: {}", zx::make_result(status));
    }
  }

  irq_handler_dispatcher_.reset();
}

zx::result<> Capture::Init() {
  zx_status_t status = irq_handler_.Begin(irq_handler_dispatcher_.async_dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to bind the capture interrupt handler to the async loop: {}",
               zx::make_result(status));
    return zx::error(status);
  }

  return zx::ok();
}

void Capture::InterruptHandler(async_dispatcher_t* dispatcher, async::IrqBase* irq,
                               zx_status_t status, const zx_packet_interrupt_t* interrupt) {
  if (status == ZX_ERR_CANCELED) {
    fdf::info("Capture finished interrupt wait is cancelled.");
    return;
  }
  if (status != ZX_OK) {
    fdf::error("Capture finished interrupt wait failed: {}", zx::make_result(status));
    // A failed async interrupt wait doesn't remove the interrupt from the
    // async loop, so we have to manually cancel it.
    irq->Cancel();
    return;
  }

  OnCaptureComplete();

  // For interrupts bound to ports (including those bound to async loops), the
  // interrupt must be re-armed using zx_interrupt_ack() for each incoming
  // interrupt request. This is best done after the interrupt has been fully
  // processed.
  zx::unowned_interrupt(irq->object())->ack();
}

void Capture::OnCaptureComplete() { on_capture_complete_(); }

}  // namespace amlogic_display
