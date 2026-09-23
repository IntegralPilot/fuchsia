// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_PAGING_TRAITS_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_PAGING_TRAITS_H_

#include <lib/arch/arm64/system.h>
#include <lib/arch/paging.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <array>
#include <cstdint>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>

#include <fbl/bits.h>
#include <hwreg/bitfields.h>

#include "memory.h"

//
// The definitions here just deal with stage 1 translation for now.
//

namespace arch {

enum class ArmPagingConfiguration {
  // 4KiB pages, 2 levels of paging: 30-bit virtual addresses.
  k4k30Bit,
  // 4KiB pages, 3 levels of paging: 39-bit virtual addresses.
  k4k39Bit,
  // 4KiB pages, 4 levels of paging: 48-bit virtual addresses.
  k4k48Bit,
  // 16KiB pages, 1 level of paging: 25-bit virtual addresses.
  k16k25Bit,
  // 16KiB pages, 2 levels of paging: 36-bit virtual addresses.
  k16k36Bit,
  // 16KiB pages, 3 levels of paging: 47-bit virtual addresses.
  k16k47Bit,
  // 64KiB pages, 1 level of paging: 28-bit virtual addresses.
  k64k28Bit,
  // 64KiB pages, 2 levels of paging: 42-bit virtual addresses.
  k64k42Bit,
};

// Associates a conventional string name with each arm64 paging configuration.
constexpr ArmPagingConfiguration ArmPagingConfigurationFromString(std::string_view name) {
  using namespace std::string_view_literals;

  if (name == "4k-30bit"sv) {
    return ArmPagingConfiguration::k4k30Bit;
  }
  if (name == "4k-39bit"sv) {
    return ArmPagingConfiguration::k4k39Bit;
  }
  if (name == "4k-48bit"sv) {
    return ArmPagingConfiguration::k4k48Bit;
  }
  if (name == "16k-25bit"sv) {
    return ArmPagingConfiguration::k16k25Bit;
  }
  if (name == "16k-36bit"sv) {
    return ArmPagingConfiguration::k16k36Bit;
  }
  if (name == "16k-47bit"sv) {
    return ArmPagingConfiguration::k16k47Bit;
  }
  if (name == "64k-28bit"sv) {
    return ArmPagingConfiguration::k64k28Bit;
  }
  if (name == "64k-42bit"sv) {
    return ArmPagingConfiguration::k64k42Bit;
  }
  ZX_PANIC("Unknown arm64 paging configuration name: %.*s", static_cast<int>(name.size()),
           name.data());
}

// [arm/v8]: D5.1.3  VMSA address types and address spaces
//
// One of two possible maximum virtual address widths.
enum class ArmMaximumVirtualAddressWidth {
  k48Bits = 48,
  k52Bits = 52,
};

// [arm/v8]: D5.2.4  Memory translation granule size
//
// Numeric value gives the base-2 logarithm of the size in bytes for
// convenience in page-related arithmetic.
enum class ArmGranuleSize {
  k4KiB = 12,
  k16KiB = 14,
  k64KiB = 16,
};

// Convert from granule size to either arch::ArmTcrTg0Value or arch::ArmTcrTg1Value
template <typename TcrValueField>
  requires(std::is_same_v<TcrValueField, ArmTcrTg0Value> ||
           std::is_same_v<TcrValueField, ArmTcrTg1Value>)
constexpr TcrValueField ArmGranuleSizeToTcr(arch::ArmGranuleSize granule) {
  switch (granule) {
    case arch::ArmGranuleSize::k4KiB:
      return TcrValueField::k4KiB;
    case arch::ArmGranuleSize::k16KiB:
      return TcrValueField::k16KiB;
    case arch::ArmGranuleSize::k64KiB:
      return TcrValueField::k64KiB;
  }
}

// [arm/v8]: D5.2 The VMSAv8-64 address translation system
enum class ArmAddressTranslationLevel {
  kMinus1,
  k0,
  k1,
  k2,
  k3,
};

// Forward-declared; defined below.
template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationDescriptor;

//
// Implementations of the PagingTraits API (defined in <lib/arch/paging.h>) for
// ARM.
//
// We just define the 4KiB granule, max 48-bit wide versions of these for now,
// as these are all that are currently used.
//

// Specifies the upper or lower virtual address range (i.e., the
// 1- or 0- extended ranges, respectively), which are configured separately.
enum class ArmVirtualAddressRange {
  // Configured by TCR_EL1.T0SZ.
  kLower,

