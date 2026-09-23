// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/arch.h"

// Notes on ARM64 architecture:
//
// Information was obtained from the Arm® Architecture Reference Manual Armv8, for Armv8-A
// architecture profile:
//
// https://developer.arm.com/docs/ddi0487/latest/arm-architecture-reference-manual-armv8-for-armv8-a-architecture-profile
//
// In order to obtain information about the registers below, the easiest way to do it is to do
// search (ctrl-f) in the browser and the hit will probably will a link that you can press into
// the corresponding definition (eg. search for "dbgwcr" and then click on the link).
//
// See zircon/system/public/hw/debug/arm64.h for more detailed information.
//
// Hardware Breakpoints
// -------------------------------------------------------------------------------------------------
//
// Hardware breakpoints permits to stop a thread when it accesses an address setup in one of the
// hw breakpoints registers. They will work independent whether the address in question is
// read-only or not.
// ARMv8 assures at least 2 hardware breakpoints.
//
// DBGBVR<n>: HW Breakpoint Value Register.
//
// This register defines the value of the hw breakpoint <n> within the system. How that value is
// interpreted depends on the correspondent value of DBGBCR<n>.
//
// DBGBCR<n>: HW Breakpoint Control Register.
//
// Control register for HW breakpoints. There is one for each HW breakpoint present within the
// system. They go numbering by DBGBCR0, DBGBCR1, ... until the value defined in ID_AADFR0_EL1.
//
// For each control register, there is an equivalent DBGBVR<n> that holds the address the thread
// will compare against.
//
// The only register bit that needs to be set by the user is E (Bit 1). The other bits are opaque
// and should be handled by the kernel.
//
// Watchpoints
// -------------------------------------------------------------------------------------------------
//
// Watchpoints permits to stop a thread when it read/writes to a particular address in memory.
// This will work even if the address is read-only memory (for a read, of course).
// ARMv8 assures at least 2 watchpoints.
//
// DBGWVR<n>: Watchpoint Value Register.
//
// This register defines the value of the watchpoint <n> within the system. How that value is
// interpreted depends on the correspondent value of DBGWCR<n>.
//
// DBGWCR<n>: Watchpoint Control Register.
//
// Control register for watchpoints. There is one for each watchpoint present within the system.
// They go numbering by DBGWCR0, DBGWCR1, ... until the value defined ID_AAFR0_EL1.
// For each control register, there is an equivalent DBGWCR<n> that holds the address the thread
// will compare against. How this address is interpreted depends upon the configuration of the
// associated control register.
//
// The following are the bits that are most important,
//
// - E (Bit 1): Defines whether the watchpoint is enabled or not.
//
// - LSC (bits 3-4): Defines how the watchpoint works:
//                   01: Read from address.
//                   10: Write to address.
//                   11: Read/Write to address.
//
// - BAS (Bits 5-12): Defines which bytes are to be "matched" starting from the one defined in the
//                    value register. Each bit defines what bytes to match onto:
//
//                    0bxxxx'xxx1: Match DBGWVR<n> + 0
//                    0bxxxx'xx1x: Match DBGWVR<n> + 1
//                    0bxxxx'x1xx: Match DBGWVR<n> + 2
//                    0bxxxx'1xxx: Match DBGWVR<n> + 3
//                    0bxxx1'xxxx: Match DBGWVR<n> + 4
//                    0bxx1x'xxxx: Match DBGWVR<n> + 5
//                    0bx1xx'xxxx: Match DBGWVR<n> + 6
//                    0b1xxx'xxxx: Match DBGWVR<n> + 7
//
//                    These bits must be set contiguosly (there cannot be gaps between the first
//                    set bit and the last). Having DBGWVR not be 4-bytes aligned is deprecated.

namespace debug_agent {
namespace arch {

// "BRK 0" instruction.
// - Low 5 bits = 0.
// - High 11 bits = 11010100001
// - In between 16 bits is the argument to the BRK instruction (in this case zero).
const BreakInstructionType kBreakInstruction = 0xd4200000;

// ARM reports the exception for the exception instruction itself.
const int64_t kExceptionOffsetForSoftwareBreakpoint = 0;

::debug::Arch GetCurrentArch() { return ::debug::Arch::kArm64; }

bool IsBreakpointInstruction(BreakInstructionType instruction) {
  // The BRK instruction could have any number associated with it, even though we only write "BRK
  // 0", so check for the low 5 and high 11 bytes as described above.
  constexpr BreakInstructionType kMask = 0b11111111111000000000000000011111;
  return (instruction & kMask) == kBreakInstruction;
}

uint64_t BreakpointInstructionForHardwareExceptionAddress(uint64_t exception_addr) {
  // arm64 will return the address of the instruction *about* to be executed.
  return exception_addr;
}

}  // namespace arch
}  // namespace debug_agent
