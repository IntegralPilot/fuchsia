// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_SYSTEM_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_SYSTEM_H_

#include <lib/arch/arm64/feature.h>
#include <lib/arch/hwreg.h>
#include <lib/arch/sysreg.h>

#include <functional>
#include <optional>

#include "memory.h"

namespace arch {

// This file defines hwreg accessor types for some of the AArch64 system
// registers used for the top-level generic control things.
//
// The names here are approximately the expanded names used in the [arm/sysreg]
// manual text.  This only defines the bit layouts and can be used portably.
// The ARCH_SYSREG types used to access the registers directly on hardware are
// declared in <lib/arch/sysreg.h>.  Both headers must be included to use the
// accessors for specific registers with the right layout types.

// [arm/sysreg]/currentel: CurrentEL, Current Exception Level
struct ArmCurrentEl : public SysRegBase<ArmCurrentEl, uint64_t> {
  // This returns call(el1) or call(el2) or call(el3) depending on current EL.
  // It uses perfect forwarding.  All three overloads of call must all have the
  // same return type, which may be void.
  template <typename El1, typename El2, typename El3, typename Call>
  constexpr decltype(auto) ForThisEl(El1&& el1, El2&& el2, El3&& el3, Call&& call) {
    switch (el()) {
      default:
      case 1:
        return std::forward<Call>(call)(std::forward<El1>(el1));
      case 2:
        return std::forward<Call>(call)(std::forward<El2>(el2));
      case 3:
        return std::forward<Call>(call)(std::forward<El3>(el3));
    }
  }

  // This does each of call(el3), call(el2), and call(el1) in turn going from
  // the current EL to each lower EL (with no call for EL0).  It uses perfect
  // forwarding for elx objects.
  template <typename El1, typename El2, typename El3, typename Call>
  constexpr void ForEachEl(El1&& el1, El2&& el2, El3&& el3, Call&& call) {
    switch (el()) {
      case 3:
        call(std::forward<El3>(el3));
        [[fallthrough]];
      case 2:
        call(std::forward<El2>(el2));
        [[fallthrough]];
      case 1:
        call(std::forward<El1>(el1));
        [[fallthrough]];
      default:
        break;
    }
  }

  DEF_FIELD(3, 2, el);
};
ARCH_ARM64_SYSREG(ArmCurrentEl, "CurrentEL");

// If we are executing at an exception level higher than EL1, this disables EL2
// monitoring, ensures would-be EL2 traps are routed to EL3, installs the
// current stack in SP_EL1 (if any), and then drops to EL1. If we are already
// at EL1, then this call is a no-op.
//
// This function is safe to call in any context. Care is taken to avoid using
// the stack and only uses scratch registers.
extern "C" void ArmDropToEl1WithoutEl2Monitor();

// This type covers the following register formats:
// * [arm/sysreg]/mpidr_el1: Multiprocessor Affinity Register (EL1)
// * [arm/sysreg]/vmpidr_el2: Virtualization Multiprocessor ID Register (EL2)
struct ArmMultiprocessorAffinityRegister
    : public SysRegDerivedBase<ArmMultiprocessorAffinityRegister, uint64_t> {
  uint64_t affinity() const { return reg_value() & kAffinityMask; }

  DEF_RSVDZ_FIELD(63, 40);
  DEF_FIELD(39, 32, aff3);  // Affinity level 3
  DEF_BIT(31, res1_bit32);  // RES1
  DEF_BIT(30, u);           // Uniprocessor system
  DEF_RSVDZ_FIELD(29, 25);  // RES0
  DEF_BIT(24, mt);          // Multithreading
  DEF_FIELD(23, 16, aff2);  // Affinity level 2
  DEF_FIELD(15, 8, aff1);   // Affinity level 1
  DEF_FIELD(7, 0, aff0);    // Affinity level 0

 private:
  static constexpr uint64_t kAff0Mask = 0xffULL << 0;   // Bits [7:0]
  static constexpr uint64_t kAff1Mask = 0xffULL << 8;   // Bits [15:8]
  static constexpr uint64_t kAff2Mask = 0xffULL << 16;  // Bits [23:16]
  static constexpr uint64_t kAff3Mask = 0xffULL << 32;  // Bits [39:32]
  static constexpr uint64_t kAffinityMask = kAff0Mask | kAff1Mask | kAff2Mask | kAff3Mask;
};

struct ArmMpidrEl1 : public arch::SysRegDerived<ArmMpidrEl1, ArmMultiprocessorAffinityRegister> {};
ARCH_ARM64_SYSREG(ArmMpidrEl1, "mpidr_el1");

// This type covers three register formats:
//  * [arm/sysreg]/sctlr_el1: System Control Register (EL1)
//  * [arm/sysreg]/sctlr_el2: System Control Register (EL2)
//  * [arm/sysreg]/sctlr_el3: System Control Register (EL3)
// Some fields (mostly things relating to EL0) are only used in EL1 and are
// reserved in the other registers.  Missing bits are reserved in all cases.
struct ArmSystemControlRegister : public SysRegDerivedBase<ArmSystemControlRegister, uint64_t> {
  enum class TagCheckFault : uint64_t {
    kNone = 0b00,             // Faults have no effect.
    kSynchronous = 0b01,      // All faults cause a synchronous exception.
    kAsynchronous = 0b10,     // All faults accumulate asynchronously.
    kSynchronousRead = 0b11,  // Synchronous for read, asynchronous for write.
  };

  std::optional<uint64_t> twedel_cycles() const {
    if (tweden()) {
      return 1u << twedel() << 8;  // This is the minimum delay in cycles.
    }
    return std::nullopt;  // Implementation-defined.
  }

