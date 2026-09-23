// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_COMPONENT_MANAGER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_COMPONENT_MANAGER_H_

#include <map>

#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/system_interface.h"

namespace debug_agent {

enum class FakeEventType : uint32_t {
  kDebugStarted = 0,
  kStopped,
  kLast,
};

class MockSystemInterface;

class MockComponentManager : public ComponentManager {
 public:
  explicit MockComponentManager(MockSystemInterface* system_interface)
      : ComponentManager(reinterpret_cast<SystemInterface*>(system_interface)),
        mock_system_interface_(system_interface) {}
  ~MockComponentManager() override = default;

  auto& component_info() { return component_info_; }

  // ComponentManager implementation.
  void SetDebugAgent(DebugAgent* agent) override { debug_agent_ = agent; }

  std::vector<debug_ipc::ComponentInfo> FindComponentInfo(zx_koid_t job_koid) const override;
  const std::map<std::string, debug_ipc::ComponentInfo>& GetNonElfComponentInfo() const override {
    return non_elf_component_info_;
  }

  // Updates |component_info_| and |moniker_to_job_| with the given information.
  void AddComponentInfo(zx_koid_t job_koid, debug_ipc::ComponentInfo info);

  // Updates |non_elf_component_info_| with the given information.
  void AddNonElfComponentInfo(debug_ipc::ComponentInfo info);

  // Simulates the given event type coming from ComponentManager in a real system. |koid| is only
  // used if the type is |kDebugStarted|, in which case it will be used to populate the known
  // component information and add the koid as a child of the root job in MockSystemInterface.
  void InjectComponentEvent(FakeEventType type, const std::string& moniker, const std::string& url,
                            zx_koid_t koid = ZX_KOID_INVALID);

  debug::Status LaunchComponent(std::string url) override { return debug::Status("Not supported"); }

  debug::Status LaunchTest(std::string url, std::optional<std::string> realm,
                           std::vector<std::string> case_filters) override {
    return debug::Status("Not supported");
  }

  bool OnProcessStart(const ProcessHandle& process, StdioHandles* out_stdio,
                      std::string* process_name_override) override {
    return false;
  }

 private:
  std::vector<debug_ipc::ComponentInfo> FindComponentInfoWithComparator(
      fit::function<bool(const debug_ipc::ComponentInfo&)> is_match) const override;

  DebugAgent* debug_agent_;
  MockSystemInterface* mock_system_interface_;
  std::multimap<zx_koid_t, debug_ipc::ComponentInfo> component_info_;
  std::multimap<std::string, zx_koid_t> moniker_to_job_;
  std::map<std::string, debug_ipc::ComponentInfo> non_elf_component_info_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_COMPONENT_MANAGER_H_
