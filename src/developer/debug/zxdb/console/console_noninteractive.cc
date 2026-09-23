// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/console_noninteractive.h"

#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/console/command_parser.h"

namespace zxdb {

void ConsoleNoninteractive::Init() {
  // This is safe to do here (particularly before calling |InitConsoleMode|) since this setting is
  // typically set by parsing command line options.
  context().SetConsoleMode(ClientSettings::System::kConsoleMode_NonInteractive);
  context().InitConsoleMode();
}

void ConsoleNoninteractive::Quit() { debug::MessageLoop::Current()->QuitNow(); }

void ConsoleNoninteractive::Write(const OutputBuffer& output, bool add_newline) {
  output.WriteToStdout(add_newline);

  for (auto& observer : output_observers_) {
    observer.OnOutput(output);
  }
}

void ConsoleNoninteractive::ModalGetOption(const line_input::ModalPromptOptions& options,
                                           OutputBuffer message, const std::string& prompt,
                                           line_input::ModalLineInput::ModalCompletionCallback cb) {
  LOGS(Error) << "Modal is not supported in non-interactive console";
  cb(options.cancel_option);
}

void ConsoleNoninteractive::ProcessInputLine(const std::string& line,
                                             fxl::RefPtr<CommandContext> cmd_context,
                                             bool add_to_history) {
  if (line.empty())
    return;

  if (!cmd_context)
    cmd_context = fxl::MakeRefCounted<ConsoleCommandContext>(this);

  Command cmd;
  if (Err err = ParseCommand(line, &cmd); err.has_error())
    return cmd_context->ReportError(err);

  // The command is parsed now, we can build a more complete report to send when the command is
  // completed. By building the report here, we don't need to worry about the lifetime of the actual
  // Command object.
  cmd_context->SetCommandReport(cmd.BuildReport());

  if (Err err = context_.FillOutCommand(&cmd); err.has_error())
    return cmd_context->ReportError(err);

  DispatchCommand(cmd, cmd_context);
}

}  // namespace zxdb