  // Configured by TCR_EL1.T1SZ.
  kUpper,
};

struct ArmSystemPagingState {
  template <class CurrentEl>
  static ArmSystemPagingState Create(ArmMemoryAttrIndirectionRegister mair,
                                     ArmShareabilityAttribute sh) {
    return {
        .mair = mair,
        .shareability = sh,
        .el1 = CurrentEl::Read().el() == 1,
    };
  }

  // Defaults to only configuring non-cacheable normal memory.
  ArmMemoryAttrIndirectionRegister mair =
      ArmMemoryAttrIndirectionRegister::Get().FromValue(0).SetAttribute(0, {});
  ArmShareabilityAttribute shareability = ArmShareabilityAttribute::kNone;

  // TODO(https://fxbug.dev/42081692): This is included only to account for the
  // associated bug.
  bool el1 = true;
};

using ArmPagingSettings = internal::PagingSettings<ArmMairAttribute>;

template <ArmPagingConfiguration Config, ArmVirtualAddressRange Range>
struct ArmPagingTraits {
  static constexpr ArmGranuleSize kGranuleSize = []() {
    switch (Config) {
      case ArmPagingConfiguration::k4k30Bit:
      case ArmPagingConfiguration::k4k39Bit:
      case ArmPagingConfiguration::k4k48Bit:
        return ArmGranuleSize::k4KiB;
      case ArmPagingConfiguration::k16k25Bit:
      case ArmPagingConfiguration::k16k36Bit:
      case ArmPagingConfiguration::k16k47Bit:
        return ArmGranuleSize::k16KiB;
      case ArmPagingConfiguration::k64k28Bit:
      case ArmPagingConfiguration::k64k42Bit:
        return ArmGranuleSize::k64KiB;
    }
    __UNREACHABLE;
  }();

  static constexpr size_t kNumberOfLevels = []() {
    switch (Config) {
      case ArmPagingConfiguration::k4k30Bit:
        return 2;
      case ArmPagingConfiguration::k4k39Bit:
        return 3;
      case ArmPagingConfiguration::k4k48Bit:
        return 4;
      case ArmPagingConfiguration::k16k25Bit:
        return 1;
      case ArmPagingConfiguration::k16k36Bit:
        return 2;
      case ArmPagingConfiguration::k16k47Bit:
        return 3;
      case ArmPagingConfiguration::k64k28Bit:
        return 1;
      case ArmPagingConfiguration::k64k42Bit:
        return 2;
    }
    __UNREACHABLE;
  }();

  using LevelType = ArmAddressTranslationLevel;

  using MemoryType = ArmMairAttribute;

  using SystemState = ArmSystemPagingState;

  using PagingSettings = ArmPagingSettings;

  template <ArmAddressTranslationLevel Level>
  using TableEntry =
      ArmAddressTranslationDescriptor<Level, kGranuleSize, ArmMaximumVirtualAddressWidth::k48Bits>;

  static constexpr unsigned int kMaxPhysicalAddressSize = 48;

  static constexpr unsigned int kTableAlignmentLog2 = static_cast<unsigned int>(kGranuleSize);

  static constexpr unsigned int kNumTableEntriesLog2 = static_cast<unsigned int>(kGranuleSize) - 3;

  static constexpr bool kNonTerminalAccessPermissions = true;

  static constexpr auto kVirtualAddressExtension = Range == ArmVirtualAddressRange::kLower
                                                       ? VirtualAddressExtension::k0
                                                       : VirtualAddressExtension::k1;

  static constexpr std::array kAllLevels = {
      ArmAddressTranslationLevel::k0,
      ArmAddressTranslationLevel::k1,
      ArmAddressTranslationLevel::k2,
      ArmAddressTranslationLevel::k3,
  };

  static constexpr auto kLevels = std::span{kAllLevels}.last(kNumberOfLevels);

  static constexpr bool kExecuteOnlyAllowed = false;

