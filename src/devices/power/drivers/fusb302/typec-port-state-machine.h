// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_TYPEC_PORT_STATE_MACHINE_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_TYPEC_PORT_STATE_MACHINE_H_

#include <lib/inspect/cpp/vmo/types.h>

#include <cstdint>

#include "src/devices/power/drivers/fusb302/state-machine-base.h"

namespace fusb302 {

class Fusb302;

// Changes signaled to the Type C Port Config Channel state machine.
enum class TypeCPortInput : uint32_t {
  kPortStateChanged = 1,
  kTimerFired = 2,
};

// States for the Type C Port Config Channel state machine.
//
// The states are defined in Section 4.5.2 "CC Functional and Behavioral
// Requirements" in the USB Type C spec. The state values match the
// corresponding sub-section numbers in Section 4.5.2.2 "Connection State
// Machine Requirements".
enum class TypeCPortState : uint32_t {
  kSinkUnattached = 3,  // Unattached.SNK
  kSinkAttached = 5,    // Attached.SNK
  kSourceAttached = 9,  // Attached.SRC
};

// Implements the Type C Port Config Channel state machine.
//
// typec2.2 4.5.2 "CC Functional and Behavioral Requirements"
class TypeCPortStateMachine
    : public StateMachineBase<TypeCPortStateMachine, TypeCPortState, TypeCPortInput> {
 public:
  // `device` must remain alive throughout the new instance's lifetime.
  explicit TypeCPortStateMachine(Fusb302& device, inspect::Node inspect_root)
      : StateMachineBase(TypeCPortState::kSinkUnattached, std::move(inspect_root), "Type C Port"),
        device_(device) {}

  TypeCPortStateMachine(const TypeCPortStateMachine&) = delete;
  TypeCPortStateMachine& operator=(const TypeCPortStateMachine&) = delete;

  ~TypeCPortStateMachine() override = default;

  const char* StateToString(TypeCPortState state) const override;

 protected:
  void EnterState(TypeCPortState state) override;
  void ExitState(TypeCPortState state) override;
  TypeCPortState NextState(TypeCPortInput input, TypeCPortState current_state) override;

 private:
  // The referenced instance is guaranteed to outlive this instance, because
  // it's owned by this instance's owner (Fusb302).
  Fusb302& device_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_TYPEC_PORT_STATE_MACHINE_H_
