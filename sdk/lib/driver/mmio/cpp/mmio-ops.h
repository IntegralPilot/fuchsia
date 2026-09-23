// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_MMIO_CPP_MMIO_OPS_H_
#define LIB_DRIVER_MMIO_CPP_MMIO_OPS_H_

#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

typedef struct {
  // |vaddr| points to the content starting at |offset| in |vmo|.
  MMIO_PTR void* vaddr;
  zx_off_t offset;
  size_t size;
  zx_handle_t vmo;
} mmio_buffer_t;

__END_CDECLS

#ifdef __cplusplus
namespace fdf {
struct MmioBufferOps {
  uint8_t (*Read8)(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);
  uint16_t (*Read16)(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);
  uint32_t (*Read32)(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);
  uint64_t (*Read64)(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);
  void (*ReadBuffer)(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs, void* buffer,
                     size_t size);

  void (*Write8)(const void* ctx, const mmio_buffer_t& mmio, uint8_t val, zx_off_t offs);
  void (*Write16)(const void* ctx, const mmio_buffer_t& mmio, uint16_t val, zx_off_t offs);
  void (*Write32)(const void* ctx, const mmio_buffer_t& mmio, uint32_t val, zx_off_t offs);
  void (*Write64)(const void* ctx, const mmio_buffer_t& mmio, uint64_t val, zx_off_t offs);
  void (*WriteBuffer)(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs, const void* buffer,
                      size_t size);
};
}  // namespace fdf
#endif

#endif  // LIB_DRIVER_MMIO_CPP_MMIO_OPS_H_
