// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

#include <wlan/drivers/log.h>

#include "wlantap-ctl.h"
#include "wlantap-driver-context.h"

namespace wlan {

// The actual driver class. This main responsibility of this class is to serve the WlantapCtl
// protocol over fuchsia.wlan.tap.Service so that it's discoverable. It also passes on a
// WlantapDriverContext to the spawned WlantapCtl instance so that WlantapCtl can add child nodes
// and serve new protocols.
class WlantapDriver : public fdf::DriverBase2 {
  static constexpr std::string_view kDriverName = "wlantapctl";

 public:
  explicit WlantapDriver() : fdf::DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override {
    WLAN_TRACE_DURATION();
    node_.Bind(take_node());
    driver_context_ = std::make_shared<WlantapDriverContext>(&logger(), outgoing(), &node_);
    fidl::Arena arena;

    fuchsia_wlan_tap::Service::InstanceHandler handler;
    zx::result<> add_service_result =
        handler.add_wlantap_ctl(fit::bind_member<&WlantapDriver::Serve>(this));
    if (add_service_result.is_error()) {
      return add_service_result.take_error();
    }
    add_service_result = outgoing()->AddService<fuchsia_wlan_tap::Service>(std::move(handler));
    if (add_service_result.is_error()) {
      return add_service_result.take_error();
    }

    fuchsia_driver_framework::NodeAddArgs args;
    args.name(std::string(kDriverName));

    zx::result controller_endpoints =
        fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
    ZX_ASSERT(controller_endpoints.is_ok());

    auto result = node_->AddChild({{.args = std::move(args),
                                    .controller = std::move(controller_endpoints->server),
                                    .node = {}}});
    if (result.is_error()) {
      fdf::error("Failed to add child: {}", result.error_value().FormatDescription());
      return zx::error(ZX_ERR_INTERNAL);
    }

    return zx::ok();
  }

 private:
  void Serve(fidl::ServerEnd<fuchsia_wlan_tap::WlantapCtl> server) {
    WLAN_TRACE_DURATION();
    // Give the dispatcher ownership of server_impl
    auto server_impl = std::make_unique<WlantapCtlServer>(*driver_context_);
    fidl::BindServer(dispatcher(), std::move(server), std::move(server_impl));
  }

  // The node client. This lets WlantapDriver and related classes add child nodes, which is the DFv2
  // equivalent of calling device_add().
  fidl::SyncClient<fuchsia_driver_framework::Node> node_;

  std::shared_ptr<WlantapDriverContext> driver_context_;
};

}  // namespace wlan
FUCHSIA_DRIVER_EXPORT2(wlan::WlantapDriver);
