// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_XHCI_TESTS_TEST_ENV_H_
#define SRC_DEVICES_USB_DRIVERS_XHCI_TESTS_TEST_ENV_H_

#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fpromise/promise.h>
#include <lib/synchronous-executor/executor.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

#include "src/devices/usb/drivers/xhci/usb-xhci.h"
#include "src/lib/testing/predicates/status.h"

namespace usb_xhci {

const zx::bti kFakeBti(42);

// Test-only helper. Provides the error-handling policy used by unit tests that stub
// EventRing::ScheduleTask without linking xhci-event-ring.cc. Production code does not
// use this helper - EventRing::ScheduleTask in xhci-event-ring.cc applies the same
// conditional logic inline.
inline void SchedulePromiseWithXhciExecutorPolicy(
    synchronous_executor::synchronous_executor& executor, UsbXhci* hci,
    fpromise::promise<void, zx_status_t> promise) {
  auto continuation =
      promise.or_else([hci](const zx_status_t& status) -> fpromise::result<void, zx_status_t> {
        if (status == ZX_ERR_BAD_STATE) {
          fdf::error("Scheduled task returned a fatal error, shutting down");
          hci->Shutdown(status);
        } else if (status != ZX_ERR_IO_NOT_PRESENT) {
          fdf::warn("Scheduled task failed: {}", zx_status_get_string(status));
        }
        return fpromise::ok();
      });
  executor.schedule_task(std::move(continuation));
}

class EmptyTestEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override { return zx::ok(); }
};

class EmptyTestConfig final {
 public:
  using DriverType = usb_xhci::UsbXhci;
  using EnvironmentType = EmptyTestEnvironment;
};

}  // namespace usb_xhci

#endif  // SRC_DEVICES_USB_DRIVERS_XHCI_TESTS_TEST_ENV_H_
