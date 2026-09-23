// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_BACKTRACE_UTILS_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_BACKTRACE_UTILS_H_

#include <string>

namespace debug_agent {

class ProcessHandle;
class ThreadHandle;

std::string GetBacktraceMarkupForThread(const ProcessHandle& process, const ThreadHandle& thread);

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_BACKTRACE_UTILS_H_