  DEF_BIT(63, tidcp);                           // EL1, EL2
  DEF_BIT(62, spintmask);                       // EL1, EL2, EL3
  DEF_BIT(61, nmi);                             // EL1, EL2, EL3
  DEF_BIT(60, entp2);                           // EL1, EL2
  DEF_BIT(59, tsco);                            // EL1, EL2, EL3
  DEF_BIT(58, tsco0);                           // EL1, EL2
  DEF_BIT(57, epan);                            // EL1, EL2
  DEF_BIT(56, enals);                           // EL1, EL2
  DEF_BIT(55, enas0);                           // EL1, EL2
  DEF_BIT(54, enasr);                           // EL1, EL2
  DEF_BIT(53, tme);                             // EL1, EL2, EL3
  DEF_BIT(52, tme0);                            // EL1, EL2
  DEF_BIT(51, tmt);                             // EL1, EL2, EL3
  DEF_BIT(50, tmt0);                            // EL1, EL2
  DEF_FIELD(49, 46, twedel);                    // EL1
  DEF_BIT(45, tweden);                          // EL1
  DEF_BIT(44, dsbss);                           // EL1, EL2, EL3
  DEF_BIT(43, ata);                             // EL1, EL2, EL3
  DEF_BIT(42, ata0);                            // EL1
  DEF_ENUM_FIELD(TagCheckFault, 41, 40, tcf);   // EL1, EL2, EL3
  DEF_ENUM_FIELD(TagCheckFault, 39, 38, tcf0);  // EL1
  DEF_BIT(37, itfsb);                           // EL1, EL2, EL3
  DEF_BIT(36, bt);                              // EL1, EL2, EL3
  DEF_BIT(35, bt0);                             // EL1
  DEF_BIT(33, mscen);                           // EL1, EL2
  DEF_BIT(32, cmow);                            // EL1, EL2
  DEF_BIT(31, enia);                            // EL1, EL2, EL3
  DEF_BIT(30, enib);                            // EL1, EL2, EL3
  DEF_BIT(29, lsmaoe);                          // EL1
  DEF_BIT(28, ntlsmd);                          // EL1
  DEF_BIT(27, enda);                            // EL1, EL2, EL3
  DEF_BIT(26, uci);                             // EL1
  DEF_BIT(25, ee);                              // EL1, EL2, EL3
  DEF_BIT(24, e0e);                             // EL1
  DEF_BIT(23, span);                            // EL1
  DEF_BIT(22, eis);                             // EL1, EL2, EL3
  DEF_BIT(21, iesb);                            // EL1, EL2, EL3
  DEF_BIT(20, tscxt);                           // EL1
  DEF_BIT(19, wxn);                             // EL1, EL2, EL3
  DEF_BIT(18, ntwe);                            // EL1
  DEF_BIT(16, ntwi);                            // EL1
  DEF_BIT(15, uct);                             // EL1
  DEF_BIT(14, dze);                             // EL1, EL2, EL3
  DEF_BIT(13, endb);                            // EL1, EL2, EL3
  DEF_BIT(12, i);                               // EL1, EL2, EL3
  DEF_BIT(11, eos);                             // EL1, EL2, EL3
  DEF_BIT(10, enrctx);                          // EL1
  DEF_BIT(9, uma);                              // EL1
  DEF_BIT(8, sed);                              // EL1
  DEF_BIT(7, itd);                              // EL1
  DEF_BIT(6, naa);                              // EL1, EL2, EL3
  DEF_BIT(5, cp15ben);                          // EL1
  DEF_BIT(4, sa0);                              // EL1
  DEF_BIT(3, sa);                               // EL1, EL2, EL3
  DEF_BIT(2, c);                                // EL1, EL2, EL3
  DEF_BIT(1, a);                                // EL1, EL2, EL3
  DEF_BIT(0, m);                                // EL1, EL2, EL3
};

struct ArmSctlrEl1 : public arch::SysRegDerived<ArmSctlrEl1, ArmSystemControlRegister> {};
ARCH_ARM64_SYSREG(ArmSctlrEl1, "sctlr_el1");

struct ArmSctlrEl2 : public arch::SysRegDerived<ArmSctlrEl2, ArmSystemControlRegister> {};
ARCH_ARM64_SYSREG(ArmSctlrEl2, "sctlr_el2");

struct ArmSctlrEl3 : public arch::SysRegDerived<ArmSctlrEl3, ArmSystemControlRegister> {};
ARCH_ARM64_SYSREG(ArmSctlrEl3, "sctlr_el3");

struct ArmSystemControlRegister2 : public SysRegDerivedBase<ArmSystemControlRegister2, uint64_t> {
  DEF_BIT(6, enidcp128);  // EL1, EL2
  DEF_BIT(5, ease);       // EL1, EL2
  DEF_BIT(4, enanerr);    // EL1, EL2, EL3
  DEF_BIT(3, enaderr);    // EL1, EL2, EL3
  DEF_BIT(2, nmea);       // EL1, EL2
  DEF_BIT(1, emec);       // EL2, EL3
};

// Use alternate register description to work around GCC not allowing the use of the name.
struct ArmSctlr2El1 : public arch::SysRegDerived<ArmSctlr2El1, ArmSystemControlRegister2> {};
ARCH_ARM64_SYSREG(ArmSctlr2El1, "S3_0_c1_c0_3");  // SCTLR_EL1

struct ArmSctlr2El2 : public arch::SysRegDerived<ArmSctlr2El2, ArmSystemControlRegister2> {};
ARCH_ARM64_SYSREG(ArmSctlr2El2, "S3_4_c1_c0_3");  // SCTLR_EL2

struct ArmSctlr2El3 : public arch::SysRegDerived<ArmSctlr2El3, ArmSystemControlRegister2> {};
ARCH_ARM64_SYSREG(ArmSctlr2El3, "S3_6_c1_c0_3");  // SCTLR_EL3

// [arm/sysreg]/scr_el3: Secure Configuration Register
struct ArmScrEl3 : public SysRegBase<ArmScrEl3, uint64_t> {
  DEF_RSVDZ_FIELD(63, 39);
  DEF_BIT(38, hxen);
  DEF_BIT(37, aden);
  DEF_BIT(36, enas0);
  DEF_BIT(35, amvoffen);
  DEF_RSVDZ_BIT(34);
  DEF_FIELD(33, 30, twedel);
  DEF_BIT(29, tweden);
  DEF_BIT(28, ecven);
  DEF_BIT(27, fgten);
  DEF_BIT(26, ata);
  DEF_BIT(25, enscxt);
  DEF_RSVDZ_FIELD(24, 22);
  DEF_BIT(21, fien);
  DEF_BIT(20, nmea);
  DEF_BIT(19, ease);
  DEF_BIT(18, eel2);
  DEF_BIT(17, api);
  DEF_BIT(16, apk);
  DEF_BIT(15, terr);
  DEF_BIT(14, tlor);
  DEF_BIT(13, twe);
  DEF_BIT(12, twi);
  DEF_BIT(11, st);
  DEF_BIT(10, rw);
  DEF_BIT(9, sif);
  DEF_BIT(8, hce);
  DEF_BIT(7, smd);
  DEF_RSVDZ_BIT(6);
  // Bits 5:4 are RES1 (reserved, always one).
  DEF_BIT(3, ea);
  DEF_BIT(2, fiq);
  DEF_BIT(1, irq);
  DEF_BIT(0, ns);
};
ARCH_ARM64_SYSREG(ArmScrEl3, "scr_el3");

// Granule size values for the TCR_EL1 and TCR_EL2 fields.
//
// WARNING: The encodings for the TG0 field and TG1 field are different.
//
// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
// [arm/v8]: D13.2.121 TCR_EL2, Translation Control Register (EL2)
enum class ArmTcrTg0Value {
  k4KiB = 0b00,
  k16KiB = 0b10,
  k64KiB = 0b01,
};
enum class ArmTcrTg1Value {
  k4KiB = 0b10,
  k16KiB = 0b01,
  k64KiB = 0b11,
};

// Forward declaration, defined below.
struct ArmTcrEl2;

// Translation Control Register (TCR) for EL1.
//
// The TCR controls the settings relating to the page table, including
// the layout (such as granule size setting and size of the address
// space).
//
// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
class ArmTcrEl1 : public SysRegBase<ArmTcrEl1> {
 public:
  // Copy all the fields that have direct equivalents in TCR_EL2.
  inline ArmTcrEl1& CopyEl2(const ArmTcrEl2& tcr_el2);

