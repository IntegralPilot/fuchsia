// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LAUNCHER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LAUNCHER_H_

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.debugger/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest_prod.h>

// This implements the |Launcher| fidl protocol, and manages instance(s) of DebugAgent. When a
// |Launch| request is received, a new DebugAgent process is spun up and passed the server_end of
// the DebugAgent protocol as a startup handle. That handle is bound to the MessageLoop and serves
// requests from the corresponding client.
class DebugAgentLauncher : public fidl::Server<fuchsia_debugger::Launcher> {
 public:
  void Launch(LaunchRequest& request, LaunchCompleter::Sync& completer) override;

  void GetAgents(GetAgentsRequest& request, GetAgentsCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_debugger::Launcher> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(WARNING) << "Unknown method: " << metadata.method_ordinal;
  }

  static fuchsia_component_decl::Child CreateChildDecl(
      const std::string& name, const fuchsia_debugger::LaunchOptions& options);

 private:
  FRIEND_TEST(DebugAgentLauncherTest, CreateChildDecl);

  void LaunchDebugAgent(fidl::ServerEnd<fuchsia_debugger::DebugAgent> server_end,
                        const fuchsia_debugger::LaunchOptions& options, const std::string& name,
                        fit::callback<void(zx_status_t)> cb);
};

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LAUNCHER_H_
