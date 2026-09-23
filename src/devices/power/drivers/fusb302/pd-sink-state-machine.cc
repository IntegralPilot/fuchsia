// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/pd-sink-state-machine.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <cinttypes>
#include <cstdint>
#include <utility>

#include "src/devices/power/drivers/fusb302/fusb302.h"
#include "src/devices/power/drivers/fusb302/registers.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-objects.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"

namespace fusb302 {

namespace {

// If we're still waiting for Sink_Capabilities after this time, Soft Reset.
//
// We use a timeout significantly larger than recommended by the USB PD spec.
// This lets us do the PD re-negotiation after most Fuchsia components have
// started up, which maximizes the chance that we'll meet the
// Request-to-Source_Capabilities timing request.
constexpr zx::duration kWaitForSourceCapabilitiesTimeout = zx::msec(10'000);

}  // namespace

void SinkPolicyEngineStateMachine::Reset() {
  ForceStateTransition(SinkPolicyEngineState::kStartup);
}

void SinkPolicyEngineStateMachine::DidReceiveSoftReset() {
  ForceStateTransition(SinkPolicyEngineState::kSoftReset);
}

// The implementation follows Section 8.3.3.3 "Policy Engine Sink Port State
// Diagram" and Section 8.3.3.4.2 "SOP Sink Port Soft Reset and Protocol Error
// State Diagram" of the USB PD spec.

void SinkPolicyEngineStateMachine::EnterState(SinkPolicyEngineState state) {
  switch (state) {
    case SinkPolicyEngineState::kStartup:
      CreateTimersIfNeeded();
      InitializeProtocolLayer();
      return;

    case SinkPolicyEngineState::kDiscovery:
      // No setup needed.
      return;

    case SinkPolicyEngineState::kWaitForCapabilities:
      ArmWaitForSourceCapabilitiesTimer();
      return;

    case SinkPolicyEngineState::kEvaluateCapability:
      return;

    case SinkPolicyEngineState::kSelectCapability: {
      const uint32_t data_objects[] = {static_cast<uint32_t>(policy_.GetPowerRequest())};
      usb_pd::Message power_request(usb_pd::MessageType::kRequestPower,
                                    device_.protocol().next_transmitted_message_id(),
                                    device_.controls().power_role(), usb_pd::SpecRevision::kRev2,
                                    device_.controls().data_role(), data_objects);
      [[maybe_unused]] zx::result<> result = device_.protocol().Transmit(power_request);
      return;
    }

    case SinkPolicyEngineState::kTransitionSink:
    case SinkPolicyEngineState::kReady:
      // No setup needed.
      return;

    case SinkPolicyEngineState::kGiveSinkCapabilities: {
      ZX_DEBUG_ASSERT(device_.protocol().HasUnreadMessage());
      [[maybe_unused]] zx::result<> result = device_.protocol().MarkMessageAsRead();

      usb_pd::Message sink_capabilities(
          usb_pd::MessageType::kSinkCapabilities, device_.protocol().next_transmitted_message_id(),
          device_.controls().power_role(), device_.controls().spec_revision(),
          device_.controls().data_role(), policy_.GetSinkCapabilities());
      result = device_.protocol().Transmit(sink_capabilities);
      return;
    }

    case SinkPolicyEngineState::kGetSourceCapabilities: {
      usb_pd::Message get_source_capabilities(
          usb_pd::MessageType::kGetSourceCapabilities,
          device_.protocol().next_transmitted_message_id(), device_.controls().power_role(),
          device_.controls().spec_revision(), device_.controls().data_role(), {});
      [[maybe_unused]] zx::result<> result = device_.protocol().Transmit(get_source_capabilities);
      return;
    }

    case SinkPolicyEngineState::kSendSoftReset:
      ResetProtocolLayer();
      device_.protocol().FullReset();
      {
        usb_pd::Message soft_reset(
            usb_pd::MessageType::kSoftReset, device_.protocol().next_transmitted_message_id(),
            device_.controls().power_role(), device_.controls().spec_revision(),
            device_.controls().data_role(), {});
        [[maybe_unused]] zx::result<> result = device_.protocol().Transmit(soft_reset);
      }
      return;

    case SinkPolicyEngineState::kSoftReset:
      ResetProtocolLayer();
      device_.protocol().DidReceiveSoftReset();
      {
        usb_pd::Message accept_reset(
            usb_pd::MessageType::kAccept, device_.protocol().next_transmitted_message_id(),
            device_.controls().power_role(), device_.controls().spec_revision(),
            device_.controls().data_role(), {});
        [[maybe_unused]] zx::result<> result = device_.protocol().Transmit(accept_reset);
      }
      return;
  }

  fdf::error("Invalid state: {}", static_cast<int32_t>(state));
}

SinkPolicyEngineState SinkPolicyEngineStateMachine::NextState(SinkPolicyEngineInput input,
                                                              SinkPolicyEngineState current_state) {
  switch (current_state) {
    case SinkPolicyEngineState::kStartup:
      return SinkPolicyEngineState::kDiscovery;

    case SinkPolicyEngineState::kDiscovery:
      if (device_.sensors().vbus_power_good()) {
        return SinkPolicyEngineState::kWaitForCapabilities;
      }
      return current_state;

    case SinkPolicyEngineState::kWaitForCapabilities:
      if (input == SinkPolicyEngineInput::kMessageReceived &&
          device_.protocol().HasUnreadMessage()) {
        const usb_pd::Message& message = device_.protocol().FirstUnreadMessage();
        if (message.header().message_type() == usb_pd::MessageType::kSourceCapabilities) {
          policy_.DidReceiveSourceCapabilities(message);
          [[maybe_unused]] zx::result<> result = device_.protocol().MarkMessageAsRead();
          return SinkPolicyEngineState::kEvaluateCapability;
        }
      }
      if (input == SinkPolicyEngineInput::kTimerFired) {
        fdf::trace("Checking if timer signal is from the source capabilities timer");
        const bool done_waiting = wait_for_source_capabilities_timer_.wait_one(
                                      ZX_TIMER_SIGNALED, zx::time(0), nullptr) == ZX_OK;
        if (done_waiting) {
          fdf::warn("Timed out waiting for Source_Capabilities. Trying a reset");
          return SinkPolicyEngineState::kSendSoftReset;
        }
      }
      return current_state;

    case SinkPolicyEngineState::kEvaluateCapability:
      // This state could be used to track an asynchronous FIDL exchange.
      return SinkPolicyEngineState::kSelectCapability;

    case SinkPolicyEngineState::kSelectCapability:
      if (input == SinkPolicyEngineInput::kMessageReceived &&
          device_.protocol().HasUnreadMessage()) {
        const usb_pd::Message& message = device_.protocol().FirstUnreadMessage();
        if (message.header().message_type() == usb_pd::MessageType::kAccept) {
          // Accept doesn't need a reply. We're done processing this message.
          [[maybe_unused]] zx::result<> result = device_.protocol().MarkMessageAsRead();
          return SinkPolicyEngineState::kTransitionSink;
        }
      }
      return current_state;

    case SinkPolicyEngineState::kTransitionSink:
      if (input == SinkPolicyEngineInput::kMessageReceived &&
          device_.protocol().HasUnreadMessage()) {
        const usb_pd::Message& message = device_.protocol().FirstUnreadMessage();
        if (message.header().message_type() == usb_pd::MessageType::kPowerSupplyReady) {
          // PS_RDY doesn't need a reply. We're done processing this message.
          [[maybe_unused]] zx::result<> result = device_.protocol().MarkMessageAsRead();
          return SinkPolicyEngineState::kReady;
        }
      }
      return current_state;

    case SinkPolicyEngineState::kReady:
      // TODO(rdzhuang): also accept requests from FIDL
      if (input == SinkPolicyEngineInput::kMessageReceived &&
          device_.protocol().HasUnreadMessage()) {
        return ProcessMessageInReady();
      }
      return current_state;

    case SinkPolicyEngineState::kGiveSinkCapabilities:
      return SinkPolicyEngineState::kReady;

    case SinkPolicyEngineState::kGetSourceCapabilities:
      return SinkPolicyEngineState::kWaitForCapabilities;

    case SinkPolicyEngineState::kSendSoftReset:
      if (input == SinkPolicyEngineInput::kMessageReceived &&
          device_.protocol().HasUnreadMessage()) {
        const usb_pd::Message& message = device_.protocol().FirstUnreadMessage();
        if (message.header().message_type() == usb_pd::MessageType::kAccept) {
          // Accept doesn't need a reply. We're done processing this message.
          [[maybe_unused]] zx::result<> result = device_.protocol().MarkMessageAsRead();
          return SinkPolicyEngineState::kWaitForCapabilities;
        }
      }
      return current_state;

    case SinkPolicyEngineState::kSoftReset:
      return SinkPolicyEngineState::kWaitForCapabilities;
  }

  fdf::error("Invalid state: {}", static_cast<int32_t>(current_state));
  return current_state;
}

void SinkPolicyEngineStateMachine::ExitState(SinkPolicyEngineState state) {
  switch (state) {
    case SinkPolicyEngineState::kStartup:
    case SinkPolicyEngineState::kDiscovery:
      // No cleanup needed.
      return;

    case SinkPolicyEngineState::kWaitForCapabilities:
      wait_for_source_capabilities_timer_.cancel();
      return;

    case SinkPolicyEngineState::kEvaluateCapability:
    case SinkPolicyEngineState::kSelectCapability:
    case SinkPolicyEngineState::kTransitionSink:
    case SinkPolicyEngineState::kReady:
    case SinkPolicyEngineState::kGiveSinkCapabilities:
    case SinkPolicyEngineState::kGetSourceCapabilities:
    case SinkPolicyEngineState::kSendSoftReset:
    case SinkPolicyEngineState::kSoftReset:
      // No cleanup needed.
      return;
  }

  fdf::error("Invalid state: {}", static_cast<int32_t>(state));
}

void SinkPolicyEngineStateMachine::InitializeProtocolLayer() {
  // Revision 2.0 is always safe for GoodCRC packets.
  const usb_pd::SpecRevision spec_revision = usb_pd::SpecRevision::kRev2;

  const usb_pd::PowerRole power_role = device_.sensors().detected_power_role();
  const usb_pd::DataRole data_role = (power_role == usb_pd::PowerRole::kSource)
                                         ? usb_pd::DataRole::kDownstreamFacingPort
                                         : usb_pd::DataRole::kUpstreamFacingPort;
  [[maybe_unused]] zx::result<> result = device_.controls().ConfigureAllRoles(
      device_.sensors().detected_wired_cc_pin(), device_.sensors().detected_power_role(), data_role,
      spec_revision);
  device_.protocol().FullReset();
  device_.protocol().SetGoodCrcTemplate(usb_pd::Header(
      usb_pd::MessageType::kGoodCrc,
      /*data_object_count=*/0, usb_pd::MessageId(0), power_role, spec_revision, data_role));
}

void SinkPolicyEngineStateMachine::ResetProtocolLayer() {
  [[maybe_unused]] zx::result<> result = device_.controls().ConfigureAllRoles(
      device_.controls().wired_cc_pin(), device_.controls().power_role(),
      device_.controls().data_role(), device_.controls().spec_revision());
}

void SinkPolicyEngineStateMachine::CreateTimersIfNeeded() {
  if (!wait_for_source_capabilities_timer_.is_valid()) {
    zx_status_t status = zx::timer::create(ZX_TIMER_SLACK_CENTER, ZX_CLOCK_MONOTONIC,
                                           &wait_for_source_capabilities_timer_);
    if (status != ZX_OK) {
      fdf::error("Failed to create Source_Capabilities wait timer: {}",
                 zx_status_get_string(status));
    }
  }

  // New timer initialization goes above this line.
}

void SinkPolicyEngineStateMachine::ArmWaitForSourceCapabilitiesTimer() {
  wait_for_source_capabilities_timer_.cancel();
  zx_status_t status =
      wait_for_source_capabilities_timer_.set(zx::deadline_after(kWaitForSourceCapabilitiesTimeout),
                                              /*slack=*/zx::duration(0));
  if (status != ZX_OK) {
    fdf::error("Failed to arm timer: {}", zx_status_get_string(status));
    return;
  }

  [[maybe_unused]] zx::result<> result =
      device_.WaitAsyncForTimer(wait_for_source_capabilities_timer_);
}

SinkPolicyEngineState SinkPolicyEngineStateMachine::ProcessMessageInReady() {
  ZX_DEBUG_ASSERT(device_.protocol().HasUnreadMessage());
  const usb_pd::Message& message = device_.protocol().FirstUnreadMessage();
  const usb_pd::MessageType message_type = message.header().message_type();

  if (message_type == usb_pd::MessageType::kSourceCapabilities) {
    // The message will be read when entering this state.
    return SinkPolicyEngineState::kWaitForCapabilities;
  }

  // The MacBookPro M1 USB-C ports issue a Hard Reset if this isn't handled.
  if (message_type == usb_pd::MessageType::kGetSinkCapabilities) {
    // The message will be read when entering this state.
    return SinkPolicyEngineState::kGiveSinkCapabilities;
  }

  // For now, handle every other message as an unsupported message.
  fdf::warn("Received unsupported PD message type {} in Ready state",
            usb_pd::MessageTypeToString(message_type));

  [[maybe_unused]] zx::result<> result = device_.protocol().MarkMessageAsRead();

  // This is an intentional USB PD spec deviation.
  //
  // Section 6.13.5 "Applicability of Structured VDM Commands" states that DFPs
  // and UFPs (Downward / Upward Facing Ports) that don't support Structured
  // VDM Commands reply with a Not_Supported message.
  //
  // However, the MacBookPro M1 USB-C ports issue a Soft Reset (and eventually
  // Hard Reset, after a few loops) if we reply with Not_Supported to
  // a Discover_Identity Structured VDM (Vendor_Defined_Message).
  if (message_type != usb_pd::MessageType::kVendorDefined) {
    usb_pd::Message not_supported(usb_pd::MessageType::kNotSupported,
                                  device_.protocol().next_transmitted_message_id(),
                                  device_.controls().power_role(), usb_pd::SpecRevision::kRev2,
                                  device_.controls().data_role(), {});
    result = device_.protocol().Transmit(not_supported);
  }
  return SinkPolicyEngineState::kReady;
}

void SinkPolicyEngineStateMachine::ProcessUnexpectedMessage() {
  // This is an intentional USB PD spec deviation.
  //
  // Section 6.13 "Message Applicability" states that unexpected messages may
  // only occur in the Ready state (PE_SNK_Ready for Sinks). In other states,
  // unexpected messages are protocol errors. Section 6.8.1 "Soft Reset and
  // Protocol Error" states that protocol errors are handled via a Soft reset in
  // most cases.
  //
  // We prefer to avoid Soft Reset, because the bug that caused us to receive
  // this message may be deterministic, in which case we'd be stuck in a Soft
  // Reset loop, which eventually leads to a Hard Reset.

  ZX_DEBUG_ASSERT(device_.protocol().HasUnreadMessage());
  const usb_pd::Message& message = device_.protocol().FirstUnreadMessage();
  const usb_pd::MessageType message_type = message.header().message_type();

  fdf::warn("Received unexpected PD message type {} in state {}",
            usb_pd::MessageTypeToString(message_type), StateToString(current_state()));

  if (message_type == usb_pd::MessageType::kSourceCapabilities) {
    // The message will be read when entering this state.
    ForceStateTransition(SinkPolicyEngineState::kWaitForCapabilities);
    return;
  }

  if (message_type == usb_pd::MessageType::kGetSinkCapabilities) {
    // The message will be read when entering this state.
    ForceStateTransition(SinkPolicyEngineState::kGiveSinkCapabilities);
    return;
  }

  // The state transitions below assume that the message has been read.
  [[maybe_unused]] zx::result<> result = device_.protocol().MarkMessageAsRead();

  // See ProcessMessageInReady() for the rationale for ignoring VDM
  // (vendor-defined messages).
  if (message_type != usb_pd::MessageType::kVendorDefined) {
    usb_pd::Message not_supported(usb_pd::MessageType::kNotSupported,
                                  device_.protocol().next_transmitted_message_id(),
                                  device_.controls().power_role(), usb_pd::SpecRevision::kRev2,
                                  device_.controls().data_role(), {});
    result = device_.protocol().Transmit(not_supported);
  }
}

const char* SinkPolicyEngineStateMachine::StateToString(SinkPolicyEngineState state) const {
  switch (state) {
    case SinkPolicyEngineState::kStartup:
      return "Startup";
    case SinkPolicyEngineState::kDiscovery:
      return "Discovery";
    case SinkPolicyEngineState::kWaitForCapabilities:
      return "WaitForCapabilities";
    case SinkPolicyEngineState::kEvaluateCapability:
      return "EvaluateCapability";
    case SinkPolicyEngineState::kSelectCapability:
      return "SelectCapability";
    case SinkPolicyEngineState::kTransitionSink:
      return "TransitionSink";
    case SinkPolicyEngineState::kReady:
      return "Ready";
    case SinkPolicyEngineState::kGiveSinkCapabilities:
      return "GiveSourceCapabilities";
    case SinkPolicyEngineState::kGetSourceCapabilities:
      return "GetSourceCapabilities";
    case SinkPolicyEngineState::kSendSoftReset:
      return "SendSoftReset";
    case SinkPolicyEngineState::kSoftReset:
      return "SoftReset";
  }

  ZX_DEBUG_ASSERT_MSG(false, "Invalid SinkPolicyEngineState: %" PRId32,
                      static_cast<int32_t>(state));
  return "(invalid)";
}

}  // namespace fusb302
