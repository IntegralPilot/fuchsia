// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_VMSAV8_64_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_VMSAV8_64_H_

#include <assert.h>
#include <stdint.h>
#include <sys/types.h>

#include <dev/iommu/common.h>

// Constants and inline helpers used when working with translation tables which
// use the VMSAv8-64 format.
namespace arm_smmu {
namespace vmsav8_64 {

// VMSAv8-64 with 4k granules means 512 64-bit entries per page.
constexpr uint32_t kEntriesPerPage = 512;

// All of the addresses we are using here should all be (at most) 48-bit
// addresses which are 4k page aligned.  This includes:
//
// 1) All device virtual addresses (the addresses we will translate).
// 2) All translation table physical page addresses.
// 3) All of the final translated PA/IPAs.
//
constexpr uint64_t kValidAddrMask = uint64_t{0x0000'FFFF'FFFF'F000};
constexpr bool IsValidAddr(paddr_t addr) { return (addr & ~kValidAddrMask) == 0; }

// Regardless of the translation table entry type (Table, Block, or Page), the
// "valid" bit is always bit zero.
constexpr uint64_t kValidEntryBit = 0x1;
constexpr inline bool IsValidEntry(uint64_t e) { return (e & kValidEntryBit) != 0; }

// For levels [0, 2], entries with bit one set are Table entries.  When cleared,
// they are block entries.
constexpr uint64_t kTableEntryBit = 0x2;
constexpr inline bool IsTableEntry(uint64_t e) { return (e & kTableEntryBit) != 0; }
constexpr inline bool IsBlockEntry(uint64_t e, uint32_t level) {
  return IsValidEntry(e) && (level < 3) && !IsTableEntry(e);
}

// At level 3, all entries are Page entries and should have bit one set.
constexpr uint64_t kPageEntryBit = 0x2;

// Extract the physical address of a "Table" entry.
constexpr inline paddr_t GetTableEntryPAddr(uint64_t e) { return e & kValidAddrMask; }

// Create a "Table" entry which points at the physical address of the next level
// of the translation tables.  Used in levels [0, 2] of the translation tables.
//
// Note: The passed address must be valid, and we debug assert this, but we also
// unconditionally mask the passed address just in case.
inline uint64_t MakeTableEntry(paddr_t addr) {
  ZX_DEBUG_ASSERT_MSG(IsValidAddr(addr), "Bad Address %016lx", addr);
  return (addr & kValidAddrMask) | kValidEntryBit | kTableEntryBit;
}

// Create a Page entry which points at the final physical address of the
// translation, and encodes the access attributes as well.
constexpr inline uint64_t MakePerms(uint64_t UXN, uint64_t PXN, uint64_t AP21) {
  // See Section D8.3.1.2 Figure D8-16 of the ARM ARM
  return ((UXN & 0x1) << 54) | ((PXN & 0x1) << 53) | ((AP21 & 0x3) << 6);
}

constexpr inline uint64_t GetPageEntryPerms(uint32_t perms) {
  // Permission mappings from basic RWX to Page Entry Attributes is, sadly, not
  // a straightforward exercise.  There are many factors in play, not all of
  // them are 100% under our control.
  //
  // Effective permissions are determined by several fields encoded in the
  // Page/Block entry, as well as the WXN/UWXN bits encoded in the Context
  // Bank's SCTLR register.  These bits are:
  //
  // + UXN (unprivileged execute never)
  // + PXN (privileged execute never)
  // + AP21 (the Access Permission bits)
  // + WXN (writeable execute never)
  // + UWXN (unprivileged execute never)
  //
  // Additionally, accesses can come in two flavors: Privileged and Unprivileged.  In
  // the AP world, an unprivileged access is one which comes from EL0, while a
  // privileged access is one which comes from EL1-3.  In the SMMU world, it is
  // not clear what initially configures the access flavor, but it is most
  // likely a decision that a system designer makes on a per-HW-unit basis.
  //
  // Based on the permission bits in the Page/Block entry and the SCTLR
  // register, along with whether the access is privileged or unprivileged, the
  // final effective permissions are determined.  A table showing the possible
  // combinations can be found in the ARM ARM, Section D8.4.1.2.6 Table D8-62.
  //
  // The current SMMU driver attempts to do the following for user-created BTIs.
  //
  // + Force all transactions to be considered unprivileged using the
  //   S2CR.PRIVCFG field in the BTI's SMRG(s).  Whether or not this override
  //   take effect depends on static SMMU configuration which can be read via
  //   IDR2.DIPANS.
  // + Disable WXN/UWXN via the context bank's SCTLR register.
  //
  // The final set of possible mappings from BTI_PERM flags to effective
  // permissions is given in the LUT below.  It is not possible to create a
  // perfect mapping from flags to permissions, and the issue is complicated by
  // the different final effective permissions based on privileged vs.
  // unprivileged access.  This said, it is _mostly_ correct, however there are
  // a few notes to keep in mind.
  //
  // + It is not possible to revoke Read access from a privileged access.  If
  //   there exists any valid translation, it will always allow privileged
  //   accesses to read memory.
  // + Regardless of the access flavor, Write access implies Read access.  IOW -
  //   it is not possible to create Write-only memory for a device.  We do our
  //   best to inform user-mode of this limitation by rejecting requests to
  //   create write only mappings at a higher level of the stack.
  // + It is not possible to allow both write and execute for privileged
  //   accesses, however it is for unprivileged accesses.
  //
  // See also ARM ARM Table D8-63.
  //
  constexpr uint64_t kPermLUT[] = {
      MakePerms(1, 1, 2),  // --- ==> pR-- : u---
      MakePerms(1, 1, 3),  // R-- ==> pR-- : uR--
      MakePerms(1, 1, 1),  // -W- ==> pRW- : uRW-
      MakePerms(1, 1, 1),  // RW- ==> pRw- : uRW-
      MakePerms(0, 0, 2),  // --X ==> pR-X : u--X
      MakePerms(0, 0, 3),  // R-X ==> pR-X : uR-X
      MakePerms(0, 0, 1),  // -WX ==> pRW- : uRWX
      MakePerms(0, 0, 1),  // RWX ==> pRW- : uRWX
  };

  static_assert(IOMMU_FLAG_PERM_READ == 0x1);
  static_assert(IOMMU_FLAG_PERM_WRITE == 0x2);
  static_assert(IOMMU_FLAG_PERM_EXECUTE == 0x4);
  constexpr uint32_t kAllPermFlags =
      IOMMU_FLAG_PERM_READ | IOMMU_FLAG_PERM_WRITE | IOMMU_FLAG_PERM_EXECUTE;

  return kPermLUT[perms & kAllPermFlags];
}

inline uint64_t MakePageEntry(paddr_t addr, uint32_t perms) {
  constexpr uint64_t kPageEntryAccessFlagBit = uint64_t{1} << 10;

  // Note: We set the access flag bit to 1 from the start because we don't make
  // use of it, and we don't want TLB fills to be writing to the page
  // descriptors.
  ZX_DEBUG_ASSERT(IsValidAddr(addr));
  return (addr & kValidAddrMask) | kValidEntryBit | kPageEntryBit | kPageEntryAccessFlagBit |
         GetPageEntryPerms(perms);
}

}  // namespace vmsav8_64
}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_VMSAV8_64_H_
