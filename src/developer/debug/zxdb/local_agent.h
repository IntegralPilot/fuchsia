// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_LOCAL_AGENT_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_LOCAL_AGENT_H_

#include <memory>

#include <fbl/unique_fd.h>

#include "lib/fit/function.h"
#include "src/developer/debug/shared/buffered_bidi_pipe.h"

namespace zxdb {

struct LocalAgentResult {
  enum Status {
    // The fork request or something similar failed. An error message will have already been printed
    // to the screen and the caller should exit with the exit_code.
    kFailed,

    // The fork succeeded and the caller should continue. The agent_pid and pipe members will be
    // valid and should be used to communicate with the forked agent. The caller should be sure
    // to waitpid() on the agent_pid before exiting to prevent a zombie.
    kSuccess,

    // This is the forked process running the debug agent. The caller should exit with the exit_code
    // (representing the return value of the forked process).
    kInForked
  };
  Status status = kFailed;

  // Valid for kFailed or kInForked.
  int exit_code = 0;

  // Valid only for kSuccess.
  int agent_pid = 0;  // PID of the forked debug_agent process.
  std::unique_ptr<debug::BufferedBidiPipe> pipe;

  LocalAgentResult(Status status, int exit_code) : status(status), exit_code(exit_code) {}
  LocalAgentResult(int agent_pid, std::unique_ptr<debug::BufferedBidiPipe> pipe)
      : status(kSuccess), agent_pid(agent_pid), pipe(std::move(pipe)) {}
};

// Creates a debug_agent by forking this process and running the agent code in the resulting
// process.
LocalAgentResult ForkLocalAgent();

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_LOCAL_AGENT_H_
