// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ARCH_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ARCH_H_

#if defined(__linux__)
#include <sys/user.h>
#elif defined(__Fuchsia__)
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#endif

#include "src/developer/debug/debug_agent/arch_types.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/shared/register_info.h"

namespace debug_agent {

class DebuggedThread;
class ThreadHandle;

namespace arch {

#if defined(__linux__)
using PlatformGeneralRegisters = struct user_regs_struct;
#else
using PlatformGeneralRegisters = zx_thread_state_general_regs_t;
#endif

// This file contains architecture-specific low-level helper functions. It is like zircon_utils but
// the functions will have different implementations depending on CPU architecture.
//
// The functions here should be very low-level and are designed for the real (*_zircon.cc)
// implementations of the the various primitives. Cross-platform code should use interfaces like
// ThreadHandle for anything that might need mocking out.

// Our canonical breakpoint instruction for the current architecture. This is what we'll write for
// software breakpoints. Some platforms may have alternate encodings for software breakpoints, so to
// check if something is a breakpoint instruction, use arch::IsBreakpointInstruction() rather than
// checking for equality with this value.
extern const BreakInstructionType kBreakInstruction;

// The size of the breakpoint instruction. In theory it could be different from
// sizeof(BreakInstructionType) but in practice they are the same for all architectures.
constexpr size_t kBreakInstructionSize = sizeof(BreakInstructionType);

// Distance offset from a software breakpoint instruction that the exception will be reported as
// thrown at. Architectures differ about what address is reported for a software breakpoint
// exception.
//
//  * To convert from a software breakpoint address to the exception address, add this.
//  * To convert from an exception address to the breakpoint instruction address, subtract this.
extern const int64_t kExceptionOffsetForSoftwareBreakpoint;

debug::Arch GetCurrentArch();

// Returns the number of hardware breakpoints and watchpoints on the current system.
uint32_t GetHardwareBreakpointCount();
uint32_t GetHardwareWatchpointCount();

// Converts the given register structure to a vector of debug_ipc registers.
void SaveGeneralRegs(const PlatformGeneralRegisters& input, std::vector<debug::RegisterValue>& out);

// Given the current register value in |regs|, applies to it the new updated values for the
// registers listed in |updates|.
zx_status_t WriteGeneralRegisters(const std::vector<debug::RegisterValue>& updates,
                                  PlatformGeneralRegisters* regs);

// Writes the register data to the given output variable, checking that the register data is
// the same size as the output.
template <typename RegType>
zx_status_t WriteRegisterValue(const debug::RegisterValue& reg, RegType* dest) {
  if (reg.data.size() != sizeof(RegType))
    return ZX_ERR_INVALID_ARGS;
  memcpy(dest, reg.data.data(), sizeof(RegType));
  return ZX_OK;
}

// Returns true if the given opcode is a breakpoint instruction. This checked for equality with
// kBreakInstruction and also checks other possible breakpoint encodings for the current platform.
bool IsBreakpointInstruction(BreakInstructionType instruction);

// Returns the address of the instruction that hit the exception from the address reported by the
// exception.
uint64_t BreakpointInstructionForHardwareExceptionAddress(uint64_t exception_addr);

}  // namespace arch
}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ARCH_H_
