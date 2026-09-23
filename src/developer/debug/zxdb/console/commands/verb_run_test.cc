// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_run_test.h"

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

constexpr int kRealmSwitch = 1;

const char kRunTestShortHelp[] = "run-test: Run the test.";
const char kRunTestUsage[] = "run-test [ -r <realm> ] <url> [ <case filter>* ]";
const char kRunTestHelp[] = R"(
  Runs the test with the given URL. Optional case filters can be provided to
  specify the test cases to run. The test will be launched in a similar fashion
  as "ffx test run" on host or "run-test-suite" on Fuchsia.

  Since Fuchsia test runners usually start one process for each test case,
  running one test could spawns many processes in the debugger. The process name
  of these processes will be overridden as the test case name, making it easier
  to navigate between test cases.

Arguments

  -r <realm>
  --realm <realm>
      The realm to launch the test in. Required for non-hermetic tests.

  <url>
      The URL of the test to run.

  <case filter>*
      Glob patterns for matching tests. Can be specified multiple times to pass
      in multiple patterns. Tests may be excluded by prepending a '-' to the
      glob pattern.

Examples

  run-test fuchsia-pkg://fuchsia.com/pkg#meta/some_test.cm SomeTest.Case1
)";

void RunTest(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  // No nouns should be provided.
  if (Err err = cmd.ValidateNouns({}); err.has_error()) {
    return cmd_context->ReportError(err);
  }

  if (cmd.args().empty()) {
    return cmd_context->ReportError(Err("No test to run. Try \"run-test <url>\"."));
  }

  if (!debug::StringContains(cmd.args()[0], "://") ||
      !debug::StringEndsWith(cmd.args()[0], ".cm")) {
    return cmd_context->ReportError(
        Err("The first argument must be a component URL. Try \"help run-test\"."));
  }

  // Launch the test.
  if (cmd.target()->session()->ipc_version() < 56) {
    // For compatibility.
    // TODO: remove me after kMinimumProtocolVersion >= 56.
    if (cmd.HasSwitch(kRealmSwitch)) {
      return cmd_context->ReportError(Err("\"--realm\" is not supported in this IPC version."));
    }
    debug_ipc::RunBinaryRequest request;
    request.argv = cmd.args();

    cmd.target()->session()->remote_api()->RunBinary(
        request, [cmd_context](Err err, debug_ipc::RunBinaryReply reply) mutable {
          if (!err.has_error() && reply.status.has_error()) {
            err = Err("Could not start test: %s", reply.status.message().c_str());
          }
          if (err.has_error()) {
            cmd_context->ReportError(err);
          }
        });
    return;
  }

  debug_ipc::RunTestRequest request;
  request.url = cmd.args()[0];
  if (cmd.HasSwitch(kRealmSwitch)) {
    request.realm = cmd.GetSwitchValue(kRealmSwitch);
  }
  request.case_filters = {cmd.args().begin() + 1, cmd.args().end()};

  cmd.target()->session()->remote_api()->RunTest(
      request, [cmd_context](Err err, debug_ipc::RunTestReply reply) mutable {
        if (!err.has_error() && reply.status.has_error()) {
          err = Err("Could not start test: %s", reply.status.message().c_str());
        }
        if (err.has_error()) {
          cmd_context->ReportError(err);
        }
      });
}

}  // namespace

VerbRecord GetRunTestVerbRecord() {
  VerbRecord verb{&RunTest,      {"run-test"}, kRunTestShortHelp,
                  kRunTestUsage, kRunTestHelp, CommandGroup::kProcess};
  verb.switches.emplace_back(kRealmSwitch, true, "realm", 'r');
  return verb;
}

}  // namespace zxdb
