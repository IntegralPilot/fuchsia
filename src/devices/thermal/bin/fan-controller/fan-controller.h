// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_THERMAL_BIN_FAN_CONTROLLER_FAN_CONTROLLER_H_
#define SRC_DEVICES_THERMAL_BIN_FAN_CONTROLLER_FAN_CONTROLLER_H_

#include <fidl/fuchsia.hardware.fan/cpp/fidl.h>
#include <fidl/fuchsia.thermal/cpp/fidl.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>

#include <list>

namespace fan_controller {

class FanController {
 public:
  explicit FanController(async_dispatcher_t* dispatcher,
                         fidl::ClientEnd<fuchsia_thermal::ClientStateConnector> connector,
                         fidl::ClientEnd<fuchsia_io::Directory> svc_root)
      : dispatcher_(dispatcher),
        connector_(std::move(connector)),
        svc_root_(std::move(svc_root)),
        watcher_(svc_root_.borrow()) {
    ZX_ASSERT(watcher_.Begin(dispatcher, fit::bind_member<&FanController::NewFan>(this)).is_ok());
  }

#ifdef FAN_CONTROLLER_TEST
  size_t controller_fan_count(const std::string& client_type) {
    return controllers_.find(client_type) == controllers_.end()
               ? 0
               : controllers_.at(client_type).fans_.size();
  }
#endif

 private:
  void NewFan(fidl::ClientEnd<fuchsia_hardware_fan::Device> client_end);
  zx::result<fidl::ClientEnd<fuchsia_thermal::ClientStateWatcher>> ConnectToWatcher(
      const std::string& client_type);

  async_dispatcher_t* dispatcher_;
  fidl::SyncClient<fuchsia_thermal::ClientStateConnector> connector_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;
  component::ServiceMemberWatcher<fuchsia_hardware_fan::Service::Device> watcher_;

  struct ControllerInstance {
    fidl::Client<fuchsia_thermal::ClientStateWatcher> watcher_;
    std::list<fidl::SyncClient<fuchsia_hardware_fan::Device>> fans_;

    void WatchCallback(fidl::Result<fuchsia_thermal::ClientStateWatcher::Watch>& result);
  };
  std::map<std::string, ControllerInstance> controllers_;
};

}  // namespace fan_controller

#endif  // SRC_DEVICES_THERMAL_BIN_FAN_CONTROLLER_FAN_CONTROLLER_H_
