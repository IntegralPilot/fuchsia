// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <cstdint>
#include <utility>

#include "src/devices/power/drivers/fusb302/registers.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"

namespace fusb302 {

Fusb302Sensors::Fusb302Sensors(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel,
                               inspect::Node root_node)
    : i2c_(i2c_channel),
      root_node_(std::move(root_node)),
      vbus_power_good_(&root_node_, "VBusPowerGood", false),
      cc_termination_(&root_node_, "CCTermination", usb_pd::ConfigChannelTermination::kUnknown),
      power_role_detection_state_(&root_node, "PowerRoleDetection",
                                  PowerRoleDetectionState::kDetecting),
      detected_power_role_(&root_node, "DetectedPowerRole", usb_pd::PowerRole::kSink),
      detected_wired_cc_pin_(&root_node, "DetectedCCPin", usb_pd::ConfigChannelPinSwitch::kNone) {}

Fusb302Sensors::~Fusb302Sensors() = default;

bool Fusb302Sensors::UpdateComparatorsResult() {
  auto status0 = Status0Reg::ReadFrom(i2c_);

  const bool vbus_power_good = status0.vbusok();

  usb_pd::ConfigChannelTermination cc_termination;
  if (configured_wired_cc_pin_ == usb_pd::ConfigChannelPinSwitch::kNone) {
    // Measure block disconnected from CC pin.
    cc_termination = usb_pd::ConfigChannelTermination::kUnknown;
  } else if (configured_power_role_ == usb_pd::PowerRole::kSink) {
    cc_termination = ConfigChannelTerminationFromFixedComparatorResult(status0.bc_lvl());
    if (cc_termination == usb_pd::ConfigChannelTermination::kRp3000mA && status0.comp()) {
      fdf::warn("{} pin voltage exceeds 2.05V (c/o variable comparator)",
                ConfigChannelPinSwitchToString(configured_wired_cc_pin_));
      cc_termination = usb_pd::ConfigChannelTermination::kUnknown;
    }
  } else {
    ZX_DEBUG_ASSERT_MSG(false, "Source sensing not implemented");
    cc_termination = usb_pd::ConfigChannelTermination::kUnknown;
  }

  const bool state_changed =
      (vbus_power_good_.get() != vbus_power_good) || (cc_termination_.get() != cc_termination);
  vbus_power_good_.set(vbus_power_good);
  cc_termination_.set(cc_termination);

  if (state_changed) {
    fdf::info("Voltage comparators result: VBUS {}, CC wire termination {}",
              vbus_power_good ? ">= 4.0 V" : "< 4.0 V",
              ConfigChannelTerminationToString(cc_termination));
  }
  return state_changed;
}

bool Fusb302Sensors::UpdatePowerRoleDetectionResult() {
  const PowerRoleDetectionState power_role_detection_state = Status1AReg::ReadFrom(i2c_).togss();

  const bool state_changed = power_role_detection_state_.get() != power_role_detection_state;
  power_role_detection_state_.set(power_role_detection_state);
  if (state_changed) {
    fdf::info("Power role detection state: {}",
              PowerRoleDetectionStateToString(power_role_detection_state));
    SyncDerivedPowerRoleDetectionState();
  }
  return state_changed;
}

void Fusb302Sensors::SyncDerivedPowerRoleDetectionState() {
  const PowerRoleDetectionState power_role_detection_state = power_role_detection_state_.get();
  const usb_pd::ConfigChannelPinSwitch wired_cc_pin =
      WiredCcPinFromPowerRoleDetectionState(power_role_detection_state);
  detected_wired_cc_pin_.set(wired_cc_pin);

  if (wired_cc_pin == usb_pd::ConfigChannelPinSwitch::kNone) {
    fdf::trace("Power role detection result: running");
    detected_power_role_.set(usb_pd::PowerRole::kSink);
    return;
  }

  // Only safe to do after we know that `togss` corresponds
  // to an attached state.
  const usb_pd::PowerRole power_role = PowerRoleFromDetectionState(power_role_detection_state);
  detected_power_role_.set(power_role);

  fdf::info("Power role detection result: state {}, power {}, Config Channel on {}",
            PowerRoleDetectionStateToString(power_role_detection_state),
            usb_pd::PowerRoleToString(power_role),
            usb_pd::ConfigChannelPinSwitchToString(wired_cc_pin));
}

}  // namespace fusb302
