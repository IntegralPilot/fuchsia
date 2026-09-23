// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace/app.h"

#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include "src/performance/trace/commands/list_categories.h"
#include "src/performance/trace/commands/record.h"
#include "src/performance/trace/commands/time.h"

namespace tracing {

App::App() {
  RegisterCommand(ListCategoriesCommand::Describe());
  RegisterCommand(RecordCommand::Describe());
  RegisterCommand(TimeCommand::Describe());
}

void App::Start(const fxl::CommandLine& command_line) {
  if (command_line.HasOption("help")) {
    PrintHelp();
    Done(EXIT_SUCCESS);
    return;
  }

  const auto& positional_args = command_line.positional_args();

  if (positional_args.empty()) {
    FX_LOGS(ERROR) << "Command missing - aborting";
    PrintHelp();
    Done(EXIT_FAILURE);
    return;
  }

  auto it = known_commands_.find(positional_args.front());
  if (it == known_commands_.end()) {
    FX_LOGS(ERROR) << "Unknown command '" << positional_args.front() << "' - aborting";
    PrintHelp();
    Done(EXIT_FAILURE);
    return;
  }

  command_ = it->second.factory();
  command_->Run(fxl::CommandLineFromIteratorsWithArgv0(
                    positional_args.front(), positional_args.begin() + 1, positional_args.end()),
                [this](int32_t return_code) { Done(return_code); });
}

void App::RegisterCommand(Command::Info info) { known_commands_[info.name] = std::move(info); }

void App::PrintHelp() {
  out() << "trace [options] command [command-specific options]\n";
  out() << "  --help: Produce this help message\n\n";
  for (const auto& pair : known_commands_) {
    out() << "  " << pair.second.name << " - " << pair.second.usage << '\n';
    for (const auto& option : pair.second.options)
      out() << "    --" << option.first << ": " << option.second << '\n';
  }
}

}  // namespace tracing
