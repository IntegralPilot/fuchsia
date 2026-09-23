// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/connectivity/wlan/drivers/lib/components/cpp/include/wlan/drivers/components/network_port.h"

#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>

namespace wlan::drivers::components {

NetworkPort::Callbacks::~Callbacks() = default;

NetworkPort::NetworkPort(
    fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>&& netdev_ifc,
    Callbacks& iface, uint8_t port_id)
    : iface_(iface), netdev_ifc_(std::move(netdev_ifc)), port_id_(port_id) {}

NetworkPort::~NetworkPort() {
  ZX_ASSERT_MSG(!port_binding_.has_value(), "Port server still bound, remove port first.");
  ZX_ASSERT_MSG(!mac_binding_.has_value(), "Mac server still bound, remove port first.");
  ZX_ASSERT_MSG(!netdev_ifc_.is_valid(), "NetDevIfc still bound, remove port first.");
  ZX_ASSERT_MSG(dispatcher_ == nullptr, "Dispatcher still valid, remove port first.");
  ZX_ASSERT_MSG(!on_removed_, "RemovePort callback still valid, wait for port removal first.");
}

void NetworkPort::Init(Role role, fdf_dispatcher_t* dispatcher,
                       fit::callback<void(zx_status_t)>&& on_complete) {
  const zx_status_t status = [&]() {
    std::lock_guard lock(mutex_);
    role_ = role;
    if (!netdev_ifc_.is_valid()) {
      fdf::warn("netdev_ifc_ invalid, port likely removed.");
      return ZX_ERR_BAD_STATE;
    }
    if (dispatcher_ != nullptr) {
      fdf::warn("NetworkPort already initialized");
      return ZX_ERR_ALREADY_BOUND;
    }
    fdf::Arena arena('NETD');
    zx::result endpoints = fdf::CreateEndpoints<fuchsia_hardware_network_driver::NetworkPort>();
    if (endpoints.is_error()) {
      fdf::error("Failed to create NetworkPort endpoints: {}", endpoints.status_string());
      return endpoints.status_value();
    }

    dispatcher_ = dispatcher;

    port_binding_ = fdf::BindServer(
        dispatcher_, std::move(endpoints->server), this,
        [this](NetworkPort*, fidl::UnbindInfo info,
               fdf::ServerEnd<fuchsia_hardware_network_driver::NetworkPort>) {
          // RemovePortLocked will unlock the mutex.
          mutex_.lock();
          if (!info.is_user_initiated()) {
            // on_removed_ means removal started so a peer closed is highly likely.
            // Logging a warning at that point adds no value.
            if (!on_removed_ || info.status() != ZX_ERR_PEER_CLOSED) {
              fdf::warn("Port binding unbound unexpectedly: {}", info.FormatDescription());
            }
          }
          port_binding_.reset();
          RemovePortLocked(nullptr);
        });

    netdev_ifc_.buffer(arena)
        ->AddPort(port_id_, std::move(endpoints->client))
        .Then(
            // We capture |dispatcher| in case |RemovePort| was called in the meanwhile and
            // |dispatcher_| was cleared.
            [this, on_complete = std::move(on_complete), dispatcher](
                fdf::WireUnownedResult<fuchsia_hardware_network_driver::NetworkDeviceIfc::AddPort>&
                    result) mutable {
              const zx_status_t status = [&] {
                if (!result.ok()) {
                  fdf::error("Failed to add port: {}", result.FormatDescription());
                  return result.status();
                }
                if (result->status != ZX_OK) {
                  fdf::error("Failed to add port: {}", zx_status_get_string(result->status));
                  return result->status;
                }
                return ZX_OK;
              }();
              // Since the |AddPort| call from the port to the netdev_ifc may have been inlined
              // by the driver runtime (called directly in the same stack), we may deadlock
              // if trying to acquire the port's lock again. Post a task instead to avoid this.
              async::PostTask(
                  fdf_dispatcher_get_async_dispatcher(dispatcher),
                  [this, status = status, on_complete = std::move(on_complete)]() mutable {
                    if (status != ZX_OK) {
                      // If adding the port failed, go through the entire removal process. There
                      // won't be a port to remove but there are still bindings to unbind and state
                      // to clean up. Call the on_complete callback once the removal is complete to
                      // indicate that the port object is ready to be destroyed. Clear out
                      // netdev_ifc_ so that RemovePort doesn't attempt to remove the port, it was
                      // never added. RemovePortLocked will unlock the mutex.
                      mutex_.lock();
                      netdev_ifc_ = {};
                      RemovePortLocked([on_complete = std::move(on_complete),
                                        status](zx_status_t) mutable { on_complete(status); });
                      return;
                    }
                    on_complete(status);
                  });
            });
    return ZX_OK;
  }();
  if (status != ZX_OK) {
    // Something went wrong, call the complete callback immediately.
    on_complete(status);
    return;
  }
  // At this point the initialization is asynchronous and the callback will be called later.
}

void NetworkPort::RemovePort(fit::callback<void(zx_status_t)>&& on_complete) {
  // RemovePortLocked will unlock the mutex.
  mutex_.lock();
  RemovePortLocked(std::move(on_complete));
}

void NetworkPort::RemovePortLocked(fit::callback<void(zx_status_t)>&& on_complete) {
  auto post_task = [](fdf_dispatcher_t* dispatcher, fit::callback<void(zx_status_t)>&& on_complete,
                      zx_status_t status) {
    async::PostTask(
        fdf_dispatcher_get_async_dispatcher(dispatcher),
        [on_complete = std::move(on_complete), status]() mutable { on_complete(status); });
  };

  // Run this as a lambda that returns a call to make so that we can ensure proper unlocking of the
  // mutex for every path. The lambda should return an empty callback if no call should be made.
  // This happens when an asynchronous removal flow has been initiated.
  auto complete = [&]() __TA_REQUIRES(mutex_) -> fit::callback<void()> {
    if (dispatcher_ == nullptr) {
      // The port was never initialized or already removed, there's nothing to do. Clear out
      // netdev_ifc_ to indicate that the port is ready for destruction.
      netdev_ifc_ = {};
      // Make sure the mutex isn't locked during the callback.
      if (on_complete) {
        // Since there is no dispatcher to post this on it has to be done in-line.
        return [&] { on_complete(ZX_ERR_NOT_FOUND); };
      }
      return nullptr;
    }

    if (on_complete) {
      if (on_removed_) {
        // This was a user request (as indicated by the presence of on_complete) and port removal is
        // already in progress. Fail this request.
        return [&] { post_task(dispatcher_, std::move(on_complete), ZX_ERR_ALREADY_EXISTS); };
      }
      on_removed_ = std::move(on_complete);
    }

    if (netdev_ifc_.is_valid()) {
      fdf::Arena arena('NETD');
      auto status = netdev_ifc_.buffer(arena)->RemovePort(port_id_);
      if (status.ok()) {
        // At this point the port removal process will take care of the rest. It will eventually
        // cause the port server to unbind which will trigger another call to RemovePort which will
        // continue the removal.
        netdev_ifc_ = {};
        return nullptr;
      }
      // Port removal failed, assume that this is because netdev_ifc_ has somehow become invalid.
      // Log an error and continue the removal. Don't attempt to use netdev_ifc_ again.
      netdev_ifc_ = {};
      fdf::error("Failed to remove port {}: {}", port_id_, status.FormatDescription());
    }

    // At this point the port binding should already be unbound after the port removal above. But if
    // the port removal above failed, the mac binding unbound first, or the netdev_ifc_ client was
    // somehow invalidated before, we need to unbind it here. The unbind handler will continue the
    // removal process.
    if (port_binding_.has_value()) {
      port_binding_->Unbind();
      return nullptr;
    }
    // Same thing applies to the mac binding.
    if (mac_binding_.has_value()) {
      mac_binding_->Unbind();
      return nullptr;
    }

    // Clear out the dispatcher here to indicate that the port object is ready for destruction. The
    // on complete callback or PortRemoved call should be able to delete this object so at this
    // point everything has to be ready for destruction. Keep a pointer to the dispatcher so that we
    // can post the on_removed callback.
    fdf_dispatcher_t* dispatcher = dispatcher_;
    dispatcher_ = nullptr;

    if (on_removed_) {
      // This was a user-initiated removal, call the on_removed_ handler provided by the user.
      return [dispatcher, on_removed = std::move(on_removed_), &post_task]() mutable {
        post_task(dispatcher, std::move(on_removed), ZX_OK);
      };
    }
    // This was an unexpected removal. Call the PortRemoved callback. This does not need to be
    // posted on the dispatcher as any unexpected removal can only happen on the dispatcher. I.e.
    // either NetworkPort::Removed was called, or the NetworkPort or MacAddr server was unbound, all
    // of which use the dispatcher.
    return [&] { iface_.PortRemoved(); };
  }();

  // The mutex cannot be locked during the callback. The callback might destroy the object. If it
  // does then any mutex unlock after this would access free'd memory.
  mutex_.unlock();

  if (complete) {
    complete();
  }
}

void NetworkPort::SetPortOnline(bool online) {
  std::lock_guard lock(mutex_);
  if (online_ == online) {
    return;
  }
  online_ = online;
  fuchsia_hardware_network::PortStatus port_status;
  GetPortStatusLocked(&port_status);

  if (!netdev_ifc_.is_valid()) {
    fdf::warn("netdev_ifc_ invalid, port likely removed.");
    return;
  }

  fdf::Arena arena('NETD');
  auto status =
      netdev_ifc_.buffer(arena)->PortStatusChanged(port_id_, fidl::ToWire(arena, port_status));
  if (!status.ok()) {
    fdf::error("Call to PortStatusChanged failed: {}", status.FormatDescription());
  }
}

bool NetworkPort::IsOnline() const {
  std::lock_guard lock(mutex_);
  return online_;
}

void NetworkPort::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  static constexpr fuchsia_hardware_network::wire::FrameType kSupportedRxTypes[] = {
      fuchsia_hardware_network::wire::FrameType::kEthernet};

