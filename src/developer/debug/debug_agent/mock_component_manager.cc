// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/mock_component_manager.h"

#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/debug_agent/mock_system_interface.h"

namespace debug_agent {

std::vector<debug_ipc::ComponentInfo> MockComponentManager::FindComponentInfo(
    zx_koid_t job_koid) const {
  auto [start, end] = component_info_.equal_range(job_koid);
  if (start == component_info_.end()) {
    // Not found.
    return {};
  }

  std::vector<debug_ipc::ComponentInfo> components;
  components.reserve(std::distance(start, end));
  for (auto& i = start; i != end; ++i) {
    components.push_back(i->second);
  }
  return components;
}

std::vector<debug_ipc::ComponentInfo> MockComponentManager::FindComponentInfoWithComparator(
    fit::function<bool(const debug_ipc::ComponentInfo&)> is_match) const {
  std::vector<debug_ipc::ComponentInfo> result;
  for (const auto& [_, info] : component_info_) {
    if (is_match(info)) {
      result.push_back(info);
    }
  }

  // Also check the non-ELF component info.
  for (const auto& [_, info] : non_elf_component_info_) {
    if (is_match(info)) {
      result.push_back(info);
    }
  }

  return result;
}

void MockComponentManager::AddComponentInfo(zx_koid_t job_koid, debug_ipc::ComponentInfo info) {
  component_info_.emplace(job_koid, info);
  moniker_to_job_.emplace(info.moniker, job_koid);
}

void MockComponentManager::AddNonElfComponentInfo(debug_ipc::ComponentInfo info) {
  non_elf_component_info_.emplace(info.moniker, info);
}

// Simulates the given event type coming from ComponentManager in a real system. |koid| is only
// used if the type is |kDebugStarted|, in which case it will be used to populate the known
// component information.
void MockComponentManager::InjectComponentEvent(FakeEventType type, const std::string& moniker,
                                                const std::string& url, zx_koid_t koid) {
  switch (type) {
    case FakeEventType::kDebugStarted: {
      zx_koid_t job_koid = koid;
      if (auto found = moniker_to_job_.find(moniker); found != moniker_to_job_.end()) {
        job_koid = found->second;
      } else if (job_koid != ZX_KOID_INVALID) {
        AddComponentInfo(job_koid, {.moniker = moniker, .url = url});
        mock_system_interface_->AddJob(job_koid, {{.moniker = moniker, .url = url}});
      }
      debug_agent_->OnComponentStarted(moniker, url, job_koid);
      break;
    }
    case FakeEventType::kStopped: {
      debug_agent_->OnComponentExited(moniker, url);
      break;
    }
    default: {
      FX_NOTREACHED();
      break;
    }
  }
}

}  // namespace debug_agent
