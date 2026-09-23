// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zbi-format/zbi.h>

#include "src/graphics/display/lib/framebuffer-display/boot-framebuffer.h"
#include "src/graphics/display/lib/framebuffer-display/framebuffer-display-driver.h"

namespace framebuffer_display {

class BootFramebufferDisplay final : public FramebufferDisplayDriver {
 public:
  BootFramebufferDisplay() : FramebufferDisplayDriver("boot-framebuffer-display") {}

  zx::result<> ConfigureHardware() override {
    auto client = incoming().Connect<fuchsia_boot::Items>();
    if (client.is_error()) {
      return client.take_error();
    }
    auto result = fidl::WireCall(*client)->Get2(ZBI_TYPE_FRAMEBUFFER, {});
    if (!result.ok()) {
      return zx::error(result.status());
    }
    if (result->is_error()) {
      return zx::error(result->error_value());
    }
    auto items = result->value()->retrieved_items;
    if (items.size() != 1 || items[0].length != sizeof(info_)) {
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    zx_status_t status = items[0].payload.read(&info_, 0, sizeof(info_));
    if (status != ZX_OK) {
      return zx::error(status);
    }
    if (!BootFramebufferSize(info_)) {
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    fdf::info("Boot framebuffer: {}x{}, stride {}, format {:#x}",
              info_.width, info_.height, info_.stride, info_.format);
    return zx::ok();
  }

  zx::result<fdf::MmioBuffer> GetFrameBufferMmioBuffer() override {
    auto client = incoming().Connect<fuchsia_hardware_platform_device::Service::Device>();
    if (client.is_error()) {
      return client.take_error();
    }
    auto result = fidl::WireCall(*client)->GetMmioById(0);
    if (!result.ok()) {
      return zx::error(result.status());
    }
    if (result->is_error()) {
      return zx::error(result->error_value());
    }
    auto& mmio = *result->value();
    if (!mmio.has_vmo() || !mmio.has_offset() || !mmio.has_size() ||
        mmio.size() < *BootFramebufferSize(info_)) {
      return zx::error(ZX_ERR_BAD_STATE);
    }
    return fdf::MmioBuffer::Create(mmio.offset(), mmio.size(), std::move(mmio.vmo()),
                                   ZX_CACHE_POLICY_WRITE_COMBINING);
  }

  zx::result<DisplayProperties> GetDisplayProperties() override {
    return zx::ok(DisplayProperties{
        .width_px = static_cast<int32_t>(info_.width),
        .height_px = static_cast<int32_t>(info_.height),
        .row_stride_px = static_cast<int32_t>(info_.stride),
        .pixel_format = display::PixelFormat::kB8G8R8A8,
        .framebuffer_format = info_.format,
    });
  }

 private:
  zbi_swfb_t info_{};
};

}  // namespace framebuffer_display

FUCHSIA_DRIVER_EXPORT2(framebuffer_display::BootFramebufferDisplay);
