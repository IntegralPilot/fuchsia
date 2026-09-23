// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TEST_REALM_SRC_SYSTEM_STATE_H_
#define LIB_DRIVER_TEST_REALM_SRC_SYSTEM_STATE_H_

#include <fidl/fuchsia.system.state/cpp/wire.h>

namespace driver_test_realm {

class SystemStateTransition final
    : public fidl::WireServer<fuchsia_system_state::SystemStateTransition> {
 public:
  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_system_state::SystemStateTransition> server_end) {
    bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  }

  fidl::ProtocolHandler<fuchsia_system_state::SystemStateTransition> CreateHandler(
      async_dispatcher_t* dispatcher) {
    return bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure);
  }

  void GetTerminationSystemState(GetTerminationSystemStateCompleter::Sync& completer) override {
    completer.Reply(fuchsia_system_state::SystemPowerState::kFullyOn);
  }

  void GetMexecZbis(GetMexecZbisCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  fidl::ServerBindingGroup<fuchsia_system_state::SystemStateTransition> bindings_;
};

}  // namespace driver_test_realm

#endif  // LIB_DRIVER_TEST_REALM_SRC_SYSTEM_STATE_H_
