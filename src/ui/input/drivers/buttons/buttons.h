// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_BUTTONS_BUTTONS_H_
#define SRC_UI_INPUT_DRIVERS_BUTTONS_BUTTONS_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include "src/ui/input/drivers/buttons/buttons-device.h"
#include "src/ui/input/drivers/buttons/buttons_config.h"

namespace buttons {

static constexpr char kDeviceName[] = "buttons";

class Buttons : public fdf::DriverBase2 {
 public:
  explicit Buttons()
      : fdf::DriverBase2(kDeviceName), devfs_connector_(fit::bind_member<&Buttons::Serve>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

 private:
  zx::result<> CreateDevfsNode();
  void Serve(fidl::ServerEnd<fuchsia_input_report::InputDevice> server) {
    input_report_bindings_.AddBinding(dispatcher(), std::move(server), device_.get(),
                                      fidl::kIgnoreBindingClosure);
  }

  std::unique_ptr<ButtonsDevice> device_;
  fidl::ServerBindingGroup<fuchsia_input_report::InputDevice> input_report_bindings_;
  fdf::OwnedChildNode child_;
  driver_devfs::Connector<fuchsia_input_report::InputDevice> devfs_connector_;
  buttons_config::Config config_;
};

}  // namespace buttons

#endif  // SRC_UI_INPUT_DRIVERS_BUTTONS_BUTTONS_H_
