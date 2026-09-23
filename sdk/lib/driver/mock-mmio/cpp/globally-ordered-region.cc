// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mock-mmio/cpp/globally-ordered-region.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <cstdint>

namespace mock_mmio {

namespace {

fdf::MmioBuffer CreateMmioBuffer(size_t size, uint32_t cache_policy, const fdf::MmioBufferOps* ops,
                                 void* ctx) {
  zx::vmo vmo;
  ZX_ASSERT(zx::vmo::create(/*size=*/size, 0, &vmo) == ZX_OK);
  mmio_buffer_t mmio{};
  ZX_ASSERT(mmio_buffer_init(&mmio, 0, size, vmo.release(), cache_policy) == ZX_OK);
  return fdf::MmioBuffer(mmio, ops, ctx);
}

int SizeInt(GloballyOrderedRegion::Size access_size) { return static_cast<int>(access_size); }

}  // namespace

void GloballyOrderedRegion::Expect(cpp20::span<const GloballyOrderedRegion::Access> accesses) {
  const std::lock_guard<std::mutex> lock(mutex_);
  for (const auto& access : accesses) {
    access_list_.push_back({
        .address = access.address,
        .value = access.value,
        .write = access.write,
        .size = access.size == GloballyOrderedRegion::Size::kUseDefault ? default_access_size_
                                                                        : access.size,
    });
  }
}

void GloballyOrderedRegion::CheckAllAccessesReplayed() {
  const std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(access_list_.size() == access_index_,
                "Expected %zu MMIO accesses, only received %zu", access_list_.size(),
                access_index_);
}

fdf::MmioBuffer GloballyOrderedRegion::GetMmioBuffer() {
  static constexpr fdf::MmioBufferOps kMockMmioOps = {
      .Read8 = GloballyOrderedRegion::Read8,
      .Read16 = GloballyOrderedRegion::Read16,
      .Read32 = GloballyOrderedRegion::Read32,
      .Read64 = GloballyOrderedRegion::Read64,
      .Write8 = GloballyOrderedRegion::Write8,
      .Write16 = GloballyOrderedRegion::Write16,
      .Write32 = GloballyOrderedRegion::Write32,
      .Write64 = GloballyOrderedRegion::Write64,
  };

  return CreateMmioBuffer(region_size_, ZX_CACHE_POLICY_CACHED, &kMockMmioOps, this);
}

uint64_t GloballyOrderedRegion::Read(zx_off_t address, GloballyOrderedRegion::Size size) const {
  const std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(access_index_ < access_list_.size(),
                "Unexpected MMIO access after all expected accesses completed. "
                "Got %d-byte read at address 0x%" PRIx64,
                SizeInt(size), uint64_t{address});

  const GloballyOrderedRegion::Access& expected_access = access_list_[access_index_];
  ++access_index_;

  ZX_ASSERT_MSG(expected_access.address == address,
                "Expected %d-byte %s of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64
                ", got %d-byte read at address 0x%" PRIx64,
                SizeInt(expected_access.size), expected_access.write ? "write" : "read",
                expected_access.value, expected_access.value, uint64_t{expected_access.address},
                SizeInt(size), uint64_t{address});

  ZX_ASSERT_MSG(expected_access.write == false,
                "Expected %d-byte %s of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64
                ", got %d-byte read at address 0x%" PRIx64,
                SizeInt(expected_access.size), expected_access.write ? "write" : "read",
                expected_access.value, expected_access.value, uint64_t{expected_access.address},
                SizeInt(size), uint64_t{address});

  ZX_ASSERT_MSG(expected_access.size == size,
                "Expected %d-byte %s of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64
                ", got %d-byte read at address 0x%" PRIx64,
                SizeInt(expected_access.size), expected_access.write ? "write" : "read",
                expected_access.value, expected_access.value, uint64_t{expected_access.address},
                SizeInt(size), uint64_t{address});
  return expected_access.value;
}

void GloballyOrderedRegion::Write(zx_off_t address, uint64_t value,
                                  GloballyOrderedRegion::Size size) const {
  const std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(access_index_ < access_list_.size(),
                "Unexpected MMIO access after all expected accesses completed. "
                "Got %d-byte write of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64,
                SizeInt(size), value, value, uint64_t{address});

  const GloballyOrderedRegion::Access& expected_access = access_list_[access_index_];
  ++access_index_;

  ZX_ASSERT_MSG(expected_access.address == address,
                "Expected %d-byte %s of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64
                ", got %d-byte write of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64,
                SizeInt(expected_access.size), expected_access.write ? "write" : "read",
                expected_access.value, expected_access.value, uint64_t{expected_access.address},
                SizeInt(size), value, value, uint64_t{address});

  ZX_ASSERT_MSG(expected_access.write == true,
                "Expected %d-byte %s of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64
                ", got %d-byte write of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64,
                SizeInt(expected_access.size), expected_access.write ? "write" : "read",
                expected_access.value, expected_access.value, uint64_t{expected_access.address},
                SizeInt(size), value, value, uint64_t{address});

  ZX_ASSERT_MSG(expected_access.size == size,
                "Expected %d-byte %s of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64
                ", got %d-byte write of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64,
                SizeInt(expected_access.size), expected_access.write ? "write" : "read",
                expected_access.value, expected_access.value, uint64_t{expected_access.address},
                SizeInt(size), value, value, uint64_t{address});

  ZX_ASSERT_MSG(expected_access.value == value,
                "Expected %d-byte %s of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64
                ", got %d-byte write of %" PRIu64 " (0x%" PRIx64 ") at address 0x%" PRIx64,
                SizeInt(expected_access.size), expected_access.write ? "write" : "read",
                expected_access.value, expected_access.value, uint64_t{expected_access.address},
                SizeInt(size), value, value, uint64_t{address});
}

}  // namespace mock_mmio
