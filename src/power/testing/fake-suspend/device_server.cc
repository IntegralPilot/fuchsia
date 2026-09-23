// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/power/testing/fake-suspend/device_server.h"

#include <optional>
#include <utility>

namespace fake_suspend {

using fuchsia_hardware_power_suspend::SuspenderGetSuspendStatesResponse;
using fuchsia_hardware_power_suspend::SuspenderSuspendResponse;
using test_suspendcontrol::DeviceAwaitSuspendResponse;
using test_suspendcontrol::DeviceResumeRequest;

// fuchsia.hardware.power.suspend/Suspender.*

void DeviceServer::GetSuspendStates(GetSuspendStatesCompleter::Sync& completer) {
  get_suspend_states_completers_.push_back(completer.ToAsync());
  if (suspend_states_.has_value()) {
    SendGetSuspendStatesResponses();
  }
}

void DeviceServer::Suspend(SuspendRequest& request, SuspendCompleter::Sync& completer) {
  if (!suspend_states_.has_value()) {
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  if (request.state_index() >= suspend_states_->size()) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  last_state_index_ = request.state_index();
  suspend_completer_ = completer.ToAsync();

  if (await_suspend_completer_) {
    await_suspend_completer_->Reply(
        zx::ok(DeviceAwaitSuspendResponse().state_index(last_state_index_)));
    await_suspend_completer_.reset();
  }
}

void DeviceServer::ForceLowestPowerMode(ForceLowestPowerModeRequest& request,
                                        ForceLowestPowerModeCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

// test.suspendcontrol/Device.*

void DeviceServer::SetSuspendStates(SetSuspendStatesRequest& request,
                                    SetSuspendStatesCompleter::Sync& completer) {
  suspend_states_ = std::move(request.suspend_states().value());
  SendGetSuspendStatesResponses();
  completer.Reply(zx::ok());
}

void DeviceServer::AwaitSuspend(AwaitSuspendCompleter::Sync& completer) {
  if (suspend_completer_.has_value()) {
    completer.Reply(zx::ok(DeviceAwaitSuspendResponse().state_index(last_state_index_)));
    return;
  }

  await_suspend_completer_ = completer.ToAsync();
}

void DeviceServer::Resume(ResumeRequest& request, ResumeCompleter::Sync& completer) {
  if (!suspend_completer_.has_value()) {
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  if (request.Which() == DeviceResumeRequest::Tag::kResult) {
    suspend_completer_->Reply(zx::ok(SuspenderSuspendResponse()
                                         .reason(request.result()->reason())
                                         .suspend_duration(request.result()->suspend_duration())
                                         .suspend_overhead(request.result()->suspend_overhead())));
  } else {
    suspend_completer_->Reply(zx::error(request.error().value()));
  }

  suspend_completer_.reset();
  completer.Reply(zx::ok());
}

// Server methods

void DeviceServer::Serve(async_dispatcher_t* dispatcher,
                         fidl::ServerEnd<fuchsia_hardware_power_suspend::Suspender> server) {
  suspender_bindings_.AddBinding(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
}

void DeviceServer::Serve(async_dispatcher_t* dispatcher,
                         fidl::ServerEnd<test_suspendcontrol::Device> server) {
  device_bindings_.AddBinding(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
}

// private methods

void DeviceServer::SendGetSuspendStatesResponses() {
  ZX_ASSERT(suspend_states_.has_value());

  const std::vector<fuchsia_hardware_power_suspend::SuspendState>& states = suspend_states_.value();
  // NB: |std::vector|'s move constructor guarantees that |get_suspend_states_completers_|
  // be empty, but |std::vector|'s move assignment only guarantees that will be in a
  // valid but undefined state.
  std::vector<GetSuspendStatesCompleter::Async> completers(
      std::move(get_suspend_states_completers_));

  for (auto& completer : completers) {
    completer.Reply(zx::ok(SuspenderGetSuspendStatesResponse().suspend_states(states)));
  }
}

}  // namespace fake_suspend
