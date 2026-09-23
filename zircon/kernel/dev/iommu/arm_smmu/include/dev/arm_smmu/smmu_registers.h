// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_REGISTERS_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_REGISTERS_H_

namespace arm_smmu {

#if LK_DEBUGLEVEL > 0
using EnablePrinting = ::hwreg::EnablePrinter;
#else
using EnablePrinting = void;
#endif

// Global Register Space 0:
//
// Register offsets are relative to Smmu::gr0_base_.
//
namespace gr0 {

class CR0 : public hwreg::RegisterBase<CR0, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<CR0>(0x00); }

  DEF_BIT(31, VMID16EN);
  DEF_BIT(30, HYPMODE);
  DEF_FIELD(29, 28, NSCFG);
  DEF_FIELD(27, 26, WACFG);
  DEF_FIELD(25, 24, RACFG);
  DEF_FIELD(23, 22, SHCFG);
  DEF_BIT(21, SMCFCFG);
  DEF_BIT(20, MTCFG);
  DEF_FIELD(19, 16, MemAttr);
  DEF_FIELD(15, 14, BSU);
  DEF_BIT(13, FB);
  DEF_BIT(11, VMIDPNE);
  DEF_BIT(12, PTM);
  DEF_BIT(10, USFCFG);
  DEF_BIT(9, GSE);
  DEF_BIT(8, STALLD);
  DEF_FIELD(7, 6, TRASNIENTCFG);
  DEF_BIT(5, GCFGFIE);
  DEF_BIT(4, GCFGFRE);
  DEF_BIT(3, EXIDENABLE);
  DEF_BIT(2, GFIE);
  DEF_BIT(1, GFRE);
  DEF_BIT(0, CLIENTPD);
};

class CR2 : public hwreg::RegisterBase<CR2, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<CR2>(0x08); }

  DEF_BIT(31, EXSMRGENABLE);
  DEF_RSVDZ_BIT(30);
  DEF_BIT(29, COMPINDEXENABLE);
  DEF_RSVDZ_FIELD(26, 16);
  DEF_FIELD(7, 0, BPVMID);
};

class ACR : public hwreg::RegisterBase<ACR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<ACR>(0x10); }

  DEF_FIELD(31, 0, Impl);
};

class IDR0 : public hwreg::RegisterBase<IDR0, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<IDR0>(0x20); }

  DEF_BIT(31, SES);
  DEF_BIT(30, S1TS);
  DEF_BIT(29, S2TS);
  DEF_BIT(28, NTS);
  DEF_BIT(27, SMS);
  DEF_BIT(26, ATOSNS);
  DEF_FIELD(25, 24, PTFS);
  DEF_FIELD(23, 16, NUMIRPT);
  DEF_BIT(15, EXSMRGS);
  DEF_BIT(14, CTTW);
  DEF_BIT(13, BTM);
  DEF_FIELD(12, 9, NUMSIDB);
  DEF_BIT(8, EXIDS);
  DEF_FIELD(7, 0, NUMSMRG);
};

class IDR1 : public hwreg::RegisterBase<IDR1, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<IDR1>(0x24); }

  DEF_BIT(31, PAGESIZE);
  DEF_FIELD(30, 28, NUMPAGENDXB);
  DEF_FIELD(25, 24, HAFDBS);
  DEF_FIELD(23, 16, NUMS2CB);
  DEF_BIT(15, SMCD);
  DEF_FIELD(13, 12, SSDTP);
  DEF_FIELD(11, 8, NUMSSDNDXB);
  DEF_FIELD(7, 0, NUMCB);
};

class IDR2 : public hwreg::RegisterBase<IDR2, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<IDR2>(0x28); }

  DEF_BIT(30, DIPANS);
  DEF_BIT(29, COMPINDEXS);
  DEF_BIT(28, HADS);
  DEF_BIT(27, E2HS);
  DEF_FIELD(26, 16, EXNUMSMGR);
  DEF_BIT(15, VMID16S);
  DEF_BIT(14, PTFSv8_64kb);
  DEF_BIT(13, PTFSv8_16kb);
  DEF_BIT(12, PTFSv8_4kb);
  DEF_FIELD(11, 8, UBS);
  DEF_FIELD(7, 4, OAS);
  DEF_FIELD(3, 0, IAS);
};

class IDR7 : public hwreg::RegisterBase<IDR7, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<IDR7>(0x3c); }

  DEF_FIELD(7, 4, major);
  DEF_FIELD(3, 0, minor);
};

// Note; the address in the fault address register has N active bits, where N is
// determined by the value of IDR2.UBS (upstream bus size);
class GFAR : public hwreg::RegisterBase<GFAR, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GFAR>(0x40); }

  DEF_FIELD(63, 0, FADDR);
};

class GFSR : public hwreg::RegisterBase<GFSR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GFSR>(0x48); }

  DEF_BIT(31, MULTI);
  DEF_BIT(8, UUT);
  DEF_BIT(7, PF);
  DEF_BIT(6, EF);
  DEF_BIT(5, CAF);
  DEF_BIT(4, UCIF);
  DEF_BIT(3, UCBF);
  DEF_BIT(2, SMCF);
  DEF_BIT(1, USF);
  DEF_BIT(0, ICF);
};

class GFSRRESTORE : public hwreg::RegisterBase<GFSRRESTORE, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GFSRRESTORE>(0x4c); }

  DEF_BIT(31, MULTI);
  DEF_BIT(8, UUT);
  DEF_BIT(7, PF);
  DEF_BIT(6, EF);
  DEF_BIT(5, CAF);
  DEF_BIT(4, UCIF);
  DEF_BIT(3, UCBF);
  DEF_BIT(2, SMCF);
  DEF_BIT(1, USF);
  DEF_BIT(0, ICF);
};

class GFSYNR0 : public hwreg::RegisterBase<GFSYNR0, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GFSYNR0>(0x50); }

  DEF_FIELD(15, 8, Impl);
  DEF_BIT(6, ATS);
  DEF_BIT(5, NSATTR);
  DEF_BIT(4, NSSTATE);
  DEF_BIT(3, IND);
  DEF_BIT(2, PNU);
  DEF_BIT(1, WNR);
  DEF_BIT(0, Nested);
};

