// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/vsync-receiver.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/threads.h>

#include <memory>
#include <utility>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/board-resources.h"

namespace amlogic_display {

// static
zx::result<std::unique_ptr<VsyncReceiver>> VsyncReceiver::Create(
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
    VsyncHandler on_vsync) {
  ZX_DEBUG_ASSERT(platform_device.is_valid());

  zx::result<zx::interrupt> vsync_irq_result =
      GetInterrupt(kInterruptNameViu1Vsync, platform_device);
  if (vsync_irq_result.is_error()) {
    return vsync_irq_result.take_error();
  }

  static constexpr std::string_view kRoleName =
      "fuchsia.graphics.display.drivers.amlogic-display.vsync";
  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(
          fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "vsync-interrupt-thread",
          /*shutdown_handler=*/[](fdf_dispatcher_t*) {}, kRoleName);
  if (create_dispatcher_result.is_error()) {
    fdf::error("Failed to create vsync Dispatcher: {}", create_dispatcher_result);
    return create_dispatcher_result.take_error();
  }
  fdf::SynchronizedDispatcher dispatcher = std::move(create_dispatcher_result).value();

  fbl::AllocChecker alloc_checker;
  auto vsync_receiver =
      fbl::make_unique_checked<VsyncReceiver>(&alloc_checker, std::move(vsync_irq_result).value(),
                                              std::move(on_vsync), std::move(dispatcher));
  if (!alloc_checker.check()) {
    fdf::error("Out of memory while allocating VsyncReceiver");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> start_result = vsync_receiver->SetReceivingState(/*receiving=*/true);
  if (start_result.is_error()) {
    fdf::error("Failed to start VsyncReceiver: {}", start_result);
    return start_result.take_error();
  }

  return zx::ok(std::move(vsync_receiver));
}

VsyncReceiver::VsyncReceiver(zx::interrupt vsync_irq, VsyncHandler on_vsync,
                             fdf::SynchronizedDispatcher irq_handler_dispatcher)
    : vsync_irq_(std::move(vsync_irq)),
      on_vsync_(std::move(on_vsync)),
      irq_handler_dispatcher_(std::move(irq_handler_dispatcher)) {
  irq_handler_.set_object(vsync_irq_.get());
}

VsyncReceiver::~VsyncReceiver() {
  // In order to shut down the interrupt handler and join the thread, the
  // interrupt must be destroyed first.
  if (vsync_irq_.is_valid()) {
    zx_status_t status = vsync_irq_.destroy();
    if (status != ZX_OK) {
      fdf::error("VsyncReceiver done interrupt destroy failed: {}", zx::make_result(status));
    }
  }

  irq_handler_dispatcher_.reset();
}

zx::result<> VsyncReceiver::SetReceivingState(bool receiving) {
  if (is_receiving_ == receiving) {
    return zx::ok();
  }
  if (receiving) {
    return PostStart();
  }
  return PostStop();
}

zx::result<> VsyncReceiver::PostStart() {
  ZX_DEBUG_ASSERT(!is_receiving_);

  zx_status_t post_task_status =
      async::PostTask(irq_handler_dispatcher_.async_dispatcher(), [this] {
        zx_status_t status = irq_handler_.Begin(irq_handler_dispatcher_.async_dispatcher());
        if (status != ZX_OK) {
          fdf::error("Failed to bind the Vsync handler to the async loop: {}",
                     zx::make_result(status));
        }
      });

  if (post_task_status != ZX_OK) {
    fdf::error("Failed to post the Vsync handler begin task: {}",
               zx::make_result(post_task_status));
    return zx::error(post_task_status);
  }
  is_receiving_ = true;
  return zx::ok();
}

zx::result<> VsyncReceiver::PostStop() {
  ZX_DEBUG_ASSERT(is_receiving_);

  // DFv2-backed async dispatchers requires all interrupt handlers to be
  // unbound from the same dispatcher they were bound to.
  zx_status_t post_task_status =
      async::PostTask(irq_handler_dispatcher_.async_dispatcher(), [this]() {
        zx_status_t status = irq_handler_.Cancel();
        if (status != ZX_OK) {
          fdf::error("Failed to cancel the Vsync handler: {}", zx::make_result(status));
        }
      });

  if (post_task_status != ZX_OK) {
    fdf::error("Failed to post the Vsync handler cancel task: {}",
               zx::make_result(post_task_status));
    return zx::error(post_task_status);
  }

  is_receiving_ = false;
  return zx::ok();
}

void VsyncReceiver::InterruptHandler(async_dispatcher_t* dispatcher, async::IrqBase* irq,
                                     zx_status_t status, const zx_packet_interrupt_t* interrupt) {
  if (status == ZX_ERR_CANCELED) {
    fdf::info("Vsync interrupt wait is cancelled.");
    return;
  }
  if (status != ZX_OK) {
    fdf::error("Vsync interrupt wait failed: {}", zx::make_result(status));
    // A failed async interrupt wait doesn't remove the interrupt from the
    // async loop, so we have to manually cancel it.
    irq->Cancel();
    return;
  }

  OnVsync(zx::time_monotonic(interrupt->timestamp));

  // For interrupts bound to ports (including those bound to async loops), the
  // interrupt must be re-armed using zx_interrupt_ack() for each incoming
  // interrupt request. This is best done after the interrupt has been fully
  // processed.
  zx::unowned_interrupt(irq->object())->ack();
}

void VsyncReceiver::OnVsync(zx::time_monotonic timestamp) { on_vsync_(timestamp); }

}  // namespace amlogic_display
