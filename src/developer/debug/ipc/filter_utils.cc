// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/ipc/filter_utils.h"

#include <algorithm>
#include <random>
#include <string>

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/string_util.h"

namespace debug_ipc {

namespace {

bool MatchComponentUrl(std::string_view url, std::string_view pattern) {
  // Only deals with the most common case: the target URL contains a hash but the pattern doesn't.
  // The hash will look like "?hash=xxx#".
  const char* hash = "?hash=";
  if (debug::StringContains(url, hash) && url.find_last_of('#') != std::string_view::npos &&
      !debug::StringContains(pattern, hash)) {
    std::string new_url(url.substr(0, url.find(hash)));
    new_url += url.substr(url.find_last_of('#'));
    return new_url == pattern;
  }
  return url == pattern;
}

bool FilterApplies(const Filter* filter, const MatchedTask& task) {
  // We have to be generous when matching to accommodate old IPC users, which just used opaque koid
  // values without knowing whether or not they were a job or process, so |TaskType::kUnknown| can
  // apply to all filters.
  return task.type == TaskType::kUnknown ||
         (filter->config.job_only && task.type == TaskType::kJob) ||
         (!filter->config.job_only && task.type == TaskType::kProcess);
}

}  // namespace

bool FilterMatches(const Filter& filter, const std::string& process_name,
                   const std::vector<ComponentInfo>& components) {
  if (filter.type == Filter::Type::kProcessNameSubstr) {
    return debug::StringContains(process_name, filter.pattern);
  } else if (filter.type == Filter::Type::kProcessName) {
    return process_name == filter.pattern;
  } else if (filter.type == Filter::Type::kUnset || filter.type == Filter::Type::kLast) {
    return false;
  }

  return std::any_of(components.cbegin(), components.cend(), [&](const ComponentInfo& component) {
    switch (filter.type) {
      case Filter::Type::kComponentName:
        return component.url.substr(component.url.find_last_of('/') + 1) == filter.pattern;
      case Filter::Type::kComponentUrl:
        return MatchComponentUrl(component.url, filter.pattern);
      case Filter::Type::kComponentMoniker:
        return component.moniker == filter.pattern;
      case Filter::Type::kComponentMonikerSuffix:
        return debug::StringEndsWith(component.moniker, filter.pattern);
      case Filter::Type::kComponentMonikerPrefix:
        return debug::StringStartsWith(component.moniker, filter.pattern);
      default:
        return false;
    }
  });
}

const Filter* GetFilterForId(const std::vector<const Filter*>& filters,
                             const Filter::Identifier& id) {
  const auto& filter = std::ranges::find_if(
      filters, [id](const Filter* filter) { return filter ? id == filter->id : false; });
  return filter != filters.end() ? *filter : nullptr;
}

uint32_t GenerateFilterIdValue() {
  static std::independent_bits_engine<std::default_random_engine, 24, uint32_t> engine;

  uint32_t value = engine();
  FX_CHECK((value & (0xff << 24)) == 0) << "Generated filter ID value is too big!";

  return value;
}

std::map<uint64_t, AttachConfig> GetAttachConfigsForFilterMatches(
    const std::vector<FilterMatch>& matches, const std::vector<const Filter*>& installed_filters) {
  std::map<uint64_t, debug_ipc::AttachConfig> pids_to_attach;

  for (const auto& match : matches) {
    auto matched_filter = GetFilterForId(installed_filters, match.id);
    if (matched_filter == nullptr) {
      continue;
    }

    for (const auto& match : match.matches) {
      if (FilterApplies(matched_filter, match)) {
        // At this point, we know we care about how the configuration of this filter will affect the
        // final attach configuration. Go ahead and derive this filter's AttachConfig and then
        // resolve the attach priority below if we need to.
        //
        // This way, we don't have to worry about whether or not this filter is "job only" or not.
        const auto& attach_config = FilterConfig::ToAttachConfig(matched_filter->config);

        auto inserted = pids_to_attach.insert({match.koid, attach_config});

        // Make sure we double check the mode after the insertion. If the pid had already been
        // added to the map by a weak filter and this is a strong filter that also matched, then we
        // should strongly attach. Conversely, a strong filter should never be overruled by a weak
        // filter. If the filter id for this match is invalid or isn't found, perform a strong
        // attach.
        //
        // A "no attach" configuration does not require weak to be set, but will also not override
        // weakly attaching. Conversely, "no attach" will be overridden by a weak configuration
        // (e.g.
        //
        // For Example:
        //   Filter 1 {
        //     ..
        //     weak = true,
        //     never_attach = false,
        //   }
        //
        //   Filter 2 {
        //     ..
        //     weak = false,
        //     never_attach = true,
        //   }
        //
        // Filter 1 will override Filter 2, resulting in the claiming of the debug exception channel
        // for the given matching target. In this example, this will result in an AttachConfig that
        // looks like this:
        //
        //   AttachConfig {
        //     ..
        //     priority = kWeak,
        //   }
        //
        // Likewise, if the positions are reversed, then weak will override never_attach, creating
        // the same AttachConfig as above.
        inserted.first->second.priority =
            std::max(attach_config.priority, inserted.first->second.priority);
      }
    }
  }

  return pids_to_attach;
}

Filter::Type ResolveFilterType(std::string& pattern, const FilterResolutionOptions& options) {
  constexpr size_t kMaxProcessNameLength = 31;

  if (options.exact) {
    if (pattern.size() > kMaxProcessNameLength) {
      pattern.resize(kMaxProcessNameLength);
    }
    return Filter::Type::kProcessName;
  }

  if (debug::StringStartsWith(pattern, "fuchsia-pkg://") ||
      debug::StringStartsWith(pattern, "fuchsia-boot://")) {
    return Filter::Type::kComponentUrl;
  }

  if (debug::StringStartsWith(pattern, "/")) {
    return Filter::Type::kComponentMoniker;
  }

  if (debug::StringContains(pattern, "/")) {
    return Filter::Type::kComponentMonikerSuffix;
  }

  if (debug::StringEndsWith(pattern, ".cm")) {
    return Filter::Type::kComponentName;
  }

  // Default is process name substring.
  if (pattern.size() > kMaxProcessNameLength) {
    pattern.resize(kMaxProcessNameLength);
  }
  return Filter::Type::kProcessNameSubstr;
}

}  // namespace debug_ipc
