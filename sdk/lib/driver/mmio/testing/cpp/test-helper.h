// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef LIB_DRIVER_MMIO_TESTING_CPP_TEST_HELPER_H_
#define LIB_DRIVER_MMIO_TESTING_CPP_TEST_HELPER_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <variant>

namespace fdf_testing {
// This provides a helper to quickly create an MmioBuffer for use in tests.
// TODO(https://fxbug.dev/42067247): This use of mmio_buffer_t is temporary to ease the
// transition of clients over to MmioBuffer, and can be switched over once we
// have no unmigrated users touching MmioBufferOps.
[[maybe_unused]] static fdf::MmioBuffer CreateMmioBuffer(
    size_t size, uint32_t cache_policy = ZX_CACHE_POLICY_UNCACHED_DEVICE,
    const ::fdf::MmioBufferOps* ops = &::fdf::internal::kDefaultOps, void* ctx = nullptr) {
  zx::vmo vmo;
  ZX_ASSERT(zx::vmo::create(/*size=*/size, 0, &vmo) == ZX_OK);
  mmio_buffer_t mmio{};
  ZX_ASSERT(mmio_buffer_init(&mmio, 0, size, vmo.release(), cache_policy) == ZX_OK);
  return fdf::MmioBuffer(mmio, ops, ctx);
}

[[maybe_unused]] static fdf::MmioBuffer CreateMmioBuffer(
    zx::vmo vmo, uint32_t cache_policy = ZX_CACHE_POLICY_UNCACHED_DEVICE,
    const ::fdf::MmioBufferOps* ops = &::fdf::internal::kDefaultOps, void* ctx = nullptr) {
  zx_info_vmo_t info{};
  ZX_ASSERT(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), /*actual_count=*/0, /*avail_count=*/0) ==
            ZX_OK);
  mmio_buffer_t mmio{};
  ZX_ASSERT(mmio_buffer_init(&mmio, 0, info.size_bytes, vmo.release(), cache_policy) == ZX_OK);
  return fdf::MmioBuffer(mmio, ops, ctx);
}

}  // namespace fdf_testing

#endif  // LIB_DRIVER_MMIO_TESTING_CPP_TEST_HELPER_H_
