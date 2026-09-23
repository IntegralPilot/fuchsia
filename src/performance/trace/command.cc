// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace/command.h"

#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include <iostream>

namespace tracing {

Command::Command() = default;

Command::~Command() = default;

std::ostream& Command::out() {
  // Returning std::cerr on purpose. std::cout is redirected and consumed
  // by the enclosing context.
  return std::cerr;
}

std::istream& Command::in() { return std::cin; }

void Command::Run(const fxl::CommandLine& command_line, OnDoneCallback on_done) {
  if (return_code_ >= 0) {
    on_done(return_code_);
  } else {
    on_done_ = std::move(on_done);
    Start(command_line);
  }
}

void Command::Done(int32_t return_code) {
  return_code_ = return_code;
  if (on_done_) {
    on_done_(return_code_);
    on_done_ = nullptr;
  }
}

void Command::on_fidl_error(fidl::UnbindInfo error) {
  FX_LOGS(ERROR) << "Trace controller disconnected unexpectedly";
  Done(EXIT_FAILURE);
}

void Command::handle_unknown_event(fidl::UnknownEventMetadata<controller::Provisioner> metadata) {
  FX_LOGS(ERROR) << "Unknown event: " << metadata.event_ordinal;
}

CommandWithController::CommandWithController() {
  zx::result client_end = component::Connect<controller::Provisioner>();
  if (client_end.is_error()) {
    FX_PLOGS(ERROR, client_end.error_value()) << "Trace Controller failed to connect";
    Done(EXIT_FAILURE);
  }
  provisioner_ = fidl::Client<controller::Provisioner>{*std::move(client_end),
                                                       async_get_default_dispatcher(), this};
}

}  // namespace tracing