  DEF_RSVDZ_FIELD(63, 62);
  DEF_BIT(61, mtx1);
  DEF_BIT(60, mtx0);
  DEF_BIT(59, ds);
  DEF_BIT(58, tcma1);
  DEF_BIT(57, tcma0);
  DEF_BIT(56, e0pd1);
  DEF_BIT(55, e0pd0);
  DEF_BIT(54, nfd1);
  DEF_BIT(53, nfd0);
  DEF_BIT(52, tbid1);  // TTBR1 Top Byte Ignored for Data only
  DEF_BIT(51, tbid0);  // TTBR0 Top Byte Ignored for Data only
  DEF_BIT(50, hwu162);
  DEF_BIT(49, hwu161);
  DEF_BIT(48, hwu160);
  DEF_BIT(47, hwu159);
  DEF_BIT(46, hwu062);
  DEF_BIT(45, hwu061);
  DEF_BIT(44, hwu060);
  DEF_BIT(43, hwu059);
  DEF_BIT(42, hpd1);  // TTBR0 Hierarchical Permission Disable
  DEF_BIT(41, hpd0);  // TTBR0 Hierarchical Permission Disable
  DEF_BIT(40, hd);    // Hardware Dirty state management
  DEF_BIT(39, ha);    // Hardware Access flag updated
  DEF_BIT(38, tbi1);  // TTBR1 Top Byte Ignored
  DEF_BIT(37, tbi0);  // TTBR0 Top Byte Ignored
  DEF_BIT(36, as);    // ASID size: 0 = 8-bit, 1 = 16-bit
  DEF_RSVDZ_BIT(35);
  DEF_ENUM_FIELD(ArmPhysicalAddressSize, 34, 32, ips);      // Intermediate physical address size.
  DEF_ENUM_FIELD(ArmTcrTg1Value, 31, 30, tg1);              // TTBR1 granule size
  DEF_ENUM_FIELD(ArmShareabilityAttribute, 29, 28, sh1);    // TTBR1 cache sharability
  DEF_ENUM_FIELD(ArmCacheabilityAttribute, 27, 26, orgn1);  // TTBR1 outer cacheability
  DEF_ENUM_FIELD(ArmCacheabilityAttribute, 25, 24, irgn1);  // TTBR1 inner cacheability
  DEF_BIT(23, epd1);                                        // TTBR1 table walks disabled
  DEF_BIT(22, a1);                                          // ASID select: 0 = TTBR0, 1 = TTBR1
  DEF_FIELD(21, 16, t1sz);                                  // TTBR0 size offset
  DEF_ENUM_FIELD(ArmTcrTg0Value, 15, 14, tg0);              // TTBR0 granule size
  DEF_ENUM_FIELD(ArmShareabilityAttribute, 13, 12, sh0);    // TTBR0 cache sharability
  DEF_ENUM_FIELD(ArmCacheabilityAttribute, 11, 10, orgn0);  // TTBR0 outer cacheability
  DEF_ENUM_FIELD(ArmCacheabilityAttribute, 9, 8, irgn0);    // TTBR0 inner cacheability
  DEF_BIT(7, epd0);                                         // TTBR0 table walks disabled
  DEF_RSVDZ_BIT(6);
  DEF_FIELD(5, 0, t0sz);  // TTBR0 size offset
};

ARCH_ARM64_SYSREG(ArmTcrEl1, "tcr_el1");

// This is the common base for TCR_EL2 and VTCR_EL2.  See below.
struct ArmTranslationControlRegisterEl2Base
    : public SysRegDerivedBase<ArmTranslationControlRegisterEl2Base, uint64_t> {
  ArmTranslationControlRegisterEl2Base() {
    // Bits marked RES1 need to be either preserved or set to 1. If constructing
    // the register from scratch, set them to 1.
    //
    // TODO(https://fxbug.dev/42155079): Consider adding RES1 support to hwreg library.
    set_res1_bit32(1);
    set_res1_bit23(1);
  }

  DEF_RSVDZ_FIELD(63, 62);
  DEF_BIT(33, mtx);
  DEF_BIT(32, ds);
  DEF_BIT(31, res1_bit32);  // RES1: should be preserved or written as 1.
  // Bits [30:29] differ between TCR_EL2 and VTCR_EL2.  See below.
  DEF_BIT(28, hwu62);
  DEF_BIT(27, hwu61);
  DEF_BIT(26, hwu60);
  DEF_BIT(25, hwu59);
  // Bit 24 differs between TCR_EL2 and VTCR_EL2.  See below.
  DEF_BIT(23, res1_bit23);  // RES1: should be preserved or written as 1.
  DEF_BIT(22, hd);          // Hardware Dirty state management
  DEF_BIT(21, ha);          // Hardware Access flag updated
  // Bits [20:19] differ between TCR_EL2 and VTCR_EL2.  See below.
  DEF_ENUM_FIELD(ArmPhysicalAddressSize, 18, 16, ps);       // Physical address size
  DEF_ENUM_FIELD(ArmTcrTg0Value, 15, 14, tg0);              // TTBR0 Granule size
  DEF_ENUM_FIELD(ArmShareabilityAttribute, 13, 12, sh0);    // TTBR0 Cache sharability
  DEF_ENUM_FIELD(ArmCacheabilityAttribute, 11, 10, orgn0);  // TTBR0 Outer cacheability
  DEF_ENUM_FIELD(ArmCacheabilityAttribute, 9, 8, irgn0);    // TTBR0 Inner cacheability
  // Bits [7:6] differ between TCR_EL2 and VTCR_EL2.  See below.
  DEF_FIELD(5, 0, t0sz);  // TTBR0 size offset
};

// Translation Control Register (TCR) for EL2.
//
// This register layout is only valid when HCR_EL2.E2H == 0 (that is,
// Virtualization Host Extensions are disabled).
//
// [arm/v8]: D13.2.121 TCR_EL2, Translation Control Register (EL2)
struct ArmTcrEl2 : public SysRegDerived<ArmTcrEl2, ArmTranslationControlRegisterEl2Base> {
  // Copy values that have direct equivalents in TCR_EL1.
  ArmTcrEl2& CopyEl1(const ArmTcrEl1& tcr_el1) {
    set_ds(tcr_el1.ds());
    set_tcma(tcr_el1.tcma0());
    set_tbid(tcr_el1.tbid0());
    set_hpd(tcr_el1.hpd0());
    set_hd(tcr_el1.hd());
    set_ha(tcr_el1.ha());
    set_tbi(tcr_el1.tbi0());
    set_tg0(tcr_el1.tg0());
    set_sh0(tcr_el1.sh0());
    set_orgn0(tcr_el1.orgn0());
    set_irgn0(tcr_el1.irgn0());
    set_t0sz(tcr_el1.t0sz());
    return *this;
  }

