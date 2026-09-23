// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_PAGING_TRAITS_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_PAGING_TRAITS_H_

#include <inttypes.h>
#include <lib/arch/paging.h>
#include <lib/arch/sysreg.h>
#include <zircon/assert.h>

#include <array>
#include <cstdint>
#include <span>
#include <string_view>

#include <fbl/bits.h>
#include <hwreg/bitfields.h>

namespace arch {

struct RiscvSatp : public SysRegBase<RiscvSatp, uint64_t> {
  enum class Mode : uint8_t {
    kBare = 0,  // No translation or protection.
    // 1-7 are reserved for standard use.
    kSv39 = 8,
    kSv48 = 9,
    kSv57 = 10,
    kSv64 = 11,
    // 12-13 are reserved for standard use.
    // 14-15 are reserved for custom use.
  };

  RiscvSatp& SetBareMode() {
    // Apart from Mode::kBare, all other fields must be zeroed out.
    return set_reg_value(0);
  }

  uint64_t root_address() const { return ppn() << 12; }

  RiscvSatp& set_root_address(uint64_t addr) {
    ZX_DEBUG_ASSERT((fbl::ExtractBits<11, 0, uint64_t>(addr) == 0));
    return set_ppn(addr >> 12);
  }

  DEF_ENUM_FIELD(Mode, 63, 60, mode);
  DEF_FIELD(59, 44, asid);
  DEF_FIELD(43, 0, ppn);
};
ARCH_RISCV64_SYSREG(RiscvSatp, "satp");

// Associates a conventional string name with each riscv64 paging configuration.
constexpr RiscvSatp::Mode RiscvSatpModeFromString(std::string_view name) {
  using namespace std::string_view_literals;

  if (name == "sv39"sv) {
    return RiscvSatp::Mode::kSv39;
  }
  if (name == "sv48"sv) {
    return RiscvSatp::Mode::kSv48;
  }
  if (name == "sv57"sv) {
    return RiscvSatp::Mode::kSv57;
  }
  if (name == "sv64"sv) {
    return RiscvSatp::Mode::kSv64;
  }
  ZX_PANIC("Unknown riscv64 paging configuration name: %.*s", static_cast<int>(name.size()),
           name.data());
}

enum class RiscvPagingLevel {
  k4 = 4,
  k3 = 3,
  k2 = 2,
  k1 = 1,
  k0 = 0,
};

// When the Svpbmt extension is available, a page table entry's pbmt field
// determines the physical memory type being accessed.  Without that extension,
// those bits must be all zero, which is also the PMA type (meaning the memory
// type is controlled by the Physical Memory Attributes specification instead).
enum class RiscvMemoryType {
  kPma = 0,  // None
  kNc = 1,   // Non-cacheable, idempotent, weakly-ordered (RVWMO), main memory
  kIo = 2,   // Non-cacheable, non-idempotent, strongly-ordered (I/O ordering), I/O
};

//
// Implementations of the PagingTraits API (defined in <lib/arch/paging.h>) for
// RISC-V.
//

struct RiscvPagingTraitsBase {
  using LevelType = RiscvPagingLevel;

  template <RiscvPagingLevel Level>
  class TableEntry;

  using MemoryType = RiscvMemoryType;

  static constexpr MemoryType kDefaultMemory = RiscvMemoryType::kPma;

  struct SystemState {};

  using PagingSettings = internal::PagingSettings<MemoryType, kDefaultMemory>;

  static constexpr std::array kAllLevels = {
      RiscvPagingLevel::k4, RiscvPagingLevel::k3, RiscvPagingLevel::k2,
      RiscvPagingLevel::k1, RiscvPagingLevel::k0,
  };

  static constexpr unsigned int kMaxPhysicalAddressSize = 57;

  static constexpr unsigned int kTableAlignmentLog2 = 12;

  static constexpr unsigned int kNumTableEntriesLog2 = 9;

  static constexpr bool kNonTerminalAccessPermissions = false;

  static constexpr auto kVirtualAddressExtension = VirtualAddressExtension::kCanonical;

  static constexpr bool kExecuteOnlyAllowed = true;

  template <RiscvPagingLevel Level>
  static constexpr bool LevelCanBeTerminal(const SystemState& state) {
    return true;
  }
};

template <RiscvSatp::Mode Mode>
struct RiscvPagingTraits : public RiscvPagingTraitsBase {
  static_assert(Mode != RiscvSatp::Mode::kBare, "bare means the absence of paging");
  static_assert(Mode != RiscvSatp::Mode::kSv64, "sv64 is not supported");

  static constexpr auto kMode = Mode;

  static constexpr size_t kNumberOfLevels = []() {
    switch (Mode) {
      case RiscvSatp::Mode::kSv39:
        return 3;
      case RiscvSatp::Mode::kSv48:
        return 4;
      case RiscvSatp::Mode::kSv57:
        return 5;
      default:
        __UNREACHABLE;
    }
  }();

