// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_run.h"

#include <lib/syslog/cpp/macros.h>

#include <utility>

#include "src/developer/debug/shared/platform.h"
#include "src/developer/debug/shared/zx_status.h"
#include "src/developer/debug/zxdb/client/remote_api.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/developer/debug/zxdb/format/string_util.h"

namespace zxdb {

namespace {

const char kRunShortHelp[] = "run / r: Run the program. (Linux-only)";
const char kRunUsage[] = "run [ <binary path> <program args>* ]";
const char kRunHelp[] = R"(
  Alias: "r"

  Run the given binary with the given arguments. This command is supported only
  when debugging programs running on Linux.

Why "run" is not supported on Fuchsia

  On Fuchsia, programs run in namespaces defined by their components. There is
  "default" namespace in which to run a raw binary, which means there is no
  sensible way to implement the "run" command.

  Instead, consider

    * Create a filter by "attach <process name>/<component url>/etc." and start
      your program outside of the debugger.
    * Use "ffx component start --debug" to start a component with the debugger
      attached.
    * Use "run-test" to run a test.

Examples

  run /bin/ps
  run /bin/echo Hello.
)";

void RunVerbRun(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  debug::Platform platform = cmd_context->GetConsoleContext()->session()->platform();
  if (platform != debug::Platform::kLinux) {
    std::string message = "Run is supported only for Linux targets. Current target is ";
    message += debug::PlatformToString(platform);
    message += ". See \"help run\".";
    cmd_context->ReportError(Err(message));
    return;
  }

  // Only a process can be run.
  Err err = cmd.ValidateNouns({Noun::kProcess});
  if (err.has_error())
    return cmd_context->ReportError(err);

  // May need to create a new target.
  auto err_or_target = GetRunnableTarget(cmd_context->GetConsoleContext(), cmd);
  if (err_or_target.has_error())
    return cmd_context->ReportError(err_or_target.err());
  Target* target = err_or_target.value();

  if (cmd.args().empty()) {
    // Use the args already set on the target.
    if (target->GetArgs().empty())
      return cmd_context->ReportError(Err("No program to run. Try \"run <program name>\"."));
  } else {
    target->SetArgs(cmd.args());
  }

  target->Launch(
      [cmd_context](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) mutable {
        // The ConsoleContext displays messages for new processes, so don't display messages when
        // successfully starting.
        ProcessCommandCallback(std::move(target), false, err, cmd_context);
      });
}

}  // namespace

VerbRecord GetRunVerbRecord() {
  VerbRecord run(&RunVerbRun, {"run", "r"}, kRunShortHelp, kRunUsage, kRunHelp,
                 CommandGroup::kProcess);
  return run;
}

}  // namespace zxdb