  DEF_BIT(30, tcma);
  DEF_BIT(29, tbid);
  DEF_BIT(24, hpd);  // Hierarchical Permission Disable
  DEF_BIT(20, tbi);  // Top byte ignored
  DEF_RSVDZ_FIELD(7, 6);
};
ARCH_ARM64_SYSREG(ArmTcrEl2, "tcr_el2");

// Copy values that have direct equivalents in TCR_EL2.
inline ArmTcrEl1& ArmTcrEl1::CopyEl2(const ArmTcrEl2& tcr_el2) {
  set_mtx0(tcr_el2.mtx());
  set_ds(tcr_el2.ds());
  set_tcma0(tcr_el2.tcma());
  set_tbid0(tcr_el2.tbid());
  set_hpd0(tcr_el2.hpd());
  set_hd(tcr_el2.hd());
  set_ha(tcr_el2.ha());
  set_tbi0(tcr_el2.tbi());
  set_tg0(tcr_el2.tg0());
  set_sh0(tcr_el2.sh0());
  set_orgn0(tcr_el2.orgn0());
  set_irgn0(tcr_el2.irgn0());
  set_t0sz(tcr_el2.t0sz());
  return *this;
}

// Virtualization Translation Control Register (VTCR_EL2).
//
// [arm/v8]: VTCR_EL2, Virtualization Translation Control Register
struct ArmVtcrEl2 : public SysRegDerived<ArmVtcrEl2, ArmTranslationControlRegisterEl2Base> {
  // Most fields are the same as in TCR_EL2, but these few differ.
  DEF_RSVDZ_FIELD(63, 46);
  DEF_BIT(45, hdbss);
  DEF_BIT(44, haft);
  DEF_RSVDZ_FIELD(43, 42);
  DEF_BIT(41, tl0);
  DEF_BIT(40, gcsh);
  DEF_RSVDZ_BIT(39);
  DEF_BIT(38, d128);
  DEF_BIT(37, s2poe);
  DEF_BIT(36, s2pie);
  DEF_BIT(35, tl1);
  DEF_BIT(34, assured_only);
  DEF_BIT(33, sl2);
  DEF_BIT(30, nsa);
  DEF_BIT(29, nsw);
  DEF_RSVDZ_BIT(24);
  DEF_BIT(19, vs);
  DEF_FIELD(7, 6, sl0);
};
ARCH_ARM64_SYSREG(ArmVtcrEl2, "vtcr_el2");

// Extended Translation Control Register (TCR2_EL1)
//
// [arm/v9]: D23.2.172 TCR2_EL1, Translation Control Register (EL1)
class ArmTcr2El1 : public SysRegBase<ArmTcr2El1> {
 public:
  DEF_RSVDZ_FIELD(63, 22);
  DEF_BIT(21, fngna1);  // Force non-global for unassured using TTBR1. (FEAT_THE)
  DEF_BIT(20, fngna0);  // Force non-global for unassured using TTBR0. (FEAT_THE)
  DEF_RSVDZ_BIT(19);
  DEF_BIT(18, fng1);    // Force non-global translations for TTBR1. (FEAT_ASID2)
  DEF_BIT(17, fng0);    // Force non-global translations for TTBR0. (FEAT_ASID2)
  DEF_BIT(16, a2);      // Enable use of two ASIDs. (FEAT_ASID2
  DEF_BIT(15, disch1);  // Disable contiguous bit for start table for TTBR1. (FEAT_D128)
  DEF_BIT(14, disch0);  // Disable contiguous bit for start table for TTBR0. (FEAT_D128)
  DEF_RSVDZ_FIELD(13, 12);
  DEF_BIT(11, haft);   // Hardware managed access flag for table descriptors. (FEAT_HAFT)
  DEF_BIT(10, pttwi);  // Permit translation table walk incoherence. (FEAT_THE)
  DEF_RSVDZ_FIELD(9, 6);
  DEF_BIT(5, d128);   // Enable 128 bit translation tables. (FEAT_D128)
  DEF_BIT(4, aie);    // Enable attribute indexing extension. (FEAT_AIE)
  DEF_BIT(3, poe);    // Enable permission overlays for privileged accesses. (FEAT_S1POE)
  DEF_BIT(2, e0poe);  // Enable permission overlays for unprivileged accesses. (FEAT_S1POE)
  DEF_BIT(1, pie);    // Enable indirect permission scheme. (FEAT_S1PIE)
  DEF_BIT(0, pnch);   // Enable protected attribute enable. (FEAT_THE)
};

// Use alternate register description to work around GCC not allowing the use of the name.
ARCH_ARM64_SYSREG(ArmTcr2El1, "S3_0_c2_c0_3");  // TCR2_EL1

// Page table root pointer.
//
// This common format is used for several registers which contain
// the root of the page table.
//
// [arm/v8]: D13.2.132 TTBR0_EL1, Translation Table Base Register 0 (EL1)
// [arm/v8]: D13.2.133 TTBR0_EL2, Translation Table Base Register 0 (EL2)
// [arm/v8]: D13.2.134 TTBR0_EL3, Translation Table Base Register 0 (EL3)
// [arm/v8]: D13.2.135 TTBR1_EL1, Translation Table Base Register 1 (EL1)
// [arm/v8]: D13.2.136 TTBR1_EL2, Translation Table Base Register 1 (EL2)
struct ArmTranslationTableBaseRegister
    : public SysRegDerivedBase<ArmTranslationTableBaseRegister, uint64_t> {
  DEF_FIELD(63, 48, asid);
  DEF_UNSHIFTED_FIELD(47, 1, addr);  // Bits [47:1] of the root table physical address.
  DEF_BIT(0, cnp);                   // Common not private.
};

struct ArmTtbr0El1 : public arch::SysRegDerived<ArmTtbr0El1, ArmTranslationTableBaseRegister> {};
ARCH_ARM64_SYSREG(ArmTtbr0El1, "ttbr0_el1");

struct ArmTtbr0El2 : public arch::SysRegDerived<ArmTtbr0El2, ArmTranslationTableBaseRegister> {};
ARCH_ARM64_SYSREG(ArmTtbr0El2, "ttbr0_el2");

struct ArmTtbr0El3 : public arch::SysRegDerived<ArmTtbr0El3, ArmTranslationTableBaseRegister> {};
ARCH_ARM64_SYSREG(ArmTtbr0El3, "ttbr0_el3");

struct ArmTtbr1El1 : public arch::SysRegDerived<ArmTtbr1El1, ArmTranslationTableBaseRegister> {};
ARCH_ARM64_SYSREG(ArmTtbr1El1, "ttbr1_el1");

struct ArmTtbr1El2 : public arch::SysRegDerived<ArmTtbr1El2, ArmTranslationTableBaseRegister> {};
ARCH_ARM64_SYSREG(ArmTtbr1El2, "ttbr1_el2");

// [arm/v8]: VTTBR_EL2, Virtualization Translation Table Base Register (EL2)
struct ArmVttbrEl2 : public arch::SysRegDerived<ArmVttbrEl2, ArmTranslationTableBaseRegister> {
  // The layout is the same ar TTBR0_ELx, but the ASID field is called VMID.
  uint64_t vmid() const { return asid(); }
  void set_vmid(uint64_t vmid) { set_asid(vmid); }
};
ARCH_ARM64_SYSREG(ArmVttbrEl2, "vttbr_el2");

// This state is accessed via multiple registers with different bit placements.
// The three registers DAIF, DAIFSet, and DAIFClr are specified in:
// [arm/sysreg]/currentel: DAIF, Interrupt Mask Bits
struct ArmDaif : public SysRegBase<ArmDaif, uint64_t> {
  DEF_BIT(9, d);
  DEF_BIT(8, a);
  DEF_BIT(7, i);
  DEF_BIT(6, f);
};
ARCH_ARM64_SYSREG(ArmDaif, "daif");

// This is the bit layout used in DAIFSet and DAIFClr for the same bits that
// can be read or modified with different placements via DAIF.  These two
// pseudo-registers are accessed via a special MSR instruction form that takes
// only a four-bit immediate value.  These registers can't really be used from
// C++ through the normal mechanism, because their intrinsics only accept a
// constant argument and any layers of inline function around the intrinsics
// prevent the compiler from allowing a value to be passed down even if it's
// all done as constexpr.
struct ArmDaifSetClr : public SysRegBase<ArmDaifSetClr, uint64_t> {
  DEF_BIT(3, d);
  DEF_BIT(2, a);
  DEF_BIT(1, i);
  DEF_BIT(0, f);
};

// [arm/sysreg]/vbar_el1: Vector Base Address Register (EL1)
// [arm/sysreg]/vbar_el2: Vector Base Address Register (EL2)
// [arm/sysreg]/vbar_el3: Vector Base Address Register (EL3)
struct ArmVectorBaseAddressRegister : public SysRegDerivedBase<ArmVectorBaseAddressRegister> {
  DEF_UNSHIFTED_FIELD(63, 11, addr);
  DEF_RSVDZ_FIELD(10, 0);
};

struct ArmVbarEl1 : public arch::SysRegDerived<ArmVbarEl1, ArmVectorBaseAddressRegister> {};
ARCH_ARM64_SYSREG(ArmVbarEl1, "vbar_el1");

struct ArmVbarEl2 : public arch::SysRegDerived<ArmVbarEl2, ArmVectorBaseAddressRegister> {};
ARCH_ARM64_SYSREG(ArmVbarEl2, "vbar_el2");

struct ArmVbarEl3 : public arch::SysRegDerived<ArmVbarEl3, ArmVectorBaseAddressRegister> {};
ARCH_ARM64_SYSREG(ArmVbarEl3, "vbar_el3");

// [arm/sysreg]/elr_el1: Exception Link Register (EL1)
// [arm/sysreg]/elr_el2: Exception Link Register (EL2)
// [arm/sysreg]/elr_el3: Exception Link Register (EL3)
struct ArmVectorExceptionLinkRegister : public SysRegDerivedBase<ArmVectorExceptionLinkRegister> {
  DEF_FIELD(63, 0, pc);
};

struct ArmElrEl1 : public arch::SysRegDerived<ArmElrEl1, ArmVectorExceptionLinkRegister> {};
ARCH_ARM64_SYSREG(ArmElrEl1, "elr_el1");

struct ArmElrEl2 : public arch::SysRegDerived<ArmElrEl2, ArmVectorExceptionLinkRegister> {};
ARCH_ARM64_SYSREG(ArmElrEl2, "elr_el2");

struct ArmElrEl3 : public arch::SysRegDerived<ArmElrEl3, ArmVectorExceptionLinkRegister> {};
ARCH_ARM64_SYSREG(ArmElrEl3, "elr_el3");

// [arm/sysreg]/sp_el0: Stack Pointer (EL0)
// [arm/sysreg]/sp_el1: Stack Pointer (EL1)
// [arm/sysreg]/sp_el2: Stack Pointer (EL2)
struct ArmStackPointerRegister : public SysRegDerivedBase<ArmStackPointerRegister> {
  DEF_FIELD(63, 0, sp);
};

struct ArmSpEl0 : public arch::SysRegDerived<ArmSpEl0, ArmStackPointerRegister> {};
ARCH_ARM64_SYSREG(ArmSpEl0, "sp_el0");

struct ArmSpEl1 : public arch::SysRegDerived<ArmSpEl1, ArmStackPointerRegister> {};
ARCH_ARM64_SYSREG(ArmSpEl1, "sp_el1");

struct ArmSpEl2 : public arch::SysRegDerived<ArmSpEl2, ArmStackPointerRegister> {};
ARCH_ARM64_SYSREG(ArmSpEl2, "sp_el2");

// [arm/sysreg]/spsr_el1: Saved Program Status Register (El1)
// [arm/sysreg]/spsr_el2: Saved Program Status Register (El2)
// [arm/sysreg]/spsr_el3: Saved Program Status Register (El3)
//
// These are the assignments when an exception is taken from AArch64 state.
struct ArmSavedProgramStatusRegister : public SysRegDerivedBase<ArmSavedProgramStatusRegister> {
  enum class ExceptionLevel : uint32_t {
    kEl0t = 0b0000,  // EL0 using SP_EL0
    kEl1t = 0b0100,  // EL1 using SP_EL0
    kEl1h = 0b0101,  // EL1 using SP_EL1
    kEl2t = 0b1000,  // EL2 using SP_EL0
    kEl2h = 0b1001,  // EL2 using SP_EL2
    kEl3t = 0b1100,  // EL3 using SP_EL0
    kEl3h = 0b1101,  // EL3 using SP_EL3
  };

