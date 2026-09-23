// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_exception_info.h"

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/format_exception.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"

namespace zxdb {

namespace {

const char kExceptionInfoShortHelp[] =
    "exception-info/ excp: Display info about the current exception if any.";
const char kExceptionInfoUsage[] = "exception-info / excp";
const char kExceptionInfoHelp[] = R"(
  Prints information about exceptions. Exceptions are only available when the
  given thread is stopped in the "Exception" state. Threads that are not
  currently in this state will report "No exception".

  With no additional context, this command will attempt to print information
  about an exception on the currently selected thread of the active process (see
  `process` and `thread`).

  Supply additional context to select different processes and/or threads.

  Examples
    excp
    exception-info
      Print exception information for the currently selected thread.

    t 2 excp
    thread 2 exception-info
      Print exception information for thread 2 in the active process.

    pr 3 t 5 exception-info
    process 3 thread 5 exception-info
      Print exception information for thread 5 in process 3.

    t * excp
    thread * exception-info
      Print exception information for all threads in the active process.

    pr 2 t * excp
    process 2 thread * exception-info
      Print exception information for all threads in process 2.
)";

void RunVerbExceptionInfo(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  if (Err err = cmd.ValidateNouns({Noun::kProcess, Noun::kThread}, true); err.has_error())
    return cmd_context->ReportError(err);

  // Should always be present because we were called synchronously.
  ConsoleContext* console_context = cmd_context->GetConsoleContext();

  std::vector<Thread*> threads;

  if (cmd.HasNoun(Noun::kThread)) {
    if (cmd.GetNounIndex(Noun::kThread) == Command::kWildcard) {
      auto active_process = console_context->GetActiveTarget()->GetProcess();
      threads = active_process->GetThreads();
    } else {
      threads.push_back(cmd.thread());
    }
  } else if (cmd.HasNoun(Noun::kProcess) &&
             cmd.GetNounIndex(Noun::kProcess) == Command::kWildcard) {
    // All threads for process.
    return cmd_context->ReportError(Err("process * exception-info is not supported."));
  } else if (auto active_thread = console_context->GetActiveThreadForTarget(cmd.target())) {
    threads.push_back(active_thread);
  } else {
    return cmd_context->ReportError(Err("What thread?"));
  }

  for (const auto& thread : threads) {
    if (auto stop_info = thread->CurrentStopInfo()) {
      auto buf = FormatException(console_context, thread, stop_info->exception_record);
      buf.Append("\n");
      cmd_context->Output(buf);
    } else {
      OutputBuffer buf;
      buf.Append("Thread ");
      buf.Append(std::to_string(thread->GetKoid()));
      buf.Append(" has no exception.");
      cmd_context->Output(buf);
    }
  }
}

}  // namespace

VerbRecord GetExceptionInfoVerbRecord() {
  VerbRecord record{&RunVerbExceptionInfo,  {"exception-info", "excp"}, kExceptionInfoShortHelp,
                    kExceptionInfoUsage,    kExceptionInfoHelp,         CommandGroup::kQuery,
                    SourceAffinity::kSource};
  return record;
}

}  // namespace zxdb
