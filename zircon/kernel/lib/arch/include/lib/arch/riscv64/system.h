// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_SYSTEM_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_SYSTEM_H_

#include <lib/arch/hwreg.h>
#include <lib/arch/sysreg.h>

namespace arch {

struct RiscvSstatus : public SysRegBase<RiscvSstatus, uint64_t> {
  enum class Xlen : uint64_t {
    k32 = 1,
    k64 = 2,
    k128 = 3,
  };

  enum class ExtState : uint64_t {
    kOff = 0,      // Off (FS, VS); All off (XS)
    kInitial = 1,  // Initial (FS, VS); None dirty or clean, some on (XS)
    kClean = 2,    // Clean (FS, VS); None dirty, some clean (XS)
    kDirty = 3,    // Dirty (FS, VS); Some dirty (XS)
  };

  // Bits omitted here are all WPRI (Reserved Writes Preserve Values, Reads
  // Ignore Values): only the exact values read from them should ever be
  // written back; writing other values may have no effect or may be invalid.

  DEF_BIT(63, sd);                       // State Dirty
  DEF_ENUM_FIELD(Xlen, 33, 32, uxl);     // UXLEN
  DEF_BIT(19, mxr);                      // Make eXecutable Readable
  DEF_BIT(18, sum);                      // Supervisor User Memory
  DEF_ENUM_FIELD(ExtState, 16, 15, xs);  // other eXtension State
  DEF_ENUM_FIELD(ExtState, 14, 13, fs);  // F extension State
  DEF_ENUM_FIELD(ExtState, 10, 9, vs);   // V extension State
  DEF_BIT(8, spp);                       // Supervisor Previous Privilege
  DEF_BIT(6, ube);                       // User Big-Endian
  DEF_BIT(5, spie);                      // Supervisor Previous Interrupt Enable
  DEF_BIT(1, sie);                       // Supervisor Interrupt Enable
};
ARCH_RISCV64_SYSREG(RiscvSstatus, "sstatus");

struct RiscvStvec : public SysRegBase<RiscvStvec, uint64_t> {
  enum class Mode : uint64_t {
    kDirect = 0,
    kVectored = 1,
  };

  DEF_UNSHIFTED_FIELD(63, 2, base);
  DEF_ENUM_FIELD(Mode, 1, 0, mode);
};
ARCH_RISCV64_SYSREG(RiscvStvec, "stvec");

struct RiscvScause : public SysRegBase<RiscvScause, uint64_t> {
  // exception_code values when interrupt is set.
  enum InterruptException : uint64_t {
    kSoftwareInterrupt = 1,
    kTimerInterrupt = 5,
    kExternalInterrupt = 9,
  };

  // exception_code values when interrupt is clear.
  enum Exception : uint64_t {
    kInstructionAddressMisaligned = 0,
    kInstructionAccessFault = 1,
    kIllegalInstruction = 2,
    kBreakpoint = 3,
    kLoadAddressMisaligned = 4,
    kLoadAccessFault = 5,
    kStoreAddressMisaligned = 6,
    kStoreAccessFault = 7,
    kEcallUmode = 8,
    kEcallSmode = 9,
    kInstructionPageFault = 12,
    kLoadPageFault = 13,
    kStorePageFault = 15,
  };

  DEF_BIT(63, interrupt);
  DEF_FIELD(62, 0, exception_code);

  const char* description() const {
    if (interrupt()) {
      switch (static_cast<InterruptException>(exception_code())) {
        case kSoftwareInterrupt:
          return "Supervisor software interrupt";
        case kTimerInterrupt:
          return "Supervisor timer interrupt";
        case kExternalInterrupt:
          return "Supervisor external interrupt";
      }
      return "Unknown interrupt";
    }
    switch (static_cast<Exception>(exception_code())) {
      case kInstructionAddressMisaligned:
        return "Instruction address misaligned";
      case kInstructionAccessFault:
        return "Instruction access fault";
      case kIllegalInstruction:
        return "Illegal instruction";
      case kBreakpoint:
        return "Breakpoint";
      case kLoadAddressMisaligned:
        return "Load address misaligned";
      case kLoadAccessFault:
        return "Load access fault";
      case kStoreAddressMisaligned:
        return "Store/AMO address misaligned";
      case kStoreAccessFault:
        return "Store/AMO access fault";
      case kEcallUmode:
        return "Environment call from U-mode";
      case kEcallSmode:
        return "Environment call from S-mode";
      case kInstructionPageFault:
        return "Instruction page fault";
      case kLoadPageFault:
        return "Load page fault";
      case kStorePageFault:
        return "Store/AMO page fault";
    }
    return "Unknown exception";
  }
};
ARCH_RISCV64_SYSREG(RiscvScause, "scause");

struct RiscvStval : public SysRegBase<RiscvStval, uint64_t> {
  DEF_FIELD(63, 0, value);
};
ARCH_RISCV64_SYSREG(RiscvStval, "stval");

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_SYSTEM_H_
