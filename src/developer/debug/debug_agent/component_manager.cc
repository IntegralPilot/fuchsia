// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/component_manager.h"

#include "src/developer/debug/debug_agent/process_handle.h"
#include "src/developer/debug/debug_agent/system_interface.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_agent {

namespace {
// Returns a function that can be used to compare other |debug_ipc::ComponentInfo| objects to the
// supplied |needle|. The |needle| controls what is compared by setting only the desired fields to
// be compared. Setting all fields is equivalent to directly comparing the two ComponentInfo
// objects. This is intended to be shared between all subclasses that provide an implementation for
// |FindComponentInfo| with a debug_ipc::ComponentInfo argument without being opinionated about how
// the derived classes keep track of the actual component information being looked up.
auto MakeComponentInfoComparator(const debug_ipc::ComponentInfo& needle) {
  // We check either or both strings depending on what's populated from the caller.
  bool check_moniker = !needle.moniker.empty();
  bool check_url = !needle.url.empty();

  return [=, &needle](const debug_ipc::ComponentInfo& info) -> bool {
    // We can check all fields of the component info in this manner: if any check is enabled, then
    // it must pass. If more than one check is enabled, then they must all pass, effectively
    // performing a logical "and" over all enabled checks as provided by the input needle. Any
    // checks that are not enabled are ignored.
    if (check_moniker && needle.moniker != info.moniker) {
      return false;
    }

    if (check_url && needle.url != info.url) {
      return false;
    }

    // All checks passed, this is a match.
    return true;
  };
}
}  // namespace

std::vector<debug_ipc::ComponentInfo> ComponentManager::FindComponentInfo(
    const ProcessHandle& process) const {
  zx_koid_t job_koid = process.GetJobKoid();
  while (job_koid) {
    auto components = FindComponentInfo(job_koid);
    if (!components.empty())
      return components;
    job_koid = system_interface_->GetParentJobKoid(job_koid);
  }
  return {};
}

std::vector<debug_ipc::ComponentInfo> ComponentManager::FindComponentInfo(
    const debug_ipc::ComponentInfo& needle) const {
  if (needle.moniker.empty() && needle.url.empty()) {
    return {};
  }

  return FindComponentInfoWithComparator(MakeComponentInfoComparator(needle));
}

std::optional<debug_ipc::ComponentInfo> ComponentManager::FindComponentInfoByMoniker(
    const std::string& moniker) const {
  debug_ipc::ComponentInfo info;
  info.moniker = moniker;

  auto result = FindComponentInfo(info);
  if (result.empty()) {
    return std::nullopt;
  }

  FX_DCHECK(result.size() == 1);
  return result[0];
}

std::vector<debug_ipc::ComponentInfo> ComponentManager::FindComponentInfoByUrl(
    const std::string& url) const {
  debug_ipc::ComponentInfo info;
  info.url = url;

  return FindComponentInfo(info);
}

}  // namespace debug_agent
