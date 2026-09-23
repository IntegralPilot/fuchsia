// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_SENSORS_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_SENSORS_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/result.h>

#include "src/devices/power/drivers/fusb302/inspectable-types.h"
#include "src/devices/power/drivers/fusb302/registers.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"

namespace fusb302 {

// Owns the device's Status* registers.
//
// This class handles the FUSB302's voltage and temperature sensors, and the
// power role detector's output.
class Fusb302Sensors {
 public:
  // `i2c_channel` must remain alive throughout the new instance's lifetime.
  explicit Fusb302Sensors(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel,
                          inspect::Node root_node);

  Fusb302Sensors(const Fusb302Sensors&) = delete;
  Fusb302Sensors& operator=(const Fusb302Sensors&) = delete;

  ~Fusb302Sensors();

  // Returns true if something changed.
  bool UpdateComparatorsResult();
  bool UpdatePowerRoleDetectionResult();

  // Cached sensor state.
  bool vbus_power_good() const { return vbus_power_good_.get(); }
  usb_pd::ConfigChannelTermination cc_termination() const { return cc_termination_.get(); }
  PowerRoleDetectionState power_role_detection_state() const {
    return power_role_detection_state_.get();
  }
  usb_pd::PowerRole detected_power_role() const { return detected_power_role_.get(); }
  usb_pd::ConfigChannelPinSwitch detected_wired_cc_pin() const {
    return detected_wired_cc_pin_.get();
  }

  // Must be called when the switch and measure block configuration is updated.
  void SetConfiguration(usb_pd::PowerRole power_role, usb_pd::ConfigChannelPinSwitch wired_cc_pin) {
    configured_power_role_ = power_role;
    configured_wired_cc_pin_ = wired_cc_pin;
  }

 private:
  void SyncDerivedPowerRoleDetectionState();

  // The referenced instances are guaranteed to outlive this instance, because
  // they're owned by this instance's owner (Fusb302).
  fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_;

  inspect::Node root_node_;
  InspectableBool<bool> vbus_power_good_;
  InspectableInt<usb_pd::ConfigChannelTermination> cc_termination_;
  InspectableInt<PowerRoleDetectionState> power_role_detection_state_;
  InspectableInt<usb_pd::PowerRole> detected_power_role_;
  InspectableInt<usb_pd::ConfigChannelPinSwitch> detected_wired_cc_pin_;

  usb_pd::PowerRole configured_power_role_;
  usb_pd::ConfigChannelPinSwitch configured_wired_cc_pin_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_SENSORS_H_