  template <ArmAddressTranslationLevel Level>
  static constexpr bool LevelCanBeTerminal(const SystemState& state) {
    using Page = typename TableEntry<Level>::Page;
    using Block = typename TableEntry<Level>::Block;
    return Page::kValid || Block::kValid;
  }
};

template <ArmPagingConfiguration Config>
using ArmLowerPagingTraits = ArmPagingTraits<Config, ArmVirtualAddressRange::kLower>;

template <ArmPagingConfiguration Config>
using ArmUpperPagingTraits = ArmPagingTraits<Config, ArmVirtualAddressRange::kUpper>;

//
// Forward declarations of the different descriptor layouts; defined below.
//

template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationTableDescriptor;

template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationPageDescriptor;

template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationBlockDescriptor;

// [arm/v8]: Table D5-29 Data access permissions for stage 1 translations
//
// Access permission for page table entries.
enum class ArmAddressTranslationAccessPermissions : uint8_t {
  kSupervisorReadWrite = 0b00,  // EL1+ can read/write, EL0 no access.
  kReadWrite = 0b01,            // All levels can read/write.
  kSupervisorReadOnly = 0b10,   // EL1+ can read, EL0 no access.
  kReadOnly = 0b11,             // All levels can read.
};

enum class ArmAddressTranslationTableAccessPermissions : uint8_t {
  kNoEffect = 0b00,
  kNoEl0Access = 0b01,
  kNoWriteAccess = 0b10,
  kNoWriteOrEl0Access = 0b11,
};

// [arm/v8]: D5.3.1 VMSAv8-64 translation table level 0, level 1, and level 2
// descriptor formats.
//
// [arm/v8]: D5.3.2 Armv8 translation table level 3 descriptor formats
//
enum class ArmAddressTranslationDescriptorFormat : uint8_t {
  // Block descriptor for levels {0, 1, 2}. Invalid for level 3.
  kBlock = 0b0,

