// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_TESTING_FAKE_SUSPEND_DEVICE_SERVER_H_
#define SRC_POWER_TESTING_FAKE_SUSPEND_DEVICE_SERVER_H_

#include <fidl/fuchsia.hardware.power.suspend/cpp/fidl.h>
#include <fidl/test.suspendcontrol/cpp/fidl.h>
#include <fidl/test.suspendcontrol/cpp/natural_types.h>

#include <memory>
#include <optional>
#include <vector>

namespace fake_suspend {

// Protocol served to client components over devfs.
class DeviceServer : public fidl::Server<fuchsia_hardware_power_suspend::Suspender>,
                     public fidl::Server<test_suspendcontrol::Device> {
 public:
  // fidl::Server<fuchsia_hardware_power_suspend::Suspender> overrides.
  void GetSuspendStates(GetSuspendStatesCompleter::Sync& completer) override;
  void Suspend(SuspendRequest& request, SuspendCompleter::Sync& completer) override;
  void ForceLowestPowerMode(ForceLowestPowerModeRequest& request,
                            ForceLowestPowerModeCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_power_suspend::Suspender> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  // fidl::Server<test_suspendcontrol::Device> overrides.
  void SetSuspendStates(SetSuspendStatesRequest& request,
                        SetSuspendStatesCompleter::Sync& completer) override;
  void AwaitSuspend(AwaitSuspendCompleter::Sync& completer) override;
  void Resume(ResumeRequest& request, ResumeCompleter::Sync& completer) override;

  // Serve methods
  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_hardware_power_suspend::Suspender> server);
  void Serve(async_dispatcher_t* dispatcher, fidl::ServerEnd<test_suspendcontrol::Device> server);

 private:
  void SendGetSuspendStatesResponses();

  fidl::ServerBindingGroup<test_suspendcontrol::Device> device_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_power_suspend::Suspender> suspender_bindings_;

  std::optional<std::vector<fuchsia_hardware_power_suspend::SuspendState>> suspend_states_;
  std::vector<GetSuspendStatesCompleter::Async> get_suspend_states_completers_;

  std::optional<uint64_t> last_state_index_;
  std::optional<SuspendCompleter::Async> suspend_completer_;
  std::optional<AwaitSuspendCompleter::Async> await_suspend_completer_;
};

}  // namespace fake_suspend

#endif  // SRC_POWER_TESTING_FAKE_SUSPEND_DEVICE_SERVER_H_
