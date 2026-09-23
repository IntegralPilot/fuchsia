// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

zx_status_t mmio_buffer_pin(mmio_buffer_t* buffer, zx_handle_t bti, mmio_pinned_buffer_t* out) {
  zx_paddr_t paddr;
  zx_handle_t pmt;
  const uint32_t options = ZX_BTI_PERM_WRITE | ZX_BTI_PERM_READ | ZX_BTI_CONTIGUOUS;
  const size_t vmo_offset = MMIO_ROUNDDOWN(buffer->offset, zx_system_get_page_size());
  const size_t page_offset = buffer->offset - vmo_offset;
  const size_t vmo_size = MMIO_ROUNDUP(buffer->size + page_offset, zx_system_get_page_size());

  zx_status_t status = zx_bti_pin(bti, options, buffer->vmo, vmo_offset, vmo_size, &paddr, 1, &pmt);
  if (status != ZX_OK) {
    return status;
  }

  out->mmio = buffer;
  out->paddr = paddr + page_offset;
  out->pmt = pmt;

  return ZX_OK;
}

void mmio_buffer_unpin(mmio_pinned_buffer_t* buffer) {
  if (buffer->pmt != ZX_HANDLE_INVALID) {
    zx_pmt_unpin(buffer->pmt);
    buffer->pmt = ZX_HANDLE_INVALID;
  }
}
