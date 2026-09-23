// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/usage_reporter_impl.h"

#include <fuchsia/media/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

namespace media::audio {

fidl::InterfaceRequestHandler<fuchsia::media::UsageReporter>
UsageReporterImpl::GetFidlRequestHandler() {
  return bindings_.GetHandler(this);
}

void UsageReporterImpl::Watch(
    fuchsia::media::Usage _usage,
    fidl::InterfaceHandle<fuchsia::media::UsageWatcher> usage_state_watcher) {
  auto usage = ToFidlUsage2(_usage);
  auto watcher = usage_state_watcher.Bind();
  auto& set = watcher_set(usage);
  int current_id = next_watcher_id_++;
  watcher.set_error_handler(
      [&set, current_id](zx_status_t status) { set.watchers.erase(current_id); });
  watcher->OnStateChanged(fidl::Clone(_usage), fidl::Clone(set.cached_state), [current_id, &set]() {
    --set.watchers[current_id].outstanding_ack_count;
  });

  // Initialize outstanding_ack_count as 1 to count first OnStateChange message
  set.watchers[current_id] = {.watcher_ptr = std::move(watcher), .outstanding_ack_count = 1};
}

void UsageReporterImpl::Watch2(
    fuchsia::media::Usage2 usage,
    fidl::InterfaceHandle<fuchsia::media::UsageWatcher2> usage_state_watcher) {
  if (usage.is_render_usage() && usage.render_usage() >= fuchsia::media::RENDER_USAGE2_COUNT) {
    FX_LOGS(WARNING) << "Invalid render usage for Watch2; ignoring.";
    return;
  }
  if (usage.is_capture_usage() && usage.capture_usage() >= fuchsia::media::CAPTURE_USAGE2_COUNT) {
    FX_LOGS(WARNING) << "Invalid capture usage for Watch2; ignoring.";
    return;
  }

  auto watcher = usage_state_watcher.Bind();
  auto& set = watcher_set2(usage);
  int current_id = next_watcher_id_++;
  watcher.set_error_handler(
      [&set, current_id](zx_status_t status) { set.watchers.erase(current_id); });
  watcher->OnStateChanged(fidl::Clone(usage), fidl::Clone(set.cached_state), [current_id, &set]() {
    --set.watchers[current_id].outstanding_ack_count;
  });

  // Initialize outstanding_ack_count as 1 to count first OnStateChange message
  set.watchers[current_id] = {.watcher_ptr = std::move(watcher), .outstanding_ack_count = 1};
}

void UsageReporterImpl::ReportPolicyAction(fuchsia::media::Usage2 usage,
                                           fuchsia::media::Behavior policy_action) {
  const auto state = [&policy_action] {
    fuchsia::media::UsageState usage_state;
    if (policy_action == fuchsia::media::Behavior::NONE) {
      usage_state.set_unadjusted({});
    } else if (policy_action == fuchsia::media::Behavior::DUCK) {
      usage_state.set_ducked({});
    } else if (policy_action == fuchsia::media::Behavior::MUTE) {
      usage_state.set_muted({});
    } else {
      FX_LOGS(ERROR) << "Encountered unknown fuchsia::media::Behavior enum";
    }
    return usage_state;
  }();

  {
    auto& set2 = watcher_set2(usage);
    set2.cached_state = fidl::Clone(state);
    auto it = set2.watchers.begin();
    while (it != set2.watchers.end()) {
      if (it->second.outstanding_ack_count > MAX_STATES) {
        FX_LOGS(WARNING) << "Disconnecting unresponsive watcher";
        it = set2.watchers.erase(it);
      } else {
        ++it->second.outstanding_ack_count;
        it->second.watcher_ptr->OnStateChanged(fidl::Clone(usage), fidl::Clone(state),
                                               [it]() { --it->second.outstanding_ack_count; });
        ++it;
      }
    }
  }

  {
    auto usage1 = ToFidlUsageTry(usage);
    if (!usage1.has_value()) {
      return;
    }

    auto& set = watcher_set(usage);
    set.cached_state = fidl::Clone(state);
    auto it = set.watchers.begin();
    while (it != set.watchers.end()) {
      if (it->second.outstanding_ack_count > MAX_STATES) {
        FX_LOGS(WARNING) << "Disconnecting unresponsive watcher";
        it = set.watchers.erase(it);
      } else {
        ++it->second.outstanding_ack_count;
        it->second.watcher_ptr->OnStateChanged(fidl::Clone(*usage1), fidl::Clone(state),
                                               [it]() { --it->second.outstanding_ack_count; });
        ++it;
      }
    }
  }
}

UsageReporterImpl::WatcherSet& UsageReporterImpl::watcher_set(const fuchsia::media::Usage2& usage) {
  if (usage.is_render_usage()) {
    return render_usage_watchers_[ToIndex(usage.render_usage())];
  }
  return capture_usage_watchers_[ToIndex(usage.capture_usage())];
}

UsageReporterImpl::WatcherSet2& UsageReporterImpl::watcher_set2(
    const fuchsia::media::Usage2& usage) {
  if (usage.is_render_usage()) {
    return render_usage2_watchers_[ToIndex(usage.render_usage())];
  }
  return capture_usage2_watchers_[ToIndex(usage.capture_usage())];
}

}  // namespace media::audio
