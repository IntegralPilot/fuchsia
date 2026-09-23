// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "virtio_gpu_control.h"

#include <lib/magma/platform/platform_logger.h>
#include <lib/magma/util/macros.h>

zx::result<> VirtioGpuControlFidl::Init(std::shared_ptr<fdf::Namespace> incoming) {
  auto control = incoming->Connect<fuchsia_gpu_virtio::Service::Control>();
  if (control.is_error()) {
    MAGMA_LOG(ERROR, "Error requesting virtio gpu control service: %s", control.status_string());
    return control.take_error();
  }

  control_ = fidl::WireSyncClient(std::move(*control));

  MAGMA_DMESSAGE("VirtioGpuControlFidl::Init success");

  return zx::ok();
}

uint64_t VirtioGpuControlFidl::GetCapabilitySetLimit() {
  auto result = control_->GetCapabilitySetLimit();
  if (!result.ok()) {
    MAGMA_DASSERT(false);
    return 0;
  }
  return result.value().limit;
}

zx::result<> VirtioGpuControlFidl::SendHardwareCommand(
    cpp20::span<uint8_t> request, std::function<void(cpp20::span<uint8_t>)> callback) {
  auto wire_result = control_->SendHardwareCommand(
      fidl::VectorView<uint8_t>::FromExternal(request.data(), request.size()));
  if (!wire_result.ok()) {
    return zx::error(wire_result.status());
  }
  auto result = wire_result.value();
  if (result.is_error()) {
    return result.take_error();
  }
  auto response = result.value();
  callback(cpp20::span<uint8_t>(response->response.data(), response->response.size()));
  return zx::ok();
}
