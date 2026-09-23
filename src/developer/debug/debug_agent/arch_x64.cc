// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/arch.h"

// Notes on x64 architecture:
//
// Intel® 64 and IA-32 Architectures Software Developer’s Manual Volume 3 (3A, 3B, 3C & 3D):
// Chapter 17 holds the debug spefications:
// https://software.intel.com/sites/default/files/managed/a4/60/325383-sdm-vol-2abcd.pdf
//
// Hardware Breakpoints/Watchpoints
// -------------------------------------------------------------------------------------------------
//
// Hardware breakpoints permits to stop a thread when it accesses an address setup in one of the
// hw breakpoints registers. They will work independent whether the address in question is
// read-only or not.
//
// Watchpoints are meant to throw an exception whenever the given address is read or written to,
// depending on the configuration.
//
// DR0 to DR4 registers: There registers are the address to which the hw breakpoint/watchpoint
// refers to. How it is interpreted depends on the associated configuration on the register DR7.
//
// DR6: Debug Status Register.
//
// This register is updated when the CPU encounters a #DB harware exception. This registers permits
// users to interpret the result of an exception, such as if it was a single-step, hardware
// breakpoint, etc.
//
// zircon/system/public/zircon/hw/debug/x86.h holds a good description of what each bit within the
// register means.
//
// DR7: Debug Control Register.
//
// This register is used to establish the breakpoint conditions for the address breakpoint registers
// (DR0-DR3) and to enable debug exceptions for each of them individually.
//
// The following fields are accepted by the user. All other fields are ignored (masked):
//
// - L0, L1, L2, L3: These defines whether breakpoint/watchpoint <n> is enabled or not.
//
// - LEN0, LEN1, LEN2, LEN3: Defines the "length" of the breakpoint/watchpoint.
//                           00: 1 byte.
//                           01: 2 byte. DRn must be 2 byte aligned.
//                           10: 8 byte. DRn must be 8 byte aligned.
//                           11: 4 byte. DRn must be 4 byte aligned.
//                           p
// - RW0, RW1, RW2, RW3: The "mode" of the registers.
//                       00: Only instruction execution (hw breakpoint).
//                       01: Only data write (write watchpoint).
//                       10: Dependant by CR4.DE. Not supported by Zircon.
//                       - CR4.DE = 0: Undefined.
//                       - CR4.DE = 1: Only on I/0 read/write.
//                       11: Only on data read/write (read/write watchpoint).

namespace debug_agent {
namespace arch {

const BreakInstructionType kBreakInstruction = 0xCC;

// An X86 exception is 1 byte and a breakpoint exception is triggered with RIP pointing to the
// following instruction.
const int64_t kExceptionOffsetForSoftwareBreakpoint = 1;

::debug::Arch GetCurrentArch() { return ::debug::Arch::kX64; }

bool IsBreakpointInstruction(BreakInstructionType instruction) {
  // This handles the normal encoding of debug breakpoints (0xCC). It's also possible to cause an
  // interrupt 3 to happen using the opcode sequence 0xCD 0x03 but this has slightly different
  // semantics and no assemblers emit this. We can't easily check for that here since the
  // computation for the instruction address that is passed in assumes a 1-byte instruction. It
  // should be OK to ignore this case in practice.
  return instruction == kBreakInstruction;
}

uint64_t BreakpointInstructionForHardwareExceptionAddress(uint64_t exception_addr) {
  // x86 returns the instruction *about* to be executed when hitting the hw breakpoint.
  return exception_addr;
}

}  // namespace arch
}  // namespace debug_agent
