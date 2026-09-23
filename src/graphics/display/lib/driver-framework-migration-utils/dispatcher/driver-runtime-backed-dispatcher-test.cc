// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/irq.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fpromise/promise.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/testing/dfv2-driver-with-dispatcher.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

class DriverDispatcherTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override { return zx::ok(); }
};

class TestConfig final {
 public:
  using DriverType = display::testing::Dfv2DriverWithDispatcher;
  using EnvironmentType = DriverDispatcherTestEnvironment;
};

class DriverDispatcherTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void TearDown() override {
    StopDriver();
    driver_test().ShutdownAndDestroyDriver();
  }

  // Stops the driver, shuts down its background dispatcher.
  //
  // Must be called only from the main test thread.
  void StopDriver() {
    if (driver_stopped_) {
      return;
    }
    zx::result<> stop_result = driver_test().StopDriver();
    EXPECT_OK(stop_result);
    if (driver_test().driver() != nullptr) {
      driver_test().driver()->ShutdownDispatcher();
    }
    driver_stopped_ = true;
  }

 protected:
  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
  bool driver_stopped_ = false;
};

TEST_F(DriverDispatcherTest, DispatchAsyncTask) {
  fpromise::bridge<uint32_t> bridge;
  static constexpr uint32_t kValueToPass = 0xabcd1234;
  zx::result<> post_task_result = driver_test().driver()->PostTask(
      [completer = std::move(bridge.completer)]() mutable { completer.complete_ok(kValueToPass); });
  ASSERT_OK(post_task_result.status_value());

  fpromise::promise<uint32_t> promise = std::move(bridge.consumer).promise();
  fpromise::result<uint32_t> promise_result =
      driver_test().runtime().RunPromise(std::move(promise));
  ASSERT_TRUE(promise_result.is_ok());
  EXPECT_EQ(promise_result.value(), kValueToPass);

  StopDriver();

  // After the driver stops, no task can be posted to the driver's async
  // dispatcher.
  zx::result<> post_task_after_driver_stop_result = driver_test().driver()->PostTask(
      [completer = std::move(bridge.completer)]() mutable { completer.complete_ok(0x1234abcd); });
  EXPECT_NE(ZX_OK, post_task_after_driver_stop_result.status_value());
}

TEST_F(DriverDispatcherTest, HandleIrq) {
  zx::interrupt virtual_interrupt;
  zx_status_t status =
      zx::interrupt::create(zx::resource{}, 0u, ZX_INTERRUPT_VIRTUAL, &virtual_interrupt);
  ASSERT_OK(status);

  zx::interrupt virtual_interrupt_driver_dup;
  status = virtual_interrupt.duplicate(ZX_RIGHT_SAME_RIGHTS, &virtual_interrupt_driver_dup);
  ASSERT_OK(status);

  zx::time_boot latest_handled_irq_timestamp;
  libsync::Completion irq_handler_invoked;
  libsync::Completion irq_handler_canceled;

  async::Irq::Handler handler = [&latest_handled_irq_timestamp, &irq_handler_invoked,
                                 &irq_handler_canceled](async_dispatcher_t* dispatcher,
                                                        async::Irq* irq, zx_status_t status,
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
  };

  zx::result<> start_irq_handler_result = driver_test().driver()->StartIrqHandler(
      std::move(virtual_interrupt_driver_dup), std::move(handler));
  ASSERT_OK(start_irq_handler_result.status_value());

  // Manually trigger the virtual interrupt.
  static constexpr zx::time_boot kIrqTimestamp1 = zx::time_boot(0x12345678);
  status = virtual_interrupt.trigger(0u, kIrqTimestamp1);
  ASSERT_OK(status);

  // The interrupt handler is invoked when the interrupt is triggered.
  irq_handler_invoked.Wait();
  EXPECT_EQ(latest_handled_irq_timestamp, kIrqTimestamp1);

  // Manually trigger the virtual interrupt again.
  irq_handler_invoked.Reset();
  static constexpr zx::time_boot kIrqTimestamp2 = zx::time_boot(0x23456789);
  status = virtual_interrupt.trigger(0u, kIrqTimestamp2);
  ASSERT_OK(status);

  // The interrupt handler can be invoked again when the interrupt is triggered
  // again.
  irq_handler_invoked.Wait();
  EXPECT_EQ(latest_handled_irq_timestamp, kIrqTimestamp2);

  // Stop the driver and its dispatchers. The handler should receive a
  // ZX_ERR_CANCELED signal.
  StopDriver();
  irq_handler_canceled.Wait();
}

}  // namespace

}  // namespace display
