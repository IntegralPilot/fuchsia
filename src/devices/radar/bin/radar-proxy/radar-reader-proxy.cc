// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "radar-reader-proxy.h"

#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>

namespace radar {

std::unique_ptr<RadarProxy> RadarProxy::Create(async_dispatcher_t* dispatcher,
                                               RadarDeviceConnector* connector) {
  FX_LOGS(INFO) << "Burst reader proxying enabled";
  auto proxy = std::make_unique<radar::RadarReaderProxy>(dispatcher, connector);
  zx::result<> status = proxy->Init(dispatcher);
  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to initialize RadarProxy: " << status.status_string();
    return nullptr;
  }
  return proxy;
}

RadarReaderProxy::~RadarReaderProxy() {
  // Close out any outstanding requests before destruction to avoid triggering an assert.
  for (auto& [server, completer] : connect_requests_) {
    completer.Close(ZX_ERR_PEER_CLOSED);
  }
}

void RadarReaderProxy::Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) {
  if (radar_client_) {
    // Our driver client is bound, so we must have already set these fields.
    ZX_DEBUG_ASSERT(burst_properties_);

    instances_.emplace_back(
        std::make_unique<ReaderInstance>(dispatcher_, std::move(request.server()), this));
    completer.Reply(fit::ok());
  } else {
    connect_requests_.emplace_back(std::move(request.server()), completer.ToAsync());
  }
}

void RadarReaderProxy::OnDeviceFound(
    fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) {
  if (radar_client_) {
    return;
  }
  ValidateDevice(std::move(client_end));
}

zx::result<> RadarReaderProxy::AddProtocols(component::OutgoingDirectory* const outgoing) {
  zx::result result =
      outgoing->AddUnmanagedProtocol<fuchsia_hardware_radar::RadarBurstReaderProvider>(
          [&](fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> server_end) {
            fidl::BindServer(dispatcher_, std::move(server_end), this);
          });
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add RadarBurstReaderProvider protocol: " << result.status_string();
    return result;
  }

  result = outgoing->AddUnmanagedProtocol<fuchsia_hardware_radar::RadarBurstInjector>(
      [&](fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstInjector> server_end) {
        BindInjector(std::move(server_end));
      });
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add RadarBurstInjector protocol: " << result.status_string();
    return result;
  }

  return zx::ok();
}

void RadarReaderProxy::ResizeVmoPool(const size_t count) {
  ZX_DEBUG_ASSERT(burst_properties_);

  if (count <= vmo_pool_.size()) {
    return;
  }

  const size_t vmos_to_register = count - vmo_pool_.size();

  std::vector<zx::vmo> vmos(vmos_to_register);
  std::vector<uint32_t> vmo_ids(vmos_to_register);

  for (size_t i = 0; i < vmos.size(); i++) {
    vmo_ids[i] = static_cast<uint32_t>(vmo_pool_.size());

    MappedVmo vmo;

    // The radar driver writes to these VMOs, so we only need read access.
    zx_status_t status =
        vmo.mapped_vmo.CreateAndMap(burst_properties_->size(), ZX_VM_PERM_READ, nullptr, &vmo.vmo);
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to create and map VMO";
      HandleFatalError(status);
      return;
    }

    if ((status = vmo.vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmos[i])) != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to duplicate VMO";
      HandleFatalError(status);
      return;
    }

    vmo_pool_.push_back(std::move(vmo));
  }

  if (!radar_client_) {
    return;
  }

  radar_client_->RegisterVmos({std::move(vmo_ids), std::move(vmos)}).Then([&](const auto& result) {
    if (result.is_error()) {
      zx_status_t status = ZX_ERR_BAD_STATE;
      if (result.error_value().is_framework_error()) {
        status = result.error_value().is_framework_error();
        FX_PLOGS(ERROR, status) << "Failed to send register VMOs request";
      } else {
        FX_LOGS(ERROR) << "Failed to register VMOs";
      }
      HandleFatalError(status);
    }
  });
}

void RadarReaderProxy::StartBursts() {
  if (radar_client_ && !inject_bursts_) {
    if (auto result = radar_client_->StartBursts(); result.is_error()) {
      FX_PLOGS(ERROR, result.error_value().status()) << "Failed to send start bursts request";
      HandleFatalError(result.error_value().status());
    }
  }
}

void RadarReaderProxy::RequestStopBursts() {
  for (const auto& instance : instances_) {
    // Don't stop bursts if any client still wants to receive them.
    if (instance->bursts_started()) {
      return;
    }
  }

  StopBursts();
}

void RadarReaderProxy::OnInstanceUnbound(ReaderInstance* const instance) {
  // The instance was unbound, remove it from our list and let it be deleted.
  for (auto it = instances_.begin(); it != instances_.end(); it++) {
    if (it->get() == instance) {
      instances_.erase(it);
      RequestStopBursts();
      return;
    }
  }
}

fuchsia_hardware_radar::RadarBurstReaderGetBurstPropertiesResponse
RadarReaderProxy::burst_properties() const {
  ZX_DEBUG_ASSERT(burst_properties_);
  return *burst_properties_;
}

zx::time RadarReaderProxy::StartBurstInjection() {
  inject_bursts_ = true;
  StopBursts();
  return last_burst_timestamp_;
}

void RadarReaderProxy::StopBurstInjection() {
  inject_bursts_ = false;
  RequestStartBursts();
}

void RadarReaderProxy::SendBurst(cpp20::span<const uint8_t> burst, zx::time timestamp) {
  for (auto& instance : instances_) {
    instance->SendBurst(burst, timestamp);
  }
}

void RadarReaderProxy::OnInjectorUnbound(BurstInjector* const injector) {
  injector_.reset();
  StopBurstInjection();
}

