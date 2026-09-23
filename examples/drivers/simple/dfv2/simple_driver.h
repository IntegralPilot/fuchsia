// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_SIMPLE_DFV2_SIMPLE_DRIVER_H_
#define EXAMPLES_DRIVERS_SIMPLE_DFV2_SIMPLE_DRIVER_H_

#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/node/cpp/add_child.h>

namespace simple {

// Simple driver that demonstrates how the Start(), PrepareStop(), and Stop() functions are invoked.
// This driver also shows how to add logging and how to add a child node.
class SimpleDriver : public fdf::DriverBase2 {
 public:
  SimpleDriver();

  // Only implemented in this example to demonstrate the driver lifecycle. Drivers should avoid
  // implementing the destructor and perform in Start() and PrepareStop().
  ~SimpleDriver();

  // Called by the Driver Framework to initialize the driver instance.
  zx::result<> Start(fdf::DriverContext context) override;

  // Called by the Driver Framework before it shutdowns all of the driver's fdf_dispatchers.
  // The driver should use this function to initiate any teardowns on the fdf_dispatchers before
  // they're stopped and deallocated by the Driver Framework.
  void Stop(fdf::StopCompleter completer) override;

  // Exposed for testing.
  std::string_view GetName() const { return name(); }

 private:
  // Required for maintaining the topological path.
  compat::SyncInitializedDeviceServer compat_server_;

  // Client for controller the child node.
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> child_controller_;

  // Owned child node.
  fdf::OwnedChildNode owned_child_;
};

}  // namespace simple

#endif  // EXAMPLES_DRIVERS_SIMPLE_DFV2_SIMPLE_DRIVER_H_
