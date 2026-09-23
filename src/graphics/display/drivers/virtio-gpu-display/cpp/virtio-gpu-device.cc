// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/virtio-gpu-device.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cinttypes>
#include <cstdint>
#include <memory>
#include <optional>
#include <span>

#include <fbl/alloc_checker.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/virtio-pci-device.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/lib/virtio/virtio-abi.h"

namespace virtio_display {

std::optional<virtio_abi::ResourceFormat> PixelFormatToVirtioResourceFormat(
    display::PixelFormat pixel_format) {
  // TODO(https://fxbug.dev/42073721): Support more formats.
  if (pixel_format == display::PixelFormat::kB8G8R8A8) {
    return virtio_abi::ResourceFormat::kBgra32;
  }
  if (pixel_format == display::PixelFormat::kR8G8B8A8) {
    return virtio_abi::ResourceFormat::kR8g8b8a8;
  }

  return std::nullopt;
}

VirtioGpuDevice::VirtioGpuDevice(std::unique_ptr<VirtioPciDevice> virtio_device)
    : virtio_device_(std::move(virtio_device)) {
  ZX_DEBUG_ASSERT(virtio_device_);
}

VirtioGpuDevice::~VirtioGpuDevice() = default;

bool VirtioGpuDevice::UseBlobResource() {
  return (pci_device().Features() & virtio_abi::GpuDeviceFeatures::kGpuResourceBlob) != 0;
}

zx::result<uint32_t> VirtioGpuDevice::UpdateCursor() {
  const virtio_abi::UpdateCursorCommand command = {
      .header = {.type = virtio_abi::ControlType::kUpdateCursorCommand},
      .resource_id = next_resource_id_++,
  };

  const auto& response =
      virtio_device_->ExchangeCursorqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::warn("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
              static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }

  return zx::ok(command.resource_id);
}

zx::result<uint32_t> VirtioGpuDevice::SetCursorPosition(uint32_t scanout_id, uint32_t x,
                                                        uint32_t y) {
  const virtio_abi::UpdateCursorCommand command = {
      .header = {.type = virtio_abi::ControlType::kMoveCursorCommand},
      .position = {.scanout_id = scanout_id, .x = x, .y = y},
      // The fields below are ignored by the Move Cursor command.
      .resource_id = 0,
      .hot_x = 0,
      .hot_y = 0,
      .padding = 0,
  };

  const auto& response =
      virtio_device_->ExchangeCursorqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::warn("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
              static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }

  return zx::ok(command.resource_id);
}  // namespace virtio_display

zx::result<fbl::Vector<DisplayInfo>> VirtioGpuDevice::GetDisplayInfo() {
  const virtio_abi::GetDisplayInfoCommand command = {
      .header = {.type = virtio_abi::ControlType::kGetDisplayInfoCommand},
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::DisplayInfoResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kDisplayInfoResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }

  size_t enabled_scanout_count = 0;
  for (const virtio_abi::ScanoutInfo& scanout : response.scanouts) {
    if (scanout.enabled) {
      ++enabled_scanout_count;
    }
  }

  fbl::AllocChecker alloc_checker;
  fbl::Vector<DisplayInfo> display_infos;
  display_infos.reserve(enabled_scanout_count, &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayInfo vector");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  for (int i = 0; i < virtio_abi::kMaxScanouts; ++i) {
    const virtio_abi::ScanoutInfo& scanout = response.scanouts[i];
    if (!scanout.enabled) {
      continue;
    }

    fdf::trace("Scanout {}: placement ({}, {}), resolution {}x{} flags 0x{:08x}", i,
               scanout.geometry.x, scanout.geometry.y, scanout.geometry.width,
               scanout.geometry.height, scanout.flags);

    ZX_DEBUG_ASSERT(display_infos.size() < enabled_scanout_count);
    display_infos.push_back({.scanout_info = scanout, .scanout_id = i}, &alloc_checker);
    ZX_DEBUG_ASSERT(alloc_checker.check());
  }
  return zx::ok(std::move(display_infos));
}

zx::result<fbl::Vector<uint8_t>> VirtioGpuDevice::GetDisplayEdid(uint32_t scanout_id) {
  if ((pci_device().Features() & virtio_abi::GpuDeviceFeatures::kGpuEdid) == 0) {
    // EDID support is optional, and this driver can work without it.
    fdf::trace("virtio implementation does not support EDID");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  const virtio_abi::GetExtendedDisplayIdCommand command = {
      .header = {.type = virtio_abi::ControlType::kGetExtendedDisplayIdCommand},
      .scanout_id = scanout_id,
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::ExtendedDisplayIdResponse>(
          command);
  if (response.header.type != virtio_abi::ControlType::kExtendedDisplayIdResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }

  if (response.edid_size > virtio_abi::ExtendedDisplayIdResponse::kMaxEdidSize) {
    fdf::error("Reported EDID size {} exceeds maximum supported size {}", response.edid_size,
               virtio_abi::ExtendedDisplayIdResponse::kMaxEdidSize);
    return zx::error(ZX_ERR_IO);
  }
  const std::span<const uint8_t> response_edid_bytes(response.edid_bytes, response.edid_size);

  fbl::AllocChecker alloc_checker;
  fbl::Vector<uint8_t> edid_bytes;
  edid_bytes.resize(response_edid_bytes.size(), &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for EDID bytes");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  std::ranges::copy(response_edid_bytes, edid_bytes.begin());
  ZX_DEBUG_ASSERT(edid_bytes.size() == response_edid_bytes.size());

  return zx::ok(std::move(edid_bytes));
}

zx::result<uint32_t> VirtioGpuDevice::Create2DResource(uint32_t width, uint32_t height,
                                                       display::PixelFormat pixel_format) {
  fdf::trace("Allocate2DResource");

  std::optional<virtio_abi::ResourceFormat> resource_format =
      PixelFormatToVirtioResourceFormat(pixel_format);
  if (!resource_format.has_value()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  const virtio_abi::Create2DResourceCommand command = {
      .header = {.type = virtio_abi::ControlType::kCreate2DResourceCommand},
      .resource_id = next_resource_id_++,
      .format = *resource_format,
      .width = width,
      .height = height,
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok(command.resource_id);
}

zx::result<uint32_t> VirtioGpuDevice::CreateBlobResource(zx_paddr_t ptr, uint32_t size) {
  fdf::trace("CreateBlobResource");

  const virtio_abi::CreateBlobResourceCommand<1> command = {
      .header = {.type = virtio_abi::ControlType::kCreateBlobCommand},
      .resource_id = next_resource_id_++,
      .blob_mem = static_cast<uint32_t>(virtio_abi::BlobMem::kGuest),
      .blob_flags = static_cast<uint32_t>(virtio_abi::BlobFlags::kUseShareable),
      .blob_id = 0,  // needed only for Host3D/Host3D_Guest
      .size = size,
      .entries =
          {
              {.address = ptr, .length = static_cast<uint32_t>(size)},
          },
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::error("CreateBlobResource - Unexpected response type: {} (0x{:04x})",
               ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }

  return zx::ok(command.resource_id);
}

zx::result<> VirtioGpuDevice::AttachResourceBacking(uint32_t resource_id, zx_paddr_t ptr,
                                                    size_t buf_len) {
  ZX_ASSERT(ptr);

  fdf::trace("AttachResourceBacking - resource ID {}, address 0x{:x}, length {}", resource_id, ptr,
             buf_len);

  const virtio_abi::AttachResourceBackingCommand<1> command = {
      .header = {.type = virtio_abi::ControlType::kAttachResourceBackingCommand},
      .resource_id = resource_id,
      .entries =
          {
              {.address = ptr, .length = static_cast<uint32_t>(buf_len)},
          },
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok();
}

zx::result<> VirtioGpuDevice::SetScanoutProperties(uint32_t scanout_id, uint32_t resource_id,
                                                   uint32_t width, uint32_t height) {
  fdf::trace("SetScanoutProperties - scanout ID {}, resource ID {}, size {}x{}", scanout_id,
             resource_id, width, height);

  const virtio_abi::SetScanoutCommand command = {
      .header = {.type = virtio_abi::ControlType::kSetScanoutCommand},
      .image_source = {.x = 0, .y = 0, .width = width, .height = height},
      .scanout_id = scanout_id,
      .resource_id = resource_id,
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok();
}

zx::result<> VirtioGpuDevice::SetScanoutBlob(uint32_t scanout_id, uint32_t resource_id,
                                             virtio_abi::ResourceFormat resource_format,
                                             uint32_t width, uint32_t height, uint32_t stride) {
  fdf::trace("SetScanoutBlob - scanout ID {}, resource ID {}, size {}x{}", scanout_id, resource_id,
             width, height);

  const virtio_abi::SetScanoutBlobCommand command = {
      .header = {.type = virtio_abi::ControlType::kSetScanoutBlobCommand},
      .image_source = {.x = 0, .y = 0, .width = width, .height = height},
      .scanout_id = scanout_id,
      .resource_id = resource_id,
      .width = width,
      .height = height,
      .format = static_cast<uint32_t>(resource_format),
      .padding = 0,
      .strides = {stride, 0, 0, 0},
      .offsets = {0, 0, 0, 0},
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok();
}

zx::result<> VirtioGpuDevice::FlushResource(uint32_t resource_id, uint32_t width, uint32_t height) {
  fdf::trace("FlushResource - resource ID {}, size {}x{}", resource_id, width, height);

  virtio_abi::FlushResourceCommand command = {
      .header = {.type = virtio_abi::ControlType::kFlushResourceCommand},
      .image_source = {.x = 0, .y = 0, .width = width, .height = height},
      .resource_id = resource_id,
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok();
}

zx::result<> VirtioGpuDevice::TransferToHost2D(uint32_t resource_id, uint32_t width,
                                               uint32_t height) {
  fdf::trace("Transfer2DResourceToHost - resource ID {}, size {}x{}", resource_id, width, height);

  virtio_abi::Transfer2DResourceToHostCommand command = {
      .header = {.type = virtio_abi::ControlType::kTransfer2DResourceToHostCommand},
      .image_source = {.x = 0, .y = 0, .width = width, .height = height},
      .destination_offset = 0,
      .resource_id = resource_id,
  };

  const auto& response =
      virtio_device_->ExchangeControlqRequestResponse<virtio_abi::EmptyResponse>(command);
  if (response.header.type != virtio_abi::ControlType::kEmptyResponse) {
    fdf::error("Unexpected response type: {} (0x{:04x})", ControlTypeToString(response.header.type),
               static_cast<uint32_t>(response.header.type));
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok();
}

}  // namespace virtio_display