class GFSYNR1 : public hwreg::RegisterBase<GFSYNR1, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GFSYNR1>(0x54); }

  DEF_FIELD(31, 16, SSD_Index);
  DEF_FIELD(15, 0, StreamID);
};

class GFSYNR2 : public hwreg::RegisterBase<GFSYNR2, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GFSYNR2>(0x58); }

  DEF_FIELD(31, 0, Impl);
};

class STLBIALL : public hwreg::RegisterBase<STLBIALL, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<STLBIALL>(0x60); }
  DEF_BIT(0, Trigger);
};

class TLBIVMID : public hwreg::RegisterBase<TLBIVMID, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVMID>(0x64); }
  DEF_BIT(0, Trigger);
};

class TLBIALLNSNH : public hwreg::RegisterBase<TLBIALLNSNH, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIALLNSNH>(0x68); }
  DEF_BIT(0, Trigger);
};

class TLBIALLH : public hwreg::RegisterBase<TLBIALLH, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIALLH>(0x6c); }
  DEF_BIT(0, Trigger);
};

class TLBGSYNC : public hwreg::RegisterBase<TLBGSYNC, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBGSYNC>(0x70); }
  DEF_BIT(0, Trigger);
};

class TLBGSTATUS : public hwreg::RegisterBase<TLBGSTATUS, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBGSTATUS>(0x74); }
  DEF_BIT(0, GSACTIVE);
};

class TLBIIVAH : public hwreg::RegisterBase<TLBIIVAH, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIVAH>(0x78); }
  DEF_FIELD(31, 12, Address);
};

class TLBIIVALM : public hwreg::RegisterBase<TLBIIVALM, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIVALM>(0xA0); }
  DEF_FIELD(43, 0, Address);
};

class TLBIIVAM : public hwreg::RegisterBase<TLBIIVAM, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIVAM>(0xA8); }
  DEF_FIELD(43, 0, Address);
};

class TLBIIVALH64 : public hwreg::RegisterBase<TLBIIVALH64, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIVALH64>(0xB0); }
  DEF_FIELD(43, 0, Address);
};

class TLBIIVMIDS1 : public hwreg::RegisterBase<TLBIIVMIDS1, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIVMIDS1>(0xB8); }
  DEF_FIELD(31, 12, Address);
};

class TLBIIALLM : public hwreg::RegisterBase<TLBIIALLM, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIALLM>(0xBC); }
  DEF_BIT(0, Trigger);
};

class TLBIIVAH64 : public hwreg::RegisterBase<TLBIIVAH64, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIVAH64>(0xC0); }
  DEF_FIELD(43, 0, Address);
};

class GATS1UR : public hwreg::RegisterBase<GATS1UR, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS1UR>(0x100); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GATS1UW : public hwreg::RegisterBase<GATS1UW, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS1UW>(0x108); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GATS1PR : public hwreg::RegisterBase<GATS1PR, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS1PR>(0x110); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GATS1PW : public hwreg::RegisterBase<GATS1PW, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS1PW>(0x118); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GATS12UR : public hwreg::RegisterBase<GATS12UR, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS12UR>(0x120); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GATS12UW : public hwreg::RegisterBase<GATS12UW, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS12UW>(0x128); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GATS12PR : public hwreg::RegisterBase<GATS12PR, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS12PR>(0x130); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GATS12PW : public hwreg::RegisterBase<GATS12PW, uint64_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATS12PW>(0x138); }

  DEF_FIELD(63, 12, Addr);
  DEF_FIELD(7, 0, NDX);
};

class GPAR : public hwreg::RegisterBase<GPAR, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GPAR>(0x180); }

  DEF_FIELD(63, 56, MATTR);
  DEF_FIELD(47, 12, PA);
  DEF_BIT(10, IMP);
  DEF_BIT(9, NS);
  DEF_FIELD(8, 7, SH);
  DEF_BIT(0, F);
};

class GATSR : public hwreg::RegisterBase<GATSR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<GATSR>(0x188); }

  DEF_BIT(0, ACTIVE);
};

// There may be up to 128 stream match registers in the implementation, numbered
// as [SMR0 .. SMR127].  Use Get(<N>) to fetch the proper index at runtime.
//
// SMR registers are a part of a stream match register group.  Read IDR0.NUMSMRG
// to determine the total number of stream match register groups in the system.
//
class SMR : public hwreg::RegisterBase<SMR, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<SMR>(0x800 + (ndx << 2)); }

  DEF_BIT(31, VALID);
  DEF_FIELD(30, 16, MASK);
  DEF_FIELD(14, 0, ID);
};

// There may be up to 128 stream-to-context registers in the implementation, numbered
// as [S2CR0 .. S2CR127].  Use Get(<N>) to fetch the proper index at runtime.
//
// S2CR registers are a part of a stream match register group.  Read IDR0.NUMSMRG
// to determine the total number of stream match register groups in the system.
//
// Additionally, there are 3 different encodings for the fields of the S2CR
// registers, depending on the value present in "TYPE".  They are:
//
// TYPE = 0 : This is a "Translation" context.
// TYPE = 1 : This entry is in "Bypass mode", no translation takes place.
// TYPE = 2 : This is a "Fault" context. Any transaction that maps to this
//            Stream mapping group incurs an invalid context fault.
//
// When reading, users may use the base version of the register to perform the
// read and examine the type field.  Afterwards, they can instantiate an
// alternate definition of the register based on the type in order to access
// type specific field.s
//
// When writing, users may directly use the properly typed version, but they are
// responsible for correctly populating the TYPE field.  It will *not* be done
// automatically.
//
class S2CR : public hwreg::RegisterBase<S2CR, uint32_t, EnablePrinting> {
 public:
  enum class Type {
    kTranslation = 0,
    kBypass = 1,
    kFault = 2,
    kInvalid = 3,
  };

  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<S2CR>(0xC00 + (ndx << 2)); }

  DEF_ENUM_FIELD(Type, 17, 16, TYPE);
};

