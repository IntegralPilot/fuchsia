// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device_port.h"

#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <lib/async/cpp/task.h>

#include "device_interface.h"
#include "log.h"

namespace network::internal {

void DevicePort::Create(
    DeviceInterface* parent, async_dispatcher_t* dispatcher, netdev::wire::PortId id,
    fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>&& port_client,
    fdf_dispatcher_t* mac_dispatcher, TeardownCallback&& on_teardown, OnCreated&& on_created) {
  if (parent == nullptr) {
    LOGF_ERROR("null parent provided");
    on_created(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  fbl::AllocChecker ac;
  std::unique_ptr<DevicePort> port(
      new (&ac) DevicePort(parent, dispatcher, id, std::move(port_client), std::move(on_teardown)));
  if (!ac.check()) {
    LOGF_ERROR("Failed to allocate memory for port");
    on_created(zx::error(ZX_ERR_NO_MEMORY));
    return;
  }

  // Keep a raw pointer for making the call below, the unique ptr will have been moved.
  DevicePort* port_ptr = port.get();
  port_ptr->Init(mac_dispatcher, [on_created = std::move(on_created),
                                  port = std::move(port)](zx_status_t status) mutable {
    if (status != ZX_OK) {
      // Reset the port client to ensure that the DevicePort object doesn't try to do anything with
      // it on destruction.
      port->port_ = fdf::WireSharedClient<netdriver::NetworkPort>();
      on_created(zx::error(status));
      return;
    }
    on_created(zx::ok(std::move(port)));
  });
}

DevicePort::DevicePort(DeviceInterface* parent, async_dispatcher_t* dispatcher,
                       netdev::wire::PortId id,
                       fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>&& port,
                       TeardownCallback&& on_teardown)
    : parent_(parent),
      dispatcher_(dispatcher),
      id_(id),
      port_(std::move(port)),
      on_teardown_(std::move(on_teardown)) {}

DevicePort::~DevicePort() {
  fdf::Arena arena('NETD');
  if (port_.is_valid()) {
    fidl::OneWayStatus status = port_.buffer(arena)->Removed();
    if (!status.ok()) {
      LOGF_ERROR("Failed to remove port: %s", status.FormatDescription().c_str());
    }
  }
}

void DevicePort::Init(fdf_dispatcher_t* mac_dispatcher,
                      fit::callback<void(zx_status_t)>&& on_complete) {
  if (id_event_.is_valid()) {
    LOGF_ERROR("Init already called");
    on_complete(ZX_ERR_BAD_STATE);
    return;
  }
  if (zx_status_t status = zx::event::create(0, &id_event_); status != ZX_OK) {
    LOGF_ERROR("Failed to create ID event: %s", zx_status_get_string(status));
    on_complete(status);
    return;
  }
  GetMac([this, mac_dispatcher, on_complete = std::move(on_complete)](
             zx::result<::fdf::ClientEnd<netdriver::MacAddr>> result) mutable {
    if (result.is_error()) {
      on_complete(result.status_value());
      return;
    }

    // Pre-create the callback here because it needs to be shared between two separate paths below
    // and the on_complete callback can only be captured in one place.
    fit::callback<void(zx_status_t)> get_port_info = [this, on_complete = std::move(on_complete)](
                                                         zx_status_t status) mutable {
      if (status != ZX_OK) {
        on_complete(status);
        return;
      }
      GetInitialPortInfo([this, on_complete = std::move(on_complete)](zx_status_t status) mutable {
        if (status != ZX_OK) {
          on_complete(status);
          return;
        }
        GetInitialStatus([on_complete = std::move(on_complete)](zx_status_t status) mutable {
          on_complete(status);
        });
      });
    };

    if (result.value().is_valid()) {
      CreateMacInterface(std::move(result.value()), mac_dispatcher, get_port_info.share());
      return;
    }
    get_port_info(ZX_OK);
  });
}

void DevicePort::GetMac(
    fit::callback<void(zx::result<::fdf::ClientEnd<netdriver::MacAddr>>)>&& on_complete) {
  fdf::Arena arena('PORT');
  port_.buffer(arena)->GetMac().Then(
      [on_complete = std::move(on_complete)](
          fdf::WireUnownedResult<netdriver::NetworkPort::GetMac>& result) mutable {
        if (!result.ok()) {
          LOGF_ERROR("Failed to get Mac interface: %s", result.FormatDescription().c_str());
          on_complete(zx::error(result.status()));
          return;
        }
        on_complete(zx::ok(std::move(result->mac_ifc)));
      });
}

void DevicePort::CreateMacInterface(::fdf::ClientEnd<netdriver::MacAddr>&& client_end,
                                    fdf_dispatcher_t* mac_dispatcher,
                                    fit::callback<void(zx_status_t)>&& on_complete) {
  MacAddrDeviceInterface::Create(
      std::move(client_end), mac_dispatcher,
      [this, on_complete = std::move(on_complete)](
          zx::result<std::shared_ptr<MacAddrDeviceInterface>> result) mutable {
        if (result.is_error()) {
          on_complete(result.status_value());
          return;
        }
        fbl::AutoLock lock(&lock_);
        mac_ = std::move(result.value());
        on_complete(ZX_OK);
      });
}

void DevicePort::GetInitialPortInfo(fit::callback<void(zx_status_t)>&& on_complete) {
  fdf::Arena arena('PORT');
  port_.buffer(arena)->GetInfo().Then(
      [this, on_complete = std::move(on_complete)](
          ::fdf::WireUnownedResult<netdriver::NetworkPort::GetInfo>& result) mutable {
        if (!result.ok()) {
          LOGF_ERROR("Failed to get initial port info: %s", result.FormatDescription().c_str());
          on_complete(result.status());
          return;
        }

        if (!result->info.has_port_class()) {
          LOGF_ERROR("missing port class");
          on_complete(ZX_ERR_INVALID_ARGS);
          return;
        }

        if (result->info.rx_types().size() > netdev::wire::kMaxFrameTypes) {
          LOGF_ERROR("too many port rx types: %ld > %d", result->info.rx_types().size(),
                     netdev::wire::kMaxFrameTypes);
          on_complete(ZX_ERR_INVALID_ARGS);
          return;
        }

        if (result->info.tx_types().size() > netdev::wire::kMaxFrameTypes) {
          LOGF_ERROR("too many port tx types: %ld > %d", result->info.tx_types().size(),
                     netdev::wire::kMaxFrameTypes);
          on_complete(ZX_ERR_INVALID_ARGS);
          return;
        }

        port_class_ = result->info.port_class();

        ZX_ASSERT(supported_rx_.empty());
        if (result->info.has_rx_types()) {
          std::copy(result->info.rx_types().begin(), result->info.rx_types().end(),
                    std::back_inserter(supported_rx_));
        }
        ZX_ASSERT(supported_tx_.empty());
        if (result->info.has_tx_types()) {
          std::copy(result->info.tx_types().begin(), result->info.tx_types().end(),
                    std::back_inserter(supported_tx_));
        }
        on_complete(ZX_OK);
      });
}

void DevicePort::GetInitialStatus(fit::callback<void(zx_status_t)>&& on_complete) {
  fdf::Arena arena('PORT');
  port_.buffer(arena)->GetStatus().Then(
      [this, on_complete = std::move(on_complete)](
          ::fdf::WireUnownedResult<netdriver::NetworkPort::GetStatus>& result) mutable {
        if (!result.ok()) {
          LOGF_ERROR("Failed to get initial port status: %s", result.FormatDescription().c_str());
          on_complete(result.status());
          return;
        }
        status_ = fidl::ToNatural(result->status);
        on_complete(ZX_OK);
      });
}

void DevicePort::StatusChanged(const netdev::wire::PortStatus& new_status) {
  fbl::AutoLock lock(&lock_);
  status_ = fidl::ToNatural(new_status);
  for (auto& w : watchers_) {
    w.PushStatus(new_status);
  }
}

void DevicePort::GetStatusWatcher(GetStatusWatcherRequestView request,
                                  GetStatusWatcherCompleter::Sync& _completer) {
  fbl::AutoLock lock(&lock_);
  if (teardown_started_) {
    // Don't install new watchers after teardown has started.
    return;
  }

  fbl::AllocChecker ac;
  auto n_watcher = fbl::make_unique_checked<StatusWatcher>(&ac, request->buffer);
  if (!ac.check()) {
    return;
  }

  zx_status_t status =
      n_watcher->Bind(dispatcher_, std::move(request->watcher), [this](StatusWatcher* watcher) {
        fbl::AutoLock lock(&lock_);
        watchers_.erase(*watcher);
        MaybeFinishTeardown();
      });

  if (status != ZX_OK) {
    LOGF_ERROR("failed to bind watcher: %s", zx_status_get_string(status));
    return;
  }

  fdf::Arena arena('NETD');
  n_watcher->PushStatus(fidl::ToWire(arena, status_));
  watchers_.push_back(std::move(n_watcher));
}

bool DevicePort::MaybeFinishTeardown() {
  if (teardown_started_ && on_teardown_ && watchers_.is_empty() && !mac_ && bindings_.is_empty()) {
    // Always finish teardown on dispatcher to evade deadlock opportunity on DeviceInterface ports
    // lock.
    async::PostTask(dispatcher_, [this, call = std::move(on_teardown_)]() mutable { call(*this); });
    return true;
  }
  return false;
}

void DevicePort::Teardown() {
  fbl::AutoLock lock(&lock_);
  if (teardown_started_) {
    return;
  }
  teardown_started_ = true;
  // Attempt to conclude the teardown immediately if we have no live resources.
  if (MaybeFinishTeardown()) {
    return;
  }
  for (auto& watcher : watchers_) {
    watcher.Unbind();
  }
  for (auto& binding : bindings_) {
    binding.Unbind();
  }
  if (mac_) {
    mac_->Teardown([this]() {
      // Always dispatch mac teardown callback to our dispatcher.
      async::PostTask(dispatcher_, [this]() {
        fbl::AutoLock lock(&lock_);
        // Dispose of mac entirely on teardown complete.
        mac_ = nullptr;
        MaybeFinishTeardown();
      });
    });
  }
}

void DevicePort::GetMac(GetMacRequestView request, GetMacCompleter::Sync& _completer) {
  fidl::ServerEnd req = std::move(request->mac);

  fbl::AutoLock lock(&lock_);
  if (teardown_started_) {
    return;
  }
  if (!mac_) {
    req.Close(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  zx_status_t status = mac_->Bind(dispatcher_, std::move(req));
  if (status != ZX_OK) {
    LOGF_ERROR("failed to bind to MacAddr on port %d: %s", id_.base, zx_status_get_string(status));
  }
}

void DevicePort::SessionAttached() {
  fbl::AutoLock lock(&lock_);
  ZX_ASSERT(!has_session_attached_);
  has_session_attached_ = true;
  ApplyPortStatus();
}

void DevicePort::SessionDetached() {
  fbl::AutoLock lock(&lock_);
  ZX_ASSERT_MSG(has_session_attached_, "detached the same port twice");
  has_session_attached_ = false;
  ApplyPortStatus();
}

void DevicePort::ApplyPortStatus() {
  if (teardown_started_) {
    // Skip all notifications if tearing down.
    return;
  }
  // Always post notifications for later on dispatcher so the port implementation can safely call
  // back into the core device with no risk of deadlocks.
  async::PostTask(dispatcher_, [this, active = has_session_attached_]() {
    fdf::Arena arena('NETD');
    fidl::OneWayStatus result = port_.buffer(arena)->SetActive(active);
    if (!result.ok()) {
      LOGF_ERROR("SetActive failed with error: %s", result.FormatDescription().c_str());
    }
  });
}

bool DevicePort::IsValidRxFrameType(netdev::wire::FrameType frame_type) const {
  cpp20::span rx_types(supported_rx_.begin(), supported_rx_.end());
  return std::any_of(rx_types.begin(), rx_types.end(),
                     [frame_type](const netdev::wire::FrameType& t) { return t == frame_type; });
}

bool DevicePort::IsValidTxFrameType(netdev::wire::FrameType frame_type) const {
  cpp20::span tx_types(supported_tx_.begin(), supported_tx_.end());
  return std::any_of(
      tx_types.begin(), tx_types.end(),
      [frame_type](const netdev::wire::FrameTypeSupport& t) { return t.type == frame_type; });
}

void DevicePort::Bind(fidl::ServerEnd<netdev::Port> req) {
  fbl::AllocChecker ac;
  std::unique_ptr<Binding> binding(new (&ac) Binding);
  if (!ac.check()) {
    req.Close(ZX_ERR_NO_MEMORY);
    return;
  }

  fbl::AutoLock lock(&lock_);
  // Disallow binding a new request if teardown already started to prevent races
  // with the dispatched unbind below.
  if (teardown_started_) {
    return;
  }
  // Capture a pointer to the binding so we can erase it in the unbound function.
  Binding* binding_ptr = binding.get();
  binding->Bind(fidl::BindServer(dispatcher_, std::move(req), this,
                                 [binding_ptr](DevicePort* port, fidl::UnbindInfo /*unused*/,
                                               fidl::ServerEnd<netdev::Port> /*unused*/) {
                                   // Always complete unbind later to avoid deadlock in case bind
                                   // fails synchronously.
                                   async::PostTask(port->dispatcher_, [port, binding_ptr]() {
                                     fbl::AutoLock lock(&port->lock_);
                                     port->bindings_.erase(*binding_ptr);
                                     port->MaybeFinishTeardown();
                                   });
                                 }));

  bindings_.push_front(std::move(binding));
}

void DevicePort::GetInfo(GetInfoCompleter::Sync& completer) {
  fidl::Arena arena;

  netdev::wire::PortBaseInfo port_base_info = netdev::wire::PortBaseInfo::Builder(arena)
                                                  .port_class(port_class_)
                                                  .tx_types(supported_tx_)
                                                  .rx_types(supported_rx_)
                                                  .Build();

  netdev::wire::PortInfo port_info =
      netdev::wire::PortInfo::Builder(arena).id(id_).base_info(port_base_info).Build();
  completer.Reply(port_info);
}

void DevicePort::GetStatus(GetStatusCompleter::Sync& completer) {
  fdf::Arena arena('NETD');
  port_.buffer(arena)->GetStatus().Then(
      [completer = completer.ToAsync()](
          fdf::WireUnownedResult<fuchsia_hardware_network_driver::NetworkPort::GetStatus>&
              result) mutable {
        if (!result.ok()) {
          LOGF_ERROR("GetStatus() failed: %s", result.FormatDescription().c_str());
          completer.Close(result.status());
          return;
        }
        completer.Reply(result->status);
      });
}

void DevicePort::GetDevice(GetDeviceRequestView request, GetDeviceCompleter::Sync& _completer) {
  if (zx_status_t status = parent_->Bind(std::move(request->device)); status != ZX_OK) {
    LOGF_ERROR("bind failed %s", zx_status_get_string(status));
  }
}

void DevicePort::Clone(CloneRequestView request, CloneCompleter::Sync& _completer) {
  Bind(std::move(request->port));
}

void DevicePort::GetCounters(GetCountersCompleter::Sync& completer) {
  fidl::Arena arena;

  netdev::wire::PortGetCountersResponse rsp = netdev::wire::PortGetCountersResponse::Builder(arena)
                                                  .tx_frames(counters_.tx_frames)
                                                  .tx_bytes(counters_.tx_bytes)
                                                  .rx_frames(counters_.rx_frames)
                                                  .rx_bytes(counters_.rx_bytes)
                                                  .Build();
  completer.Reply(rsp);
}

void DevicePort::GetDiagnostics(GetDiagnosticsRequestView request,
                                GetDiagnosticsCompleter::Sync& _completer) {
  parent_->diagnostics().Bind(std::move(request->diagnostics));
}

void DevicePort::GetIdentity(GetIdentityCompleter::Sync& completer) {
  zx::event event;
  const zx_status_t status = id_event_.duplicate(ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER, &event);
  // This should never fail. Port creation should fail if the event cannot be created. Other errors
  // should only happen if the rights set at event creation are incorrect.
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to duplicate event: %s", zx_status_get_string(status));
  completer.Reply(std::move(event));
}

}  // namespace network::internal
