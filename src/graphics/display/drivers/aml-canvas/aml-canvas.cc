// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/aml-canvas/aml-canvas.h"

#include <lib/driver/compat/cpp/logging.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zx/bti.h>
#include <safemath/checked_math.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <bit>
#include <cstdint>
#include <string>
#include <type_traits>
#include <utility>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>

#include "src/graphics/display/drivers/aml-canvas/dmc-regs.h"

namespace aml_canvas {

namespace {

template <typename T, typename _ = std::enable_if<std::is_unsigned_v<T>>>
constexpr bool IsAligned(T address_or_size, T alignment) {
  ZX_DEBUG_ASSERT(std::has_single_bit(alignment));

  const T alignment_mask = alignment - 1;
  return (address_or_size & alignment_mask) == 0;
}

}  // namespace

AmlCanvas::AmlCanvas(fdf::MmioBuffer mmio, zx::bti bti, inspect::Inspector inspector)
    : inspector_(std::move(inspector)), dmc_regs_(std::move(mmio)), bti_(std::move(bti)) {
  inspect_root_ = inspector_.GetRoot().CreateChild("aml-canvas");
}

AmlCanvas::~AmlCanvas() {
  fbl::AutoLock lock(&lock_);
  for (uint32_t index = 0; index < kNumCanvasEntries; index++) {
    entries_[index] = CanvasEntry();
  }
}

void AmlCanvas::Config(ConfigRequestView request, ConfigCompleter::Sync& completer) {
  fuchsia_hardware_amlogiccanvas::wire::CanvasInfo* info = &(request->info);
  zx::vmo vmo = std::move(request->vmo);
  uint64_t offset = request->offset;

  uint32_t page_size = zx_system_get_page_size();

  // Use checked arithmetic to detect overflow in the canvas size computation.
  // stride_bytes and height are uint32_t values from untrusted FIDL input;
  // their product can wrap in uint32_t (e.g., 0x80000000 * 8 = 0x400000000,
  // which wraps to 0). The wrapped value would pin a tiny DMA region while
  // the hardware is programmed with the original dimensions, allowing the DMA
  // engine to read or write physical memory beyond the pinned region.
  uint32_t image_size_bytes;
  if (!safemath::CheckMul(info->stride_bytes, info->height)
           .AssignIfValid(&image_size_bytes)) {
    zxlogf(ERROR, "Canvas size overflow: %u * %u bytes",
           info->stride_bytes, info->height);
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  uint32_t buffer_size_bytes;
  if (!safemath::CheckAdd(image_size_bytes,
                static_cast<uint32_t>(offset % page_size))
           .AssignIfValid(&buffer_size_bytes)) {
    zxlogf(ERROR, "Canvas size overflow adding offset: %u + %lu",
           image_size_bytes, offset % page_size);
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }
  uint32_t size = fbl::round_up<uint32_t, uint32_t>(buffer_size_bytes, page_size);
  uint32_t index;

  uint32_t height = info->height;
  uint32_t width = info->stride_bytes;

  if (!(info->flags & fuchsia_hardware_amlogiccanvas::CanvasFlags::kWrapVertical)) {
    // The precise height of the canvas doesn't matter if wrapping isn't in
    // use (as long as the user doesn't try to read or write outside of
    // the defined area).
    height = fbl::round_up(height, uint32_t{8});
  }

  if (!IsAligned(height, uint32_t{8}) || !IsAligned(width, uint32_t{8})) {
    zxlogf(ERROR, "Height or width not a multiple of 8");
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  // find an unused canvas index
  fbl::AutoLock al(&lock_);
  for (index = 0; index < kNumCanvasEntries; index++) {
    if (!entries_[index].pmt.is_valid()) {
      break;
    }
  }
  if (index == kNumCanvasEntries) {
    zxlogf(ERROR, "All canvas indices are currently in use");
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  uint32_t pin_flags = ZX_BTI_CONTIGUOUS;
  if (info->flags & fuchsia_hardware_amlogiccanvas::CanvasFlags::kRead) {
    pin_flags |= ZX_BTI_PERM_READ;
  }
  if (info->flags & fuchsia_hardware_amlogiccanvas::CanvasFlags::kWrite) {
    pin_flags |= ZX_BTI_PERM_WRITE;
  }

  zx::pmt pmt;
  zx_paddr_t paddr;
  zx_status_t status = bti_.pin(pin_flags, vmo, fbl::round_down<size_t, size_t>(offset, page_size),
                                size, &paddr, 1, &pmt);
  if (status != ZX_OK) {
    zxlogf(ERROR, "zx_bti_pin() failed: %s", zx_status_get_string(status));
    completer.ReplyError(status);
    return;
  }

  if (!IsAligned(paddr, zx_paddr_t{8})) {
    zxlogf(ERROR, "Physical address is not aligned\n");
    status = ZX_ERR_INVALID_ARGS;
    pmt.unpin();
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  CanvasEntry entry;
  entry.pmt = std::move(pmt);
  entry.vmo = std::move(vmo);
  entry.node = inspect_root_.CreateChild(std::to_string(index));
  entry.node.RecordUint("width", width);
  entry.node.RecordUint("height", height);
  entry.node.RecordUint("pin_flags", pin_flags);

  entries_[index] = std::move(entry);

  zx_paddr_t start_addr = paddr + (offset % page_size);

  // Populate the canvas entry that will be written.
  auto data_low = CanvasLutDataLow::Get().FromValue(0);
  data_low.SetDmcCavWidth(width >> 3);
  data_low.set_dmc_cav_addr(static_cast<unsigned int>(start_addr >> 3));
  data_low.WriteTo(&dmc_regs_);

  auto data_high = CanvasLutDataHigh::Get().FromValue(0);
  data_high.SetDmcCavWidth(width >> 3);
  data_high.set_dmc_cav_height(height);
  data_high.set_dmc_cav_blkmode(static_cast<uint32_t>(info->blkmode));
  data_high.set_dmc_cav_xwrap(
      info->flags & fuchsia_hardware_amlogiccanvas::CanvasFlags::kWrapHorizontal ? 1 : 0);
  data_high.set_dmc_cav_ywrap(
      info->flags & fuchsia_hardware_amlogiccanvas::CanvasFlags::kWrapVertical ? 1 : 0);
  data_high.set_dmc_cav_endianness(static_cast<uint32_t>(info->endianness));
  data_high.WriteTo(&dmc_regs_);

  auto lut_addr = CanvasLutAddr::Get().FromValue(0);
  lut_addr.set_dmc_cav_addr_index(index);
  lut_addr.set_dmc_cav_addr_wr(1);
  lut_addr.WriteTo(&dmc_regs_);

  // Perform a MMIO read posted to the DMC's configuration bus. When it
  // completes, the writes above were certainly flushed.
  CanvasLutDataHigh::Get().ReadFrom(&dmc_regs_);

  completer.ReplySuccess(static_cast<uint8_t>(index));
}

void AmlCanvas::Free(FreeRequestView request, FreeCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  auto& entry = entries_[request->canvas_idx];

  if (!entry.pmt.is_valid()) {
    zxlogf(ERROR, "Refusing to free invalid canvas index: %d", int{request->canvas_idx});
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  entry = CanvasEntry();
  completer.ReplySuccess();
}

zx_status_t AmlCanvas::ServeOutgoing(std::shared_ptr<fdf::OutgoingDirectory>& outgoing) {
  fuchsia_hardware_amlogiccanvas::Service::InstanceHandler handler({
      .device = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
  });
  auto result = outgoing->AddService<fuchsia_hardware_amlogiccanvas::Service>(std::move(handler));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to add amlogiccanvas service to the outgoing directory.");
    return result.status_value();
  }
  return ZX_OK;
}

}  // namespace aml_canvas
