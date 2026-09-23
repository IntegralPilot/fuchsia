// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mac_interface.h"

#include <lib/async/cpp/task.h>

#include <fbl/alloc_checker.h>

#include "log.h"

namespace network {

void MacAddrDeviceInterface::Create(fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> parent,
                                    fdf_dispatcher_t* dispatcher, OnCreated&& on_created) {
  internal::MacInterface::Create(std::move(parent), dispatcher, std::move(on_created));
}

namespace internal {

constexpr uint8_t kMacMulticast = 0x01;

MacInterface::~MacInterface() {
  ZX_ASSERT_MSG(clients_.is_empty(),
                "can't dispose MacInterface while clients are still attached (%ld clients left).",
                clients_.size_slow());
  ZX_ASSERT_MSG(dead_clients_.is_empty(),
                "can't dispose MacInterface while clients are still closing (%ld clients left).",
                dead_clients_.size_slow());
}

void MacInterface::Create(fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> parent,
                          fdf_dispatcher_t* dispatcher, OnCreated&& on_created) {
  fbl::AllocChecker ac;
  std::shared_ptr<MacInterface> mac(new (&ac) MacInterface());
  if (!ac.check()) {
    LOGF_ERROR("Could not allocate MacInterface");
    on_created(zx::error(ZX_ERR_NO_MEMORY));
    return;
  }
  // Keep a raw pointer for making the call below, the unique ptr will have been moved.
  MacInterface* mac_ptr = mac.get();
  mac_ptr->impl_.Bind(std::move(parent), dispatcher,
                      fidl::ObserveTeardown([mac_ptr]() { mac_ptr->OnImplTeardown(); }));
  mac_ptr->Init(
      [mac = std::move(mac), on_created = std::move(on_created)](zx_status_t status) mutable {
        if (status != ZX_OK) {
          MacInterface* mac_ptr = mac.get();
          mac_ptr->Teardown([mac = std::move(mac), on_created = std::move(on_created),
                             status]() mutable { on_created(zx::error(status)); });
          return;
        }
        on_created(zx::ok(std::move(mac)));
      });
}

void MacInterface::OnImplTeardown() {
  lock_.Acquire();
  impl_torn_down_ = true;
  // The parent binding is torn down so the dead clients' consolidation
  // operations may never finish. Clear them out and check if we can finish
  // tearing down.
  dead_clients_.clear();
  MaybeFinishTeardown();
}

void MacInterface::MaybeFinishTeardown() {
  if (clients_.is_empty() && dead_clients_.is_empty() && impl_torn_down_ && teardown_callback_) {
    fit::callback<void()> teardown = std::move(teardown_callback_);
    lock_.Release();
    teardown();
  } else {
    lock_.Release();
  }
}

void MacInterface::Init(fit::callback<void(zx_status_t)>&& on_complete) {
  GetFeatures([this, on_complete = std::move(on_complete)](zx_status_t status) mutable {
    if (status != ZX_OK) {
      on_complete(status);
      return;
    }
    SetDefaultMode([on_complete = std::move(on_complete)](zx_status_t status) mutable {
      on_complete(status);
    });
  });
}

zx_status_t MacInterface::Bind(async_dispatcher_t* dispatcher,
                               fidl::ServerEnd<netdev::MacAddressing> req) {
  fbl::AutoLock lock(&lock_);
  if (teardown_callback_ || impl_torn_down_) {
    // Don't allow new bindings if we're tearing down.
    return ZX_ERR_BAD_STATE;
  }
  fbl::AllocChecker ac;
  std::unique_ptr<MacClientInstance> client_instance(new (&ac)
                                                         MacClientInstance(this, default_mode_));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  zx_status_t status = client_instance->Bind(dispatcher, std::move(req));
  if (status != ZX_OK) {
    return status;
  }

  clients_.push_back(std::move(client_instance));
  // TODO(https://fxbug.dev/42051219): Improve communication with parent driver.
  // MacInterface relies heavily on synchronous communication which can be
  // problematic and cause lock inversions with the parent driver. We need a
  // better strategy here that is going to be more compatible with DFv2. For
  // now, dispatching to do the work eliminates known deadlocks.
  async::PostTask(dispatcher, [weak_this = weak_from_this()] {
    if (std::shared_ptr<MacInterface> mac_interface = weak_this.lock()) {
      mac_interface->lock_.Acquire();
      mac_interface->Consolidate([](zx_status_t /*unused*/) {});
    }
  });
  return ZX_OK;
}

std::optional<netdev::wire::MacFilterMode> MacInterface::ConvertMode(
    const netdev::wire::MacFilterMode& mode) const {
  using fuchsia_hardware_network_driver::wire::SupportedMacFilterMode;
  using netdev::wire::MacFilterMode;

  if (!features_.supported_modes().has_value()) {
    return std::nullopt;
  }

  const SupportedMacFilterMode supported_modes = features_.supported_modes().value();

  switch (mode) {
    case MacFilterMode::kMulticastFilter:
      if (supported_modes & SupportedMacFilterMode::kMulticastFilter) {
        return MacFilterMode::kMulticastFilter;
      }
      // Multicast filter not supported, attempt to fall back on multicast promiscuous.
      [[fallthrough]];
    case MacFilterMode::kMulticastPromiscuous:
      if (supported_modes & SupportedMacFilterMode::kMulticastPromiscuous) {
        return MacFilterMode::kMulticastPromiscuous;
      }
      // Multicast promiscuous not supported, attempt to fall back on promiscuous.
      [[fallthrough]];
    case MacFilterMode::kPromiscuous:
      if (supported_modes & SupportedMacFilterMode::kPromiscuous) {
        return MacFilterMode::kPromiscuous;
      }
      // Promiscuous not supported, fail.
      [[fallthrough]];
    default:
      return std::nullopt;
  }
}

void MacInterface::Consolidate(fit::function<void(zx_status_t)> callback) {
  if (!impl_.is_valid() || impl_torn_down_) {
    // Avoid deadlocking as a result of the callback attempting to acquire the
    // lock.
    lock_.Release();
    callback(ZX_ERR_BAD_STATE);
    return;
  }
  netdev::wire::MacFilterMode mode = default_mode_;
  // Gather the most permissive mode that the clients want.
  for (auto& c : clients_) {
    if (static_cast<uint32_t>(c.state().filter_mode) > static_cast<uint32_t>(mode)) {
      mode = c.state().filter_mode;
    }
  }

  std::vector<MacAddress> addr_buff;
  // If selected mode is multicast filter, then collect all the unique addresses.
  if (mode == netdev::wire::MacFilterMode::kMulticastFilter) {
    uint32_t multicast_filter_count = features_.multicast_filter_count().value_or(0);
    std::unordered_set<ClientState::Addr, ClientState::MacHasher> addresses;
    for (const auto& c : clients_) {
      const auto& client_addresses = c.state().addresses;
      addresses.insert(client_addresses.begin(), client_addresses.end());
      if (addresses.size() > multicast_filter_count) {
        // Try to go into multicast_promiscuous mode, if it's supported.
        auto try_mode = ConvertMode(netdev::wire::MacFilterMode::kMulticastPromiscuous);
        // If it's not supported (meaning that neither multicast promiscuous nor promiscuous is
        // supported, since ConvertMode will fall back to the more permissive mode), we have no
        // option but to truncate the address list.
        if (try_mode.has_value()) {
          mode = try_mode.value();
        } else {
          // If neither are supported we have no option but to truncate the address list.
          LOGF_WARN(
              "MAC filter list is full, but more permissive modes are not supported. Multicast MAC "
              "filter list is being truncated to %d entries",
              multicast_filter_count);
        }
        break;
      }
    }
    // If the mode didn't change out of multicast filter, build the multicast list.
    if (mode == fuchsia_hardware_network::wire::MacFilterMode::kMulticastFilter) {
      for (const auto& address : addresses) {
        if (addr_buff.size() >= fuchsia_hardware_network_driver::wire::kMaxMacFilter ||
            addr_buff.size() >= multicast_filter_count) {
          break;
        }
        addr_buff.push_back(address.address);
      }
    }
  }

  fdf::Arena fdf_arena('NMAC');
  impl_.buffer(fdf_arena)
      ->SetMode(mode, fidl::VectorView<MacAddress>::FromExternal(addr_buff))
      .Then([callback = std::move(callback)](
                fdf::WireUnownedResult<fuchsia_hardware_network_driver::MacAddr::SetMode>& result) {
        if (!result.ok()) {
          LOGF_ERROR("SetMode() failed: %s", result.error().FormatDescription().c_str());
          callback(ZX_ERR_INTERNAL);
          return;
        }
        callback(ZX_OK);
      });
  lock_.Release();
}

void MacInterface::CloseClient(MacClientInstance* client) {
  lock_.Acquire();
  std::unique_ptr<MacClientInstance> client_ptr;
  if (client->InContainer()) {
    client_ptr = clients_.erase(*client);
  }
  if (impl_torn_down_ || !impl_.is_valid()) {
    // If the parent is dead, skip consolidation and check if we're ready to
    // complete teardown.
    MaybeFinishTeardown();
    return;
  }

  if (client_ptr) {
    // Keep the client alive until consolidation completes. Otherwise another
    // client closure could observe an empty list of clients and call the
    // teardown callback before this consolidation has completed. The client
    // cannot be kept in clients_ as it must not take part in consolidation now
    // that it's closed.
    dead_clients_.push_back(std::move(client_ptr));
  }
  Consolidate([client, this](zx_status_t /*status*/) {
    lock_.Acquire();
    if (client->InContainer()) {
      dead_clients_.erase(*client);
    }
    MaybeFinishTeardown();
  });
}

void MacInterface::GetFeatures(fit::callback<void(zx_status_t)>&& on_complete) {
  fdf::Arena arena('NMAC');
  impl_.buffer(arena)->GetFeatures().Then(
      [this, on_complete = std::move(on_complete)](
          fdf::WireUnownedResult<netdriver::MacAddr::GetFeatures>& result) mutable {
        if (!result.ok()) {
          on_complete(result.status());
          return;
        }

        features_ = fidl::ToNatural(result.value().features);
        on_complete(ZX_OK);
      });
}

void MacInterface::SetDefaultMode(fit::callback<void(zx_status_t)>&& on_complete) {
  const netdriver::SupportedMacFilterMode supported_modes =
      features_.supported_modes().value_or(static_cast<netdriver::SupportedMacFilterMode>(0));
  if (supported_modes & ~netdriver::SupportedMacFilterMode::kMask) {
    LOGF_ERROR("mac-addr-device:Init: Invalid supported modes bitmask: %08X",
               static_cast<uint32_t>(supported_modes));
    on_complete(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  netdev::wire::MacFilterMode mode;
  if (supported_modes & netdriver::SupportedMacFilterMode::kMulticastFilter) {
    mode = netdev::wire::MacFilterMode::kMulticastFilter;
  } else if (supported_modes & netdriver::SupportedMacFilterMode::kMulticastPromiscuous) {
    mode = netdev::wire::MacFilterMode::kMulticastPromiscuous;
  } else if (supported_modes & netdriver::wire::SupportedMacFilterMode::kPromiscuous) {
    mode = netdev::wire::MacFilterMode::kPromiscuous;
  } else {
    // No supported modes.
    LOGF_ERROR("mac-addr-device:Init: Invalid supported modes bitmask: %08X",
               static_cast<uint32_t>(supported_modes));
    on_complete(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  fidl::VectorView<::fuchsia_net::wire::MacAddress> macs(nullptr, 0);
  fdf::Arena arena('NMAC');
  // Set the default mode to the parent on initialization.
  impl_.buffer(arena)
      ->SetMode(mode, macs)
      .Then([this, mode, on_complete = std::move(on_complete)](
                fdf::WireUnownedResult<netdriver::MacAddr::SetMode>& result) mutable {
        if (!result.ok()) {
          LOGF_ERROR("SetMode() failed: %s", result.error().FormatDescription().c_str());
          on_complete(ZX_ERR_INTERNAL);
          return;
        }
        default_mode_ = mode;
        on_complete(ZX_OK);
      });
}

void MacInterface::Teardown(fit::callback<void()> callback) {
  lock_.Acquire();
  // Can't call teardown if already tearing down.
  ZX_ASSERT(!teardown_callback_);
  teardown_callback_ = std::move(callback);
  for (auto& client : clients_) {
    client.Unbind();
  }
  impl_.AsyncTeardown();
  MaybeFinishTeardown();
}

void MacClientInstance::GetUnicastAddress(GetUnicastAddressCompleter::Sync& completer) {
  fdf::Arena arena('NMAC');
  parent_->impl_.buffer(arena)->GetAddress().Then(
      [completer = completer.ToAsync()](
          fdf::WireUnownedResult<fuchsia_hardware_network_driver::MacAddr::GetAddress>&
              result) mutable {
        if (!result.ok()) {
          LOGF_ERROR("GetAddress() failed: %s", result.error().FormatDescription().c_str());
          completer.Close(result.status());
          return;
        }
        completer.Reply(result->mac);
      });
}

void MacClientInstance::SetMode(SetModeRequestView request, SetModeCompleter::Sync& completer) {
  auto resolved_mode = parent_->ConvertMode(request->mode);
  if (resolved_mode.has_value()) {
    parent_->lock_.Acquire();
    state_.filter_mode = resolved_mode.value();
    parent_->Consolidate(
        [completer = completer.ToAsync()](zx_status_t status) mutable { completer.Reply(status); });
  } else {
    completer.Reply(ZX_ERR_NOT_SUPPORTED);
  }
}

void MacClientInstance::AddMulticastAddress(AddMulticastAddressRequestView request,
                                            AddMulticastAddressCompleter::Sync& completer) {
  if ((request->address.octets[0] & kMacMulticast) == 0) {
    completer.Reply(ZX_ERR_INVALID_ARGS);
  } else {
    parent_->lock_.Acquire();
    if (state_.addresses.size() >= netdriver::wire::kMaxMacFilter) {
      parent_->lock_.Release();
      completer.Reply(ZX_ERR_NO_RESOURCES);
      return;
    }
    state_.addresses.insert(ClientState::Addr{request->address});
    parent_->Consolidate(
        [completer = completer.ToAsync()](zx_status_t status) mutable { completer.Reply(status); });
  }
}

void MacClientInstance::RemoveMulticastAddress(RemoveMulticastAddressRequestView request,
                                               RemoveMulticastAddressCompleter::Sync& completer) {
  if ((request->address.octets[0] & kMacMulticast) == 0) {
    completer.Reply(ZX_ERR_INVALID_ARGS);
  } else {
    parent_->lock_.Acquire();
    state_.addresses.erase(ClientState::Addr{request->address});
    parent_->Consolidate(
        [completer = completer.ToAsync()](zx_status_t status) mutable { completer.Reply(status); });
  }
}

MacClientInstance::MacClientInstance(MacInterface* parent, netdev::wire::MacFilterMode default_mode)
    : parent_(parent), state_(default_mode) {}

zx_status_t MacClientInstance::Bind(async_dispatcher_t* dispatcher,
                                    fidl::ServerEnd<netdev::MacAddressing> req) {
  binding_ =
      fidl::BindServer(dispatcher, std::move(req), this,
                       [](MacClientInstance* client_instance, fidl::UnbindInfo /*unused*/,
                          fidl::ServerEnd<fuchsia_hardware_network::MacAddressing> /*unused*/) {
                         client_instance->parent_->CloseClient(client_instance);
                       });
  return ZX_OK;
}

void MacClientInstance::Unbind() {
  if (binding_.has_value()) {
    binding_->Unbind();
    binding_.reset();
  }
}

}  // namespace internal
}  // namespace network
