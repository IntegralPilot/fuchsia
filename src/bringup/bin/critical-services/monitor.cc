// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/critical-services/monitor.h"

#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <zircon/status.h>

namespace pwrbtn {

namespace statecontrol_fidl = fuchsia_hardware_power_statecontrol;

fidl::ProtocolHandler<fuchsia_power_button::Monitor> PowerButtonMonitor::Publish() {
  return bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure);
}

void PowerButtonMonitor::GetAction(GetActionCompleter::Sync& completer) {
  completer.Reply(action_);
}

void PowerButtonMonitor::SetAction(SetActionRequestView view, SetActionCompleter::Sync& completer) {
  action_ = view->action;
  completer.Reply();
}

zx_status_t PowerButtonMonitor::DoAction() {
  switch (action_) {
    case Action::kIgnore:
      return ZX_OK;
    case Action::kShutdown:
      return SendPoweroff();
    default:
      printf("critical-services: unknown action %d\n", fidl::ToUnderlying(action_));
      return ZX_ERR_NOT_SUPPORTED;
  }
}

zx_status_t PowerButtonMonitor::SendButtonEvent(ButtonEvent event) {
  zx_status_t all_status = ZX_OK;
  bindings_.ForEachBinding(
      [event, &all_status](const fidl::ServerBinding<fuchsia_power_button::Monitor>& binding) {
        auto result = fidl::WireSendEvent(binding)->OnButtonEvent(
            fuchsia_power_button::wire::PowerButtonEvent(event));
        if (!result.ok()) {
          printf("critical-services: failed to send button event: %s\n",
                 result.FormatDescription().c_str());
          all_status = result.status();
        }
      });
  return all_status;
}

zx_status_t PowerButtonMonitor::SendPoweroff() {
  auto connect_result = component::Connect<statecontrol_fidl::Admin>();
  if (connect_result.is_error()) {
    printf("critical-services: Failed to connect to statecontrol service: %s\n",
           zx_status_get_string(connect_result.status_value()));
    return connect_result.status_value();
  }
  fidl::WireSyncClient admin_client{std::move(connect_result.value())};

  fidl::Arena arena;
  auto builder = statecontrol_fidl::wire::ShutdownOptions::Builder(arena);
  statecontrol_fidl::ShutdownReason reasons[] = {statecontrol_fidl::ShutdownReason::kUserRequest};
  auto vector_view = fidl::VectorView<statecontrol_fidl::ShutdownReason>::FromExternal(reasons);
  builder.reasons(vector_view);
  builder.action(statecontrol_fidl::ShutdownAction::kPoweroff);

  auto resp = admin_client->Shutdown(builder.Build());

  // Check if there was any transport error, note that we don't actually wait
  // to see if the request is successful. In the success case the reboot call
  // blocks until the device turns off. In the failure case we'll get a error,
  // but there's nothing we can really do with it so instead we return so we
  // can listne for more key events.
  if (!resp.ok()) {
    printf("critical-services: Call to statecontrol failed: %s\n",
           resp.FormatDescription().c_str());
    return resp.status();
  }

  return ZX_OK;
}

}  // namespace pwrbtn
