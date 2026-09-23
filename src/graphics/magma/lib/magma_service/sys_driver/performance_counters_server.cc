// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "performance_counters_server.h"

#include <lib/driver/logging/cpp/logger.h>

namespace msd::internal {
zx::result<> PerformanceCountersServer::Create(fdf::OutgoingDirectory* outgoing) {
  zx_status_t status = zx::event::create(0, &event_);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  auto perf_counter_access_protocol =
      [this](fidl::ServerEnd<fuchsia_gpu_magma::PerformanceCounterAccess> server_end) mutable {
        fidl::BindServer(dispatcher_, std::move(server_end), this);
      };

  fuchsia_gpu_magma::PerformanceCounterService::InstanceHandler handler({
      .access = std::move(perf_counter_access_protocol),
  });

  {
    auto status = outgoing->template AddService<fuchsia_gpu_magma::PerformanceCounterService>(
        std::move(handler));
    if (status.is_error()) {
      fdf::error("{}(): Failed to add service to outgoing directory: {}", __func__, status);
      return status.take_error();
    }
  }

  return zx::ok();
}

zx_koid_t PerformanceCountersServer::GetEventKoid() {
  zx_info_handle_basic_t info;
  if (event_.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr) != ZX_OK)
    return 0;
  return info.koid;
}

void PerformanceCountersServer::GetPerformanceCountToken(
    GetPerformanceCountTokenCompleter::Sync& completer) {
  zx::event event_duplicate;
  zx_status_t status = event_.duplicate(ZX_RIGHT_SAME_RIGHTS, &event_duplicate);
  if (status != ZX_OK) {
    completer.Close(status);
  } else {
    completer.Reply(std::move(event_duplicate));
  }
}

}  // namespace msd::internal
