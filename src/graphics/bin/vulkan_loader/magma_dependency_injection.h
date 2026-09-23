// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_MAGMA_DEPENDENCY_INJECTION_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_MAGMA_DEPENDENCY_INJECTION_H_

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <fidl/fuchsia.memorypressure/cpp/wire.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>

class MagmaDependencyInjection {
 public:
  using WatcherType =
      component::ServiceMemberWatcher<fuchsia_gpu_magma::DependencyInjectionService::Device>;

  static zx::result<MagmaDependencyInjection> Create(
      async_dispatcher_t* dispatcher,
      fit::function<zx::result<fidl::ClientEnd<fuchsia_memorypressure::Provider>>()>
          provider_factory);

  static fit::function<void(fidl::ClientEnd<fuchsia_gpu_magma::DependencyInjection> client_end)>
  ServiceInstanceHandler(
      fit::function<zx::result<fidl::ClientEnd<fuchsia_memorypressure::Provider>>()>
          provider_factory);

 private:
  explicit MagmaDependencyInjection(std::unique_ptr<WatcherType> watcher)
      : watcher_(std::move(watcher)) {}

  std::unique_ptr<WatcherType> watcher_;
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_MAGMA_DEPENDENCY_INJECTION_H_
