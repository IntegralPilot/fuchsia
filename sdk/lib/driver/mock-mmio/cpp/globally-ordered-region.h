// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_MOCK_MMIO_CPP_GLOBALLY_ORDERED_REGION_H_
#define LIB_DRIVER_MOCK_MMIO_CPP_GLOBALLY_ORDERED_REGION_H_

#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/stdcompat/span.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <cstdint>
#include <mutex>
#include <vector>

namespace mock_mmio {

// An MMIO range that responds to a list of pre-determined memory accesses.
//
// GloballyOrderedRegion enforces a global ordering on all accesses to the mocked MMIO
// range. This is stricter than Region, which accepts any interleaving of the access
// lists specified at the register level. So, GloballyOrderedRegion results in more brittle
// mocks, and should only be used when there is a single acceptable access ordering.
//
// Example usage:
//   constexpr static size_t kMmioRegionSize = 0x4000;
//   GloballyOrderedRegion region_{kMmioRegionSize, GloballyOrderedRegion::Size::k32};
//   fdf::MmioBuffer buffer_{region_.GetMmioBuffer()};
//
//   // Expect a 32-bit read at 0x1000, the read will return 0x12345678.
//   region_.Expect({.address = 0x1000, .value = 0x12345678});
//   // Expect a 32-bit write of 0x87654321 at 0x1002
//   region_.Expect({.address = 0x1002, .value = 0x87654321, .write = true});
//
//   // Test polling for a ready flag at 0x1004.
//   region_.Expect(GloballyOrderedRegion::AccessList({
//       {.address = 0x1004, .value = 0x0},
//       {.address = 0x1004, .value = 0x0},
//       {.address = 0x1004, .value = 0x0},
//       {.address = 0x1004, .value = 0x1},
//   }));
//
//   // This could go in TearDown().
//   region_.CheckAllAccessesReplayed();
//
// The following practices are not required, but are consistent with the
// recommendation of keeping testing logic simple:
//
// * Expect() calls should be at the beginning of the test case, before
//   executing the code that accesses the MMIO region.
// * A test's expectations should be grouped in a single Expect() call. In rare
//   cases, multiple cases and conditional logic may improve readability.
// * Expect() should not be called concurrently from multiple threads.
//
// GloballyOrderedRegion instances are 100% thread-safe because all MMIO accesses to the region are
// serialized using a mutex.

class GloballyOrderedRegion {
 public:
  // The supported MMIO access sizes.
  enum class Size {
    kUseDefault = 0,
    k8 = 8,    // fdf::MmioBuffer::Read8(), fdf::MmioBuffer::Write8().
    k16 = 16,  // fdf::MmioBuffer::Read16(), fdf::MmioBuffer::Write16().
    k32 = 32,  // fdf::MmioBuffer::Read32(), fdf::MmioBuffer::Write32().
    k64 = 64,  // fdf::MmioBuffer::Read64(), fdf::MmioBuffer::Write64().
  };

  // Information about an expected MMIO access. Passed into Expect().
  struct Access {
    zx_off_t address;
    uint64_t value;  // Expected by writes, returned by reads.
    bool write = false;
    Size size = Size::kUseDefault;  // Use default value size.
  };

  // Alias for conveniently calling Expect() with multiple accesses.
  using AccessList = cpp20::span<const Access>;

  // `default_access_size` is used for Access instances whose `size` is
  // `kUseDefault`.
  explicit GloballyOrderedRegion(size_t region_size, Size default_access_size = Size::k32)
      : region_size_(region_size), default_access_size_(default_access_size) {}
  ~GloballyOrderedRegion() = default;

  // Appends an entry to the list of expected memory accesses.
  //
  // To keep the testing logic simple, all Expect() calls should be performed
  // before executing the code that uses the MMIO range.
  void Expect(const Access& access) { Expect(cpp20::span<const Access>({access})); }

  // Appends the given entries to the list of expected memory accesses.
  //
  // To keep the testing logic simple, all Expect() calls should be performed
  // before executing the code that uses the MMIO range.
  void Expect(cpp20::span<const Access> accesses);

  // Asserts that the entire memory access list has been replayed.
  void CheckAllAccessesReplayed();

  // Constructs and returns a MmioBuffer object with a size that matches GloballyOrderedRegion.
  fdf::MmioBuffer GetMmioBuffer();

 private:
  // MmioBufferOps implementation.
  static uint8_t Read8(const void* ctx, const mmio_buffer_t&, zx_off_t offset) {
    return static_cast<uint8_t>(
        static_cast<const GloballyOrderedRegion*>(ctx)->Read(offset, Size::k8));
  }
  static uint16_t Read16(const void* ctx, const mmio_buffer_t&, zx_off_t offset) {
    return static_cast<uint16_t>(
        static_cast<const GloballyOrderedRegion*>(ctx)->Read(offset, Size::k16));
  }
  static uint32_t Read32(const void* ctx, const mmio_buffer_t&, zx_off_t offset) {
    return static_cast<uint32_t>(
        static_cast<const GloballyOrderedRegion*>(ctx)->Read(offset, Size::k32));
  }
  static uint64_t Read64(const void* ctx, const mmio_buffer_t&, zx_off_t offset) {
    return static_cast<const GloballyOrderedRegion*>(ctx)->Read(offset, Size::k64);
  }
  static void Write8(const void* ctx, const mmio_buffer_t&, uint8_t value, zx_off_t offset) {
    static_cast<const GloballyOrderedRegion*>(ctx)->Write(offset, value, Size::k8);
  }
  static void Write16(const void* ctx, const mmio_buffer_t&, uint16_t value, zx_off_t offset) {
    static_cast<const GloballyOrderedRegion*>(ctx)->Write(offset, value, Size::k16);
  }
  static void Write32(const void* ctx, const mmio_buffer_t&, uint32_t value, zx_off_t offset) {
    static_cast<const GloballyOrderedRegion*>(ctx)->Write(offset, value, Size::k32);
  }
  static void Write64(const void* ctx, const mmio_buffer_t&, uint64_t value, zx_off_t offset) {
    static_cast<const GloballyOrderedRegion*>(ctx)->Write(offset, value, Size::k64);
  }

  uint64_t Read(zx_off_t address, Size size) const;

  void Write(zx_off_t address, uint64_t value, Size size) const;

  mutable std::mutex mutex_;
  mutable std::vector<Access> access_list_ __TA_GUARDED(mutex_);
  mutable size_t access_index_ __TA_GUARDED(mutex_) = 0;
  const size_t region_size_;
  const Size default_access_size_;
};

}  // namespace mock_mmio

#endif  // LIB_DRIVER_MOCK_MMIO_CPP_GLOBALLY_ORDERED_REGION_H_
