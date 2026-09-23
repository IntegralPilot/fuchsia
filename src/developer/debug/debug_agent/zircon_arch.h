// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_ARCH_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_ARCH_H_

#include <lib/zx/thread.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>

#include "src/developer/debug/shared/register_info.h"

// This file contains Zircon-specific interfaces that have low-levelarchitecture-dependent
// implementations.

namespace debug_agent::arch {

// The registers in the given category are appended to the given output vector.
zx_status_t ReadRegisters(const zx::thread& thread, const debug::RegisterCategory& cat,
                          std::vector<debug::RegisterValue>& out);

// The registers must all be in the same category.
zx_status_t WriteRegisters(zx::thread& thread, const debug::RegisterCategory& cat,
                           const std::vector<debug::RegisterValue>& registers);

// Given the current register value in |regs|, applies to it the new updated values for the
// registers listed in |updates|.
zx_status_t WriteFloatingPointRegisters(const std::vector<debug::RegisterValue>& update,
                                        zx_thread_state_fp_regs_t* regs);
zx_status_t WriteVectorRegisters(const std::vector<debug::RegisterValue>& update,
                                 zx_thread_state_vector_regs_t* regs);
zx_status_t WriteDebugRegisters(const std::vector<debug::RegisterValue>& update,
                                zx_thread_state_debug_regs_t* regs);

// Converts a Zircon exception type to a debug_ipc one. Some exception types require querying the
// thread's debug registers. If needed, the given thread will be used for that.
debug_ipc::ExceptionType DecodeExceptionType(const zx::thread& thread, uint32_t exception_type);

// Converts an architecture-specific exception record to a cross-platform one.
debug_ipc::ExceptionRecord FillExceptionRecord(const zx_exception_report_t& in);

}  // namespace debug_agent::arch

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_ARCH_H_
