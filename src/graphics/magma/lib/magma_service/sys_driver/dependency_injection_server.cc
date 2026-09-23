// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dependency_injection_server.h"

#include <lib/driver/logging/cpp/logger.h>

namespace msd::internal {

zx::result<> DependencyInjectionServer::Create(fdf::OutgoingDirectory* outgoing) {
  auto dep_injection_protocol =
      [this](fidl::ServerEnd<fuchsia_gpu_magma::DependencyInjection> server_end) mutable {
        fidl::BindServer(dispatcher_, std::move(server_end), this);
      };

  fuchsia_gpu_magma::DependencyInjectionService::InstanceHandler handler({
      .device = std::move(dep_injection_protocol),
  });

  {
    auto status = outgoing->template AddService<fuchsia_gpu_magma::DependencyInjectionService>(
        std::move(handler));
    if (status.is_error()) {
      fdf::error("{}(): Failed to add service to outgoing directory: {}", __func__, status);
      return status.take_error();
    }
  }

  return zx::ok();
}

void DependencyInjectionServer::SetMemoryPressureProvider(
    fuchsia_gpu_magma::wire::DependencyInjectionSetMemoryPressureProviderRequest* request,
    SetMemoryPressureProviderCompleter::Sync& completer) {
  if (pressure_server_) {
    return;
  }
  auto endpoints = fidl::CreateEndpoints<fuchsia_memorypressure::Watcher>();
  if (!endpoints.is_ok()) {
    MAGMA_LOG(WARNING, "Failed to create fidl Endpoints");
    return;
  }
  pressure_server_ = fidl::BindServer(dispatcher_, std::move(endpoints->server), this);

  fidl::WireSyncClient provider{std::move(request->provider)};
  // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
  (void)provider->RegisterWatcher(std::move(endpoints->client));
}

void DependencyInjectionServer::OnLevelChanged(OnLevelChangedRequestView request,
                                               OnLevelChangedCompleter::Sync& completer) {
  owner_->SetMemoryPressureLevel(GetMagmaLevel(request->level));
  completer.Reply();
}

// static
MagmaMemoryPressureLevel DependencyInjectionServer::GetMagmaLevel(
    fuchsia_memorypressure::wire::Level level) {
  switch (level) {
    case fuchsia_memorypressure::wire::Level::kNormal:
      return msd::MAGMA_MEMORY_PRESSURE_LEVEL_NORMAL;
    case fuchsia_memorypressure::wire::Level::kWarning:
      return msd::MAGMA_MEMORY_PRESSURE_LEVEL_WARNING;
    case fuchsia_memorypressure::wire::Level::kCritical:
      return msd::MAGMA_MEMORY_PRESSURE_LEVEL_CRITICAL;
    default:
      return msd::MAGMA_MEMORY_PRESSURE_LEVEL_NORMAL;
  }
}

}  // namespace msd::internal
