// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/bin/vulkan_loader/magma_dependency_injection.h"

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include "lib/async/dispatcher.h"

fit::function<void(fidl::ClientEnd<fuchsia_gpu_magma::DependencyInjection> client_end)>
MagmaDependencyInjection::ServiceInstanceHandler(
    fit::function<zx::result<fidl::ClientEnd<fuchsia_memorypressure::Provider>>()>
        provider_factory) {
  return [provider_factory = std::move(provider_factory)](
             fidl::ClientEnd<fuchsia_gpu_magma::DependencyInjection> client_end) {
    auto pressure_provider = provider_factory();
    if (!pressure_provider.is_ok()) {
      FX_LOGS(ERROR) << "Failed to get pressure provider: " << pressure_provider.status_string();
      return;
    }

    if (auto result =
            fidl::WireCall(client_end)->SetMemoryPressureProvider(std::move(*pressure_provider));
        !result.ok()) {
      FX_LOGS(ERROR) << "Failed to set memory pressure provider: " << result.status_string();
      return;
    }
  };
}

zx::result<MagmaDependencyInjection> MagmaDependencyInjection::Create(
    async_dispatcher_t* dispatcher,
    fit::function<zx::result<fidl::ClientEnd<fuchsia_memorypressure::Provider>>()>
        provider_factory) {
  auto watcher = std::make_unique<
      component::ServiceMemberWatcher<fuchsia_gpu_magma::DependencyInjectionService::Device>>();

  if (auto result = watcher->Begin(dispatcher, ServiceInstanceHandler(std::move(provider_factory)));
      result.is_error()) {
    FX_LOGS(ERROR) << "Failed to begin service watcher: " << result.status_string();
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(MagmaDependencyInjection(std::move(watcher)));
}