  static constexpr fuchsia_hardware_network::wire::FrameTypeSupport kSupportedTxTypes[] = {{
      .type = fuchsia_hardware_network::wire::FrameType::kEthernet,
      .features = fuchsia_hardware_network::wire::kFrameFeaturesRaw,
      .supported_flags = fuchsia_hardware_network::wire::TxFlags{},
  }};

  fuchsia_hardware_network::wire::PortClass port_class;
  switch (role_) {
    case Role::Client:
      port_class = fuchsia_hardware_network::wire::PortClass::kWlanClient;
      break;
    case Role::Ap:
      port_class = fuchsia_hardware_network::wire::PortClass::kWlanAp;
      break;
  }

  auto builder = fuchsia_hardware_network::wire::PortBaseInfo::Builder(arena);
  builder.port_class(port_class).rx_types(kSupportedRxTypes).tx_types(kSupportedTxTypes);

  completer.buffer(arena).Reply(builder.Build());
}

void NetworkPort::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  std::lock_guard lock(mutex_);
  fuchsia_hardware_network::PortStatus status;
  GetPortStatusLocked(&status);

  completer.buffer(arena).Reply(fidl::ToWire(arena, status));
}

void NetworkPort::SetActive(
    fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request, fdf::Arena& arena,
    SetActiveCompleter::Sync& completer) {}

