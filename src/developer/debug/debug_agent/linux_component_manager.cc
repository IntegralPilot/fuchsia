// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_component_manager.h"

#include "lib/syslog/cpp/macros.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_agent {

std::vector<debug_ipc::ComponentInfo> LinuxComponentManager::FindComponentInfo(
    zx_koid_t job_koid) const {
  return {};
}

std::vector<debug_ipc::ComponentInfo> LinuxComponentManager::FindComponentInfoWithComparator(
    fit::function<bool(const debug_ipc::ComponentInfo&)> is_match) const {
  return {};
}

const std::map<std::string, debug_ipc::ComponentInfo>&
LinuxComponentManager::GetNonElfComponentInfo() const {
  FX_NOTIMPLEMENTED();
  static std::map<std::string, debug_ipc::ComponentInfo> map;
  return map;
}

void LinuxComponentManager::SetDebugAgent(DebugAgent* debug_agent) {}

debug::Status LinuxComponentManager::LaunchComponent(std::string url) {
  return debug::Status("No components on Linux");
}

debug::Status LinuxComponentManager::LaunchTest(std::string url, std::optional<std::string> realm,
                                                std::vector<std::string> case_filters) {
  return debug::Status("No components on Linux");
}

bool LinuxComponentManager::OnProcessStart(const ProcessHandle& process, StdioHandles* out_stdio,
                                           std::string* process_name_override) {
  return false;
}

}  // namespace debug_agent