class S2CR_Type0 : public hwreg::RegisterBase<S2CR_Type0, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<S2CR_Type0>(0xC00 + (ndx << 2)); }

  DEF_FIELD(31, 30, Impl);
  DEF_FIELD(29, 28, TRANSIENTCFG);
  DEF_FIELD(27, 26, INSTCFG);
  DEF_FIELD(25, 24, PRIVCFG);
  DEF_FIELD(23, 22, WACFG);
  DEF_FIELD(21, 20, RACFG);
  DEF_FIELD(19, 18, NSCFG);
  DEF_ENUM_FIELD(S2CR::Type, 17, 16, TYPE);
  DEF_FIELD(15, 12, MemAttr);
  DEF_BIT(11, MTCFG);
  DEF_BIT(10, EXIDVALID);
  DEF_FIELD(9, 8, SHCFG);
  DEF_FIELD(7, 0, CBNDX);
};

class S2CR_Type1 : public hwreg::RegisterBase<S2CR_Type1, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<S2CR_Type1>(0xC00 + (ndx << 2)); }

  DEF_FIELD(31, 30, Impl);
  DEF_FIELD(29, 28, TRANSIENTCFG);
  DEF_BIT(26, FB);
  DEF_FIELD(25, 24, BSU);
  DEF_FIELD(23, 22, WACFG);
  DEF_FIELD(21, 20, RACFG);
  DEF_FIELD(19, 18, NSCFG);
  DEF_ENUM_FIELD(S2CR::Type, 17, 16, TYPE);
  DEF_FIELD(15, 12, MemAttr);
  DEF_BIT(11, MTCFG);
  DEF_BIT(10, EXIDVALID);
  DEF_FIELD(9, 8, SHCFG);
  DEF_FIELD(7, 0, VMID);
};

class S2CR_Type2 : public hwreg::RegisterBase<S2CR_Type2, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<S2CR_Type2>(0xC00 + (ndx << 2)); }

  DEF_FIELD(31, 28, Impl);
  DEF_ENUM_FIELD(S2CR::Type, 17, 16, TYPE);
  DEF_BIT(10, EXIDVALID);
};

// Aliases which can be helpful when selecting specific versions of a S2CR
using S2CR_Translation = S2CR_Type0;
using S2CR_Bypass = S2CR_Type1;
using S2CR_Fault = S2CR_Type2;

}  // namespace gr0

// Global Register Space 1:
//
// Register offsets are relative to Smmu::gr1_base_.
//
namespace gr1 {

// Section 10.2.1: Context Bank Attribute Registers
//
// The field definitions in a CBAR depend on the value of the TYPE field in the
// register.
//
class CBAR : public hwreg::RegisterBase<CBAR, uint32_t, EnablePrinting> {
 public:
  enum class Type {
    kS2Translation = 0,
    kS1TS2Bypass = 1,
    kS1TS2Fault = 2,
    kS1TS2Translate = 3,
  };

  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CBAR>(0x0 + (ndx << 2)); }

  DEF_ENUM_FIELD(Type, 17, 16, TYPE);
};

// Type 0 CBARs are "Stage 2 Context" registers.
class CBAR_Type0 : public hwreg::RegisterBase<CBAR_Type0, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CBAR_Type0>(0x0 + (ndx << 2)); }

  DEF_FIELD(7, 0, VMID);
  DEF_ENUM_FIELD(CBAR::Type, 17, 16, TYPE);
  DEF_RSVDZ_FIELD(19, 18);
  DEF_FIELD(31, 24, IRPTNDX);
};

// Type 1 CBARs are "Stage 1 Context with stage 2 bypass" registers.
class CBAR_Type1 : public hwreg::RegisterBase<CBAR_Type1, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CBAR_Type1>(0x0 + (ndx << 2)); }

  DEF_FIELD(7, 0, VMID);
  DEF_FIELD(9, 8, BPSHCFG);

  // Bit 10 is either:
  // + The HYPC bit when:
  //   + This is a v1 SMMU, or
  //   + This is a v2 SMMU, and (IDR2.E2HS is 0 or (IDR2.E2HS is 1 and CR0.HYPMODE is 0))
  // + The E2HC bit when:
  //   + This is a v2 SMMU, and
  //   + IDR2.E2HS is 1, and
  //   + CR0.HYPMODE is 1
  //
  // `libhwreg` does not permit any field overlapping or aliasing, so we have to
  // combine these two names into one name.
  DEF_BIT(10, HYPC_E2HC);
  DEF_BIT(11, FB);
  DEF_FIELD(15, 12, MemAttr);
  DEF_ENUM_FIELD(CBAR::Type, 17, 16, TYPE);
  DEF_FIELD(19, 18, BSU);
  DEF_FIELD(21, 20, RACFG);
  DEF_FIELD(23, 22, WACFG);
  DEF_FIELD(31, 24, IRPTNDX);
};

// Type 2 CBARs are "Stage 1 Context with stage 2 fault" registers.
class CBAR_Type2 : public hwreg::RegisterBase<CBAR_Type2, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CBAR_Type2>(0x0 + (ndx << 2)); }

  DEF_ENUM_FIELD(CBAR::Type, 17, 16, TYPE);
  DEF_FIELD(19, 18, SBZ);  // SBZ == "should be zero"
  DEF_FIELD(31, 24, IRPTNDX);
};

// Type 3 CBARs are "Stage 1 followed by stage 2 translation context" registers.
class CBAR_Type3 : public hwreg::RegisterBase<CBAR_Type3, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CBAR_Type3>(0x0 + (ndx << 2)); }

  DEF_FIELD(7, 0, VMID);
  DEF_FIELD(15, 8, CBNDX);
  DEF_ENUM_FIELD(CBAR::Type, 17, 16, TYPE);
  DEF_FIELD(19, 18, SBZ);  // SBZ == "should be zero"
  DEF_FIELD(31, 24, IRPTNDX);
};

