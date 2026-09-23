// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_INCLUDE_LIB_MEMALLOC_RANGE_H_
#define ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_INCLUDE_LIB_MEMALLOC_RANGE_H_

#include <lib/zbi-format/memory.h>
#include <stdio.h>

#include <algorithm>
#include <iterator>
#include <limits>
#include <optional>
#include <span>
#include <string_view>
#include <tuple>
#include <type_traits>

namespace memalloc {

constexpr uint64_t kMinAllocatedTypeValue =
    static_cast<uint64_t>(std::numeric_limits<uint32_t>::max()) + 1;

// The type of a physical memory range. Represented by 64 bits, the lower 2^32
// values in the space are reserved for memory range types defined in the ZBI
// spec, the "base types"; the types in the upper half are referred to as
// "allocated types", and increment from kMinAllocatedTypeValue in value.
//
// As is detailed in the ZBI spec regarding overlaps, among the base types,
// kReserved and kPeripheral ranges have the highest precedence, in that order.
// Further, by definition here, an allocated type is only permitted to overlap
// with kFreeRam or the same type.
enum class Type : uint64_t {
  //
  // ZBI memory range types:
  //

  kFreeRam = ZBI_MEM_TYPE_RAM,
  kPeripheral = ZBI_MEM_TYPE_PERIPHERAL,
  kReserved = ZBI_MEM_TYPE_RESERVED,

  //
  // Allocated types:
  //

  // Reserved for internal bookkeeping.
  kPoolBookkeeping = kMinAllocatedTypeValue,

  // The phys ZBI kernel memory image.
  kPhysKernel,

  // A phys ELF memory image.
  kPhysElf,

  // A phys log file.
  kPhysLog,

  // The kernel memory image.
  kKernel,

  // A (decompressed) STORAGE_KERNEL ZBI payload.
  kKernelStorage,

  // The data ZBI, as placed by the bootloader.
  kDataZbi,

  // Memory intended to remain allocated until the end of the phys hand-off
  // phase.
  kTemporaryPhysHandoff,

  // Memory intended to remain allocated for the lifetime of the kernel.
  kPermanentPhysHandoff,

  // A vDSO memory image.
  kVdso,

  // The userboot memory image.
  kUserboot,

  // The kernel's boot machine stack.
  kBootMachineStack,

  // The kernel's boot shadow call stack, if supported.
  kBootShadowCallStack,

  // The kernel's boot unsafe stack, if safe stack is supported.
  kBootUnsafeStack,

  // The intermediate kernel memory image used to trampoline into the same image
  // loaded at a fixed address (i.e., as used by FixedAddressBootZbi).
  kFixedAddressStagingKernel,

  // The intermediate data ZBI used to trampoline into the same image
  // loaded at a fixed address (i.e., as used by FixedAddressBootZbi).
  kFixedAddressStagingDataZbi,

  // Data structures related to legacy boot protocols.
  kLegacyBootData,

  // Identity-mapping page tables intended only for the lifetime of the phys
  // program in execution.
  kTemporaryIdentityPageTables,

  // Page tables that describe mappings intended to exist into the kernel
  // proper's lifetime.
  kKernelPageTables,

  // A debug data blob of phys origin (e.g., related to instrumentation).
  kPhysDebugdata,

  // A firmware-provided devicetree blob.
  kDevicetreeBlob,

  // General scratch space used by the phys kernel, but that which is free for
  // the next kernel as of hand-off.
  kPhysScratch,

  // A generic allocated type for Pool tests.
  kPoolTestPayload,

  // A generic allocated type for ZBI tests.
  kZbiTestPayload,

  // Memory carved out for the kernel.test.ram.reserve boot option.
  kTestRamReserve,

  // Memory carved out for the ZBI_TYPE_NVRAM region.
  kNvram,

  // Low memory, in an architecture-specific context, that is deemed to be
  // unsafe for general-purpose allocation and use (e.g., BIOS-related bits
  // below 1MiB in the case of PCs).
  kReservedLow,

  // RAM 'discarded' from a truncation of the physical address space when
  // simulating booting contexts with less physical memory available.
  kTruncatedRam,

  // A placeholder value signifying the last allocated type. It must not be used
  // as an actual type value.
  kMaxAllocated,
};

static_assert(static_cast<uint64_t>(Type::kFreeRam) < kMinAllocatedTypeValue);
static_assert(static_cast<uint64_t>(Type::kPeripheral) < kMinAllocatedTypeValue);
static_assert(static_cast<uint64_t>(Type::kReserved) < kMinAllocatedTypeValue);

constexpr uint64_t kMaxAllocatedTypeValue = static_cast<uint64_t>(Type::kMaxAllocated);
constexpr size_t kNumAllocatedTypes = kMaxAllocatedTypeValue - kMinAllocatedTypeValue;
constexpr size_t kNumBaseTypes = 3;

std::string_view ToString(Type type);

constexpr bool IsAllocatedType(Type type) {
  return static_cast<uint64_t>(type) >= kMinAllocatedTypeValue;
}

constexpr bool IsRamType(Type type) { return type == Type::kFreeRam || IsAllocatedType(type); }

// A memory range type that is layout-compatible to zbi_mem_range_t, but with
// the benefit of being able to use allocated types.
struct Range {
  uint64_t addr;
  uint64_t size;
  Type type;

