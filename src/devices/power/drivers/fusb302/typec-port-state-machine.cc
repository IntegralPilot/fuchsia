// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/typec-port-state-machine.h"

#include <lib/inspect/cpp/vmo/types.h>

#include <cinttypes>
#include <utility>

#include "src/devices/power/drivers/fusb302/fusb302.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"

namespace fusb302 {

void TypeCPortStateMachine::EnterState(TypeCPortState state) {
  switch (state) {
    case TypeCPortState::kSinkUnattached:
    case TypeCPortState::kSinkAttached:
      return;
    case TypeCPortState::kSourceAttached:
      fdf::error("SourceAttached not implemented");
      return;
  }

  fdf::error("Invalid state: {}", static_cast<int32_t>(state));
}

void TypeCPortStateMachine::ExitState(TypeCPortState state) {}

TypeCPortState TypeCPortStateMachine::NextState(TypeCPortInput input,
                                                TypeCPortState current_state) {
  switch (current_state) {
    case TypeCPortState::kSinkUnattached:
      if (input != TypeCPortInput::kPortStateChanged) {
        return current_state;
      }
      if (device_.sensors().detected_wired_cc_pin() == usb_pd::ConfigChannelPinSwitch::kNone) {
        return current_state;
      }
      if (device_.sensors().detected_power_role() != usb_pd::PowerRole::kSink) {
        fdf::warn("Sink-only, ignoring Source power state");
        return current_state;
      }
      return TypeCPortState::kSinkAttached;

    case TypeCPortState::kSinkAttached:
      if (input != TypeCPortInput::kPortStateChanged) {
        return current_state;
      }
      if (device_.sensors().detected_wired_cc_pin() != usb_pd::ConfigChannelPinSwitch::kNone) {
        return current_state;
      }
      return TypeCPortState::kSinkUnattached;

    case TypeCPortState::kSourceAttached:
      // Only sink is currently implemented
      fdf::error("SourceAttached not implemented");
      return current_state;
  }

  fdf::error("Invalid state: {}", static_cast<int32_t>(current_state));
}

const char* TypeCPortStateMachine::StateToString(TypeCPortState state) const {
  switch (state) {
    case TypeCPortState::kSinkUnattached:
      return "SinkUnattached";
    case TypeCPortState::kSinkAttached:
      return "SinkAttached";
    case TypeCPortState::kSourceAttached:
      return "SourceAttached";
  }

  ZX_DEBUG_ASSERT_MSG(false, "Invalid TypeCPortState: %" PRId32, static_cast<int32_t>(state));
  return "(invalid)";
}

}  // namespace fusb302