// Aliases which can be helpful when selecting specific versions of a CBAR
using CBAR_S2Translation = CBAR_Type0;
using CBAR_S1TS2Bypass = CBAR_Type1;
using CBAR_S1TS2Fault = CBAR_Type2;
using CBAR_S1TS2Translate = CBAR_Type3;

// Section 10.2.3: Context Bank Fault Restricted Syndrome Register A
class CBFRSYNRA : public hwreg::RegisterBase<CBFRSYNRA, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CBFRSYNRA>(0x400 + (ndx << 2)); }

  DEF_FIELD(15, 0, StreamID);
  DEF_FIELD(31, 16, SSD_Index);
};

// Section 10.2.2: Context Bank Attribute Registers
class CBA2R : public hwreg::RegisterBase<CBA2R, uint32_t, EnablePrinting> {
 public:
  static auto Get(uint32_t ndx) { return hwreg::RegisterAddr<CBA2R>(0x800 + (ndx << 2)); }

  DEF_BIT(0, VA64);
  DEF_BIT(1, MONC);
  DEF_FIELD(31, 16, VMID16);
};

}  // namespace gr1

// Stage 1 Translation Context Bank Registers.
//
// Register offsets are relative to SMMU_CBn_BASE, aka: Smmu::cb_regs(n).
//
namespace s1cbr {

// Section 16.5.29: System Control Register
class SCTLR : public hwreg::RegisterBase<SCTLR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<SCTLR>(0x00); }

  DEF_BIT(0, M);
  DEF_BIT(1, TRE);
  DEF_BIT(2, AFE);
  DEF_BIT(3, AFFD);
  DEF_BIT(4, E);
  DEF_BIT(5, CFRE);
  DEF_BIT(6, CFIE);
  DEF_BIT(7, CFCFG);
  DEF_BIT(8, HUPCF);
  DEF_BIT(9, WXN);
  DEF_BIT(10, UWXN);
  DEF_BIT(11, __res1);
  DEF_BIT(12, ASIDPNE);
  DEF_FIELD(15, 14, TRANSIENTCFG);
  DEF_FIELD(19, 16, MemAttr);
  DEF_BIT(20, MTCFG);
  DEF_BIT(21, __res2);
  DEF_FIELD(23, 22, SHCFG);
  DEF_FIELD(25, 24, RACFG);
  DEF_FIELD(27, 26, WACFG);
  DEF_FIELD(29, 28, NSCFG);
  DEF_BIT(30, UCI);
  DEF_BIT(31, __res3);
};

// Section 16.5.1: Auxiliary Control Register
class ACTLR : public hwreg::RegisterBase<ACTLR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<ACTLR>(0x04); }

  // The SMMU_CBn_ACTLR bit assignments are IMPLEMENTATION DEFINED
  DEF_FIELD(31, 0, value);
};

// Section 16.5.28: Transaction Resume register
class RESUME : public hwreg::RegisterBase<RESUME, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<RESUME>(0x08); }

  DEF_BIT(0, TnR);
};

// Section 16.5.39: Translation Control Register 2
class TCR2 : public hwreg::RegisterBase<TCR2, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TCR2>(0x10); }

  DEF_FIELD(2, 0, PASize);
  DEF_BIT(3, __res1);
  DEF_BIT(4, AS);
  DEF_BIT(5, TBI0);
  DEF_BIT(6, TBI1);
  DEF_BIT(7, __res2);
  DEF_BIT(8, HAD0);
  DEF_BIT(9, HAD1);
  DEF_BIT(10, HA);
  DEF_BIT(11, HD);
  DEF_FIELD(13, 12, __res3);
  DEF_BIT(14, NSCFG0);
  DEF_FIELD(17, 15, SEP);
  DEF_FIELD(29, 18, __res4);
  DEF_BIT(30, NSCFG1);
  DEF_BIT(31, __res5);
};

// Section 16.5.40: Translation Table Base Registers
//
// TTBRs have a few different formats depending on which translation scheme this
// context bank is using.  These schemes are:
//
// + AArch32 Short-descriptor (AddrMode::k32Bit)
// + AArch32 Long-descriptor  (AddrMode::kExt32Bit)
// + AArch64                  (AddrMode::k64Bit)
//
class TTBR0_32Bit : public hwreg::RegisterBase<TTBR0_32Bit, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TTBR0_32Bit>(0x20); }

  DEF_BIT(0, IRGN1);
  DEF_BIT(1, S);
  DEF_BIT(2, IMP);
  DEF_FIELD(4, 3, RGN);
  DEF_BIT(5, NOS);
  DEF_BIT(6, IRGN1);
  DEF_FIELD(31, 7, BaseAddress);

  TTBR0_32Bit& SetBaseAddrFromValue(uint32_t base_address_value) {
    set_BaseAddress(base_address_value >> 7);
    return *this;
  }
  uint32_t BaseAddrValue() const { return BaseAddress() << 7; }
};

class TTBR0_Ext32Bit : public hwreg::RegisterBase<TTBR0_Ext32Bit, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TTBR0_Ext32Bit>(0x20); }

  DEF_RSVDZ_FIELD(2, 0);
  DEF_FIELD(47, 3, BaseAddress);
  DEF_FIELD(55, 48, ASID);
  DEF_FIELD(63, 56, __res1);

  TTBR0_Ext32Bit& SetBaseAddrFromValue(uint64_t base_address_value) {
    set_BaseAddress(base_address_value >> 3);
    return *this;
  }
  uint64_t BaseAddrValue() const { return BaseAddress() << 3; }
};

class TTBR0_64Bit : public hwreg::RegisterBase<TTBR0_64Bit, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TTBR0_64Bit>(0x20); }

  DEF_RSVDZ_FIELD(2, 0);
  DEF_FIELD(47, 3, BaseAddress);
  DEF_FIELD(63, 48, ASID);

  TTBR0_64Bit& SetBaseAddrFromValue(uint64_t base_address_value) {
    set_BaseAddress(base_address_value >> 3);
    return *this;
  }
  uint64_t BaseAddrValue() const { return BaseAddress() << 3; }
};