  static constexpr auto kLevels = std::span{kAllLevels}.last(kNumberOfLevels);
};

using RiscvSv39PagingTraits = RiscvPagingTraits<RiscvSatp::Mode::kSv39>;
using RiscvSv48PagingTraits = RiscvPagingTraits<RiscvSatp::Mode::kSv48>;
using RiscvSv57PagingTraits = RiscvPagingTraits<RiscvSatp::Mode::kSv57>;

template <RiscvPagingLevel Level>
class RiscvPagingTraitsBase::TableEntry
    : public hwreg::RegisterBase<RiscvPagingTraitsBase::TableEntry<Level>, uint64_t> {
 private:
  using SelfType = RiscvPagingTraitsBase::TableEntry<Level>;

 public:
  // When the Svnapot extension is available, setting the N bit means that
  // this PTE is part of a larger Naturally Aligned Power-of-2 (NAPOT) range.
  // The low bits of PPN indicate what power of 2 is the actual granularity of
  // range.  The hardware is free to assume that all the PTEs for the whole
  // range all have the N bit set and the access control bits (5..0) set the
  // same, so it can use a single TLB entry to represent the multiple PTEs in
  // the whole NAPOT range.
  DEF_BIT(63, n);  // RES0 without Svnapot extension

  // These bits are always RES0 in non-leaf PTEs.  When the Svpbmt extension
  // is available, in leaf PTEs they set the type of physical memory access.
  DEF_ENUM_FIELD(MemoryType, 62, 61, pbmt);  // RES0 without Svpbmt extension

  // Bits [60: 54] are reserved.

  // In an entry acting as the last level the physical address must have the
  // same alignment as the virtual address: 4KiB, 2MiB, 1GiB, or 512GiB.  The
  // low 12 bits that must always be zero aren't stored in the PTE.
  DEF_FIELD(53, 10, ppn);  // Physical Page Number

  // These bits are available for software use. The hardware ignores them.
  DEF_FIELD(9, 8, rsw);

  DEF_BIT(7, d);  // Dirty
  DEF_BIT(6, a);  // Accessed
  DEF_BIT(5, g);  // Global
  DEF_BIT(4, u);  // U mode access (also S mode access when sstatus.SUM clear)

  // --- (XWR all clear) means the PPN is a next-level page table.
  // -W- and XW- (R clear) are reserved for future use.
  // All other combinations have their natural meanings.
  DEF_BIT(3, x);  // eXecutable
  DEF_BIT(2, w);  // Writable
  DEF_BIT(1, r);  // Readable

  // If V is clear, then hardware ignores all other bits and they can be used
  // for software purposes.
  DEF_BIT(0, v);  // Valid

  //
  // This implements the PagingTraits::TableEntry API defined in
  // <lib/arch/paging.h>.
  //

  constexpr bool present() const { return v(); }

  constexpr uint64_t address() const { return ppn() << 12; }

  constexpr bool terminal() const { return r() || x(); }

  constexpr bool readable() const { return terminal() ? r() : true; }
  constexpr bool writable() const { return terminal() ? w() : true; }
  constexpr bool executable() const { return terminal() ? x() : true; }
  constexpr bool user_accessible() const { return terminal() ? u() : true; }

  constexpr RiscvMemoryType Memory(const SystemState& state) const { return pbmt(); }

  constexpr bool accessed() const { return terminal() ? a() : false; }

  constexpr bool global() const { return terminal() ? g() : false; }

  constexpr SelfType& Set(const SystemState& state, const PagingSettings& settings) {
    set_v(settings.present);
    if (!settings.present) {
      return *this;
    }

    const AccessPermissions& access = settings.access;
    if (settings.terminal) {
      ZX_DEBUG_ASSERT(access.readable || !access.writable);
      set_r(access.readable)
          .set_w(access.writable)
          .set_x(access.executable)
          .set_u(access.user_accessible)
          .set_pbmt(settings.memory)
          .set_a(true)
          .set_d(access.writable)
          .set_g(settings.global);
    } else {
      // Since access permissions cannot be applied to non-terminal levels to
      // constrain later ones, the provided permissions here are expected to be
      // maximally permissive.
      ZX_DEBUG_ASSERT(access.readable);
      ZX_DEBUG_ASSERT(access.writable);
      ZX_DEBUG_ASSERT(access.executable);
      ZX_DEBUG_ASSERT(access.user_accessible);

      // Intermediate levels must have the access permission bits cleared.
      set_r(false).set_w(false).set_x(false).set_u(false);
    }

    if (terminal()) {
      constexpr unsigned int kAlignmentLog2 = 12 + 9 * static_cast<unsigned int>(Level);
      ZX_DEBUG_ASSERT_MSG(
          (fbl::ExtractBits<kAlignmentLog2 - 1, 0, uint64_t>(settings.address) == 0),
          "level %u page address %#" PRIx64 " expected to have an alignment of %#" PRIx64,
          static_cast<unsigned int>(Level), settings.address, (uint64_t{1} << kAlignmentLog2));
    } else {
      ZX_DEBUG_ASSERT_MSG((fbl::ExtractBits<11, 0, uint64_t>(settings.address) == 0),
                          "page table address %#" PRIx64 "expected to be 4KiB-aligned",
                          settings.address);
    }
    ZX_DEBUG_ASSERT(
        (fbl::ExtractBits<63, kMaxPhysicalAddressSize, uint64_t>(settings.address) == 0));
    return set_ppn(settings.address >> 12);
  }
};

template <RiscvPagingLevel Level>
using RiscvPageTableEntry = RiscvPagingTraitsBase::TableEntry<Level>;

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_PAGING_TRAITS_H_
