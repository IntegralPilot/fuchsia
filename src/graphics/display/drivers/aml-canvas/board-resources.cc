// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/aml-canvas/board-resources.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/compat/cpp/logging.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <cinttypes>

namespace aml_canvas {

zx::result<fdf::MmioBuffer> MapMmio(
    MmioResourceIndex mmio_index,
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> pdev) {
  std::optional<fdf::MmioBuffer> mmio;
  fidl::WireResult result = fidl::WireCall(pdev)->GetMmioById(static_cast<uint32_t>(mmio_index));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to map MMIO resource #%" PRIu32 ": FIDL call failed: %s",
           static_cast<uint32_t>(mmio_index), result.status_string());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to map MMIO resource #%" PRIu32 ": Platform device failed %s",
           static_cast<uint32_t>(mmio_index), zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }
  const fuchsia_hardware_platform_device::wire::Mmio* mmio_params = result->value();
  if (!mmio_params->has_offset() || !mmio_params->has_size() || !mmio_params->has_vmo()) {
    zxlogf(ERROR, "Failed to map MMIO resource #%" PRIu32 ": Platform device provided invalid MMIO",
           static_cast<uint32_t>(mmio_index));
    return zx::error(ZX_ERR_BAD_STATE);
  };

  zx::result<fdf::MmioBuffer> create_mmio_result = fdf::MmioBuffer::Create(
      mmio_params->offset(), mmio_params->size(), std::move(mmio_params->vmo()),
      /*cache_policy=*/ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (create_mmio_result.is_error()) {
    zxlogf(ERROR, "Failed to create fdf::MmioBuffer: %s", create_mmio_result.status_string());
    return create_mmio_result.take_error();
  }
  return zx::ok(std::move(create_mmio_result).value());
}

zx::result<zx::bti> GetBti(BtiResourceIndex bti_index,
                           fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> pdev) {
  fidl::WireResult result = fidl::WireCall(pdev)->GetBtiById(static_cast<uint32_t>(bti_index));
  if (result.status() != ZX_OK) {
    zxlogf(ERROR, "Failed to get BTI resource #%" PRIu32 ": FIDL failed %s",
           static_cast<uint32_t>(bti_index), result.status_string());
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    zxlogf(ERROR, "Failed to get BTI resource #%" PRIu32 ": Platform device failed %s",
           static_cast<uint32_t>(bti_index), zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }
  zx::bti bti = std::move(result->value()->bti);
  ZX_DEBUG_ASSERT_MSG(bti.is_valid(), "GetBti() succeeded but didn't populate the out-param");
  return zx::ok(std::move(bti));
}

}  // namespace aml_canvas
