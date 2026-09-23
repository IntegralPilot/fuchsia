// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_IPC_FILTER_UTILS_H_
#define SRC_DEVELOPER_DEBUG_IPC_FILTER_UTILS_H_

#include <map>
#include <string>

#include "src/developer/debug/ipc/records.h"

namespace debug_ipc {

// Matches the filter with the given process_name or any of the components given in |components|,
// ignoring the job_koid.
bool FilterMatches(const Filter& filter, const std::string& process_name,
                   const std::vector<ComponentInfo>& components);

struct FilterResolutionOptions {
  bool exact = false;
};

// Resolves the filter type from the pattern.
// If the resolved type is one of the process name types, and the pattern exceeds
// Zircon's maximum name length (31 characters), it will be trimmed to 31 characters.
Filter::Type ResolveFilterType(std::string& pattern, const FilterResolutionOptions& options = {});

// Returns the filter from the given |filters| that has |id|.
const Filter* GetFilterForId(const std::vector<const Filter*>& filters,
                             const Filter::Identifier& id);

// Shared function for clients to generate unique values for their filters.
uint32_t GenerateFilterIdValue();

// Converts a vector of FilterMatch objects and a vector of installed filters into a map of pids to
// AttachConfigs derived from any matching filters, or the defaults if there was no matching filter.
// Correctly takes into account filters that may match the same job or process and overlays settings
// appropriately. The returned map will not have duplicates.
std::map<uint64_t, debug_ipc::AttachConfig> GetAttachConfigsForFilterMatches(
    const std::vector<debug_ipc::FilterMatch>& matches,
    const std::vector<const debug_ipc::Filter*>& installed_filters);

}  // namespace debug_ipc

#endif  // SRC_DEVELOPER_DEBUG_IPC_FILTER_UTILS_H_
