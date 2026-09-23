// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_MMIO_CPP_MMIO_INTERNAL_H_
#define LIB_DRIVER_MMIO_CPP_MMIO_INTERNAL_H_

#include <lib/mmio-ptr/mmio-ptr.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include "mmio-ops.h"

#ifdef __cplusplus

#if defined(__aarch64__)
#define mmio_hw_mb() __asm__ volatile("dmb sy" : : : "memory")
#elif defined(__x86_64__)
#define mmio_hw_mb() __asm__ volatile("mfence" ::: "memory")
#elif defined(__riscv)
#define mmio_hw_mb() __asm__ volatile("fence iorw,iorw" ::: "memory")
#endif

namespace fdf::internal {

template <typename T>
static MMIO_PTR volatile T* GetAddr(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  ZX_DEBUG_ASSERT(offs + sizeof(T) <= mmio.size);
  const uintptr_t ptr = reinterpret_cast<uintptr_t>(mmio.vaddr);
  ZX_DEBUG_ASSERT(ptr);
  return reinterpret_cast<MMIO_PTR T*>(ptr + offs);
}

static uint8_t Read8(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  return MmioRead8(GetAddr<uint8_t>(ctx, mmio, offs));
}

static uint16_t Read16(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  return MmioRead16(GetAddr<uint16_t>(ctx, mmio, offs));
}

static uint32_t Read32(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  return MmioRead32(GetAddr<uint32_t>(ctx, mmio, offs));
}

static uint64_t Read64(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  return MmioRead64(GetAddr<uint64_t>(ctx, mmio, offs));
}

static void ReadBuffer(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs, void* buffer,
                       size_t size) {
  ZX_DEBUG_ASSERT(offs + size <= mmio.size);
  return MmioReadBuffer(buffer, GetAddr<uint64_t>(ctx, mmio, offs), size);
}

static void Write8(const void* ctx, const mmio_buffer_t& mmio, uint8_t val, zx_off_t offs) {
  MmioWrite8(val, GetAddr<uint8_t>(ctx, mmio, offs));
  mmio_hw_mb();
}

static void Write16(const void* ctx, const mmio_buffer_t& mmio, uint16_t val, zx_off_t offs) {
  MmioWrite16(val, GetAddr<uint16_t>(ctx, mmio, offs));
  mmio_hw_mb();
}

static void Write32(const void* ctx, const mmio_buffer_t& mmio, uint32_t val, zx_off_t offs) {
  MmioWrite32(val, GetAddr<uint32_t>(ctx, mmio, offs));
  mmio_hw_mb();
}

static void Write64(const void* ctx, const mmio_buffer_t& mmio, uint64_t val, zx_off_t offs) {
  MmioWrite64(val, GetAddr<uint64_t>(ctx, mmio, offs));
  mmio_hw_mb();
}

static void WriteBuffer(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs,
                        const void* buffer, size_t size) {
  ZX_DEBUG_ASSERT(offs + size <= mmio.size);
  MmioWriteBuffer(GetAddr<uint64_t>(ctx, mmio, offs), buffer, size);
  mmio_hw_mb();
}

static constexpr MmioBufferOps kDefaultOps = {
    .Read8 = Read8,
    .Read16 = Read16,
    .Read32 = Read32,
    .Read64 = Read64,
    .ReadBuffer = ReadBuffer,
    .Write8 = Write8,
    .Write16 = Write16,
    .Write32 = Write32,
    .Write64 = Write64,
    .WriteBuffer = WriteBuffer,
};

#undef mmio_hw_mb

}  // namespace fdf::internal

#endif

#endif  // LIB_DRIVER_MMIO_CPP_MMIO_INTERNAL_H_
