// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/gpu-control-server.h"

#include <fidl/fuchsia.gpu.virtio/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/driver/logging/cpp/logger.h>

#include <cstdint>
#include <span>

namespace virtio_display {

GpuControlServer::GpuControlServer(Owner* owner, size_t capability_set_limit)
    : owner_(owner), capability_set_limit_(capability_set_limit) {}

fuchsia_gpu_virtio::Service::InstanceHandler GpuControlServer::GetInstanceHandler(
    async_dispatcher_t* dispatcher) {
  return fuchsia_gpu_virtio::Service::InstanceHandler({
      .control = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
  });
}

void GpuControlServer::GetCapabilitySetLimit(GetCapabilitySetLimitCompleter::Sync& completer) {
  fdf::trace("GpuControlServer::GetCapabilitySetLimit returning {}", capability_set_limit_);

  completer.Reply(capability_set_limit_);
}

void GpuControlServer::SendHardwareCommand(
    fuchsia_gpu_virtio::wire::GpuControlSendHardwareCommandRequest* request,
    SendHardwareCommandCompleter::Sync& completer) {
  fdf::trace("GpuControlServer::SendHardwareCommand");

  auto callback = [&completer](std::span<uint8_t> response) {
    completer.ReplySuccess(
        fidl::VectorView<uint8_t>::FromExternal(response.data(), response.size()));
  };

  owner_->SendHardwareCommand(std::span<uint8_t>(request->request.data(), request->request.size()),
                              std::move(callback));
}

}  // namespace virtio_display
