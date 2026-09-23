// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_X86_PAGING_TRAITS_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_X86_PAGING_TRAITS_H_

#include <inttypes.h>
#include <lib/arch/paging.h>
#include <lib/arch/x86/cpuid.h>
#include <zircon/assert.h>

#include <array>
#include <cstdint>
#include <optional>
#include <span>
#include <string_view>

#include <fbl/bits.h>
#include <hwreg/bitfields.h>

#include "feature.h"

namespace arch {

enum class X86PagingLevelCount {
  k4 = 4,
  k5 = 5,
};

// Associates a conventional string name with each x86 paging configuration.
constexpr X86PagingLevelCount X86PagingLevelCountFromString(std::string_view name) {
  using namespace std::string_view_literals;

  if (name == "4level"sv) {
    return X86PagingLevelCount::k4;
  }
  if (name == "5level"sv) {
    return X86PagingLevelCount::k5;
  }
  ZX_PANIC("Unknown x86 paging configuration name: %.*s", static_cast<int>(name.size()),
           name.data());
}

// [intel/vol3]: Table 4-2. Paging Structures in the Different Paging Modes
// [amd/vol2]: Figure 5-1. Virtual to Physical Address Translation — Long Mode
enum class X86PagingLevel {
  kPml5Table,  // Page Map Level 5 Table
  kPml4Table,  // Page Map Level 4 Table
  kPageDirectoryPointerTable,
  kPageDirectory,
  kPageTable,
};

//
// Implementations of the PagingTraits API (defined in <lib/arch/paging.h>) for
// 4- and 5-level long mode x86 paging.
//

struct X86PagingTraitsBase {
  using LevelType = X86PagingLevel;

  template <X86PagingLevel Level>
  class TableEntry;

  // Memory configuration is not page-based for x86. Rather, it is managed
  // through the MTRR MSRs.
  struct MemoryType {};

  // Captures the system state influencing x86 paging.
  struct SystemState {
    template <class MsrIo, class CpuidIo>
    static SystemState Create(MsrIo&& msr, CpuidIo&& cpuid) {
      // NXE - the feature to mark pages as non-executable - should always be
      // supported.
      ZX_DEBUG_ASSERT(X86ExtendedFeatureEnableRegisterMsr::Get().ReadFrom(&msr).nxe());

      return {.page1gb = cpuid.template Read<CpuidAmdFeatureFlagsD>().page1gb() == 1};
    }

    // Whether 1GiB pages are supported.
    bool page1gb = false;
  };

  using PagingSettings = internal::PagingSettings<MemoryType>;

  static constexpr std::array kAllLevels = {
      X86PagingLevel::kPml5Table,
      X86PagingLevel::kPml4Table,
      X86PagingLevel::kPageDirectoryPointerTable,
      X86PagingLevel::kPageDirectory,
      X86PagingLevel::kPageTable,
  };

  static constexpr unsigned int kMaxPhysicalAddressSize = 52;

  static constexpr unsigned int kTableAlignmentLog2 = 12;

  static constexpr unsigned int kNumTableEntriesLog2 = 9;

  static constexpr bool kNonTerminalAccessPermissions = true;

  static constexpr auto kVirtualAddressExtension = VirtualAddressExtension::kCanonical;

  static constexpr bool kExecuteOnlyAllowed = false;

  template <X86PagingLevel Level>
  static constexpr bool LevelCanBeTerminal(const SystemState& state) {
    if constexpr (Level == X86PagingLevel::kPml5Table || Level == X86PagingLevel::kPml4Table) {
      return false;
    } else if constexpr (Level == X86PagingLevel::kPageDirectoryPointerTable) {
      return state.page1gb;
    } else {
      return true;
    }
  }
};

template <X86PagingLevelCount Count>
struct X86PagingTraits : public X86PagingTraitsBase {
  static constexpr std::array kAllLevels = {
      X86PagingLevel::kPml5Table,
      X86PagingLevel::kPml4Table,
      X86PagingLevel::kPageDirectoryPointerTable,
      X86PagingLevel::kPageDirectory,
      X86PagingLevel::kPageTable,
  };

