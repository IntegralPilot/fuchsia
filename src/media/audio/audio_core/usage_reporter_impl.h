// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_USAGE_REPORTER_IMPL_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_USAGE_REPORTER_IMPL_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>

#include <vector>

#include "src/media/audio/audio_core/audio_admin.h"

namespace media::audio {

class UsageReporterImpl : public AudioAdmin::PolicyActionReporter,
                          public fuchsia::media::UsageReporter {
 public:
  fidl::InterfaceRequestHandler<fuchsia::media::UsageReporter> GetFidlRequestHandler();

 private:
  struct Watcher {
    fuchsia::media::UsageWatcherPtr watcher_ptr;
    int outstanding_ack_count;
  };
  struct Watcher2 {
    fuchsia::media::UsageWatcher2Ptr watcher_ptr;
    int outstanding_ack_count;
  };

  struct WatcherSet {
    std::map<int, Watcher> watchers;
    fuchsia::media::UsageState cached_state = fuchsia::media::UsageState::WithUnadjusted({});
  };
  struct WatcherSet2 {
    std::map<int, Watcher2> watchers;
    fuchsia::media::UsageState cached_state = fuchsia::media::UsageState::WithUnadjusted({});
  };

  void Watch(fuchsia::media::Usage usage,
             fidl::InterfaceHandle<fuchsia::media::UsageWatcher> usage_state_watcher) override;

  void Watch2(fuchsia::media::Usage2 usage,
              fidl::InterfaceHandle<fuchsia::media::UsageWatcher2> usage_state_watcher) override;

  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {
    FX_LOGS(ERROR) << "UsageReporterImpl: UsageReporter::handle_unknown_method(ordinal " << ordinal
                   << ", method_has_response " << method_has_response << ")";
  }

  void ReportPolicyAction(fuchsia::media::Usage2 usage,
                          fuchsia::media::Behavior policy_action) override;

  WatcherSet& watcher_set(const fuchsia::media::Usage2& usage);
  WatcherSet2& watcher_set2(const fuchsia::media::Usage2& usage);

  fidl::BindingSet<fuchsia::media::UsageReporter, UsageReporterImpl*> bindings_;
  std::array<WatcherSet, fuchsia::media::RENDER_USAGE_COUNT> render_usage_watchers_;
  std::array<WatcherSet, fuchsia::media::CAPTURE_USAGE2_COUNT> capture_usage_watchers_;
  std::array<WatcherSet2, fuchsia::media::RENDER_USAGE2_COUNT> render_usage2_watchers_;
  std::array<WatcherSet2, fuchsia::media::CAPTURE_USAGE2_COUNT> capture_usage2_watchers_;
  int next_watcher_id_ = 0;

  // Maximum number of states that can go un-acked before a watcher is disconnected
  static const int MAX_STATES = 20;

  friend class UsageReporterImplTest;
};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_USAGE_REPORTER_IMPL_H_