class TTBR1_32Bit : public hwreg::RegisterBase<TTBR1_32Bit, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TTBR1_32Bit>(0x28); }

  DEF_BIT(0, IRGN1);
  DEF_BIT(1, S);
  DEF_BIT(2, IMP);
  DEF_FIELD(4, 3, RGN);
  DEF_BIT(5, NOS);
  DEF_BIT(6, IRGN1);
  DEF_FIELD(31, 7, BaseAddress);

  TTBR1_32Bit& SetBaseAddrFromValue(uint32_t base_address_value) {
    set_BaseAddress(base_address_value >> 7);
    return *this;
  }
  uint32_t BaseAddrValue() const { return BaseAddress() << 7; }
};

class TTBR1_Ext32Bit : public hwreg::RegisterBase<TTBR1_Ext32Bit, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TTBR1_Ext32Bit>(0x28); }

  DEF_RSVDZ_FIELD(2, 0);
  DEF_FIELD(47, 3, BaseAddress);
  DEF_FIELD(55, 48, ASID);
  DEF_FIELD(63, 56, __res1);

  TTBR1_Ext32Bit& SetBaseAddrFromValue(uint64_t base_address_value) {
    set_BaseAddress(base_address_value >> 3);
    return *this;
  }
  uint64_t BaseAddrValue() const { return BaseAddress() << 3; }
};

class TTBR1_64Bit : public hwreg::RegisterBase<TTBR1_64Bit, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TTBR1_64Bit>(0x28); }

  DEF_RSVDZ_FIELD(2, 0);
  DEF_FIELD(47, 3, BaseAddress);
  DEF_FIELD(63, 48, ASID);

  TTBR1_64Bit& SetBaseAddrFromValue(uint64_t base_address_value) {
    set_BaseAddress(base_address_value >> 3);
    return *this;
  }
  uint64_t BaseAddrValue() const { return BaseAddress() << 3; }
};

// Section 16.5.38: Translation Control Register
//
// There are three possible formats for the TCR based on the values of
// CBA2R.VA64 and TCR.EAE, which determine the addressing mode for the context
// bank.
//
//        Type       | CBA2R.VA64 | TCR.EAE |
// ------------------+------------+---------+
//   32 Bit          |      0     |    0    |
//   Extended 32 Bit |      0     |    1    |
//   64 Bit          |      1     |    x    |
//
class TCR : public hwreg::RegisterBase<TCR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TCR>(0x30); }

  DEF_BIT(31, EAE);
};

class TCR_32Bit : public hwreg::RegisterBase<TCR_32Bit, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TCR_32Bit>(0x30); }

  DEF_FIELD(2, 0, T0SZ);
  DEF_RSVDZ_BIT(3);
  DEF_BIT(4, PD0);
  DEF_BIT(5, PD1);
  DEF_BIT(14, NSCFG0);
  DEF_BIT(30, NSCFG1);
  DEF_BIT(31, EAE);
};

class TCR_Ext32Bit : public hwreg::RegisterBase<TCR_Ext32Bit, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TCR_Ext32Bit>(0x30); }

  DEF_FIELD(2, 0, T0SZ);
  DEF_BIT(7, EPD0);
  DEF_FIELD(9, 8, IRGN0);
  DEF_FIELD(11, 10, ORGN0);
  DEF_FIELD(13, 12, SH0);
  DEF_BIT(14, NSCFG0);
  DEF_FIELD(18, 16, T1SZ);
  DEF_BIT(22, A1);
  DEF_BIT(23, EPD1);
  DEF_FIELD(25, 24, IRGN1);
  DEF_FIELD(27, 26, ORGN1);
  DEF_FIELD(29, 28, SH1);
  DEF_BIT(30, NSCFG0);
  DEF_BIT(31, EAE);
};

class TCR_64Bit : public hwreg::RegisterBase<TCR_64Bit, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TCR_64Bit>(0x30); }

  DEF_FIELD(5, 0, T0SZ);
  DEF_BIT(7, EPD0);
  DEF_FIELD(9, 8, IRGN0);
  DEF_FIELD(11, 10, ORGN0);
  DEF_FIELD(13, 12, SH0);
  DEF_FIELD(15, 14, TG0);
  DEF_FIELD(21, 16, T1SZ);
  DEF_BIT(22, A1);
  DEF_BIT(23, EPD1);
  DEF_FIELD(25, 24, IRGN1);
  DEF_FIELD(27, 26, ORGN1);
  DEF_FIELD(29, 28, SH1);
  DEF_FIELD(31, 30, TG1);
};

// Section 16.5.7: Context Identification Register
//
// The Context ID register has one of two forms depending on whether TCR.EAE is
// 0 or not.  The two forms are defined as CONTEXTIDR_EAE[01] here.
class CONTEXTIDR_EAE0 : public hwreg::RegisterBase<CONTEXTIDR_EAE0, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<CONTEXTIDR_EAE0>(0x34); }

  DEF_FIELD(7, 0, ASID);
  DEF_FIELD(31, 8, PROCID);
};

class CONTEXTIDR_EAE1 : public hwreg::RegisterBase<CONTEXTIDR_EAE1, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<CONTEXTIDR_EAE1>(0x34); }

  DEF_FIELD(31, 0, PROCID);
};

// Section 16.5.27: Primary Region Remap Register
//
// Used when the AArch32 Short-descriptor translation scheme is being used.
class PRRR : public hwreg::RegisterBase<PRRR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<PRRR>(0x38); }

  DEF_FIELD(1, 0, TR0);
  DEF_FIELD(3, 2, TR1);
  DEF_FIELD(5, 4, TR2);
  DEF_FIELD(7, 6, TR3);
  DEF_FIELD(9, 8, TR4);
  DEF_FIELD(11, 10, TR5);
  DEF_FIELD(13, 12, TR6);
  DEF_FIELD(15, 14, TR7);
  DEF_BIT(16, DS0);
  DEF_BIT(17, DS1);
  DEF_BIT(18, NS0);
  DEF_BIT(19, NS1);
  DEF_BIT(24, NOS0);
  DEF_BIT(25, NOS1);
  DEF_BIT(26, NOS2);
  DEF_BIT(27, NOS3);
  DEF_BIT(28, NOS4);
  DEF_BIT(29, NOS5);
  DEF_BIT(30, NOS6);
  DEF_BIT(31, NOS7);
};