void RadarReaderProxy::on_fidl_error(const fidl::UnbindInfo info) {
  FX_PLOGS(ERROR, info.status()) << "Connection to radar device closed, attempting to reconnect";
  HandleFatalError(info.status());
}

void RadarReaderProxy::OnBurst(
    fidl::Event<fuchsia_hardware_radar::RadarBurstReader::OnBurst>& event) {
  ZX_DEBUG_ASSERT(burst_properties_);

  // Make sure to unlock the VMO even if we are injecting bursts and don't need the data.
  const auto unlock_vmo = fit::defer([&]() {
    if (event.burst() && event.burst()->vmo_id() < vmo_pool_.size() && radar_client_) {
      if (auto result = radar_client_->UnlockVmo(event.burst()->vmo_id()); result.is_error()) {
        HandleFatalError(result.error_value().status());
      }
    }
  });

  if (inject_bursts_) {
    return;
  }

  if (event.IsUnknown()) {
    HandleFatalError(ZX_ERR_BAD_STATE);
    return;
  }

  if (event.error() || event.burst()->vmo_id() >= vmo_pool_.size()) {
    const auto status = event.error().value_or(fuchsia_hardware_radar::StatusCode::kVmoNotFound);
    for (auto& instance : instances_) {
      instance->SendError(status);
    }
    return;
  }

  last_burst_timestamp_ = zx::time(event.burst()->timestamp());

  const uint32_t vmo_id = event.burst()->vmo_id();
  const cpp20::span<const uint8_t> burst_data{vmo_pool_[vmo_id].start(), burst_properties_->size()};
  SendBurst(burst_data, last_burst_timestamp_);
}

void RadarReaderProxy::BindInjector(
    fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstInjector> server_end) {
  if (injector_ || pending_injector_client_) {
    server_end.Close(ZX_ERR_ALREADY_BOUND);
  } else if (burst_properties_) {
    injector_.emplace(dispatcher_, std::move(server_end), this);
  } else {
    // We haven't connected to the driver to read out the burst properties yet, so stash this
    // request until we're ready.
    pending_injector_client_ = std::move(server_end);
  }
}

bool RadarReaderProxy::ValidateDevice(
    fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) {
  fidl::SyncClient provider(std::move(client_end));

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  if (endpoints.is_error() || provider->Connect(std::move(endpoints->server)).is_error()) {
    return false;
  }

  fidl::SyncClient reader(std::move(endpoints->client));

  // If this is our first connection, save the burst size. If this is not our first connection, make
  // sure that the burst size reported by this device matches what we have saved.
  const auto burst_properties = reader->GetBurstProperties();
  if (burst_properties.is_error()) {
    return false;
  }
  if (burst_properties_) {
    if (burst_properties_->size() != burst_properties->size() ||
        burst_properties_->period() != burst_properties->period()) {
      return false;
    }
  } else {
    burst_properties_ = *burst_properties;
  }

  if (!vmo_pool_.empty()) {
    // Register all of the VMOs that are currently in the pool.
    std::vector<uint32_t> vmo_ids(vmo_pool_.size());
    std::vector<zx::vmo> vmos(vmo_pool_.size());

    for (uint32_t i = 0; i < vmo_pool_.size(); i++) {
      vmo_ids[i] = i;
      zx_status_t status = vmo_pool_[i].vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmos[i]);
      if (status != ZX_OK) {
        FX_PLOGS(ERROR, status) << "Failed to duplicate VMO";
        return false;
      }
    }

    const auto result = reader->RegisterVmos({{std::move(vmo_ids), std::move(vmos)}});
    if (result.is_error() && result.error_value().is_framework_error()) {
      FX_PLOGS(ERROR, result.error_value().framework_error().status())
          << "Failed to send register VMOs request";
      return false;
    }
    if (result.is_error() && result.error_value().is_domain_error()) {
      FX_LOGS(ERROR) << "Failed to register VMOs";
      return false;
    }
  }

  radar_client_ = fidl::Client(reader.TakeClientEnd(), dispatcher_, this);

  // Now that we are connected to a device, complete any connect requests that are outstanding.
  for (auto& [server, completer] : connect_requests_) {
    instances_.emplace_back(std::make_unique<ReaderInstance>(dispatcher_, std::move(server), this));
    completer.Reply(fit::ok());
  }
  connect_requests_.clear();

  if (pending_injector_client_) {
    ZX_DEBUG_ASSERT(!injector_);
    injector_.emplace(dispatcher_, std::move(pending_injector_client_), this);
  }

  return true;
}

void RadarReaderProxy::HandleFatalError(const zx_status_t status) {
  // Tear down the client so that we stop receiving bursts. We may be handling a FIDL error here, in
  // which case the client has already been unbound.
  radar_client_ = {};

  // Close all existing client connections and let them reconnect if needed.
  for (auto& instance : instances_) {
    instance->Close(status);
  }

  // Check for available devices now, just in case one was added before the connection closed. If
  // not, the ServiceMemberWatcher will signal to connect when a new device becomes available.
  connector_->ConnectToFirstRadarDevice(
      [&](auto client_end) { return ValidateDevice(std::move(client_end)); });
}

void RadarReaderProxy::RequestStartBursts() {
  for (const auto& instance : instances_) {
    if (instance->bursts_started()) {
      StartBursts();
      return;
    }
  }
}

void RadarReaderProxy::StopBursts() {
  if (radar_client_) {
    radar_client_->StopBursts().Then([&](const auto& result) {
      if (result.is_error()) {
        FX_PLOGS(ERROR, result.error_value().status()) << "Failed to send stop bursts request";
        HandleFatalError(result.error_value().status());
      }
    });
  }
}

}  // namespace radar