  static constexpr auto kLevels = std::span{kAllLevels}.last(static_cast<size_t>(Count));
};

using X86FourLevelPagingTraits = X86PagingTraits<X86PagingLevelCount::k4>;
using X86FiveLevelPagingTraits = X86PagingTraits<X86PagingLevelCount::k5>;

// [intel/vol3]: Figure 4-11. Formats of CR3 and Paging-Structure Entries with 4-Level Paging and
// 5-Level Paging
// [amd/vol2]: 5.3 Long-Mode Page Translation
//
// Represents a general x86 page table entry.
//
// Many of the operations below are dependent on whether the entry is terminal:
// if a constructed entry is intended to be terminal that should be set first.
template <X86PagingLevel Level>
class X86PagingTraitsBase::TableEntry
    : public hwreg::RegisterBase<X86PagingTraitsBase::TableEntry<Level>, uint64_t> {
 private:
  using SelfType = X86PagingTraitsBase::TableEntry<Level>;

 public:
  DEF_BIT(63, xd);  // eXecute Disable (or NX - No eXecute - on AMD)

  // Bits [62:59] conditionally represent the PKE field; it is handled
  // manually below.

  DEF_RSVDZ_FIELD(58, 52);

  // Bits [51:12] conditionally represent the base address and PAT fields; they
  // are handled manually below.

  DEF_FIELD(11, 9, avl);  // AVaiLable to software.
  DEF_BIT(8, g);          // Global

  // Bit 7 conditionally represents the PAT and PS fields; they are handled
  // manually below.

  DEF_BIT(6, d);    // Dirty
  DEF_BIT(5, a);    // Accessed
  DEF_BIT(4, pcd);  // Page-level Cache Disable
  DEF_BIT(3, pwt);  // Page-level Write-Through
  DEF_BIT(2, u_s);  // User/Supervisor
  DEF_BIT(1, r_w);  // Read/Write
  DEF_BIT(0, p);    // Present

  // "Page Size", available only for the PDPTE and PDE levels, indicates a
  // terminal entry.
  constexpr std::optional<bool> ps() const {
    if constexpr (kPdpte || kPde) {
      return ps_or_pat_7();
    } else {
      return {};
    }
  }

  // "Page Attribute Table", available only on terminal levels, indirectly
  // indicates the memory type used to reference the associated page.
  constexpr std::optional<bool> pat() const {
    if (!terminal()) {
      return {};
    }
    if constexpr (kPte) {
      return ps_or_pat_7();
    } else {
      return base_address_or_pat_12();
    }
  }

  // Sets the PAT bit, which is only valid on terminal levels.
  constexpr SelfType& set_pat(bool pat) {
    ZX_DEBUG_ASSERT(terminal());
    if constexpr (kPte) {
      return set_ps_or_pat_7(pat);
    } else {
      return set_base_address_or_pat_12(pat);
    }
  }

  // "Protection KEy", available on terminal levels, may be used to control
  // access rights (when supported). (Also known as MPK - Memory Protection Key
  // - on AMD.)
  constexpr std::optional<unsigned int> pke() const {
    if (terminal()) {
      return pke_62_59();
    }
    return {};
  }

  // Sets the PKE, which is only valid for a terminal entry.
  constexpr SelfType& set_pke(unsigned int pke) {
    ZX_ASSERT(terminal());
    return set_pke_62_59(pke);
  }

  //
  // This implements the PagingTraits::TableEntry API defined in
  // <lib/arch/paging.h>.
  //

  constexpr bool present() const { return p(); }

  constexpr uint64_t address() const {
    uint64_t addr = base_address_51_13();
    if (!(terminal() && (kPde || kPdpte))) {
      addr |= base_address_or_pat_12() << 12;
    }
    return addr;
  }

  constexpr bool terminal() const {
    if constexpr (kPte) {
      return true;
    } else if constexpr (kPdpte || kPde) {
      return *ps();
    } else {
      return false;
    }
  }

  constexpr bool readable() const { return true; }
  constexpr bool writable() const { return r_w(); }
  constexpr bool executable() const { return !xd(); }
  constexpr bool user_accessible() const { return u_s(); }

  constexpr MemoryType Memory(const SystemState& state) const { return {}; }

  constexpr bool accessed() const { return a(); }

  constexpr bool global() const { return terminal() ? g() : false; }

  constexpr SelfType& Set(const SystemState& state, const PagingSettings& settings) {
    set_p(settings.present);
    if (!settings.present) {
      return *this;
    }

    if constexpr (kPte) {
      ZX_DEBUG_ASSERT_MSG(settings.terminal, "page table entries are always terminal");
    } else if constexpr (kPdpte || kPde) {
      set_ps_or_pat_7(settings.terminal);
    } else {
      ZX_DEBUG_ASSERT_MSG(!settings.terminal, "PML4 and PML5 entries cannot be terminal");
    }

    const AccessPermissions& access = settings.access;
    ZX_DEBUG_ASSERT(access.readable);
    set_r_w(access.writable)
        .set_xd(!access.executable)
        .set_u_s(access.user_accessible)
        .set_a(true)
        .set_d(true);

    ZX_DEBUG_ASSERT_MSG((fbl::ExtractBits<63, 52, uint64_t>(settings.address) == 0), "%#" PRIx64,
                        settings.address);
    if (terminal()) {
      if constexpr (kPdpte) {
        ZX_DEBUG_ASSERT_MSG((fbl::ExtractBits<29, 13, uint64_t>(settings.address) == 0),
                            "%#" PRIx64, settings.address);
      } else if constexpr (kPde) {
        ZX_DEBUG_ASSERT_MSG((fbl::ExtractBits<20, 13, uint64_t>(settings.address) == 0),
                            "%#" PRIx64, settings.address);
      }

      set_g(settings.global);
    }
    set_base_address_51_13(fbl::ExtractBits<51, 13, uint64_t>(settings.address) << 13);
    if (!(terminal() && (kPde || kPdpte))) {
      set_base_address_or_pat_12((settings.address & (1 << 12)) != 0);
    }
    return *this;
  }

 private:
  static constexpr bool kPte = Level == X86PagingLevel::kPageTable;
  static constexpr bool kPde = Level == X86PagingLevel::kPageDirectory;
  static constexpr bool kPdpte = Level == X86PagingLevel::kPageDirectoryPointerTable;

  DEF_FIELD(62, 59, pke_62_59);
  DEF_UNSHIFTED_FIELD(51, 13, base_address_51_13);
  DEF_BIT(12, base_address_or_pat_12);
  DEF_BIT(7, ps_or_pat_7);
};

template <X86PagingLevel Level>
using X86PagingStructure = X86PagingTraitsBase::TableEntry<Level>;

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_X86_PAGING_TRAITS_H_