// Section 16.5.13: Normal Memory Remap Register
//
// Used when the AArch32 Short-descriptor translation scheme is being used.
class NMRR : public hwreg::RegisterBase<NMRR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<NMRR>(0x3c); }

  DEF_FIELD(1, 0, IR0);
  DEF_FIELD(3, 2, IR1);
  DEF_FIELD(5, 4, IR2);
  DEF_FIELD(7, 6, IR3);
  DEF_FIELD(9, 8, IR4);
  DEF_FIELD(11, 10, IR5);
  DEF_FIELD(13, 12, IR6);
  DEF_FIELD(15, 14, IR7);
  DEF_FIELD(17, 16, OR0);
  DEF_FIELD(19, 18, OR1);
  DEF_FIELD(21, 20, OR2);
  DEF_FIELD(23, 22, OR3);
  DEF_FIELD(25, 24, OR4);
  DEF_FIELD(27, 26, OR5);
  DEF_FIELD(29, 28, OR6);
  DEF_FIELD(31, 30, OR7);
};

// Section 16.5.12: Memory Attribute Indirection Registers
//
// Used when the AArch32 Long-descriptor, or the AArch64 translation scheme is
// being used.
class MAIR0 : public hwreg::RegisterBase<MAIR0, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<MAIR0>(0x38); }

  DEF_FIELD(7, 0, Attr0);
  DEF_FIELD(15, 8, Attr1);
  DEF_FIELD(23, 16, Attr2);
  DEF_FIELD(31, 24, Attr3);
};

class MAIR1 : public hwreg::RegisterBase<MAIR1, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<MAIR1>(0x3c); }

  DEF_FIELD(7, 0, Attr4);
  DEF_FIELD(15, 8, Attr5);
  DEF_FIELD(23, 16, Attr6);
  DEF_FIELD(31, 24, Attr7);
};

// Section 16.5.14: Physical Address Register
//
// The PAR has three different forms depending on:
//
// 1) The specific address translation scheme we are using.
// 2) Whether or not there is a fault reported in the register (F == 1)
//
// The names we use to distinguish between the forms are:
//
// + PAR          : Defines the F bit so that a specific type may be chosen.
// + PAR_Type0_F0 : AArch32 Short-descriptor, no fault (F == 0)
// + PAR_Type1_F0 : AArch32 Long-descriptor, or AArch64, no fault (F == 0)
// + PAR_F1       : Yes fault (F == 1)
//
class PAR : public hwreg::RegisterBase<PAR, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<PAR>(0x50); }

  DEF_BIT(0, F);
};

class PAR_Type0_F0 : public hwreg::RegisterBase<PAR_Type0_F0, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<PAR_Type0_F0>(0x50); }

  DEF_BIT(0, F);
  DEF_BIT(1, SS);
  DEF_FIELD(3, 2, Outer);
  DEF_FIELD(6, 4, Inner);
  DEF_BIT(7, SH);
  DEF_BIT(8, IMP);
  DEF_BIT(9, NS);
  DEF_BIT(10, NOS);
  DEF_RSVDZ_BIT(11);
  DEF_FIELD(31, 12, PA);
};

class PAR_Type1_F0 : public hwreg::RegisterBase<PAR_Type1_F0, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<PAR_Type1_F0>(0x50); }

  DEF_BIT(0, F);
  DEF_FIELD(8, 7, SH);
  DEF_BIT(9, NS);
  DEF_BIT(10, IMP);
  DEF_FIELD(47, 12, PA);
  DEF_FIELD(63, 56, MATTR);
};

class PAR_F1 : public hwreg::RegisterBase<PAR_F1, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<PAR_F1>(0x50); }

  DEF_BIT(0, F);
  DEF_BIT(1, TF);
  DEF_BIT(2, AFF);
  DEF_BIT(3, PF);
  DEF_BIT(4, EF);
  DEF_BIT(5, TLBMCF);
  DEF_BIT(6, TLBLKF);
  DEF_BIT(7, ASF);
  DEF_FIELD(9, 8, Format);
  DEF_BIT(29, ICF);
  DEF_BIT(31, ATOT);
  DEF_FIELD(33, 32, PLVL);
  DEF_BIT(35, STAGE);
};

// Section 16.5.9: Fault Status Register
class FSR : public hwreg::RegisterBase<FSR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<FSR>(0x58); }

  DEF_BIT(1, TF);
  DEF_BIT(2, AFF);
  DEF_BIT(3, PF);
  DEF_BIT(4, EF);
  DEF_BIT(5, TLBMCF);
  DEF_BIT(6, TLBLKF);
  DEF_BIT(7, ASF);
  DEF_BIT(8, UUT);
  DEF_FIELD(10, 9, Format);
  DEF_BIT(30, SS);
  DEF_BIT(31, MULTI);
};

// Section 16.5.10: Fault Status Restore Register
class FSRRESTORE : public hwreg::RegisterBase<FSRRESTORE, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<FSRRESTORE>(0x5c); }

  DEF_BIT(1, TF);
  DEF_BIT(2, AFF);
  DEF_BIT(3, PF);
  DEF_BIT(4, EF);
  DEF_BIT(5, TLBMCF);
  DEF_BIT(6, TLBLKF);
  DEF_BIT(7, ASF);
  DEF_BIT(8, UUT);
  DEF_BIT(31, MULTI);
};

