// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_DEPENDENCY_INJECTION_SERVER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_DEPENDENCY_INJECTION_SERVER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <fidl/fuchsia.memorypressure/cpp/wire.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/magma/util/macros.h>
#include <lib/magma_service/msd.h>

namespace msd::internal {
class DependencyInjectionServer : public fidl::WireServer<fuchsia_gpu_magma::DependencyInjection>,
                                  public fidl::WireServer<fuchsia_memorypressure::Watcher> {
 public:
  class Owner {
   public:
    virtual void SetMemoryPressureLevel(MagmaMemoryPressureLevel level) = 0;
  };

  explicit DependencyInjectionServer(Owner* owner, async_dispatcher_t* dispatcher)
      : owner_(owner), dispatcher_(dispatcher) {}

  zx::result<> Create(fdf::OutgoingDirectory* outgoing);

  // fuchsia:gpu::magma::DependencyInjection implementation.
  void SetMemoryPressureProvider(
      fuchsia_gpu_magma::wire::DependencyInjectionSetMemoryPressureProviderRequest* request,
      SetMemoryPressureProviderCompleter::Sync& completer) override;

  // fuchsia::memorypressure::Watcher implementation.
  void OnLevelChanged(OnLevelChangedRequestView request,
                      OnLevelChangedCompleter::Sync& completer) override;

 private:
  static MagmaMemoryPressureLevel GetMagmaLevel(fuchsia_memorypressure::wire::Level level);

  Owner* owner_;
  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBindingRef<fuchsia_memorypressure::Watcher>> pressure_server_;
};

}  // namespace msd::internal
#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_DEPENDENCY_INJECTION_SERVER_H_
