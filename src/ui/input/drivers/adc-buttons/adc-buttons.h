// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_ADC_BUTTONS_ADC_BUTTONS_H_
#define SRC_UI_INPUT_DRIVERS_ADC_BUTTONS_ADC_BUTTONS_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include "src/ui/input/drivers/adc-buttons/adc-buttons-device.h"

namespace adc_buttons {

static const std::string kDeviceName = "adc-buttons";

class AdcButtons : public fdf::DriverBase2 {
 public:
  explicit AdcButtons()
      : fdf::DriverBase2(kDeviceName),
        devfs_connector_(fit::bind_member<&AdcButtons::Serve>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

 private:
  zx::result<> CreateDevfsNode();
  void Serve(fidl::ServerEnd<fuchsia_input_report::InputDevice> server) {
    input_report_bindings_.AddBinding(dispatcher(), std::move(server), device_.get(),
                                      fidl::kIgnoreBindingClosure);
  }

  std::unique_ptr<adc_buttons_device::AdcButtonsDevice> device_;
  fidl::ServerBindingGroup<fuchsia_input_report::InputDevice> input_report_bindings_;
  fdf::OwnedChildNode child_;
  driver_devfs::Connector<fuchsia_input_report::InputDevice> devfs_connector_;
};

}  // namespace adc_buttons

#endif  // SRC_UI_INPUT_DRIVERS_ADC_BUTTONS_ADC_BUTTONS_H_