// Section 16.5.8: Fault Address Register
//
// The FADDR field of the FAR is defined as bits [N-1, 0], where N is
// implementation defined, but must be >= IDR2.UBS.  Unimplemented bits are
// required to be RAZ, so we define the field as occupying all 64 bits.
//
// Additionally, the bottom M bits (M is determined by the translation granule
// size) _may_ contain the lowest M bits of the
// address which produced the fault, but they are not _required_ to do so as
// there may be significant implementation cost in providing the bits.  So,
// _technically_ these bits are `UNKNOWN`.
class FAR : public hwreg::RegisterBase<FAR, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<FAR>(0x60); }

  DEF_FIELD(63, 0, FADDR);
};

// Section 16.5.11: Fault Syndrome Registers
class FSYNR0 : public hwreg::RegisterBase<FSYNR0, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<FSYNR0>(0x68); }

  DEF_FIELD(1, 0, PLVL);
  DEF_BIT(4, WNR);
  DEF_BIT(5, PNU);
  DEF_BIT(6, IND);
  DEF_BIT(8, NSATTR);
  DEF_BIT(10, PTWF);
  DEF_BIT(11, AFR);
  DEF_FIELD(23, 16, S1CBNDX);
};

class FSYNR1 : public hwreg::RegisterBase<FSYNR1, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<FSYNR1>(0x6c); }
  // FSYNR1 bits are all "implementation defined"
};

// Section 16.5.32: TLB Invalidate by VA
class TLBIVA_AArch64 : public hwreg::RegisterBase<TLBIVA_AArch64, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVA_AArch64>(0x600); }

  DEF_FIELD(43, 0, Address_55_12);
  DEF_FIELD(63, 48, ASID);

  TLBIVA_AArch64& set_Address(uint64_t addr) {
    set_Address_55_12(addr >> 12);
    return *this;
  }
  uint64_t Address() const { return Address_55_12() << 12; }
};

class TLBIVA_AArch32 : public hwreg::RegisterBase<TLBIVA_AArch32, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVA_AArch32>(0x600); }

  DEF_FIELD(7, 0, ASID);
  DEF_FIELD(31, 12, VA);
};

// Section 16.5.33: TLB Invalidate by VA all ASID
class TLBIVAA_AArch64 : public hwreg::RegisterBase<TLBIVAA_AArch64, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVAA_AArch64>(0x608); }

  DEF_FIELD(43, 0, Address_55_12);
};

class TLBIVAA_AArch32 : public hwreg::RegisterBase<TLBIVAA_AArch32, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVAA_AArch32>(0x608); }

  DEF_FIELD(31, 12, VA);
};

// Section 16.5.31: TLB Invalidate by ASID
//
// Note: for SMMUv1, and when using AArch32 translation schemes, the ASID is
// only 8 bits (bits [0, 7]).
class TLBIASID : public hwreg::RegisterBase<TLBIASID, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIASID>(0x610); }

  DEF_FIELD(15, 0, ASID);
};

// Section 16.5.30: TLB Invalidate All
//
// Note: no fields are formally defined for this register.  The documentation
// says "This operation requires no arguments".  Simply writing to the register
// is all that is needed, however we reserve-as-zero all of the bits so that we
// always write 0.
class TLBIALL : public hwreg::RegisterBase<TLBIALL, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIALL>(0x618); }

  DEF_RSVDZ_FIELD(31, 0);
};

// Section 16.5.35: TLB Invalidate by VA, Last level
class TLBIVAL_AArch64 : public hwreg::RegisterBase<TLBIVAL_AArch64, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVAL_AArch64>(0x620); }

  DEF_FIELD(43, 0, Address_55_12);
  DEF_FIELD(63, 48, ASID);
};

class TLBIVAL_AArch32 : public hwreg::RegisterBase<TLBIVAL_AArch32, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVAL_AArch32>(0x620); }

  DEF_FIELD(31, 12, VA);
};

// Section 16.5.34: TLB Invalidate by VA, All ASID, Last level
class TLBIVAAL_AArch64 : public hwreg::RegisterBase<TLBIVAAL_AArch64, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVAAL_AArch64>(0x628); }

  DEF_FIELD(43, 0, Address_55_12);
};

class TLBIVAAL_AArch32 : public hwreg::RegisterBase<TLBIVAAL_AArch32, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIVAAL_AArch32>(0x628); }

  DEF_FIELD(31, 12, VA);
};

// Section 16.5.37: TLB Synchronize Invalidate
//
// Note: as with TLBIALL, no fields are formally defined for this register.  A
// write operation is all that is required, we reserve all of the bits as zeros.
class TLBSYNC : public hwreg::RegisterBase<TLBSYNC, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBSYNC>(0x7F0); }

  DEF_RSVDZ_FIELD(31, 0);
};

// Section 16.5.36: TLB Status
class TLBSTATUS : public hwreg::RegisterBase<TLBSTATUS, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBSTATUS>(0x7F4); }

  DEF_BIT(0, SACTIVE);
};

// Section 16.5.2: Address Translation Stage 1 Privileged Read
class ATS1PR : public hwreg::RegisterBase<ATS1PR, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<ATS1PR>(0x800); }

  DEF_FIELD(63, 12, Addr);
};

// Section 16.5.3: Address Translation Stage 1 Privileged Write
class ATS1PW : public hwreg::RegisterBase<ATS1PW, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<ATS1PW>(0x808); }

  DEF_FIELD(63, 12, Addr);
};

// Section 16.5.4: Address Translation Stage 1 Privileged Read
class ATS1UR : public hwreg::RegisterBase<ATS1UR, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<ATS1UR>(0x810); }

  DEF_FIELD(63, 12, Addr);
};

// Section 16.5.5: Address Translation Stage 1 Privileged Write
class ATS1UW : public hwreg::RegisterBase<ATS1UW, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<ATS1UW>(0x818); }

  DEF_FIELD(63, 12, Addr);
};

// Section 16.5.6: Address Translation Status Register
class ATSR : public hwreg::RegisterBase<ATSR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<ATSR>(0x8F0); }

  DEF_BIT(0, ACTIVE);
};

// Sections 16.5.15 - 16.5.26: Performance Monitors
// Performance monitor registers live in the context bank at offsets
// [0xE00, 0xFFF].  They are not defined here.

}  // namespace s1cbr

