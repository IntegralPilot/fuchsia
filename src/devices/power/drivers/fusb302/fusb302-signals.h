// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_SIGNALS_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_SIGNALS_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/zx/result.h>

#include "src/devices/power/drivers/fusb302/fusb302-protocol.h"
#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"

struct HardwareStateChanges {
  // VBUS or CC pin voltage measurements changed. May need de-bouncing.
  bool port_state_changed = false;

  // Soft Reset message or Hard Reset ordered set.
  bool received_reset = false;

  // A timer was signaled.
  bool timer_signaled = false;
};

namespace fusb302 {

// Owns the device's Interrupt* and Mask* registers.
//
// This class manages the FUSB302's interrupt unit. It dispatches low-level
// actions directly to the classes managing FIFOs and sensors, and reports
// high-level signals interpreted by state machines.
class Fusb302Signals {
 public:
  // `i2c_channel`, `sensors` and `protocol` must remain alive throughout the new
  // instance's lifetime.
  explicit Fusb302Signals(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel,
                          Fusb302Sensors& sensors, Fusb302Protocol& protocol);

  Fusb302Signals(const Fusb302Signals&) = delete;
  Fusb302Signals& operator=(const Fusb302Signals&) = delete;

  // Trivially destructible.
  ~Fusb302Signals() = default;

  // Services all pending FUSB302 interrupts.
  //
  // Returns information about interrupt post-processing.
  HardwareStateChanges ServiceInterrupts();

  // Configure interrupt masks to receive the interrupts we can service.
  //
  // Must be called after issuing a software reset to the FUSB302 chip, before
  // enabling interrupts.
  //
  // On success, all pending interrupts are flushed. Logs an ERROR on failure.
  zx::result<> InitInterruptUnit();

 private:
  // The referenced instances are guaranteed to outlive this instance, because
  // they're owned by this instance's owner (Fusb302).
  fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_;
  Fusb302Sensors& sensors_;
  Fusb302Protocol& protocol_;

  const bool goodcrc_interrupts_enabled_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_SIGNALS_H_
