// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/connectivity/wlan/drivers/lib/components/cpp/test/test_driver.h"

#include <lib/driver/component/cpp/driver_export2.h>

namespace wlan::drivers::components::test {

TestDriver::TestDriver() : fdf::DriverBase2("netdev-test-driver") {}

void TestDriver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  completer(zx::ok());
}

void TestDriver::Stop(fdf::StopCompleter completer) {
  if (stop_handler_) {
    stop_handler_->Stop(std::move(completer));
    return;
  }
  completer(zx::ok());
}

}  // namespace wlan::drivers::components::test

FUCHSIA_DRIVER_EXPORT2(wlan::drivers::components::test::TestDriver);
