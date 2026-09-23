// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "battery_protocol_server.h"

#include <fidl/fuchsia.power.battery/cpp/natural_types.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fake_battery {

BatteryProtocolServer::BatteryProtocolServer(async_dispatcher_t* dispatcher)
    : dispatcher_((dispatcher)) {}

BatteryProtocolServer::~BatteryProtocolServer() {}

zx_status_t BatteryProtocolServer::Init(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing) {
  fuchsia_power_battery::InfoService::InstanceHandler handler(
      {.device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         fidl::kIgnoreBindingClosure)});
  zx::result result = outgoing->AddService<fuchsia_power_battery::InfoService>(std::move(handler));
  if (result.is_error()) {
    fdf::error("Failed to add InfoService to outgoing directory: {}", result.status_string());
    return result.status_value();
  }

  fdf::info("Added Service");
  return ZX_OK;
}

void BatteryProtocolServer::GetBatteryInfo(GetBatteryInfoCompleter::Sync& completer) {
  fdf::info("Getting BatteryInfo");
  completer.Reply({faked_battery_info_});
}

void BatteryProtocolServer::Watch(WatchRequest& request, WatchCompleter::Sync& completer) {
  fidl::Client<fuchsia_power_battery::BatteryInfoWatcher> watcher;
  watcher.Bind(std::move(request.watcher()), dispatcher_);
  NotifyOnceAsync(std::move(watcher));
}

void BatteryProtocolServer::NotifyOnceAsync(
    fidl::Client<fuchsia_power_battery::BatteryInfoWatcher> watcher) {
  watcher_clients_.push_back(std::move(watcher));

  auto& client = watcher_clients_.back();
  if (!client.is_valid()) {
    return;
  }

  fuchsia_power_battery::BatteryInfoWatcherOnChangeBatteryInfoRequest req;
  req.info(faked_battery_info_);
  client->OnChangeBatteryInfo(std::move(req))
      .Then(
          [](fidl::Result<fuchsia_power_battery::BatteryInfoWatcher::OnChangeBatteryInfo>& result) {
            if (result.is_error()) {
              fdf::error("OnChangeBatteryInfo failed: {}",
                         result.error_value().FormatDescription().c_str());
            } else {
              fdf::info("Successfully sent battery update and received ack.");
            }
          });
}

void BatteryProtocolServer::Serve(
    async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_power_battery::BatteryInfoProvider> server) {
  bindings_.AddBinding(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace fake_battery
