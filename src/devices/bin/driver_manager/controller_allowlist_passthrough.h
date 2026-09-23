// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_CONTROLLER_ALLOWLIST_PASSTHROUGH_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_CONTROLLER_ALLOWLIST_PASSTHROUGH_H_

#include <fidl/fuchsia.device.fs/cpp/wire.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.driver.framework/cpp/wire.h>

#include <string>
#include <utility>

#include "lib/fidl/cpp/wire/internal/transport.h"

// The ControllerAllowlistPassthrough enables the individual allowlisting of all of the
// functions of the fuchsia.device/Controller protocol.  It also serves as a switching mechanism,
// as the Controller interface is currently either handled by the Node class for DFv2
// drivers, or by the compat::Device class for drivers wrapped in the compatibility shim.
// If the optional |controller_connector| is passed into the Create function, this passthrough
// will forward Controller interface calls to that channel.  Otherwise, it will directly
// call the functions of the |node| class.
class ControllerAllowlistPassthrough : public fidl::WireServer<fuchsia_device::Controller> {
 public:
  // fidl::WireServer<fuchsia_device::Controller>
  void ConnectToDeviceFidl(ConnectToDeviceFidlRequestView request,
                           ConnectToDeviceFidlCompleter::Sync& completer) override;
  void ConnectToController(ConnectToControllerRequestView request,
                           ConnectToControllerCompleter::Sync& completer) override;
  void Bind(BindRequestView request, BindCompleter::Sync& completer) override;
  void Rebind(RebindRequestView request, RebindCompleter::Sync& completer) override;
  void UnbindChildren(UnbindChildrenCompleter::Sync& completer) override;
  void ScheduleUnbind(ScheduleUnbindCompleter::Sync& completer) override;
  void GetTopologicalPath(GetTopologicalPathCompleter::Sync& completer) override;

  static std::unique_ptr<ControllerAllowlistPassthrough> Create(
      std::optional<fidl::ClientEnd<fuchsia_device_fs::Connector>> controller_connector,
      std::weak_ptr<fidl::WireServer<fuchsia_device::Controller>> node,
      async_dispatcher_t* dispatcher, const std::string& class_name);

  zx_status_t Connect(fidl::ServerEnd<fuchsia_device::Controller> server_end);

 private:
  ControllerAllowlistPassthrough(std::weak_ptr<fidl::WireServer<fuchsia_device::Controller>> node,
                                 async_dispatcher_t* dispatcher, const std::string& class_name)
      : node_(std::move(node)), dispatcher_(dispatcher), class_name_(class_name) {}

  void CheckAllowlist(const std::string& function_name);
  std::weak_ptr<fidl::WireServer<fuchsia_device::Controller>> node_;
  async_dispatcher_t* dispatcher_;
  std::string class_name_;
  fidl::WireClient<fuchsia_device::Controller> compat_client_;
  fidl::ServerBindingGroup<fuchsia_device::Controller> dev_controller_bindings_;
};

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_CONTROLLER_ALLOWLIST_PASSTHROUGH_H_
