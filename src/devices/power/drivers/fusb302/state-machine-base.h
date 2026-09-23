// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_STATE_MACHINE_BASE_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_STATE_MACHINE_BASE_H_

#include <lib/driver/logging/cpp/logger.h>

#include <utility>

#include "src/devices/power/drivers/fusb302/inspectable-types.h"

namespace fusb302 {

// Scaffolding for implementing a state machine in the USB PD spec.
//
// `StateMachine` is the class deriving from this base class.
//
// `State` is the type representing a state. It should be a value type,
// equivalent to a scoped enum whose underlying representation is a signed
// integer. In particular, instances must be copyable (and the copy operation is
// assumed to be cheap), must be convertible to int64_t, and the copies must be
// safe to store for the state machine instance's entire lifetime.
//
// `Input` is the type representing an `EnterState` input. It must be trivially
// copyable.
template <typename StateMachine, typename State, typename Input>
class StateMachineBase {
 public:
  explicit StateMachineBase(State initial_state, inspect::Node inspect_root, const char* debug_name)
      : inspect_root_(std::move(inspect_root)),
        debug_name_(debug_name),
        current_state_(InspectableInt<State>(&inspect_root_, "State", initial_state)) {}

  StateMachineBase(const StateMachineBase&) = delete;
  StateMachineBase& operator=(const StateMachineBase&) = delete;

  virtual ~StateMachineBase() = default;

  State current_state() const { return current_state_.get(); }

  // Evaluates `input` against the state machine until it stops transitioning.
  void Run(Input input);

 protected:
  // Called on a transition into `state`.
  //
  // Implementations must not change the current state, e.g. by calling
  // `ForceStateTransition()`.
  virtual void EnterState(State state) = 0;

  // Decides if `input` received in `current_state` warrants a state change.
  //
  // Returns the state machine's new state, which can be `current_state` if no
  // transition is warranted.
  //
  // Implementations must not change the current state, e.g. via
  // `ForceStateTransition()`.
  //
  // Implementation advice: "return current_state;" is a readable way to express
  // the lack of a state transition.
  virtual State NextState(Input input, State current_state) = 0;

  // Called on a transition out of `state`.
  //
  // Implementations must not change the current state, e.g. by calling
  // `ForceStateTransition()`.
  virtual void ExitState(State state) = 0;

  // Returns a developer-friendly description of `state` suitable for logging.
  virtual const char* StateToString(State state) const = 0;

  // Subclass hook for changing the state outside of Run().
  //
  // This method can be used to allow changing a state machine's state without
  // invoking the `Run()` / `NextState()` machinery. This can help implement
  // specifications that allow "out-of-band" state changes, without having to
  // burden `NextState()` with knowledge of these out-of-band state changes.
  //
  // This method is not public so that subclasses can enforce boundaries on the
  // arbitrary state transitions. For example, a subclass may choose to only
  // allow transitioning to designated states, such as "disabled" or "ready".
  void ForceStateTransition(State new_state);

  inspect::Node& inspect_root() { return inspect_root_; }

 private:
  inspect::Node inspect_root_;
  const char* debug_name_;

  InspectableInt<State> current_state_;
  bool current_state_differs_from_previous_state_ = true;
};

template <class StateMachine, typename State, typename Input>
inline void StateMachineBase<StateMachine, State, Input>::Run(Input input) {
  do {
    if (current_state_differs_from_previous_state_) {
      current_state_differs_from_previous_state_ = false;
      const State entering_state = current_state_.get();
      fdf::trace("State machine {} entering new state {}", debug_name_,
                 StateToString(entering_state));
      EnterState(entering_state);
    }

    const State evaluate_state = current_state_.get();
    fdf::trace("State machine {} evaluating new input in state {}", debug_name_,
               StateToString(evaluate_state));
    State next_state = NextState(input, evaluate_state);

    if (next_state != evaluate_state) {
      fdf::trace("State machine {} exiting state {} preparing to enter state {}", debug_name_,
                 StateToString(evaluate_state), StateToString(next_state));
      ExitState(current_state_.get());
      current_state_differs_from_previous_state_ = true;
    }

    current_state_.set(next_state);
  } while (current_state_differs_from_previous_state_);
}

template <class StateMachine, typename State, typename Input>
inline void StateMachineBase<StateMachine, State, Input>::ForceStateTransition(State new_state) {
  const State previous_state = current_state_.get();
  if (previous_state == new_state) {
    fdf::trace(
        "State machine {} forced to transition to state {} but already there; "
        "no actions triggered",
        debug_name_, StateToString(previous_state));
    return;
  }

  if (!current_state_differs_from_previous_state_) {
    fdf::trace("State machine {} exiting state {} preparing for forced transition to state {}",
               debug_name_, StateToString(previous_state), StateToString(new_state));
    ExitState(previous_state);
  }
  current_state_differs_from_previous_state_ = true;
  current_state_.set(new_state);
}

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_STATE_MACHINE_BASE_H_
