// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_COMPONENT_MANAGER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_COMPONENT_MANAGER_H_

#include <optional>
#include <string>
#include <vector>

#include "src/developer/debug/debug_agent/stdio_handles.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/status.h"

namespace debug_agent {

class DebugAgent;
class Filter;
class ProcessHandle;
class SystemInterface;

// This class manages launching and monitoring Fuchsia components. It is a singleton owned by the
// DebugAgent.
class ComponentManager {
 public:
  // ComponentManager needs |SystemInterface::GetParentJobKoid| for |FindComponentInfo|.
  explicit ComponentManager(SystemInterface* system_interface)
      : system_interface_(system_interface) {}
  virtual ~ComponentManager() = default;

  // Find the component information if the job is the root job of an ELF component.
  virtual std::vector<debug_ipc::ComponentInfo> FindComponentInfo(zx_koid_t job_koid) const = 0;

  // Returns the current set of component information in the system that is not associated with an
  // ELF process - e.g. it doesn't have a job or a process to key the lookup off of.
  virtual const std::map<std::string, debug_ipc::ComponentInfo>& GetNonElfComponentInfo() const = 0;

  // Find the component information if the process runs in the context of a component.
  std::vector<debug_ipc::ComponentInfo> FindComponentInfo(const ProcessHandle& process) const;

  // Finds the component info that matches the given moniker or url. Note that there may be multiple
  // components that match the same url, but there will never be multiple matches for a full
  // component moniker.
  std::optional<debug_ipc::ComponentInfo> FindComponentInfoByMoniker(
      const std::string& moniker) const;
  std::vector<debug_ipc::ComponentInfo> FindComponentInfoByUrl(const std::string& url) const;

  // Set the debug_agent. ComponentManager needs a debug_agent to notify component starting and
  // exiting events.
  virtual void SetDebugAgent(DebugAgent* debug_agent) = 0;

  // Launches the component.
  virtual debug::Status LaunchComponent(std::string url) = 0;

  // Launches a test.
  virtual debug::Status LaunchTest(std::string url, std::optional<std::string> realm,
                                   std::vector<std::string> case_filters) = 0;

  // Notification that a process has started.
  //
  // If the process starts because of a |LaunchComponent|, this function will fill in the given
  // stdio handles and return true.
  //
  // If it was not a component launch, returns false (the caller normally won't know if a launch is
  // a component without asking us, so it isn't necessarily an error).
  //
  // |process_name_override| allows the component manager to override the process name observed
  // by the client and is optional.
  virtual bool OnProcessStart(const ProcessHandle& process, StdioHandles* out_stdio,
                              std::string* process_name_override) = 0;

 private:
  // This function contains the implementation details of |FindComponentInfoBy{Url,Moniker}|, while
  // allowing for sharing a common comparison functor.
  virtual std::vector<debug_ipc::ComponentInfo> FindComponentInfoWithComparator(
      fit::function<bool(const debug_ipc::ComponentInfo&)>) const = 0;

  // Find the component information matching the given component info. Only populated fields are
  // considered part of the search. Note that this can return multiple components if searching by
  // URL only, but if moniker is included then it is expected to have exactly one match.
  std::vector<debug_ipc::ComponentInfo> FindComponentInfo(
      const debug_ipc::ComponentInfo& needle) const;

  SystemInterface* system_interface_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_COMPONENT_MANAGER_H_
