// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_quit.h"

#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/console_context.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

constexpr int kForceQuitSwitch = 1;

const char kQuitShortHelp[] = "quit / q / exit: Quits the debugger.";
const char kQuitUsage[] = "quit";
const char kQuitHelp[] =
    R"(quit

  Quits the debugger. It will prompt for confirmation if there are running
  processes.

Options

  -f
  --force
      Quit without prompting, even if a process is attached or in embedded mode.
)";

void RunVerbQuit(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  if (cmd.HasSwitch(kForceQuitSwitch)) {
    cmd_context->console()->Quit();
    return;
  }

  // When we're in embedded mode, we don't want to actually quit until the streaming fd is closed.
  // Detach from everything and go back to embedded mode.
  if (cmd_context->GetConsoleContext()->GetConsoleMode() ==
      ClientSettings::System::kConsoleMode_EmbeddedInteractive) {
    cmd_context->GetConsoleContext()->session()->system().DetachFromAllTargets(
        [cmd_context](int) {});
    return;
  }

  int running_processes = 0;
  for (Target* t : cmd_context->GetConsoleContext()->session()->system().GetTargets()) {
    if (t->GetState() != Target::kNone)
      running_processes++;
  }

  if (running_processes == 0) {
    // Nothing running, quit immediately.
    cmd_context->console()->Quit();
    return;
  }

  OutputBuffer message;
  if (running_processes == 1) {
    message =
        OutputBuffer("\nAre you sure you want to quit and detach from the running process?\n");
  } else {
    message = OutputBuffer(
        fxl::StringPrintf("\nAre you sure you want to quit and detach from %d running processes?\n",
                          running_processes));
  }

  line_input::ModalPromptOptions options;
  options.require_enter = false;
  options.case_sensitive = false;
  options.options.push_back("y");
  options.options.push_back("n");
  options.cancel_option = "n";
  Console::get()->ModalGetOption(options, message, "y/n > ",
                                 [cmd_context](const std::string& answer) {
                                   if (answer == "y")
                                     cmd_context->console()->Quit();
                                 });
}

}  // namespace

VerbRecord GetQuitVerbRecord() {
  VerbRecord quit(&RunVerbQuit, {"quit", "q", "exit"}, kQuitShortHelp, kQuitUsage, kQuitHelp,
                  CommandGroup::kGeneral);
  quit.switches = {SwitchRecord(kForceQuitSwitch, false, "force", 'f')};
  return quit;
}

}  // namespace zxdb
