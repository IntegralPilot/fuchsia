// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_launch.h"

#include <lib/syslog/cpp/macros.h>

#include <dap/types.h>

#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/remote_api.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_attach.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO_EXT(LaunchRequestZxdb, LaunchRequest, "launch",
                                  DAP_FIELD(process, "process"),
                                  DAP_FIELD(launchCommand, "launchCommand"), DAP_FIELD(cwd, "cwd"))

}  // namespace dap

namespace zxdb {

void SplitDapCommand(const dap::string& cmd_string, dap::array<dap::string>& cmd) {
  // Split command string at whitespaces to an array of strings.
  // This is required by RunInTerminal request.
  size_t split_pos = cmd_string.find(' ');
  size_t start_pos = 0;

  while (split_pos != std::string::npos) {
    cmd.push_back(cmd_string.substr(start_pos, split_pos - start_pos));
    start_pos = split_pos + 1;
    split_pos = cmd_string.find(' ', start_pos);
  }

  cmd.push_back(
      cmd_string.substr(start_pos, std::min(split_pos, cmd_string.size()) - start_pos + 1));
}

dap::ResponseOrError<dap::LaunchResponse> OnRequestLaunch(DebugAdapterContext* context,
                                                          const dap::LaunchRequestZxdb& req) {
  // TODO(https://fxbug.dev/512349157): Use idiomatic Fuchsia patterns to launch test inferiors.
  // Assume that the request is always launching a fuchsia component rather than a general command
  if (!context->supports_run_in_terminal()) {
    if (!debug::StringContains(req.process, "://") || !debug::StringEndsWith(req.process, ".cm")) {
      dap::Error err;
      err.message = "The first argument must be a component URL.";
      return err;
    }
    debug_ipc::RunComponentRequest run_request;
    run_request.url = req.process;
    context->session()->remote_api()->RunComponent(
        run_request, [](Err err, debug_ipc::RunComponentReply reply) {
          if (err.has_error()) {
            LOGS(Error) << "Failed to run component: " << err.msg();
          } else if (reply.status.has_error()) {
            LOGS(Error) << "Failed to run component: " << reply.status.message();
          }
        });
    return dap::LaunchResponse();
  }

  dap::AttachRequestZxdb attach_req;
  attach_req.process = req.process;
  auto attach_resp = OnRequestAttach(context, attach_req);
  if (attach_resp.error) {
    return attach_resp.error;
  }

  dap::RunInTerminalRequest run_request;
  run_request.title = "zxdb launch";
  run_request.kind = "integrated";
  SplitDapCommand(req.launchCommand, run_request.args);
  if (req.cwd) {
    run_request.cwd = req.cwd.value();
  }
  // Send RunInTerminal request.
  // TODO(69387): Currently not waiting for the response from the client. Because the
  // response is returned as a future and waiting on it will block the MessageLoop creating a
  // deadlock, as MessageLoop should be running in order to receive the response. This can be fixed
  // by getting a response notification from cppdap.
  // Secondly, the response contains launched terminal process ID, but nothing about whether the
  // command ran successfully. It might be helpful to return error to Launch request after getting
  // error code(if any exists) from launched process.
  context->dap().send(run_request);

  return dap::LaunchResponse();
}

}  // namespace zxdb
