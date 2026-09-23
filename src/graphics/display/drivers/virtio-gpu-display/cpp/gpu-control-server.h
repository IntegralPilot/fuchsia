// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_GPU_CONTROL_SERVER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_GPU_CONTROL_SERVER_H_

#include <fidl/fuchsia.gpu.virtio/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <cstddef>
#include <functional>
#include <span>

namespace virtio_display {

class GpuControlServer : public fidl::WireServer<fuchsia_gpu_virtio::GpuControl> {
 public:
  class Owner {
   public:
    virtual void SendHardwareCommand(std::span<uint8_t> request,
                                     std::function<void(std::span<uint8_t>)> callback) = 0;
  };

  GpuControlServer(Owner* device_accessor, size_t capability_set_limit);

  GpuControlServer(const GpuControlServer&) = delete;
  GpuControlServer& operator=(const GpuControlServer&) = delete;
  GpuControlServer(GpuControlServer&&) = delete;
  GpuControlServer& operator=(GpuControlServer&&) = delete;

  ~GpuControlServer() = default;

  // fidl::WireServer<fuchsia_gpu_virtio::GpuControl>:
  void GetCapabilitySetLimit(GetCapabilitySetLimitCompleter::Sync& completer) override;
  void SendHardwareCommand(fuchsia_gpu_virtio::wire::GpuControlSendHardwareCommandRequest* request,
                           SendHardwareCommandCompleter::Sync& completer) override;

  Owner* owner() { return owner_; }

  fuchsia_gpu_virtio::Service::InstanceHandler GetInstanceHandler(async_dispatcher_t* dispatcher);

 private:
  zx::result<zx::vmo> GetCapset(uint32_t capset_id, uint32_t capset_version);

  Owner* owner_;
  size_t capability_set_limit_;

  fidl::ServerBindingGroup<fuchsia_gpu_virtio::GpuControl> bindings_;
};

}  // namespace virtio_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_GPU_CONTROL_SERVER_H_