  // The end of the memory range. This method may only be called if addr + size
  // has been normalized to not overflow.
  constexpr uint64_t end() const { return addr + size; }

  // Returns a new range aligned outward to the given alignment: The start
  // address is rounded down, and the end address is rounded up.
  constexpr Range AlignTo(uint64_t alignment) const {
    uint64_t aligned_addr = addr & -alignment;
    uint64_t aligned_end = (end() + alignment - 1) & -alignment;
    return {
        .addr = aligned_addr,
        .size = aligned_end - aligned_addr,
        .type = type,
    };
  }

  constexpr bool operator==(const Range& other) const {
    return addr == other.addr && size == other.size && type == other.type;
  }
  constexpr bool operator!=(const Range& other) const { return !(*this == other); }

  // Gives a lexicographic order on Range.
  constexpr bool operator<(const Range& other) const {
    return addr < other.addr || (addr == other.addr && size < other.size);
  }

  constexpr bool IntersectsWith(const Range& other) const {
    const auto [left, right] =
        *this < other ? std::make_tuple(this, &other) : std::make_tuple(&other, this);
    // Note: Until std reference wrappers are not constexpr.
    return left->end() > right->addr;
  }
};

// We have constrained Type so that the ZBI memory type's value space
// identically embeds into the lower 2^32 values of Type; the upper 2^32 values
// is reserved for Type's extensions. Accordingly, in order to coherently
// recast a zbi_mem_range_t as a Range, the former's `reserved` field -
// which, layout-wise, corresponds to the upper half of Type - must be zeroed
// out.
std::span<Range> AsRanges(std::span<zbi_mem_range_t> ranges);

// Gives a custom, normalized view of a container of ranges, `Ranges`.
// `NormalizeTypeFn` is a callable with signature `std::optional<Type>(Type)`:
// std::nullopt indicates that ranges of this type should not be passed to the
// callback; otherwise, the returned type is indicates how the input type should
// be normalized. Adjacent ranges of the same normalized type are merged before
// being passed to the callback.
//
// The callback itself is expected to return a boolean indicating whether it
// should continue to be called.
template <class Ranges, typename RangeCallback, typename NormalizeTypeFn>
constexpr void NormalizeRanges(Ranges&& ranges, RangeCallback&& cb,
                               NormalizeTypeFn&& normalize_type) {
  using iterator = typename std::decay_t<Ranges>::iterator;
  using value_type = typename std::iterator_traits<iterator>::value_type;
  static_assert(std::is_convertible_v<value_type, const Range&>);

  static_assert(std::is_invocable_r_v<bool, RangeCallback, const Range&>);
  static_assert(std::is_invocable_r_v<std::optional<Type>, NormalizeTypeFn, Type>);

  std::optional<Range> prev;
  for (const Range& range : ranges) {
    std::optional<Type> normalized_type = normalize_type(range.type);
    if (!normalized_type) {
      continue;
    }
    Range normalized = range;
    normalized.type = *normalized_type;
    if (!prev) {
      prev = normalized;
    } else if (prev->end() == normalized.addr && prev->type == normalized.type) {
      prev->size += normalized.size;
    } else {
      if (!cb(*prev)) {
        return;
      }
      prev = normalized;
    }
  }
  if (prev) {
    cb(*prev);
  }
}

// Provides a callback with a normalized view of RAM ranges alone, reducing
// any allocated types as kFreeRam.
template <class Ranges, typename RangeCallback>
constexpr void NormalizeRam(Ranges&& ranges, RangeCallback&& cb) {
  return NormalizeRanges(
      std::forward<Ranges>(ranges), std::forward<RangeCallback>(cb), [](Type type) {
        return IsRamType(type) ? std::make_optional(Type::kFreeRam) : std::nullopt;
      });
}

namespace internal {

// These are the components of what PrintRanges does internally,
// for use on different kinds of containers of memalloc::Range objects.
void PrintRangeHeader(const char* prefix, FILE* f);
void PrintOneRange(const Range& range, const char* prefix, FILE* f);

}  // namespace internal

// Pretty-prints the contents in a given Range container.
template <class Ranges>
inline void PrintRanges(Ranges&& ranges, const char* prefix, FILE* f = stdout) {
  using iterator = typename std::decay_t<Ranges>::iterator;
  using value_type = typename std::iterator_traits<iterator>::value_type;
  static_assert(std::is_convertible_v<value_type, const Range&>);

  internal::PrintRangeHeader(prefix, f);
  for (const memalloc::Range& range : ranges) {
    internal::PrintOneRange(range, prefix, f);
  }
}

namespace internal {

// Effectively just a span and an iterator. This is used internally to iterate
// over a variable number of range arrays.
struct RangeIterationContext {
  RangeIterationContext() = default;

  // Lexicographically sorts the ranges on construction.
  explicit RangeIterationContext(std::span<Range> ranges) : ranges_(ranges), it_(ranges.begin()) {
    std::sort(ranges_.begin(), ranges_.end());
  }

  std::span<Range> ranges_;
  typename std::span<Range>::iterator it_;
};

}  // namespace internal

}  // namespace memalloc

#endif  // ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_INCLUDE_LIB_MEMALLOC_RANGE_H_