  // Table descriptor for levels {0, 1, 2}, page descriptor for level 3.
  kTableOrPage = 0b1,
};

// [arm/v8]: D5.3VMSAv8-64 Translation Table format descriptors
//
// This represents an abstract address translation table descriptor, which
// specializes to one of three formats: block, table, or page.
template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationDescriptor
    : public hwreg::RegisterBase<ArmAddressTranslationDescriptor<Level, GranuleSize, MaxVaddrWidth>,
                                 uint64_t> {
 private:
  using SelfType = ArmAddressTranslationDescriptor<Level, GranuleSize, MaxVaddrWidth>;

 public:
  using Table = ArmAddressTranslationTableDescriptor<Level, GranuleSize, MaxVaddrWidth>;
  using Page = ArmAddressTranslationPageDescriptor<Level, GranuleSize, MaxVaddrWidth>;
  using Block = ArmAddressTranslationBlockDescriptor<Level, GranuleSize, MaxVaddrWidth>;

  // Bits [61:2] are described in the block, table, and page format subclasses.

  DEF_ENUM_FIELD(ArmAddressTranslationDescriptorFormat, 1, 1, format);
  DEF_BIT(0, valid);

  constexpr bool IsTable() const {
    if constexpr (Table::kValid) {
      return format() == ArmAddressTranslationDescriptorFormat::kTableOrPage;
    } else {
      return false;
    }
  }

  // Provided IsTable()) is true, recast this as a table descriptor.
  constexpr const Table& AsTable() const {
    return As<Table, &ArmAddressTranslationDescriptor::IsTable>(this);
  }
  constexpr Table& AsTable() { return As<Table, &ArmAddressTranslationDescriptor::IsTable>(this); }

  // Update the descriptor to represent the table format.
  constexpr Table& SetAsTable() {
    static_assert(Table::kValid);
    return static_cast<Table&>(set_format(ArmAddressTranslationDescriptorFormat::kTableOrPage));
  }

  constexpr bool IsPage() const {
    if constexpr (Page::kValid) {
      return format() == ArmAddressTranslationDescriptorFormat::kTableOrPage;
    } else {
      return false;
    }
  }

  // Provided IsPage() is true, recast this as a page descriptor.
  constexpr const Page& AsPage() const {
    return As<Page, &ArmAddressTranslationDescriptor::IsPage>(this);
  }
  constexpr Page& AsPage() { return As<Page, &ArmAddressTranslationDescriptor::IsPage>(this); }

  // Update the descriptor to represent the page format.
  constexpr Page& SetAsPage() {
    static_assert(Page::kValid);
    return static_cast<Page&>(set_format(ArmAddressTranslationDescriptorFormat::kTableOrPage));
  }

  constexpr bool IsBlock() const {
    if constexpr (Block::kValid) {
      return format() == ArmAddressTranslationDescriptorFormat::kBlock;
    } else {
      return false;
    }
  }

  // Provided IsBlock() is true, recast this as a block descriptor.
  constexpr const Block& AsBlock() const {
    return As<Block, &ArmAddressTranslationDescriptor::IsBlock>(this);
  }
  constexpr Block& AsBlock() { return As<Block, &ArmAddressTranslationDescriptor::IsBlock>(this); }

  // Update the descriptor to represent the block format.
  constexpr Block& SetAsBlock() {
    static_assert(Block::kValid);
    return static_cast<Block&>(set_format(ArmAddressTranslationDescriptorFormat::kBlock));
  }

  //
  // This implements the PagingTraits::TableEntry API defined in
  // <lib/arch/paging.h>.
  //

  constexpr bool present() const { return valid(); }

  constexpr uint64_t address() const {
    if (IsTable()) {
      return AsTable().table_address();
    }
    if (IsPage()) {
      return AsPage().output_address();
    }
    if (IsBlock()) {
      return AsBlock().output_address();
    }
    return 0;
  }

  constexpr bool terminal() const { return IsPage() || IsBlock(); }

  constexpr bool readable() const { return true; }

  constexpr bool writable() const {
    if (terminal()) {
      ArmAccessPermissions ap = [&]() {
        if (IsPage()) {
          return AsPage().ap();
        }
        return AsBlock().ap();
      }();
      return ap == ArmAccessPermissions::kReadWrite ||
             ap == ArmAccessPermissions::kSupervisorReadWrite;
    }
    ArmTableAccessPermissions ap_table = AsTable().ap_table();
    return ap_table == ArmTableAccessPermissions::kNoEffect ||
           ap_table == ArmTableAccessPermissions::kNoEl0Access;
  }

  constexpr bool executable() const {
    if (terminal()) {
      if (IsBlock()) {
        return !AsBlock().pxn();
      }
      return !AsPage().pxn();
    }
    return !AsTable().pxn_table();
  }

  constexpr bool user_accessible() const {
    if (terminal()) {
      ArmAccessPermissions ap = [&]() {
        if (IsPage()) {
          return AsPage().ap();
        }
        return AsBlock().ap();
      }();
      return ap == ArmAccessPermissions::kReadWrite || ap == ArmAccessPermissions::kReadOnly;
    }

    ArmTableAccessPermissions ap_table = AsTable().ap_table();
    return ap_table == ArmTableAccessPermissions::kNoEffect ||
           ap_table == ArmTableAccessPermissions::kNoWriteAccess;
  }

  constexpr bool accessed() const {
    return IsPage() ? AsPage().af() : IsBlock() ? AsBlock().af() : false;
  }

  constexpr bool global() const {
    return IsPage() ? !AsPage().ng() : IsBlock() ? !AsBlock().ng() : false;
  }

  constexpr ArmMairAttribute Memory(const ArmSystemPagingState& state) const {
    auto memory = [&state](const auto& desc) {
      return *state.mair.GetAttribute(static_cast<unsigned int>(desc.attr_index()));
    };
    if (IsPage()) {
      return memory(AsPage());
    }
    if (IsBlock()) {
      return memory(AsBlock());
    }
    ZX_PANIC("Memory() cannot be called on a non-terminal entry");
  }

  constexpr SelfType& Set(const ArmSystemPagingState& state, const ArmPagingSettings& settings) {
    set_valid(settings.present);
    if (!settings.present) {
      return *this;
    }

    if (settings.terminal) {
      if constexpr (Page::kValid) {
        SetAsPage();
      } else if constexpr (Block::kValid) {
        SetAsBlock();
      } else {
        ZX_PANIC("level cannot be terminal");
      }

      auto set_terminal = [&](auto& desc) {
        desc.set_af(true).set_ng(!settings.global).set_sh(state.shareability);
        std::optional<unsigned int> index = state.mair.GetIndex(settings.memory);
        ZX_DEBUG_ASSERT_MSG(index,
                            "memory attribute %#" PRIx8 " not configured in MAIR (%#" PRIx64 ")",
                            ArmMemoryAttrIndirectionRegister::AttributeToValue(settings.memory),
                            state.mair.reg_value());
        desc.set_attr_index(*index);
      };
      if (IsPage()) {
        set_terminal(AsPage());
      } else {
        set_terminal(AsBlock());
      }
    } else {
      if constexpr (Table::kValid) {
        SetAsTable();
      } else {
        ZX_PANIC("level must be terminal");
      }
    }

    Set(state, settings.access);

    if (IsTable()) {
      AsTable().set_table_address(settings.address);
    } else if (IsPage()) {
      AsPage().set_output_address(settings.address);
    } else if (IsBlock()) {
      AsBlock().set_output_address(settings.address);
    } else {
      ZX_PANIC("invalid descriptor format");
    }
    return *this;
  }

 private:
  using ArmAccessPermissions = ArmAddressTranslationAccessPermissions;
  using ArmTableAccessPermissions = ArmAddressTranslationTableAccessPermissions;

  template <class Subclass, auto IsSubclass, class Base>
  static constexpr auto& As(Base* base) {
    // Double-check that we can safely recast a Base as a Subclass.
    static_assert(std::is_base_of_v<std::remove_const_t<Base>, Subclass>);
    static_assert(std::alignment_of_v<Subclass> == std::alignment_of_v<Base>);
    ZX_ASSERT((base->*IsSubclass)());
    return *static_cast<std::conditional_t<std::is_const_v<Base>, const Subclass, Subclass>*>(base);
  }

  constexpr SelfType& Set(const ArmSystemPagingState& state, const AccessPermissions& access) {
    if (terminal()) {
      ZX_DEBUG_ASSERT(access.readable);

      ArmAccessPermissions ap = [&]() {
        if (access.writable) {
          if (access.user_accessible) {
            return ArmAccessPermissions::kReadWrite;
          }
          return ArmAccessPermissions::kSupervisorReadWrite;
        }
        if (access.user_accessible) {
          return ArmAccessPermissions::kReadOnly;
        }
        return ArmAccessPermissions::kSupervisorReadOnly;
      }();

      auto set_xn = [&](auto& desc) {
        // We do not need to support user-executable pages at this time.
        //
        // TODO(https://fxbug.dev/42081692): set UXN to false always.
        desc.set_uxn(state.el1).set_pxn(!access.executable);
      };

      if (IsPage()) {
        set_xn(AsPage().set_ap(ap));
      } else {
        set_xn(AsBlock().set_ap(ap));
      }
    } else {
      ArmTableAccessPermissions ap_table = [&]() {
        if (access.writable) {
          if (access.user_accessible) {
            return ArmTableAccessPermissions::kNoEffect;
          }
          return ArmTableAccessPermissions::kNoEl0Access;
        }
        if (access.user_accessible) {
          return ArmTableAccessPermissions::kNoWriteAccess;
        }
        return ArmTableAccessPermissions::kNoWriteOrEl0Access;
      }();

      AsTable()
          .set_ap_table(ap_table)
          // We do not need to support user-executable pages at this time.
          //
          // TODO(https://fxbug.dev/42081692): set UXN_TABLE to false always.
          .set_uxn_table(state.el1)
          .set_pxn_table(!access.executable);
    }

    return *this;
  }
};

// [arm/v8]: D5.3.1 VMSAv8-64 translation table level -1, level 0, level 1, and level 2
// descriptor formats
// [arm/v8]: D5.3.3 Memory attribute fields in the VMSAv8-64 Translation Table format descriptors
template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationTableDescriptor
    : public ArmAddressTranslationDescriptor<Level, GranuleSize, MaxVaddrWidth> {
 private:
  using SelfType = ArmAddressTranslationTableDescriptor<Level, GranuleSize, MaxVaddrWidth>;

 public:
  // Whether table descriptors are generally valid at this level.
  static constexpr bool kValid = []() {
    constexpr bool kLevelMinus1 = Level == ArmAddressTranslationLevel::kMinus1;
    constexpr bool kLevel0 = Level == ArmAddressTranslationLevel::k0;
    constexpr bool kLevel1 = Level == ArmAddressTranslationLevel::k1;
    constexpr bool kLevel2 = Level == ArmAddressTranslationLevel::k2;

    switch (MaxVaddrWidth) {
      case ArmMaximumVirtualAddressWidth::k48Bits:
        return kLevel0 || kLevel1 || kLevel2;
      case ArmMaximumVirtualAddressWidth::k52Bits:
        return kLevelMinus1 || kLevel0 || kLevel1 || kLevel2;
    }
  }();

  DEF_BIT(63, ns_table);  // Non-Secure, for Tables
  DEF_ENUM_FIELD(ArmAddressTranslationTableAccessPermissions, 62, 61,
                 ap_table);  // Access Permissions, for Tables
  DEF_BIT(60, uxn_table);    // Unprivileged eXecute Never, for Tables
  DEF_BIT(59, pxn_table);    // Privileged eXecute Never, for Tables

  // Bits [49:12] conditionally represent the table address field; it is
  // handled manually below.

  constexpr uint64_t table_address() const {
    uint64_t addr = ta();
    if constexpr (kWidth52 && (kTaHighBit == 47)) {
      addr |= (ta_51_48() << 48);
    } else if constexpr (kWidth52 && (kTaHighBit == 49)) {
      addr |= (ta_51_50() << 50);
    }
    return addr;
  }

  constexpr SelfType& set_table_address(uint64_t addr) {
    constexpr auto kAddrHighBit = static_cast<unsigned int>(MaxVaddrWidth) - 1;
    constexpr auto kAddrLowBit = static_cast<unsigned int>(GranuleSize);
    ZX_ASSERT((fbl::ExtractBits<61, kAddrHighBit + 1, uint64_t>(addr) == 0));
    ZX_ASSERT((fbl::ExtractBits<kAddrLowBit - 1, 0, uint64_t>(addr) == 0));

    if constexpr (kWidth52 && (kTaHighBit == 47)) {
      set_ta_51_48(fbl::ExtractBits<51, 48, uint64_t>(addr));
    } else if constexpr (kWidth52 && (kTaHighBit == 49)) {
      set_ta_51_50(fbl::ExtractBits<51, 50, uint64_t>(addr));
    }
    return set_ta(addr);
  }

 private:
  static constexpr bool kWidth52 = MaxVaddrWidth == ArmMaximumVirtualAddressWidth::k52Bits;

  static constexpr bool kGranule4 = GranuleSize == ArmGranuleSize::k4KiB;
  static constexpr bool kGranule16 = GranuleSize == ArmGranuleSize::k16KiB;

  static constexpr unsigned int kTaHighBit = kWidth52 && (kGranule4 || kGranule16) ? 49 : 47;
  static constexpr unsigned int kTaLowBit = static_cast<unsigned int>(GranuleSize);

  DEF_UNSHIFTED_FIELD(kTaHighBit, kTaLowBit, ta);
  DEF_COND_FIELD(15, 12, ta_51_48, kWidth52 && (kTaHighBit == 47));
  DEF_COND_FIELD(9, 8, ta_51_50, kWidth52 && (kTaHighBit == 49));
};

// [arm/v8]: D5.3.2 Translation table level 3 descriptor formats
// [arm/v8]: D5.3.3 Memory attribute fields in the VMSAv8-64 Translation Table format descriptors
template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationPageDescriptor
    : public ArmAddressTranslationDescriptor<Level, GranuleSize, MaxVaddrWidth> {
 private:
  using SelfType = ArmAddressTranslationPageDescriptor<Level, GranuleSize, MaxVaddrWidth>;

  static constexpr bool kWidth48 = MaxVaddrWidth == ArmMaximumVirtualAddressWidth::k48Bits;

 public:
  // Whether page descriptors are generally valid at this level.
  static constexpr bool kValid = Level == ArmAddressTranslationLevel::k3;

  // Bit 63 is reserved.
  DEF_FIELD(62, 59, pbha);  // Page-Based Hardware Attributes
  // Bits [58:55] are reserved.

  DEF_BIT(54, uxn);  // Unprivileged eXecute Never
  DEF_BIT(53, pxn);  // Privileged eXecute Never

  DEF_BIT(52, contiguous);
  DEF_BIT(51, dbm);  // Dirty Bit Modifier

  DEF_BIT(50, gp);  // Guarded Page (FEAT_BTI)

  // Bits [49:12] conditionally represent the output address field, which is
  // handled manually below.

  DEF_BIT(11, ng);  // Not Global
  DEF_BIT(10, af);  // Access Flag

  DEF_COND_ENUM_FIELD(ArmShareabilityAttribute, 9, 8, sh, kWidth48);  // SHareability

  // Access Permissions
  DEF_ENUM_FIELD(ArmAddressTranslationAccessPermissions, 7, 6, ap);
  DEF_BIT(5, ns);  // Non-Secure
  DEF_FIELD(4, 2, attr_index);

  constexpr uint64_t output_address() const {
    uint64_t addr = oa();
    if constexpr (kWidth52 && (kOaHighBit == 47)) {
      addr |= (oa_51_48() << 48);
    } else if constexpr (kWidth52 && (kOaHighBit == 49)) {
      addr |= (oa_51_50() << 50);
    }
    return addr;
  }

  constexpr SelfType& set_output_address(uint64_t addr) {
    constexpr auto kAddrHighBit = static_cast<unsigned int>(MaxVaddrWidth) - 1;
    constexpr auto kAddrLowBit = static_cast<unsigned int>(GranuleSize);
    ZX_DEBUG_ASSERT((fbl::ExtractBits<61, kAddrHighBit + 1, uint64_t>(addr) == 0));
    ZX_DEBUG_ASSERT((fbl::ExtractBits<kAddrLowBit - 1, 0, uint64_t>(addr) == 0));

    if constexpr (kWidth52 && (kOaHighBit == 47)) {
      set_oa_51_48(fbl::ExtractBits<51, 48, uint64_t>(addr));
    } else if constexpr (kWidth52 && (kOaHighBit == 49)) {
      set_oa_51_50(fbl::ExtractBits<51, 50, uint64_t>(addr));
    }
    return set_oa(addr);
  }

 private:
  static constexpr bool kWidth52 = MaxVaddrWidth == ArmMaximumVirtualAddressWidth::k52Bits;

  static constexpr bool kGranule4 = GranuleSize == ArmGranuleSize::k4KiB;
  static constexpr bool kGranule16 = GranuleSize == ArmGranuleSize::k16KiB;

  static constexpr unsigned int kOaHighBit = kWidth52 && (kGranule4 || kGranule16) ? 49 : 47;
  static constexpr unsigned int kOaLowBit = static_cast<unsigned int>(GranuleSize);

  DEF_UNSHIFTED_FIELD(kOaHighBit, kOaLowBit, oa);
  DEF_COND_FIELD(15, 12, oa_51_48, kWidth52 && (kOaHighBit == 47));
  DEF_COND_FIELD(9, 8, oa_51_50, kWidth52 && (kOaHighBit == 49));
};

// [arm/v8]: D5.3.1 VMSAv8-64 translation table level -1, level 0, level 1, and level 2
// descriptor formats
// [arm/v8]: D5.3.3 Memory attribute fields in the VMSAv8-64 Translation Table format descriptors
template <ArmAddressTranslationLevel Level,             //
          ArmGranuleSize GranuleSize,                   //
          ArmMaximumVirtualAddressWidth MaxVaddrWidth>  //
class ArmAddressTranslationBlockDescriptor
    : public ArmAddressTranslationDescriptor<Level, GranuleSize, MaxVaddrWidth> {
 private:
  using SelfType = ArmAddressTranslationBlockDescriptor<Level, GranuleSize, MaxVaddrWidth>;

  static constexpr bool kWidth48 = MaxVaddrWidth == ArmMaximumVirtualAddressWidth::k48Bits;

 public:
  // Whether block descriptors are generally valid at this level.
  static constexpr bool kValid = []() {
    constexpr bool kLevelMinus1 = Level == ArmAddressTranslationLevel::kMinus1;
    constexpr bool kLevel0 = Level == ArmAddressTranslationLevel::k0;
    constexpr bool kLevel1 = Level == ArmAddressTranslationLevel::k1;
    constexpr bool kLevel2 = Level == ArmAddressTranslationLevel::k2;

    switch (MaxVaddrWidth) {
      case ArmMaximumVirtualAddressWidth::k48Bits:
        switch (GranuleSize) {
          case ArmGranuleSize::k4KiB:
            return kLevel1 || kLevel2;
          case ArmGranuleSize::k16KiB:
            return kLevel2;
          case ArmGranuleSize::k64KiB:
            return kLevel2;
        }
      case ArmMaximumVirtualAddressWidth::k52Bits:
        switch (GranuleSize) {
          case ArmGranuleSize::k4KiB:
            return kLevelMinus1 || kLevel0 || kLevel1 || kLevel2;
          case ArmGranuleSize::k16KiB:
            return kLevel0 || kLevel1 || kLevel2;
          case ArmGranuleSize::k64KiB:
            return kLevel1 || kLevel2;
        }
    }
  }();

  // Bit 63 is reserved.
  DEF_FIELD(62, 59, pbha);  // Page-Based Hardware Attributes
  // Bits [58:55] are reserved.

  DEF_BIT(54, uxn);  // Unprivileged eXecute Never
  DEF_BIT(53, pxn);  // Privileged eXecute Never

  DEF_BIT(52, contiguous);
  DEF_BIT(51, dbm);  // Dirty Bit Modifier

  DEF_BIT(50, gp);  // Guard Page

  // Bits [49:17] and [15:12] conditionally represent the output address field, which is
  // handled manually below.

  DEF_BIT(16, nt);  // Block translation entry

  DEF_BIT(11, ng);  // Not Global
  DEF_BIT(10, af);  // Access Flag

  DEF_COND_ENUM_FIELD(ArmShareabilityAttribute, 9, 8, sh, kWidth48);  // SHareability

  DEF_ENUM_FIELD(ArmAddressTranslationAccessPermissions, 7, 6, ap);  // Access Permissions
  DEF_BIT(5, ns);                                                    // Non-Secure
  DEF_FIELD(4, 2, attr_index);

  constexpr uint64_t output_address() const {
    uint64_t addr = oa();
    if constexpr (kWidth52 && (kOaHighBit == 47)) {
      addr |= (oa_51_48() << 48);
    } else if constexpr (kWidth52 && (kOaHighBit == 49)) {
      addr |= (oa_51_50() << 50);
    }
    return addr;
  }

  constexpr SelfType& set_output_address(uint64_t addr) {
    ZX_ASSERT((fbl::ExtractBits<61, kAddrHighBit + 1, uint64_t>(addr) == 0));
    ZX_ASSERT((fbl::ExtractBits<kAddrLowBit - 1, 0, uint64_t>(addr) == 0));

    if constexpr (kWidth52 && (kOaHighBit == 47)) {
      set_oa_51_48(fbl::ExtractBits<51, 48, uint64_t>(addr));
    } else if constexpr (kWidth52 && (kOaHighBit == 49)) {
      set_oa_51_50(fbl::ExtractBits<51, 50, uint64_t>(addr));
    }
    return set_oa(addr);
  }

 private:
  static constexpr bool kWidth52 = MaxVaddrWidth == ArmMaximumVirtualAddressWidth::k52Bits;

  static constexpr bool kGranule4 = GranuleSize == ArmGranuleSize::k4KiB;
  static constexpr bool kGranule16 = GranuleSize == ArmGranuleSize::k16KiB;

  static constexpr unsigned int kOaHighBit = kWidth52 && (kGranule4 || kGranule16) ? 49 : 47;

  static constexpr unsigned int kAddrHighBit = static_cast<unsigned int>(MaxVaddrWidth) - 1;
  static constexpr unsigned int kAddrLowBit = []() {
    switch (GranuleSize) {
      case ArmGranuleSize::k4KiB:
        switch (Level) {
          case ArmAddressTranslationLevel::kMinus1:
            return 48u;
          case ArmAddressTranslationLevel::k0:
            return 39u;
          case ArmAddressTranslationLevel::k1:
            return 30u;
          case ArmAddressTranslationLevel::k2:
            return 21u;
          case ArmAddressTranslationLevel::k3:
            break;
        }
        break;
      case ArmGranuleSize::k16KiB:
        switch (Level) {
          case ArmAddressTranslationLevel::kMinus1:
            break;
          case ArmAddressTranslationLevel::k0:
            return 47u;
          case ArmAddressTranslationLevel::k1:
            return 36u;
          case ArmAddressTranslationLevel::k2:
            return 25u;
          case ArmAddressTranslationLevel::k3:
            break;
        }
        break;
      case ArmGranuleSize::k64KiB:
        switch (Level) {
          case ArmAddressTranslationLevel::kMinus1:
          case ArmAddressTranslationLevel::k0:
            break;
          case ArmAddressTranslationLevel::k1:
            return 42u;
          case ArmAddressTranslationLevel::k2:
            return 29u;
          case ArmAddressTranslationLevel::k3:
            break;
        }
        break;
    }
    // Conditioning use of the type on kValid will prevent this bogus value
    // from actually being used; even if it does get used though, it will
    // result in a runtime error due to overlap.
    return kAddrHighBit + 1;
  }();

  DEF_COND_RSVDZ_FIELD(49, 48, kOaHighBit == 47);
  DEF_UNSHIFTED_FIELD(kOaHighBit, 21, oa);
  DEF_COND_FIELD(15, 12, oa_51_48, kWidth52 && (kOaHighBit == 47));
  DEF_COND_FIELD(9, 8, oa_51_50, kWidth52 && (kOaHighBit == 49));
};

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_PAGING_TRAITS_H_