  // EL this exception was taken from.
  ArmCurrentEl el() const { return ArmCurrentEl::Get().FromValue(static_cast<uint64_t>(m())); }

  // SPSel state at the exception, i.e. true if it used SP_ELx.
  bool spsel() const { return static_cast<uint32_t>(m()) & 1; }

  DEF_RSVDZ_FIELD(63, 32);

  DEF_BIT(31, n);
  DEF_BIT(30, z);
  DEF_BIT(29, c);
  DEF_BIT(28, v);
  DEF_RSVDZ_FIELD(27, 26);
  DEF_BIT(25, tco);
  DEF_BIT(24, dit);
  DEF_BIT(23, uao);
  DEF_BIT(22, pan);
  DEF_BIT(21, ss);
  DEF_BIT(20, il);
  DEF_RSVDZ_FIELD(19, 13);
  DEF_BIT(12, ssbs);
  DEF_FIELD(11, 10, btype);
  DEF_BIT(9, d);
  DEF_BIT(8, a);
  DEF_BIT(7, i);
  DEF_BIT(6, f);
  DEF_RSVDZ_BIT(5);
  DEF_BIT(4, a32);  // Always zero in this format.
  DEF_ENUM_FIELD(ExceptionLevel, 3, 0, m);
};

struct ArmSpsrEl1 : public arch::SysRegDerived<ArmSpsrEl1, ArmSavedProgramStatusRegister> {};
ARCH_ARM64_SYSREG(ArmSpsrEl1, "spsr_el1");

struct ArmSpsrEl2 : public arch::SysRegDerived<ArmSpsrEl2, ArmSavedProgramStatusRegister> {};
ARCH_ARM64_SYSREG(ArmSpsrEl2, "spsr_el2");

struct ArmSpsrEl3 : public arch::SysRegDerived<ArmSpsrEl3, ArmSavedProgramStatusRegister> {};
ARCH_ARM64_SYSREG(ArmSpsrEl3, "spsr_el3");

// [arm/sysreg]/nzcv: Condition Flags
//
// This is a subset of SPSR_ELx that is accessible R/W to everyone.
struct ArmNzcv : public SysRegBase<ArmNzcv, uint64_t> {
  DEF_RSVDZ_FIELD(63, 32);
  DEF_BIT(31, n);
  DEF_BIT(30, z);
  DEF_BIT(29, c);
  DEF_BIT(28, v);
  DEF_RSVDZ_FIELD(27, 0);
};
ARCH_ARM64_SYSREG(ArmNzcv, "nzcv");

// [arm/sysreg]/esr_el1: Exception Syndrome Register (El1)
// [arm/sysreg]/esr_el2: Exception Syndrome Register (El2)
// [arm/sysreg]/esr_el3: Exception Syndrome Register (El3)
//
// These are the assignments when an exception is taken from AArch64 state.
struct ArmExceptionSyndromeRegister
    : public SysRegDerivedBase<ArmExceptionSyndromeRegister, uint64_t> {
  // Some values are only possible in ESR_EL2 and/or ESR_EL3.
  enum class ExceptionClass : uint32_t {
    kUnknown = 0b000000,
    kWf = 0b000001,          // WF
    kMcr = 0b000011,         // MCR or MRC
    kMcrr = 0b000100,        // MCRR or MRRC
    kMcrCoproc = 0b000101,   // MCR or MRC (coproc=0b1110)
    kLdc = 0b000110,         // LDC or STC
    kFp = 0b000111,          // SVE or SIMD
    kLd64b = 0b001010,       // LD64B, ST64B, ST64BV, or ST64BVO
    kMcrrCoproc = 0b001100,  // MRRC (coproc==0b1110)
    kBti = 0b001101,         // Branch target exception
    kIllegalExecution = 0b001110,
    kSvc32 = 0b010001,
    kHvc32 = 0b010010,  // EL2, EL3
    kSmc32 = 0b010011,  // EL2, EL3
    kTrappedSysreg128 = 0b010100,
    kSvc64 = 0b010101,
    kHvc64 = 0b010110,  // EL2, EL3
    kSmc64 = 0b010111,  // EL2, EL3
    kMsr = 0b011000,    // MSR, MRS, or System Instruction
    kSve = 0b011001,
    kEret = 0b011010,  // EL2, EL3
    kTstart = 0b011011,
    kPac = 0b011100,
    kSme = 0b011101,
    kImplementationDefined = 0b011111,  // EL3
    kInstructionAbortLowerEl = 0b100000,
    kInstructionAbortSameEl = 0b100001,
    kPcAlignment = 0b100010,
    kDataAbortLowerEl = 0b100100,
    kDataAbortSameEl = 0b100101,
    kSpAlignment = 0b100110,
    kMops = 0b100111,
    kFpe32 = 0b101000,
    kFpe64 = 0b101100,
    kGcs = 0b101101,
    kSerror = 0b101111,
    kBreakpointLowerEl = 0b110000,
    kBreakpointSameEl = 0b110001,
    kStepLowerEl = 0b110010,
    kStepSameEl = 0b110011,
    kWatchpointLowerEl = 0b110100,
    kWatchpointSameEl = 0b110101,
    kBkpt = 0b111000,         // AArch32 BKPT #<n>
    kVectorCatch = 0b111010,  // EL2, EL3
    kBrk = 0b111100,          // AArch64 BRK #<n>
    kProfiling = 0b111101,

    // Unused values in this range reserved for future synchronous exceptions.
    kFirstReservedSynchronous = 0b000000,
    kLastReservedSynchronous = 0b101100,

    // Unused values in this range reserved for future exceptions, possibly
    // synchronous or possibly asynchronous..
    kFirstReservedMaybeAsynchronous = 0b101101,
    kLastReservedMaybeAsynchronous = 0b111111,
  };

  DEF_RSVDZ_FIELD(63, 56);

  DEF_FIELD(55, 32, iss2);
  DEF_ENUM_FIELD(ExceptionClass, 31, 26, ec);
  DEF_BIT(25, il);
  DEF_FIELD(24, 0, iss);
};

struct ArmEsrEl1 : public arch::SysRegDerived<ArmEsrEl1, ArmExceptionSyndromeRegister> {};
ARCH_ARM64_SYSREG(ArmEsrEl1, "esr_el1");

struct ArmEsrEl2 : public arch::SysRegDerived<ArmEsrEl2, ArmExceptionSyndromeRegister> {};
ARCH_ARM64_SYSREG(ArmEsrEl2, "esr_el2");

struct ArmEsrEl3 : public arch::SysRegDerived<ArmEsrEl3, ArmExceptionSyndromeRegister> {};
ARCH_ARM64_SYSREG(ArmEsrEl3, "esr_el3");

// [arm/sysreg]/cptr_el2: Architectural Feature Trap Register (EL2)
// [arm/sysreg]/cptr_el3: Architectural Feature Trap Register (EL3)
//
// The layout is dependent upon whether there is an EL2 host running (i.e, when
// FEAT_VHE is implemented and HCR_EL2.E2H == 1)
template <bool El2Host>
struct ArmArchitecturalFeatureTrapRegister
    : public SysRegDerivedBase<ArmArchitecturalFeatureTrapRegister<El2Host>, uint64_t> {
  using SelfType = ArmArchitecturalFeatureTrapRegister<El2Host>;

  DEF_RSVDZ_FIELD(63, 32);
  DEF_BIT(31, tcpac);
  DEF_BIT(30, tam);
  DEF_RSVDZ_BIT(29);

  DEF_COND_BIT(28, tta, El2Host);
  DEF_COND_RSVDZ_BIT(28, !El2Host);

  DEF_RSVDZ_FIELD(27, 26);

  DEF_COND_FIELD(25, 24, smen, El2Host);
  DEF_COND_RSVDZ_FIELD(25, 24, !El2Host);

  DEF_RSVDZ_FIELD(23, 22);

  DEF_COND_FIELD(21, 20, fpen, El2Host);
  DEF_COND_RSVDZ_BIT(21, !El2Host);
  DEF_COND_BIT(20, tta, !El2Host);

  DEF_RSVDZ_FIELD(19, 18);

  DEF_COND_FIELD(17, 16, zen, El2Host);
  DEF_COND_RSVDZ_FIELD(15, 0, El2Host);

  DEF_COND_RSVDZ_FIELD(17, 14, !El2Host);
  // Bit 13 is reserved as 1 if !El2Host
  DEF_COND_BIT(12, tsm, !El2Host);
  DEF_COND_RSVDZ_BIT(11, !El2Host);
  DEF_COND_BIT(10, tfp, !El2Host);
  // Bit 9 is reserved as 1 if !El2Host
  DEF_COND_BIT(8, tz, !El2Host);
  DEF_COND_RSVDZ_FIELD(7, 0, !El2Host);
};

template <bool El2Host>
struct ArmCptrEl2 : public arch::SysRegDerived<ArmCptrEl2<El2Host>,
                                               ArmArchitecturalFeatureTrapRegister<El2Host>> {};
ARCH_ARM64_SYSREG(ArmCptrEl2<false>, "cptr_el2");
ARCH_ARM64_SYSREG(ArmCptrEl2<true>, "cptr_el2");

using ArmCptrEl2NoEl2Host = ArmCptrEl2<false>;
using ArmCptrEl2WithEl2Host = ArmCptrEl2<true>;

template <bool El2Host>
struct ArmCptrEl3 : public arch::SysRegDerived<ArmCptrEl3<El2Host>,
                                               ArmArchitecturalFeatureTrapRegister<El2Host>> {};

ARCH_ARM64_SYSREG(ArmCptrEl3<false>, "cptr_el3");
ARCH_ARM64_SYSREG(ArmCptrEl3<true>, "cptr_el3");

using ArmCptrEl3NoEl2Host = ArmCptrEl3<false>;
using ArmCptrEl3WithEl2Host = ArmCptrEl3<true>;

// [arm/sysreg]/hcr_el2: Hypervisor Configuration register (EL2)
struct ArmHypervisorConfigurationRegister
    : public SysRegDerivedBase<ArmHypervisorConfigurationRegister, uint64_t> {
  DEF_FIELD(63, 60, twedel);
  DEF_BIT(59, tweden);
  DEF_BIT(58, tid5);
  DEF_BIT(57, dct);
  DEF_BIT(56, ata);
  DEF_BIT(55, ttlbos);
  DEF_BIT(54, ttlbis);
  DEF_BIT(53, enscxt);
  DEF_BIT(52, tocu);
  DEF_BIT(51, amvoffen);
  DEF_BIT(50, ticab);
  DEF_BIT(49, tid4);
  DEF_BIT(48, gpf);
  DEF_BIT(47, fien);
  DEF_BIT(46, fwb);
  DEF_BIT(45, nv2);
  DEF_BIT(44, at);
  DEF_BIT(43, nv1);
  DEF_BIT(42, nv);
  DEF_BIT(41, api);
  DEF_BIT(40, apk);
  DEF_BIT(39, tme);
  DEF_BIT(38, miocnce);
  DEF_BIT(37, tea);
  DEF_BIT(36, terr);
  DEF_BIT(35, tlor);
  DEF_BIT(34, e2h);
  DEF_BIT(33, id);
  DEF_BIT(32, cd);
  DEF_BIT(31, rw);
  DEF_BIT(30, trvm);
  DEF_BIT(29, hcd);
  DEF_BIT(28, tdz);
  DEF_BIT(27, tge);
  DEF_BIT(26, tvm);
  DEF_BIT(25, ttlb);
  DEF_BIT(24, tpu);
  DEF_BIT(23, tcpc);
  DEF_BIT(22, tsw);
  DEF_BIT(21, tacr);
  DEF_BIT(20, tidcp);
  DEF_BIT(19, tsc);
  DEF_BIT(18, tid3);
  DEF_BIT(17, tid2);
  DEF_BIT(16, tid1);
  DEF_BIT(15, tid0);
  DEF_BIT(14, twe);
  DEF_BIT(13, twi);
  DEF_BIT(12, dc);
  DEF_FIELD(11, 10, bsu);
  DEF_BIT(9, fb);
  DEF_BIT(8, vse);
  DEF_BIT(7, vi);
  DEF_BIT(6, vf);
  DEF_BIT(5, amo);
  DEF_BIT(4, imo);
  DEF_BIT(3, fmo);
  DEF_BIT(2, ptw);
  DEF_BIT(1, swio);
  DEF_BIT(0, vm);
};

struct ArmHcrEl2 : public arch::SysRegDerived<ArmHcrEl2, ArmHypervisorConfigurationRegister> {};
ARCH_ARM64_SYSREG(ArmHcrEl2, "hcr_el2");

// [arm/sysreg]/hcrx_el2: Extended Hypervisor Configuration register (EL2)
struct ArmExtendedHypervisorConfigurationRegister
    : public SysRegDerivedBase<ArmExtendedHypervisorConfigurationRegister, uint64_t> {
  DEF_RSVDZ_FIELD(63, 27);  // Safe "pass-through" value for no-op EL2.
  DEF_BIT(26, srmasken);    // if FEAT_SRMASK
  DEF_RSVDZ_BIT(25);
  DEF_BIT(24, pacmen);      // if FEAT_PAuth_LR
  DEF_BIT(23, enfpm);       // if FEAT_FPMR
  DEF_BIT(22, gcsen);       // if FEAT_GCS
  DEF_BIT(21, enidcp128);   // if FEAT_SYSREG128
  DEF_BIT(20, ensderr);     // if FEAT_ADERR
  DEF_BIT(19, tmea);        // if FEAT_DoubleFault2
  DEF_BIT(18, ensnerr);     // if FEAT_ANERR
  DEF_BIT(17, d128en);      // if FEAT_D128
  DEF_BIT(16, pttwi);       // if FEAT_THE
  DEF_BIT(15, sctlr2en);    // if FEAT_SCTLR2
  DEF_BIT(14, tcr2en);      // if FEAT_TCR2
  DEF_RSVDZ_FIELD(13, 12);  // RES0
  DEF_BIT(11, mscen);       // if FEAT_MOPS
  DEF_BIT(10, mce2);        // if FEAT_MOPS
  DEF_BIT(9, cmow);         // if CEAT_CMOW
  DEF_BIT(8, vfnmi);        // if FEAT_NMI
  DEF_BIT(7, vinmi);        // if FEAT_NMI
  DEF_BIT(6, tallint);      // if FEAT_NMI
  DEF_BIT(5, smpme);        // if FEAT_SME
  DEF_BIT(4, fgtnxs);       // if FEAT_XS
  DEF_BIT(3, fnxs);         // if FEAT_XS
  DEF_BIT(2, enasr);        // if FEAT_LS64_V
  DEF_BIT(1, enals);        // if FEAT_LS64
  DEF_BIT(0, enas0);        // if FEAT_LS64_ACCDATA
};

struct ArmHcrxEl2
    : public arch::SysRegDerived<ArmHcrxEl2, ArmExtendedHypervisorConfigurationRegister> {};
ARCH_ARM64_SYSREG(ArmHcrxEl2, "hcrx_el2");

// [arm/sysreg]/cnthctl_el2: Counter-timer Hypervisor Control register (EL2)
//
// The layout is dependent upon whether there is an EL2 host running (i.e, when
// FEAT_VHE is implemented and HCR_EL2.E2H == 1)
template <bool El2Host>
struct ArmCounterTimerHypervisorControl
    : public SysRegDerivedBase<ArmCounterTimerHypervisorControl<El2Host>, uint64_t> {
  using SelfType = ArmCounterTimerHypervisorControl<El2Host>;

  DEF_RSVDZ_FIELD(63, 20);
  DEF_BIT(19, cntpmask);
  DEF_BIT(18, cntvmask);
  DEF_BIT(17, evntis);
  DEF_BIT(16, el1nvvct);
  DEF_BIT(15, el1nvpct);
  DEF_BIT(14, el1tvct);
  DEF_BIT(13, el1tvt);
  DEF_BIT(12, ecv);

  DEF_COND_BIT(11, el1pten, El2Host);
  DEF_COND_BIT(10, el1pcten, El2Host);
  DEF_COND_BIT(9, el0pten, El2Host);
  DEF_COND_BIT(8, el0vten, El2Host);
  DEF_COND_RSVDZ_FIELD(11, 8, !El2Host);

  DEF_FIELD(7, 4, evnti);
  DEF_BIT(3, evntdir);
  DEF_BIT(2, evnten);

  DEF_COND_BIT(1, el0vcten, El2Host);
  DEF_COND_BIT(1, el1pcen, !El2Host);

  DEF_COND_BIT(0, el0pcten, El2Host);
  DEF_COND_BIT(0, el1pcten, !El2Host);
};

template <bool El2Host>
struct ArmCnthctlEl2 : public arch::SysRegDerived<ArmCnthctlEl2<El2Host>,
                                                  ArmCounterTimerHypervisorControl<El2Host>> {};
ARCH_ARM64_SYSREG(ArmCnthctlEl2<false>, "cnthctl_el2");
ARCH_ARM64_SYSREG(ArmCnthctlEl2<true>, "cnthctl_el2");

using ArmCnthctlEl2NoEl2Host = ArmCnthctlEl2<false>;
using ArmCnthctlEl2WithEl2Host = ArmCnthctlEl2<true>;

// [arm/sysreg]/icc_sre_el1: Interrupt Controller System Register Enable Register (EL1)
// [arm/sysreg]/icc_sre_el2: Interrupt Controller System Register Enable Register (EL2)
// [arm/sysreg]/icc_sre_el3: Interrupt Controller System Register Enable Register (EL3)
struct ArmInterruptControllerSystemRegisterEnableRegister
    : public SysRegDerivedBase<ArmInterruptControllerSystemRegisterEnableRegister, uint64_t> {
  DEF_RSVDZ_FIELD(63, 4);
  DEF_BIT(3, enable);
  DEF_BIT(2, dib);
  DEF_BIT(1, dfb);
  DEF_BIT(0, sre);
};

struct ArmIccSreEl1
    : public arch::SysRegDerived<ArmIccSreEl1, ArmInterruptControllerSystemRegisterEnableRegister> {
};
struct ArmIccSreEl2
    : public arch::SysRegDerived<ArmIccSreEl2, ArmInterruptControllerSystemRegisterEnableRegister> {
};
struct ArmIccSreEl3
    : public arch::SysRegDerived<ArmIccSreEl3, ArmInterruptControllerSystemRegisterEnableRegister> {
};
ARCH_ARM64_SYSREG(ArmIccSreEl1, "icc_sre_el1");
ARCH_ARM64_SYSREG(ArmIccSreEl2, "icc_sre_el2");
ARCH_ARM64_SYSREG(ArmIccSreEl3, "icc_sre_el3");

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_SYSTEM_H_
