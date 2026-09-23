// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>

namespace fake_mmio {

fdf::MmioBuffer FakeMmioRegRegion::GetMmioBuffer() {
  static constexpr fdf::MmioBufferOps kFakeMmioOps = {
      .Read8 = FakeMmioRegRegion::Read8,
      .Read16 = FakeMmioRegRegion::Read16,
      .Read32 = FakeMmioRegRegion::Read32,
      .Read64 = FakeMmioRegRegion::Read64,
      .Write8 = FakeMmioRegRegion::Write8,
      .Write16 = FakeMmioRegRegion::Write16,
      .Write32 = FakeMmioRegRegion::Write32,
      .Write64 = FakeMmioRegRegion::Write64,
  };

  size_t size = reg_size_ * reg_count_;
  zx::vmo vmo;
  ZX_ASSERT(zx::vmo::create(/*size=*/size, 0, &vmo) == ZX_OK);
  mmio_buffer_t mmio{};
  ZX_ASSERT(mmio_buffer_init(&mmio, 0, size, vmo.release(), ZX_CACHE_POLICY_CACHED) == ZX_OK);
  return fdf::MmioBuffer(mmio, &kFakeMmioOps, this);
}

uint8_t FakeMmioRegRegion::Read8(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  auto& reg_region = *reinterpret_cast<FakeMmioRegRegion*>(const_cast<void*>(ctx));
  return static_cast<uint8_t>(reg_region[offs + mmio.offset].Read());
}

uint16_t FakeMmioRegRegion::Read16(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  auto& reg_region = *reinterpret_cast<FakeMmioRegRegion*>(const_cast<void*>(ctx));
  return static_cast<uint16_t>(reg_region[offs + mmio.offset].Read());
}

uint32_t FakeMmioRegRegion::Read32(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  auto& reg_region = *reinterpret_cast<FakeMmioRegRegion*>(const_cast<void*>(ctx));
  return static_cast<uint32_t>(reg_region[offs + mmio.offset].Read());
}

uint64_t FakeMmioRegRegion::Read64(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
  auto& reg_region = *reinterpret_cast<FakeMmioRegRegion*>(const_cast<void*>(ctx));
  return reg_region[offs + mmio.offset].Read();
}

void FakeMmioRegRegion::Write8(const void* ctx, const mmio_buffer_t& mmio, uint8_t val,
                               zx_off_t offs) {
  Write64(ctx, mmio, val, offs);
}

void FakeMmioRegRegion::Write16(const void* ctx, const mmio_buffer_t& mmio, uint16_t val,
                                zx_off_t offs) {
  Write64(ctx, mmio, val, offs);
}

void FakeMmioRegRegion::Write32(const void* ctx, const mmio_buffer_t& mmio, uint32_t val,
                                zx_off_t offs) {
  Write64(ctx, mmio, val, offs);
}

void FakeMmioRegRegion::Write64(const void* ctx, const mmio_buffer_t& mmio, uint64_t val,
                                zx_off_t offs) {
  auto& reg_region = *reinterpret_cast<FakeMmioRegRegion*>(const_cast<void*>(ctx));
  reg_region[offs + mmio.offset].Write(val);
}

}  // namespace fake_mmio