void NetworkPort::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  zx::result endpoints = fdf::CreateEndpoints<fuchsia_hardware_network_driver::MacAddr>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create MacAddr endpoints: {}", endpoints.status_string());
    completer.Close(endpoints.error_value());
    return;
  }

  std::lock_guard lock(mutex_);
  mac_binding_ = fdf::BindServer(dispatcher_, std::move(endpoints->server), this,
                                 [this](NetworkPort*, fidl::UnbindInfo info,
                                        fdf::ServerEnd<fuchsia_hardware_network_driver::MacAddr>) {
                                   // RemovePortLocked will unlock the mutex.
                                   mutex_.lock();
                                   if (!info.is_user_initiated()) {
                                     // on_removed_ means removal started so a peer closed is almost
                                     // guaranteed. Logging a warning at that point adds no value.
                                     if (!on_removed_ || info.status() != ZX_ERR_PEER_CLOSED) {
                                       fdf::warn("MacAddr server unbound unexpectedly: {}",
                                                 info.FormatDescription());
                                     }
                                   }

                                   mac_binding_.reset();
                                   RemovePortLocked(nullptr);
                                 });

  completer.buffer(arena).Reply(std::move(endpoints->client));
}

void NetworkPort::Removed(fdf::Arena&, RemovedCompleter::Sync&) {
  // The port was removed, either by the user or unexpectedly. Either way the port should be fully
  // removed. RemovePortLocked will unlock the mutex.
  mutex_.lock();
  netdev_ifc_ = {};
  RemovePortLocked(nullptr);
}

void NetworkPort::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  fuchsia_net::MacAddress mac;
  iface_.MacGetAddress(&mac);
  completer.buffer(arena).Reply(fidl::ToWire(arena, mac));
}

void NetworkPort::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  fuchsia_hardware_network_driver::Features features;
  iface_.MacGetFeatures(&features);
  completer.buffer(arena).Reply(fidl::ToWire(arena, std::move(features)));
}

void NetworkPort::SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
                          fdf::Arena& arena, SetModeCompleter::Sync& completer) {
  iface_.MacSetMode(request->mode,
                    cpp20::span(request->multicast_macs.begin(), request->multicast_macs.end()));
  completer.buffer(arena).Reply();
}

void NetworkPort::GetPortStatusLocked(fuchsia_hardware_network::PortStatus* out_status) {
  // Provide a reasonable default status
  using fuchsia_hardware_network::wire::StatusFlags;
  out_status->flags() = online_ ? StatusFlags::kOnline : StatusFlags(0u);
  out_status->mtu() = iface_.PortGetMtu();

  // Allow the interface implementation to modify the status if it wants to
  iface_.PortGetStatus(out_status);
}

}  // namespace wlan::drivers::components
