// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/memalloc/range.h>
#include <lib/zbi-format/memory.h>

#include <span>
#include <string_view>
#include <type_traits>

#include <pretty/cpp/sizes.h>

namespace memalloc {

namespace {

// Two hex `uint64_t`s, plus "[0x", ", 0x", and ")".
constexpr int kRangeColWidth = (2 * 16) + 3 + 4 + 1;

// A rough estimate: 4 digits, a decimal point, and a letter for a size.
constexpr int kSizeColWidth = 7;

}  // namespace

using namespace std::string_view_literals;

std::string_view ToString(Type type) {
  using namespace std::string_view_literals;

  switch (type) {
    case Type::kFreeRam:
      return "free RAM"sv;
    case Type::kReserved:
      return "reserved"sv;
    case Type::kPeripheral:
      return "peripheral"sv;
    case Type::kPoolBookkeeping:
      return "bookkeeping"sv;
    case Type::kPhysKernel:
      return "phys ZBI kernel image"sv;
    case Type::kPhysElf:
      return "phys ELF image"sv;
    case Type::kPhysLog:
      return "phys log file"sv;
    case Type::kKernel:
      return "kernel image"sv;
    case Type::kKernelStorage:
      return "decompressed kernel payload"sv;
    case Type::kDataZbi:
      return "data ZBI"sv;
    case Type::kTemporaryPhysHandoff:
      return "phys hand-off data (temporary)"sv;
    case Type::kPermanentPhysHandoff:
      return "phys hand-off data (permanent)"sv;
    case Type::kVdso:
      return "vDSO"sv;
    case Type::kUserboot:
      return "userboot"sv;
    case Type::kBootMachineStack:
      return "boot machine stack"sv;
    case Type::kBootShadowCallStack:
      return "boot shadow call stack"sv;
    case Type::kBootUnsafeStack:
      return "boot unsafe stack"sv;
    case Type::kFixedAddressStagingKernel:
      return "trampoline staging kernel image"sv;
    case Type::kFixedAddressStagingDataZbi:
      return "trampoline staging data ZBI";
    case Type::kLegacyBootData:
      return "legacy boot data";
    case Type::kTemporaryIdentityPageTables:
      return "temporary identity page tables"sv;
    case Type::kKernelPageTables:
      return "kernel page tables"sv;
    case Type::kPhysDebugdata:
      return "phys debugdata"sv;
    case Type::kDevicetreeBlob:
      return "devicetree blob"sv;
    case Type::kPhysScratch:
      return "phys scratch"sv;
    case Type::kPoolTestPayload:
      return "memalloc::Pool test payload"sv;
    case Type::kZbiTestPayload:
      return "ZBI test payload"sv;
    case Type::kTestRamReserve:
      return "kernel.test.ram.reserve"sv;
    case Type::kNvram:
      return "ZBI_TYPE_NVRAM"sv;
    case Type::kReservedLow:
      return "reserved low memory"sv;
    case Type::kTruncatedRam:
      return "truncated RAM"sv;
    case Type::kMaxAllocated:
      return "kMaxAllocated"sv;
  }
  return "unknown"sv;
}

std::span<Range> AsRanges(std::span<zbi_mem_range_t> ranges) {
  static_assert(std::is_standard_layout_v<Range>);
  static_assert(std::is_standard_layout_v<zbi_mem_range_t>);
  static_assert(offsetof(Range, addr) == offsetof(zbi_mem_range_t, paddr));
  static_assert(offsetof(Range, size) == offsetof(zbi_mem_range_t, length));
  static_assert(offsetof(Range, type) == offsetof(zbi_mem_range_t, type));

  for (zbi_mem_range_t& range : ranges) {
    range.reserved = 0;
  }
  return {reinterpret_cast<Range*>(ranges.data()), ranges.size()};
}

void internal::PrintRangeHeader(const char* prefix, FILE* f) {
  fprintf(f, "%s: | %-*s | %-*s | Type\n", prefix, kRangeColWidth, "Physical memory range",
          kSizeColWidth, "Size");
}

void internal::PrintOneRange(const memalloc::Range& range, const char* prefix, FILE* f) {
  pretty::FormattedBytes size(static_cast<size_t>(range.size));
  std::string_view type = ToString(range.type);
  fprintf(f, "%s: | [0x%016" PRIx64 ", 0x%016" PRIx64 ") | %*s | %-.*s\n",  //
          prefix, range.addr, range.end(),                                  //
          kSizeColWidth, size.c_str(), static_cast<int>(type.size()), type.data());
}

}  // namespace memalloc
