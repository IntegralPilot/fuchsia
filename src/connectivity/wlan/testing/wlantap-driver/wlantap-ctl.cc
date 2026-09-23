// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "wlantap-ctl.h"

#include <fidl/fuchsia.wlan.tap/cpp/fidl.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <wlan/drivers/log.h>

#include "wlantap-phy.h"

namespace wlan {

void WlantapCtlServer::CreatePhy(CreatePhyRequest& request, CreatePhyCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();

  const auto phy_config = request.config();
  auto instance_name = phy_config.name();

  auto endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create endpoints: {}", endpoints.status_string());
    completer.Reply(endpoints.error_value());
    return;
  }

  auto impl = WlanPhyDevice::New(driver_context_, request.proxy().TakeChannel(), phy_config,
                                 std::move(endpoints->client));

  zx_status_t status = ServeWlanPhyProtocol(instance_name, std::move(impl));
  if (status != ZX_OK) {
    fdf::error("ServeWlanPhyProtocol failed: {}", zx_status_get_string(status));
    completer.Reply(status);
    return;
  }

  status = AddWlanPhyChild(instance_name, std::move(endpoints->server));
  if (status != ZX_OK) {
    fdf::error("AddWlanPhyChild failed: {}", zx_status_get_string(status));
    completer.Reply(status);
    return;
  }

  completer.Reply(ZX_OK);
}

zx_status_t WlantapCtlServer::AddWlanPhyChild(
    std::string_view name, fidl::ServerEnd<fuchsia_driver_framework::NodeController> server) {
  WLAN_TRACE_DURATION();
  fidl::Arena arena;

  auto offers = std::vector{fdf::MakeOffer2<fuchsia_wlan_phy::Service>(std::string(name))};
  fuchsia_driver_framework::NodeAddArgs args;
  args.name(std::string(name)).offers2(std::move(offers));

  auto res = driver_context_.node_client()->AddChild(
      {{.args = std::move(args), .controller = std::move(server)}});
  if (res.is_error()) {
    fdf::error("Failed to add WlanPhy child: {}", res.error_value().FormatDescription());
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

zx_status_t WlantapCtlServer::ServeWlanPhyProtocol(std::string_view name,
                                                   std::shared_ptr<WlanPhyDevice> impl) {
  WLAN_TRACE_DURATION();
  auto protocol_handler =
      [impl = std::move(impl)](fidl::ServerEnd<fuchsia_wlan_phy::WlanPhy> request) mutable {
        fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(request),
                         std::move(impl));
      };

  fuchsia_wlan_phy::Service::InstanceHandler handler({.device = std::move(protocol_handler)});

  zx::result result =
      driver_context_.outgoing()->AddService<fuchsia_wlan_phy::Service>(std::move(handler), name);

  if (result.is_error()) {
    fdf::error("Failed to add service: {}", result);
    return result.error_value();
  }

  return ZX_OK;
}

}  // namespace wlan
