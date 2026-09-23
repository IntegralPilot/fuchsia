// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_FILTER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_FILTER_H_

#include <zircon/types.h>

#include <utility>

#include "src/developer/debug/debug_agent/job_handle.h"
#include "src/developer/debug/debug_agent/process_handle.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_agent {

class SystemInterface;

class Filter {
 public:
  explicit Filter(debug_ipc::Filter filter) : filter_(std::move(filter)) {}

  const debug_ipc::Filter& filter() const { return filter_; }

  // Creates a new kComponentMonikerPrefix filter based on the settings of this filter and the given
  // component |realm|.
  std::optional<Filter> MakeRecursiveFilter(const std::string& realm) const;

  // Returns whether the job matches the filter. SystemInterface is needed here
  // to fetch the component info about the job.
  bool MatchesJob(const JobHandle& process, SystemInterface& system_interface) const;

  // Returns whether the process matches the filter. SystemInterface is needed here to fetch
  // the component info about a process and get the parent job of a process.
  bool MatchesProcess(const ProcessHandle& process, SystemInterface& system_interface) const;

  // Returns whether the component matches the filter.
  bool MatchesComponent(const std::string& moniker, const std::string& url) const;

  // Returns a list of processes that are under a job and matches the filter.
  std::vector<debug_ipc::MatchedTask> ApplyToJob(const JobHandle& job,
                                                 SystemInterface& system_interface) const;

 private:
  debug_ipc::Filter filter_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_FILTER_H_
