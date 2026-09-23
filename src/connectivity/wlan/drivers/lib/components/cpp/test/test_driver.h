// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_TEST_DRIVER_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_TEST_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

namespace wlan::drivers::components::test {

// Since this is a library for use by drivers it doesn't contain a driver of its own. Create a
// driver for the tests to use to interact with.
class TestDriver : public fdf::DriverBase2 {
 public:
  class StopHandler {
   public:
    virtual ~StopHandler() = default;

    virtual void Stop(fdf::StopCompleter completer) = 0;
  };

  explicit TestDriver();

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
  void Stop(fdf::StopCompleter completer) override;

  void AssignStopHandler(StopHandler* stop_handler) { stop_handler_ = stop_handler; }
  std::shared_ptr<fdf::OutgoingDirectory>& outgoing() { return fdf::DriverBase2::outgoing(); }

 private:
  StopHandler* stop_handler_ = nullptr;
};

}  // namespace wlan::drivers::components::test

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_TEST_DRIVER_H_
