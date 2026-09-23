// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "port_adapter.h"

#include <utility>

#include <fbl/auto_lock.h>

namespace network {
namespace tun {

void PortAdapter::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  fuchsia_hardware_network::wire::PortBaseInfo info =
      fuchsia_hardware_network::wire::PortBaseInfo::Builder(arena)
          .port_class(config_.port_class)
          .rx_types(config_.rx_types)
          .tx_types(config_.tx_types)
          .Build();
  completer.buffer(arena).Reply(info);
}

void PortAdapter::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  fbl::AutoLock lock(&state_lock_);
  auto wire = fuchsia_hardware_network::wire::PortStatus::Builder(arena);
  port_status_.AddToBuilder(wire);
  completer.buffer(arena).Reply(wire.Build());
}

void PortAdapter::SetActive(
    fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request, fdf::Arena& arena,
    SetActiveCompleter::Sync& completer_) {
  fbl::AutoLock lock(&state_lock_);
  bool active = request->active;
  if (active != has_sessions_) {
    has_sessions_ = active;
    lock.release();
    parent_->OnHasSessionsChanged(*this);
  }
}

void PortAdapter::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  if (!mac_) {
    completer.buffer(arena).Reply(
        fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr>(fdf::Channel()));
    return;
  }
  completer.buffer(arena).Reply(mac_->BindDriver());
}

void PortAdapter::Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) {
  parent_->OnPortDestroyed(*this);
}

fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkPort> PortAdapter::BindDriver() {
  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::NetworkPort>::Create();
  fdf::BindServer(dispatcher_->get(), std::move(server), this);
  return std::move(client);
}

PortAdapter::PortAdapter(PortAdapterParent* parent, const BasePortConfig& config,
                         std::unique_ptr<MacAdapter> mac,
                         fdf::UnownedUnsynchronizedDispatcher dispatcher)
    : parent_(parent),
      dispatcher_(std::move(dispatcher)),
      mac_(std::move(mac)),
      config_(config),
      port_status_({
          .online = false,
          .mtu = config.mtu,
      }) {}

bool PortAdapter::SetOnline(bool online) {
  fbl::AutoLock lock(&state_lock_);
  if (online == port_status_.online) {
    return false;
  }
  port_status_.online = online;
  parent_->OnPortStatusChanged(*this, port_status_);
  return true;
}

bool PortAdapter::online() {
  fbl::AutoLock lock(&state_lock_);
  return port_status_.online;
}

bool PortAdapter::has_sessions() {
  fbl::AutoLock lock(&state_lock_);
  return has_sessions_;
}

}  // namespace tun
}  // namespace network
