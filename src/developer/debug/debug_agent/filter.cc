// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/filter.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/types.h>

#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/debug_agent/job_handle.h"
#include "src/developer/debug/debug_agent/system_interface.h"
#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_agent {

bool Filter::MatchesJob(const JobHandle& job, SystemInterface& system_interface) const {
  const auto& components = system_interface.GetComponentManager().FindComponentInfo(job.GetKoid());
  return debug_ipc::FilterMatches(filter_, job.GetName(), components);
}

bool Filter::MatchesProcess(const ProcessHandle& process, SystemInterface& system_interface) const {
  if (filter_.job_koid) {
    zx_koid_t job_koid = process.GetJobKoid();
    while (job_koid && job_koid != filter_.job_koid) {
      job_koid = system_interface.GetParentJobKoid(job_koid);
    }
    if (job_koid != filter_.job_koid) {
      return false;
    }
  }

  const auto& components = system_interface.GetComponentManager().FindComponentInfo(process);
  return debug_ipc::FilterMatches(filter_, process.GetName(), components);
}

bool Filter::MatchesComponent(const std::string& moniker, const std::string& url) const {
  if (filter_.type == debug_ipc::Filter::Type::kComponentMoniker ||
      filter_.type == debug_ipc::Filter::Type::kComponentMonikerSuffix ||
      filter_.type == debug_ipc::Filter::Type::kComponentMonikerPrefix ||
      filter_.type == debug_ipc::Filter::Type::kComponentName ||
      filter_.type == debug_ipc::Filter::Type::kComponentUrl) {
    return debug_ipc::FilterMatches(filter_, "",
                                    {debug_ipc::ComponentInfo{.moniker = moniker, .url = url}});
  }
  return false;
}

std::vector<debug_ipc::MatchedTask> Filter::ApplyToJob(const JobHandle& job,
                                                       SystemInterface& system_interface) const {
  std::vector<debug_ipc::MatchedTask> res;
  std::function<void(const JobHandle& job)> visit_each_job = [&](const JobHandle& job) {
    if (filter_.config.job_only) {
      // Don't add the root job.
      if (job.GetKoid() != system_interface.GetRootJob()->GetKoid() &&
          MatchesJob(job, system_interface)) {
        res.push_back({.koid = job.GetKoid(), .type = debug_ipc::TaskType::kJob});
      }
    } else {
      for (const auto& process : job.GetChildProcesses()) {
        if (MatchesProcess(*process, system_interface)) {
          res.push_back({.koid = process->GetKoid(), .type = debug_ipc::TaskType::kProcess});
        }
      }
    }
    for (const auto& child : job.GetChildJobs()) {
      visit_each_job(*child);
    }
  };
  visit_each_job(job);
  return res;
}

std::optional<Filter> Filter::MakeRecursiveFilter(const std::string& realm) const {
  // Not a recursive filter, nothing to do.
  if (!filter_.config.recursive) {
    return std::nullopt;
  }

  debug_ipc::Filter realm_filter;
  realm_filter.pattern = realm;
  realm_filter.type = debug_ipc::Filter::Type::kComponentMonikerPrefix;
  realm_filter.id = debug_ipc::Filter::Identifier(debug_ipc::GenerateFilterIdValue(),
                                                  debug_ipc::Filter::Originator::kAgent);

  if (filter_.config.job_only) {
    realm_filter.config.never_attach = true;
  } else {
    // TODO(https://fxbug.dev/450924906): Consider handling conflicting filter options.
    // For now we just let the first match win, which is good enough for the typical users of
    // recursive filters. However this might pose a problem since the non-recursive solution for
    // this is resolved at AttachConfig creation time, which imposes few restrictions on the filters
    // themselves. If two or more recursive filters match the same pattern, but have conflicting
    // |weak| options, we'll have to resolve those somehow.
    realm_filter.config.weak = filter_.config.weak;
  }

  return Filter(realm_filter);
}

}  // namespace debug_agent
