// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_STDIO_HANDLES_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_STDIO_HANDLES_H_

#if defined(__Fuchsia__)

#include <lib/zx/socket.h>

#include "src/developer/debug/shared/buffered_zx_socket.h"

#elif defined(__linux__)

#include <fbl/unique_fd.h>

#include "src/developer/debug/shared/buffered_fd.h"

#endif

namespace debug_agent {

#if defined(__Fuchsia__)
using OwnedStdioHandle = zx::socket;
using BufferedStdioHandle = debug::BufferedZxSocket;
#elif defined(__linux__)
using OwnedStdioHandle = fbl::unique_fd;
using BufferedStdioHandle = debug::BufferedFD;
#else
#error Need StdioHandles definition on this platform.
#endif

// The handles given to a launched process or component.
//
// We can add stdin in the future if we have a need.
struct StdioHandles {
  OwnedStdioHandle out;
  OwnedStdioHandle err;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_STDIO_HANDLES_H_
