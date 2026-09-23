// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_libs.h"

#include <algorithm>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/format_table.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

constexpr int kForceReloadSwitch = 1;

const char kLibsShortHelp[] = "libs: Show loaded libraries for a process.";
const char kLibsUsage[] = "libs";
const char kLibsHelp[] = R"(
  Shows the loaded library information for the given process.

  This will update the library list from the target process and will attempt to
  reload symbols for any unsymbolized modules (when the "sym-stat" command says
  "Symbols loaded: No"). Note that stripped binaries can have a few symbols
  which will confuse this checking: if you have updated the symbol file on your
  system, use "--reload" to reload.

Options

  -r
  --reload
      Force reload all symbol information for all libraries in the current
      process.

Examples

  libs
  process 2 libs
)";

// Completion callback for DoLibs().
void OnLibsComplete(std::vector<debug_ipc::Module> modules,
                    fxl::RefPtr<CommandContext> cmd_context) {
  // Sort by load address.
  std::sort(modules.begin(), modules.end(),
            [](const debug_ipc::Module& a, const debug_ipc::Module& b) { return a.base < b.base; });

  std::vector<std::vector<std::string>> rows;
  for (const auto& module : modules) {
    rows.push_back(std::vector<std::string>{to_hex_string(module.base), module.name});
  }

  OutputBuffer out;
  FormatTable({ColSpec(Align::kRight, 0, "Load address", 2), ColSpec(Align::kLeft, 0, "Name", 1)},
              rows, &out);
  cmd_context->Output(out);
}

void RunVerbLibs(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  // Only a process can be specified.
  if (Err err = cmd.ValidateNouns({Noun::kProcess}); err.has_error())
    return cmd_context->ReportError(err);

  if (!cmd.args().empty())
    return cmd_context->ReportError(Err(ErrType::kInput, "\"libs\" takes no parameters."));

  if (Err err = AssertRunningTarget(cmd_context->GetConsoleContext(), "libs", cmd.target());
      err.has_error())
    return cmd_context->ReportError(err);

  bool force_reload = cmd.HasSwitch(kForceReloadSwitch);
  cmd.target()->GetProcess()->GetModules(force_reload, [cmd_context](auto err, auto modules) {
    if (err.has_error())
      return cmd_context->ReportError(err);
    OnLibsComplete(modules, cmd_context);
  });
}

}  // namespace

VerbRecord GetLibsVerbRecord() {
  VerbRecord libs(&RunVerbLibs, {"libs"}, kLibsShortHelp, kLibsUsage, kLibsHelp,
                  CommandGroup::kQuery);
  libs.switches.push_back(SwitchRecord(kForceReloadSwitch, false, "reload", 'r'));
  return libs;
}

}  // namespace zxdb
