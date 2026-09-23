// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_COMPONENT_MANAGER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_COMPONENT_MANAGER_H_

#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_agent {

class LinuxComponentManager : public ComponentManager {
 public:
  LinuxComponentManager(SystemInterface* si) : ComponentManager(si) {}

  // ComponentManager implementation.
  std::vector<debug_ipc::ComponentInfo> FindComponentInfo(zx_koid_t job_koid) const override;
  std::vector<debug_ipc::ComponentInfo> FindComponentInfoWithComparator(
      fit::function<bool(const debug_ipc::ComponentInfo&)> is_match) const override;
  const std::map<std::string, debug_ipc::ComponentInfo>& GetNonElfComponentInfo() const override;
  void SetDebugAgent(DebugAgent* debug_agent) override;
  debug::Status LaunchComponent(std::string url) override;
  debug::Status LaunchTest(std::string url, std::optional<std::string> realm,
                           std::vector<std::string> case_filters) override;
  bool OnProcessStart(const ProcessHandle& process, StdioHandles* out_stdio,
                      std::string* process_name_override) override;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_COMPONENT_MANAGER_H_
