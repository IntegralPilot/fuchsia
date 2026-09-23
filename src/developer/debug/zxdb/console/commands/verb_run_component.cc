// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_run_component.h"

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/remote_api.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/developer/debug/zxdb/format/string_util.h"

namespace zxdb {
namespace {

const char kRunComponentShortHelp[] = "run-component: Run the component.";
const char kRunComponentUsage[] = "run-component <url> [ <args>* ]";
const char kRunComponentHelp[] = R"(
  Runs the component with the given URL.

  Components will be launched in the "ffx-laboratory" collection, similar to
  the behavior of "ffx component run --recreate". The collection provides
  a restricted set of capabilities and is only suitable for running some demo
  components. If any other capabilities are needed, it's recommended to declare
  it statically or create it elsewhere in the topology, and attach to it from
  the debugger.

  See https://fuchsia.dev/fuchsia-src/development/components/run#ffx-laboratory.

Arguments

  <url>
      The URL of the component to run.

  <args>*

      Extra arguments when launching the component, only supported in v1
      components.

Examples

  run-component fuchsia-pkg://fuchsia.com/crasher#meta/cpp_crasher.cm
)";

void RunVerbRunComponent(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  // No nouns should be provided.
  if (Err err = cmd.ValidateNouns({}); err.has_error()) {
    return cmd_context->ReportError(err);
  }

  if (cmd.args().size() != 1) {
    return cmd_context->ReportError(Err("\"run-component\" accepts exactly 1 argument."));
  }

  if (!debug::StringContains(cmd.args()[0], "://") ||
      (!debug::StringEndsWith(cmd.args()[0], ".cm"))) {
    return cmd_context->ReportError(
        Err("The first argument must be a component URL. Try \"help run-component\"."));
  }

  // Output warning about this possibly not working.
  OutputBuffer warning(Syntax::kWarning, GetExclamation());
  warning.Append(" run-component won't work for many v2 components. See \"help run-component\".\n");
  cmd_context->Output(warning);

  debug_ipc::RunComponentRequest request;
  request.url = cmd.args()[0];
  cmd.target()->session()->remote_api()->RunComponent(
      request, [cmd_context](Err err, debug_ipc::RunComponentReply reply) mutable {
        if (!err.has_error() && reply.status.has_error()) {
          return cmd_context->ReportError(
              Err("Failed to launch component: %s", reply.status.message().c_str()));
        }
        if (err.has_error()) {
          cmd_context->ReportError(err);
        }
      });
}

}  // namespace

VerbRecord GetRunComponentVerbRecord() {
  return {&RunVerbRunComponent, {"run-component"}, kRunComponentShortHelp,
          kRunComponentUsage,   kRunComponentHelp, CommandGroup::kProcess};
}

}  // namespace zxdb
