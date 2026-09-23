// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_COMMAND_H_
#define SRC_PERFORMANCE_TRACE_COMMAND_H_

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <lib/fit/function.h>

#include <iosfwd>
#include <map>
#include <memory>
#include <string>

#include "src/lib/fxl/command_line.h"

namespace tracing {

namespace controller = fuchsia_tracing_controller;

class Command : public fidl::AsyncEventHandler<controller::Provisioner> {
 public:
  // OnDoneCallback is the callback type invoked when a command finished
  // running. It takes as argument the return code to exit the process with.
  using OnDoneCallback = fit::function<void(int32_t)>;
  struct Info {
    using CommandFactory = fit::function<std::unique_ptr<Command>()>;

    CommandFactory factory;
    std::string name;
    std::string usage;
    std::map<std::string, std::string> options;
  };

  void on_fidl_error(fidl::UnbindInfo error) override;
  void handle_unknown_event(fidl::UnknownEventMetadata<controller::Provisioner> metadata) override;

  ~Command() override;

  void Run(const fxl::CommandLine& command_line, OnDoneCallback on_done);

 protected:
  static std::ostream& out();
  static std::istream& in();

  explicit Command();

  // Starts running the command.
  // The command must invoke Done() when finished.
  virtual void Start(const fxl::CommandLine& command_line) = 0;
  void Done(int32_t return_code);

 private:
  OnDoneCallback on_done_;
  int32_t return_code_ = -1;

  Command(const Command&) = delete;
  Command(Command&&) = delete;
  Command& operator=(const Command&) = delete;
  Command& operator=(Command&&) = delete;
};

class CommandWithController : public Command {
 protected:
  explicit CommandWithController();
  fidl::Client<controller::Provisioner>&& take_provisioner() { return std::move(provisioner_); }

 private:
  fidl::Client<controller::Provisioner> provisioner_;

  CommandWithController(const CommandWithController&) = delete;
  CommandWithController(CommandWithController&&) = delete;
  CommandWithController& operator=(const CommandWithController&) = delete;
  CommandWithController& operator=(CommandWithController&&) = delete;
};

}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_COMMAND_H_
