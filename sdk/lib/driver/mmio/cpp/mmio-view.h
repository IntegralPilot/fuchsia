// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_MMIO_CPP_MMIO_VIEW_H_
#define LIB_DRIVER_MMIO_CPP_MMIO_VIEW_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio-internal.h>
#include <lib/mmio-ptr/mmio-ptr.h>
#include <zircon/assert.h>

namespace fdf {

// A sliced view that of an mmio which does not unmap on close. Must not outlive
// mmio buffer it is created from.
class MmioView : public MmioBuffer {
 public:
  MmioView(const mmio_buffer_t& mmio, zx_off_t offset,
           const MmioBufferOps* ops = &internal::kDefaultOps, const void* ctx = nullptr)
      : MmioBuffer(
            mmio_buffer_t{
                .vaddr = static_cast<MMIO_PTR uint8_t*>(mmio.vaddr) + offset,
                .offset = mmio.offset + offset,
                .size = mmio.size - offset,
                .vmo = mmio.vmo,
            },
            ops, ctx) {
    ZX_ASSERT(offset < mmio.size);
  }

  MmioView(const mmio_buffer_t& mmio, zx_off_t offset, size_t size,
           const MmioBufferOps* ops = &internal::kDefaultOps, const void* ctx = nullptr)
      : MmioBuffer(
            mmio_buffer_t{
                .vaddr = static_cast<MMIO_PTR uint8_t*>(mmio.vaddr) + offset,
                .offset = mmio.offset + offset,
                .size = size,
                .vmo = mmio.vmo,
            },
            ops, ctx) {
    ZX_ASSERT(size + offset <= mmio.size);
  }

  MmioView(const MmioView& mmio) : MmioBuffer(mmio.mmio_, mmio.ops_, mmio.ctx_) {}

  virtual ~MmioView() override {
    // Prevent unmap operation from occurring.
    mmio_.vmo = ZX_HANDLE_INVALID;
  }
};

// These can't be defined inside the class because they need MmioView
// to be completely defined first.

inline MmioView MmioBuffer::View(zx_off_t off) const { return MmioView(mmio_, off, ops_, ctx_); }

inline MmioView MmioBuffer::View(zx_off_t off, size_t size) const {
  return MmioView(mmio_, off, size, ops_, ctx_);
}

}  // namespace fdf

#endif  // LIB_DRIVER_MMIO_CPP_MMIO_VIEW_H_
