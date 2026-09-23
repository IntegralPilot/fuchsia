// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_PD_SINK_STATE_MACHINE_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_PD_SINK_STATE_MACHINE_H_

#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/timer.h>

#include <cstdint>

#include "src/devices/power/drivers/fusb302/state-machine-base.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"
#include "src/devices/power/drivers/fusb302/usb-pd-sink-policy.h"

namespace fusb302 {

class Fusb302;

// Changes signaled to the Sink Policy Engine state machine.
enum class SinkPolicyEngineInput : int32_t {
  kInitialized = 1,
  kMessageReceived = 2,
  kTimerFired = 3,
};

// Sink Policy Engine States. States for SinkPolicyEngineStateMachine.
//
// The states are a subset of the Sink states listed in Table 8-80 "Policy
// Engine States" in Section 8.3.3.30 "Policy Engine States" of the USB PD spec.
enum class SinkPolicyEngineState : int32_t {
  kStartup = 1,              // PE_SNK_Startup, S 8.3.3.3.1
  kDiscovery = 2,            // PE_SNK_Discovery, S 8.3.3.3.2
  kWaitForCapabilities = 3,  // PE_SNK_Wait_for_Capabilities, S 8.3.3.3.3
  kEvaluateCapability = 4,   // PE_SNK_Evaluate_Capability, S 8.3.3.3.4
  kSelectCapability = 5,     // PE_SNK_Select_Capability, S 8.3.3.3.5
  kTransitionSink = 6,       // PE_SNK_Transition_Sink, S 8.3.3.3.6
  kReady = 7,                // PE_SNK_Ready, S 8.3.3.3.7
  // 8 is reserved for PE_SNK_Hard_Reset, S 8.3.3.3.8
  // 9 is reserved for PE_SNK_Transition_to_default, S 8.3.3.3.9
  kGiveSinkCapabilities = 10,  // PE_SNK_Give_Sink_Cap, S 8.3.3.3.10
  // 11 is reserved for PE_SNK_EPR_Keep_Alive, S 8.3.3.3.11
  kGetSourceCapabilities = 12,  // PE_SNK_Get_Source_Cap, S 8.3.3.3.12
  kSendSoftReset = 13,          // PE_SNK_Send_Soft_Reset, S 8.3.3.4.2.1
  kSoftReset = 14,              // PE_SNK_Soft_Reset, S 8.3.3.4.2.2
};

// Implements the USB PD Sink Policy Engine state machine.
//
// usbpd3.1 8.3.3 "State Diagrams", especially 8.3.3.3 "Policy Engine Sink Port
// State Diagram"
class SinkPolicyEngineStateMachine
    : public StateMachineBase<SinkPolicyEngineStateMachine, SinkPolicyEngineState,
                              SinkPolicyEngineInput> {
 public:
  // `policy` and `device` must remain alive throughout the new instance's
  // lifetime.
  explicit SinkPolicyEngineStateMachine(usb_pd::SinkPolicy& policy, Fusb302& device,
                                        inspect::Node inspect_root)
      : StateMachineBase(SinkPolicyEngineState::kStartup, std::move(inspect_root),
                         "Sink Policy Engine"),
        policy_(policy),
        device_(device) {}
  SinkPolicyEngineStateMachine(const SinkPolicyEngineStateMachine&) = delete;
  SinkPolicyEngineStateMachine& operator=(const SinkPolicyEngineStateMachine&) = delete;

  ~SinkPolicyEngineStateMachine() override = default;

  // Called when the Type C port state changes.
  void Reset();

  // Called when the driver reports the receipt of a Soft Reset message.
  //
  // This is a separate method to facilitate interacting with hardware that
  // reports Soft Reset receipt via dedicated signals, instead of simply
  // bubbling the PD message's bytes up to the driver.
  //
  // Drivers must always report Soft Reset receipt via this method. Soft Reset
  // must not also be reported as received PD messages.
  void DidReceiveSoftReset();

  // Forces this instance to adjust its state to reply to an unexpected message.
  void ProcessUnexpectedMessage();

 protected:
  void EnterState(SinkPolicyEngineState state) override;
  void ExitState(SinkPolicyEngineState state) override;
  SinkPolicyEngineState NextState(SinkPolicyEngineInput input,
                                  SinkPolicyEngineState current_state) override;
  const char* StateToString(SinkPolicyEngineState state) const override;

 private:
  // Sets up the PD controller's protocol layer after a port state change.
  void InitializeProtocolLayer();

  // Sets up the PD controller's protocol layer after a soft reset.
  void ResetProtocolLayer();

  // Initializes zx::timer data members.
  //
  // Timer creation involves zircon resource creation, which can fail. So,
  // zx::timer instances must be created in a separate initialization stage.
  void CreateTimersIfNeeded();

  // Starts the timer used to wait for a Source_Capabilities message.
  void ArmWaitForSourceCapabilitiesTimer();

  SinkPolicyEngineState ProcessMessageInReady();

  // State Machine Timers
  zx::timer wait_for_source_capabilities_timer_;

  // The referenced instances are guaranteed to outlive this instance, because
  // they're owned by this instance's owner (Fusb302).
  usb_pd::SinkPolicy& policy_;
  Fusb302& device_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_PD_SINK_STATE_MACHINE_H_
