// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_PERFORMANCE_COUNTERS_SERVER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_PERFORMANCE_COUNTERS_SERVER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/magma/util/macros.h>

namespace msd::internal {
class PerformanceCountersServer
    : public fidl::WireServer<fuchsia_gpu_magma::PerformanceCounterAccess> {
 public:
  explicit PerformanceCountersServer(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  zx::result<> Create(fdf::OutgoingDirectory* outgoing);

  zx_koid_t GetEventKoid();

 private:
  void GetPerformanceCountToken(GetPerformanceCountTokenCompleter::Sync& completer) override;

  async_dispatcher_t* dispatcher_;
  zx::event event_;
};

}  // namespace msd::internal

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_PERFORMANCE_COUNTERS_SERVER_H_