// Stage 2 Translation Context Bank Registers.
//
// Register offsets are relative to SMMU_CBn_BASE, aka: Smmu::cb_regs(n).
//
namespace s2cbr {

// Section 17.3.3: System Control Register
class SCTLR : public hwreg::RegisterBase<SCTLR, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<SCTLR>(0x00); }

  DEF_BIT(0, M);
  DEF_BIT(1, TRE);
  DEF_BIT(2, AFE);
  DEF_BIT(3, AFFD);
  DEF_BIT(4, E);
  DEF_BIT(5, CFRE);
  DEF_BIT(6, CFIE);
  DEF_BIT(7, CFCFG);
  DEF_BIT(8, HUPCF);
  DEF_FIELD(12, 9, __res1);
  DEF_BIT(13, PTW);
  DEF_FIELD(15, 14, PSU);
  DEF_FIELD(19, 16, MemAttr);
  DEF_BIT(20, __res2);
  DEF_FIELD(23, 22, SHCFG);
  DEF_FIELD(25, 24, RACFG);
  DEF_FIELD(27, 26, WACFG);
  DEF_FIELD(31, 28, __res3);
};

using ::arm_smmu::s1cbr::ACTLR;
using ::arm_smmu::s1cbr::RESUME;

// Section 17.3.9: Translation Table Base Register
class TTBR0 : public hwreg::RegisterBase<TTBR0, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TTBR0>(0x20); }
  DEF_RSVDZ_FIELD(2, 0);
  DEF_FIELD(47, 3, BaseAddress);
  DEF_FIELD(63, 48, __res1);

  TTBR0& SetBaseAddrFromValue(uint64_t base_address_value) {
    set_BaseAddress(base_address_value >> 3);
    return *this;
  }

  uint64_t BaseAddrValue() const { return BaseAddress() << 3; }
};

// Section 17.3.8: Translation Control Register
//
// There are two possible formats for the TCR based on the value of CBA2R.VA64.
//
// + CBA2R.VA64 == 0 : Extended 32-bit addressing
// + CBA2R.VA64 == 1 : 64-bit addressing
class TCR_Ext32Bit : public hwreg::RegisterBase<TCR_Ext32Bit, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TCR_Ext32Bit>(0x30); }

  DEF_FIELD(3, 0, T0SZ);
  DEF_FIELD(5, 4, __res1);
  DEF_FIELD(7, 6, SL0);
  DEF_FIELD(9, 8, IRGN0);
  DEF_FIELD(11, 10, ORGN0);
  DEF_FIELD(13, 12, SH0);
  DEF_FIELD(30, 14, __res2);
  DEF_BIT(31, EAE);
};

class TCR_64Bit : public hwreg::RegisterBase<TCR_64Bit, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TCR_64Bit>(0x30); }

  DEF_FIELD(5, 0, T0SZ);
  DEF_FIELD(7, 6, SL0);
  DEF_FIELD(9, 8, IRGN0);
  DEF_FIELD(11, 10, ORGN0);
  DEF_FIELD(13, 12, SH0);
  DEF_FIELD(15, 14, TG0);
  DEF_FIELD(18, 16, PASize);
  DEF_RSVDZ_FIELD(20, 19);
  DEF_BIT(21, HA);
  DEF_BIT(22, HB);
  DEF_RSVDZ_FIELD(31, 23);
};

// clang-format off
using ::arm_smmu::s1cbr::FSR;        // offset 0x58
using ::arm_smmu::s1cbr::FSRRESTORE; // offset 0x5c
using ::arm_smmu::s1cbr::FAR;        // offset 0x60
// clang-format on

// Section 17.3.1: Fault Syndrome Registers
class FSYNR0 : public hwreg::RegisterBase<FSYNR0, uint32_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<FSYNR0>(0x68); }

  DEF_FIELD(1, 0, PLVL);
  DEF_BIT(2, NESTED);
  DEF_BIT(3, S1PTWF);
  DEF_BIT(4, WNR);
  DEF_BIT(5, PNU);
  DEF_BIT(6, IND);
  DEF_BIT(7, UNKNOWN);
  DEF_BIT(8, NSATTR);
  DEF_BIT(9, ATOF);
  DEF_BIT(10, PTWF);
  DEF_BIT(11, AFR);
  DEF_FIELD(15, 12, __res1);
  DEF_FIELD(23, 16, S1CBNDX);
  DEF_FIELD(31, 24, __res2);
};

using ::arm_smmu::s1cbr::FSYNR1;

// Section 17.3.2: IPA Fault Address Register
//
// See the notes for s1cbr::FAR.  The same bit definition limitations apply here
// as well.
class IPAFAR : public hwreg::RegisterBase<IPAFAR, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<IPAFAR>(0x70); }

  DEF_FIELD(63, 0, FADDR);
};

// Section 17.3.4: Invalidate TLB by IPA
class TLBIIPAS2 : public hwreg::RegisterBase<TLBIIPAS2, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIPAS2>(0x630); }

  DEF_FIELD(35, 0, Address_47_12);
  DEF_FIELD(63, 36, __res1);
};

// Section 17.3.5: Invalidate TLB by IPA, Last level
class TLBIIPAS2L : public hwreg::RegisterBase<TLBIIPAS2L, uint64_t, EnablePrinting> {
 public:
  static auto Get() { return hwreg::RegisterAddr<TLBIIPAS2L>(0x638); }

  DEF_FIELD(35, 0, Address_47_12);
  DEF_FIELD(63, 36, __res1);
};

// clang-format off
using ::arm_smmu::s1cbr::TLBSYNC;    // offset 0x7f0
using ::arm_smmu::s1cbr::TLBSTATUS;  // offset 0x7f4
// clang-format on

}  // namespace s2cbr

// Enum type aliases so we can say things like CBAR_Type instead of gr1::CBAR::Type.
using CBAR_Type = gr1::CBAR::Type;
using S2CR_Type = gr0::S2CR::Type;

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_REGISTERS_H_
