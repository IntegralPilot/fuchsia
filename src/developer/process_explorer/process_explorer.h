// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_PROCESS_EXPLORER_PROCESS_EXPLORER_H_
#define SRC_DEVELOPER_PROCESS_EXPLORER_PROCESS_EXPLORER_H_

#include <fidl/fuchsia.process.explorer/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include "src/lib/fxl/command_line.h"

namespace process_explorer {

class Explorer : public fidl::Server<fuchsia_process_explorer::Query>,
                 public fidl::Server<fuchsia_process_explorer::ProcessExplorer> {
 public:
  Explorer(async_dispatcher_t* dispatcher, component::OutgoingDirectory& outgoing);
  ~Explorer() override;

  // Writes processes information to |socket| in JSON, in UTF-8.
  // See /src/developer/process_explorer/writer.h for a description of the format of the JSON.
  void WriteJsonProcessesData(WriteJsonProcessesDataRequest& request,
                              WriteJsonProcessesDataCompleter::Sync& completer) override;

  // Writes job hierarchy information to |socket| in JSON, in UTF-8.
  // See process_explorer.fidl for a description of the format of the JSON.
  void WriteJsonTaskHierarchyData(WriteJsonTaskHierarchyDataRequest& request,
                                  WriteJsonTaskHierarchyDataCompleter::Sync& completer) override;

  // fuchsia.process.exploxer/ProcessExplorer implementation.
  void GetTaskInfo(GetTaskInfoRequest& request, GetTaskInfoCompleter::Sync& completer) override;
  void GetHandleInfo(GetHandleInfoRequest& request,
                     GetHandleInfoCompleter::Sync& completer) override;
  void GetVmaps(GetVmapsRequest& request, GetVmapsCompleter::Sync& completer) override;
  void GetStackTrace(GetStackTraceRequest& request,
                     GetStackTraceCompleter::Sync& completer) override;
  void KillTask(KillTaskRequest& request, KillTaskCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_process_explorer::Query> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_process_explorer::ProcessExplorer> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  fidl::ServerBindingGroup<fuchsia_process_explorer::Query> query_bindings_;
  fidl::ServerBindingGroup<fuchsia_process_explorer::ProcessExplorer> explorer_bindings_;
};

}  // namespace process_explorer

#endif  // SRC_DEVELOPER_PROCESS_EXPLORER_PROCESS_EXPLORER_H_
