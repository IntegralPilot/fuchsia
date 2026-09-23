// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_EARLY_BOOT_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_EARLY_BOOT_H_

#include <lib/zbitl/storage-traits.h>
#include <lib/zbitl/view.h>
#include <zircon/compiler.h>

#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <ktl/type_traits.h>
#include <ktl/utility.h>
#include <phys/arch/early-boot.h>

// EarlyBootZbi represents a data ZBI in early ZBI boot when caches might be
// left disabled, during which access is guaranteed to be coherent with any
// disabled cache state. EarlyBootZbi is stateful and should be passed by
// reference are moved before calling InitMemory(), at which point an instance
// should be consumed with the class no longer being used past that point.

// EarlyBootZbi is zbitl::View<EarlyBootZbiBytes*>.  This is its storage type.
class EarlyBootZbiBytes {
 public:
  constexpr EarlyBootZbiBytes() = default;

  constexpr explicit EarlyBootZbiBytes(ktl::span<const ktl::byte> bytes) : bytes_(bytes) {}

  explicit EarlyBootZbiBytes(const void* zbi_ptr) {
    // We need to be able to read the ZBI container header in order to construct
    // the EarlyBootZbi (to determine its size).
    ArchEarlyBootSyncData(reinterpret_cast<uintptr_t>(zbi_ptr), sizeof(zbi_header_t));
    bytes_ = zbitl::StorageFromRawHeader(static_cast<const zbi_header_t*>(zbi_ptr));
  }

  // Returns the associated data, synchronizing it on the first call.
  // Subsequent calls will not re-synchronize and can be regarded as cheap.
  //
  // If these bytes represent a whole ZBI payload, then get() should not be
  // called (as that would defeat the intended minimal sync optimization
  // strategies of this abstraction).
  ktl::span<const ktl::byte> get() { return *(operator->()); }

  // Similar to get().
  ktl::span<const ktl::byte>* operator->() {
    if constexpr (kArchEarlyBootDataSynchronization) {
      if (auto unsynced = bytes_.subspan(synced_high_water_mark_); !unsynced.empty()) {
        // get() should only be called on item payloads. The water mark for
        // payloads should only ever be at the beginning or end of the range,
        // and should only be otherwise in the case of a whole ZBI.
        ZX_DEBUG_ASSERT_MSG(synced_high_water_mark_ == 0, "get() called on a whole ZBI payload?");

        ArchEarlyBootSyncData(reinterpret_cast<uintptr_t>(unsynced.data()), unsynced.size());
        synced_high_water_mark_ = bytes_.size_bytes();
      }
    }
    return &bytes_;
  }

 private:
  // Allows the traits to access the watermark, as well as the raw bytes
  // without sync'ing the entire range, which should only occur on reads
  // or direct payload usage.
  friend struct zbitl::StorageTraits<EarlyBootZbiBytes*>;

  // Intended to be compiled away in the case where we do not need to prepare
  // data access and track a synced water mark at all.
  struct NullWaterMark {
    constexpr explicit NullWaterMark(size_t) {}
    constexpr NullWaterMark& operator=(size_t) { return *this; }
    constexpr operator size_t() const { return 0; }
  };

  ktl::span<const ktl::byte> bytes_;

  // In the case where we actually need data access syncing this is a measure
  // of how far we've sync'ed so far, but it means different things in
  // practice for ZBI bytes vs. item payload bytes: for ZBI bytes, we only sync
  // header subranges, meaning the water mark is the max header offset synced
  // so far; for item payload bytes, we leave sync'ing to the caller at the
  // water mark should only ever be 0 or at the end.
  __NO_UNIQUE_ADDRESS
  ktl::conditional_t<kArchEarlyBootDataSynchronization, size_t, NullWaterMark>
      synced_high_water_mark_{0};
};

template <>
struct zbitl::StorageTraits<EarlyBootZbiBytes*> {
  using ByteTraits = zbitl::StorageTraits<ktl::span<const ktl::byte>>;

  using Storage = EarlyBootZbiBytes*;
  using payload_type = EarlyBootZbiBytes;
  using error_type = ByteTraits::error_type;

  static ktl::string_view error_string(error_type error) { return ByteTraits::error_string(error); }

  static fit::result<error_type, uint32_t> Capacity(Storage& zbi) {
    return ByteTraits::Capacity(zbi->bytes_);
  }

  static fit::result<error_type, payload_type> Payload(Storage& zbi, uint32_t offset,
                                                       uint32_t length) {
    fit::result result = ByteTraits::Payload(zbi->bytes_, offset, length);
    ZX_DEBUG_ASSERT(result.is_ok());
    return fit::ok(payload_type{result.value()});
  }

  template <typename U, bool LowLocality>
    requires(alignof(U) <= kStorageAlignment)
  static fit::result<error_type, ktl::span<const U>> Read(Storage& zbi, payload_type payload,
                                                          uint32_t length) {
    // Defensively check that `payload` does indeed correspond to a subrange
    // of `zbi`.
    ZX_DEBUG_ASSERT_MSG(zbi->bytes_.data() <= payload.bytes_.data(), "%p <= %p", zbi->bytes_.data(),
                        payload.bytes_.data());
    ZX_DEBUG_ASSERT_MSG(&payload.bytes_.back() <= &zbi->bytes_.back(), "%p <= %p",
                        &payload.bytes_.back(), &zbi->bytes_.back());

    if constexpr (kArchEarlyBootDataSynchronization) {
      size_t payload_end_offset = static_cast<size_t>(&payload.bytes_.back() - zbi->bytes_.data());

      // With internal knowledge of how zbitl storage traits are actually used
      // over the course of ZBI iteration, we assume that Read() will only ever
      // be called on headers. Accordingly, we only synchronize if we move past
      // the current high-water mark.
      if (zbi->synced_high_water_mark_ < payload_end_offset) {
        ktl::ignore = payload.get();  // Force a synchronization.
        zbi->synced_high_water_mark_ = payload_end_offset;
      }
    }
    return ByteTraits::Read<U, LowLocality>(zbi->bytes_, payload.bytes_, length);
  }
};

using EarlyBootZbi = zbitl::View<EarlyBootZbiBytes*>;

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_EARLY_BOOT_H_
