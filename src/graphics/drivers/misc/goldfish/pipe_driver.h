// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_PIPE_DRIVER_H_
#define SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_PIPE_DRIVER_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/node/cpp/add_child.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <memory>

#include <fbl/mutex.h>

#include "src/graphics/drivers/misc/goldfish/pipe_device.h"

namespace goldfish {

class PipeDriver : public fdf::DriverBase2 {
 public:
  explicit PipeDriver();
  ~PipeDriver() override;

  PipeDriver(const PipeDriver&) = delete;
  PipeDriver& operator=(const PipeDriver&) = delete;
  PipeDriver(PipeDriver&&) = delete;
  PipeDriver& operator=(PipeDriver&&) = delete;

  // `fdf::DriverBase`:
  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

 private:
  void ServePipeDevice(fidl::ServerEnd<fuchsia_hardware_goldfish::PipeDevice> server);

  // Initialized in `Start()`.
  std::unique_ptr<PipeDevice> pipe_device_;

  // `pipe_device_` must outlive `pipe_device_bindings_`.
  fidl::ServerBindingGroup<fuchsia_hardware_goldfish::PipeDevice> pipe_device_bindings_;

  driver_devfs::Connector<fuchsia_hardware_goldfish::PipeDevice> devfs_connector_{
      fit::bind_member<&PipeDriver::ServePipeDevice>(this)};

  fdf::OwnedChildNode devfs_child_node_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> control_child_;
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_PIPE_DRIVER_H_
