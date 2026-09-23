// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace/commands/list_categories.h"

#include <lib/syslog/cpp/macros.h>

namespace tracing {

Command::Info ListCategoriesCommand::Describe() {
  return Command::Info{.factory = []() { return std::make_unique<ListCategoriesCommand>(); },
                       .name = "list-categories",
                       .usage = "list all known categories",
                       .options = {}};
}

ListCategoriesCommand::ListCategoriesCommand() = default;

void ListCategoriesCommand::Start(const fxl::CommandLine& command_line) {
  if (!(command_line.options().empty() && command_line.positional_args().empty())) {
    FX_LOGS(ERROR) << "We encountered unknown options, please check your " << "command invocation";
    Done(EXIT_FAILURE);
    return;
  }

  take_provisioner()->GetKnownCategories().Then(
      [this](fidl::Result<controller::Provisioner::GetKnownCategories> result) {
        if (result.is_error()) {
          FX_LOGS(ERROR) << "Failed to get known categories: " << result.error_value() << "\n";
        }
        out() << "Known categories\n";
        for (const auto& it : result->categories()) {
          out() << "  " << it.name() << ": " << it.description() << '\n';
        }
        Done(EXIT_SUCCESS);
      });
}

}  // namespace tracing
