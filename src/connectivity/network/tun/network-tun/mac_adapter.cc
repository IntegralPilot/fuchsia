// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mac_adapter.h"

#include <lib/sync/completion.h>

#include <algorithm>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>

namespace network {
namespace tun {

zx::result<std::unique_ptr<MacAdapter>> MacAdapter::Create(
    MacAdapterParent* parent, fdf::UnownedUnsynchronizedDispatcher dispatcher,
    fuchsia_net::wire::MacAddress mac, bool promisc_only) {
  fbl::AllocChecker ac;
  std::unique_ptr<MacAdapter> adapter(
      new (&ac) MacAdapter(parent, std::move(dispatcher), mac, promisc_only));
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(adapter));
}

void MacAdapter::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  completer.buffer(arena).Reply(mac_);
}

void MacAdapter::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  auto builder = fuchsia_hardware_network_driver::wire::Features::Builder(arena);
  if (promisc_only_) {
    builder.multicast_filter_count(0);
    builder.supported_modes(
        fuchsia_hardware_network_driver::wire::SupportedMacFilterMode::kPromiscuous);
  } else {
    builder.multicast_filter_count(fuchsia_net_tun::wire::kMaxMulticastFilters);
    builder.supported_modes(
        fuchsia_hardware_network_driver::wire::SupportedMacFilterMode::kMulticastPromiscuous |
        fuchsia_hardware_network_driver::wire::SupportedMacFilterMode::kMulticastFilter |
        fuchsia_hardware_network_driver::wire::SupportedMacFilterMode::kPromiscuous);
  }
  completer.buffer(arena).Reply(builder.Build());
}

void MacAdapter::SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
                         fdf::Arena& arena, SetModeCompleter::Sync& completer) {
  {
    fbl::AutoLock lock(&state_lock_);
    fuchsia_hardware_network::wire::MacFilterMode filter_mode = request->mode;
    mac_state_.mode = filter_mode;
    mac_state_.multicast_filters.clear();
    mac_state_.multicast_filters.reserve(request->multicast_macs.size());
    std::ranges::copy(request->multicast_macs, std::back_inserter(mac_state_.multicast_filters));
    parent_->OnMacStateChanged(this);
  }
  completer.buffer(arena).Reply();
}

fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> MacAdapter::BindDriver() {
  ZX_ASSERT(!binding_);
  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::MacAddr>::Create();
  binding_ = fdf::BindServer(dispatcher_->get(), std::move(server), this);
  return std::move(client);
}

MacAdapter::~MacAdapter() {
  if (binding_) {
    binding_->Unbind();
  }
}

MacState MacAdapter::GetMacState() {
  fbl::AutoLock lock(&state_lock_);
  return mac_state_;
}

}  // namespace tun
}  // namespace network
