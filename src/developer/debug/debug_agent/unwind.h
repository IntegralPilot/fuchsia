// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_UNWIND_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_UNWIND_H_

#include <optional>
#include <vector>

#include "src/developer/debug/ipc/records.h"
#include "src/lib/unwinder/frame.h"

namespace unwinder {
class Error;
}

namespace debug_agent {

class GeneralRegisters;
class ModuleList;
class ProcessHandle;
class ThreadHandle;

// Returns the cumulative unwinder error value if the unwinding was fatally aborted - meaning that
// no unwinding strategies could unwind past the final frame in |stack| - the error message should
// include failure reasons for all attempted unwinders. Returns unwinder::Error::Success on
// successfully identified final stack frame.
unwinder::Error UnwindStack(const ProcessHandle& process, const ModuleList& modules,
                            const ThreadHandle& thread, const GeneralRegisters& regs,
                            size_t max_depth, std::vector<debug_ipc::StackFrame>* stack,
                            std::optional<unwinder::Frame::Trust> forced_unwinder = std::nullopt);

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_UNWIND_H_
