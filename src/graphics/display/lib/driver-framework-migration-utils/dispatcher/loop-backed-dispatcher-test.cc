// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/loop.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/task.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/loop-backed-dispatcher-factory.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

TEST(LoopBackedDispatcher, DispatchAsyncTasks) {
  std::shared_ptr<MockDevice> mock_root = MockDevice::FakeRootParent();
  LoopBackedDispatcherFactory factory(mock_root.get());

  zx::result<std::unique_ptr<Dispatcher>> create_dispatcher_result =
      factory.Create("test_dispatcher", /*scheduler_role=*/{});
  ASSERT_OK(create_dispatcher_result.status_value());

  std::unique_ptr<Dispatcher> dispatcher = std::move(create_dispatcher_result).value();

  libsync::Completion task_completion;
  async::PostTask(dispatcher->async_dispatcher(), [&task_completion] { task_completion.Signal(); });
  task_completion.Wait();
  ASSERT_TRUE(task_completion.signaled());
}

TEST(LoopBackedDispatcher, DispatchIrq) {
  std::shared_ptr<MockDevice> mock_root = MockDevice::FakeRootParent();
  LoopBackedDispatcherFactory factory(mock_root.get());

  zx::result<std::unique_ptr<Dispatcher>> create_dispatcher_result =
      factory.Create("irq_dispatcher", /*scheduler_role=*/{});
  ASSERT_OK(create_dispatcher_result.status_value());

  std::unique_ptr<Dispatcher> dispatcher = std::move(create_dispatcher_result).value();

  zx::interrupt irq;
  zx_status_t status = zx::interrupt::create(zx::resource{}, 0, ZX_INTERRUPT_VIRTUAL, &irq);
  ASSERT_OK(status);

  zx::time_boot latest_handled_irq_timestamp;
  libsync::Completion irq_handler_invoked;
  libsync::Completion irq_handler_canceled;

  async::Irq irq_handler(
      irq.get(), /*trigger=*/ZX_SIGNAL_NONE,
      [&irq_handler_invoked, &irq_handler_canceled, &latest_handled_irq_timestamp](
          async_dispatcher_t* dispatcher, async::Irq* irq, zx_status_t status,
          const zx_packet_interrupt_t* interrupt) {
        ASSERT_TRUE(status == ZX_OK || status == ZX_ERR_CANCELED)
            << "Invalid async Irq wait status: " << zx_status_get_string(status);
        if (status == ZX_ERR_CANCELED) {
          irq_handler_canceled.Signal();
          return;
        }
        latest_handled_irq_timestamp = zx::time_boot(interrupt->timestamp);
        irq_handler_invoked.Signal();

        // Acknowledges the interrupt so that it can be triggered again.
        zx::unowned_interrupt(irq->object())->ack();
      });
  irq_handler.Begin(dispatcher->async_dispatcher());

  status = irq_handler_invoked.Wait(zx::sec(1));
  EXPECT_STATUS(status, ZX_ERR_TIMED_OUT);

  // Manually trigger the virtual interrupt.
  static constexpr zx::time_boot kIrqTimestamp1 = zx::time_boot(0x12345678);
  status = irq.trigger(0u, kIrqTimestamp1);
  ASSERT_OK(status);

  // The interrupt handler is invoked when the interrupt is triggered.
  irq_handler_invoked.Wait();
  EXPECT_EQ(latest_handled_irq_timestamp, kIrqTimestamp1);

  // Manually trigger the virtual interrupt again.
  irq_handler_invoked.Reset();
  static constexpr zx::time_boot kIrqTimestamp2 = zx::time_boot(0x23456789);
  status = irq.trigger(0u, kIrqTimestamp2);
  ASSERT_OK(status);

  // The interrupt handler can be invoked again when the interrupt is triggered
  // again.
  irq_handler_invoked.Wait();
  EXPECT_EQ(latest_handled_irq_timestamp, kIrqTimestamp2);

  dispatcher.reset();
  irq_handler_canceled.Wait();
}

}  // namespace

}  // namespace display
