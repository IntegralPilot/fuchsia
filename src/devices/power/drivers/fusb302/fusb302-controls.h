// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_CONTROLS_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_CONTROLS_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/result.h>

#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"
#include "src/devices/power/drivers/fusb302/inspectable-types.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"

namespace fusb302 {

// Owns the device's Control* and Switch* registers.
//
// This class handles the FUSB302's entire configuration, with the exception of
// interrupt masks.
class Fusb302Controls {
 public:
  // `i2c_channel` and `sensors` must remain alive throughout the new instance's
  // lifetime.
  explicit Fusb302Controls(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel,
                           Fusb302Sensors& sensors, inspect::Node root_node);

  Fusb302Controls(const Fusb302Controls&) = delete;
  Fusb302Controls& operator=(const Fusb302Controls&) = delete;

  ~Fusb302Controls();

  // Configures the chip for automated power role discovery.
  zx::result<> ResetIntoPowerRoleDiscovery();

  // Configures the chip for a specific role and cable configuration.
  zx::result<> ConfigureAllRoles(usb_pd::ConfigChannelPinSwitch wired_cc_pin,
                                 usb_pd::PowerRole power_role, usb_pd::DataRole data_role,
                                 usb_pd::SpecRevision spec_revision);

  // Committed configuration.
  usb_pd::ConfigChannelPinSwitch wired_cc_pin() const { return wired_cc_pin_.get(); }
  usb_pd::PowerRole power_role() const { return power_role_.get(); }
  usb_pd::DataRole data_role() const { return data_role_.get(); }
  usb_pd::SpecRevision spec_revision() const { return spec_revision_.get(); }

  // Returns when the PHY layer's transmit FIFO is flushed.
  zx::result<> WaitForTransmitQueueFlush();

  // Returns when the PHY layer's receive FIFO is flushed.
  zx::result<> WaitForReceiveQueueFlush();

 private:
  // Configures and starts/stops the automated power role detection logic.
  //
  // This must be the last state push when starting power role detection, and
  // the first state push when stopping the power role detection.
  zx::result<> PushStateToPowerRoleDetectionControl();

  zx::result<> PushStateToSwitchBlocks();
  zx::result<> PushStateToBmcPhyConfig();
  zx::result<> PushStateToPowerWells();
  zx::result<> PushStateToPdProtocolConfig();

  // The referenced instances are guaranteed to outlive this instance, because
  // they're owned by this instance's owner (Fusb302).
  fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_;
  Fusb302Sensors& sensors_;

  inspect::Node root_node_;
  InspectableInt<usb_pd::ConfigChannelPinSwitch> wired_cc_pin_;
  InspectableInt<usb_pd::PowerRole> power_role_;
  InspectableInt<usb_pd::DataRole> data_role_;
  InspectableUint<usb_pd::SpecRevision> spec_revision_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_CONTROLS_H_
