// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/audio_admin.h"

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/fidl/cpp/clone.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "src/media/audio/audio_core/reporter.h"
#include "src/media/audio/audio_core/stream_volume_manager.h"

namespace media::audio {

using fuchsia::media::AudioCaptureUsage2;
using fuchsia::media::AudioRenderUsage2;
using fuchsia::media::Usage2;

AudioAdmin::AudioAdmin(StreamVolumeManager* stream_volume_manager,
                       PolicyActionReporter* policy_action_reporter,
                       ActivityDispatcher* activity_dispatcher,
                       ActiveStreamCountReporter* active_stream_count_reporter,
                       async_dispatcher_t* fidl_dispatcher, BehaviorGain behavior_gain)
    : behavior_gain_(behavior_gain),
      stream_volume_manager_(*stream_volume_manager),
      policy_action_reporter_(*policy_action_reporter),
      activity_dispatcher_(*activity_dispatcher),
      active_stream_count_reporter_(active_stream_count_reporter),
      fidl_dispatcher_(fidl_dispatcher) {
  FX_DCHECK(stream_volume_manager);
  FX_DCHECK(policy_action_reporter);
  FX_DCHECK(fidl_dispatcher);
  Reporter::Singleton().SetAudioPolicyBehaviorGain(behavior_gain_);
}

void AudioAdmin::SetInteraction(Usage2 active, Usage2 affected, fuchsia::media::Behavior behavior) {
  async::PostTask(fidl_dispatcher_, [this, active = std::move(active),
                                     affected = std::move(affected), behavior = behavior]() {
    TRACE_DURATION("audio", "AudioAdmin::SetInteraction");
    std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
    if (active.Which() == Usage2::Tag::kCaptureUsage &&
        affected.Which() == Usage2::Tag::kCaptureUsage) {
      active_rules_.SetRule(active.capture_usage(), affected.capture_usage(), behavior);
    } else if (active.Which() == Usage2::Tag::kCaptureUsage &&
               affected.Which() == Usage2::Tag::kRenderUsage) {
      active_rules_.SetRule(active.capture_usage(), affected.render_usage(), behavior);

    } else if (active.Which() == Usage2::Tag::kRenderUsage &&
               affected.Which() == Usage2::Tag::kCaptureUsage) {
      active_rules_.SetRule(active.render_usage(), affected.capture_usage(), behavior);

    } else if (active.Which() == Usage2::Tag::kRenderUsage &&
               affected.Which() == Usage2::Tag::kRenderUsage) {
      active_rules_.SetRule(active.render_usage(), affected.render_usage(), behavior);
    }
  });
}

bool AudioAdmin::IsActive(RenderUsage usage) {
  TRACE_DURATION("audio", "AudioAdmin::IsActive(Render)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  auto usage_index = static_cast<std::underlying_type_t<RenderUsage>>(usage);
  return active_streams_playback_[usage_index].size() > 0;
}

bool AudioAdmin::IsActive(CaptureUsage usage) {
  TRACE_DURATION("audio", "AudioAdmin::IsActive(Capture)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  auto usage_index = static_cast<std::underlying_type_t<CaptureUsage>>(usage);
  return active_streams_capture_[usage_index].size() > 0;
}

void AudioAdmin::SetUsageNone(AudioRenderUsage2 usage) {
  TRACE_DURATION("audio", "AudioAdmin::SetUsageNone(Render)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  stream_volume_manager_.SetUsageGainAdjustment(Usage2::WithRenderUsage(fidl::Clone(usage)),
                                                behavior_gain_.none_gain_db);
  policy_action_reporter_.ReportPolicyAction(Usage2::WithRenderUsage(std::move(usage)),
                                             fuchsia::media::Behavior::NONE);
}

void AudioAdmin::SetUsageNone(AudioCaptureUsage2 usage) {
  TRACE_DURATION("audio", "AudioAdmin::SetUsageNone(Capture)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  stream_volume_manager_.SetUsageGainAdjustment(Usage2::WithCaptureUsage(fidl::Clone(usage)),
                                                behavior_gain_.none_gain_db);
  policy_action_reporter_.ReportPolicyAction(Usage2::WithCaptureUsage(std::move(usage)),
                                             fuchsia::media::Behavior::NONE);
}

void AudioAdmin::SetUsageMute(AudioRenderUsage2 usage) {
  TRACE_DURATION("audio", "AudioAdmin::SetUsageMute(Render)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  stream_volume_manager_.SetUsageGainAdjustment(Usage2::WithRenderUsage(fidl::Clone(usage)),
                                                behavior_gain_.mute_gain_db);
  policy_action_reporter_.ReportPolicyAction(Usage2::WithRenderUsage(std::move(usage)),
                                             fuchsia::media::Behavior::MUTE);
}

void AudioAdmin::SetUsageMute(AudioCaptureUsage2 usage) {
  TRACE_DURATION("audio", "AudioAdmin::SetUsageMute(Capture)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  stream_volume_manager_.SetUsageGainAdjustment(Usage2::WithCaptureUsage(fidl::Clone(usage)),
                                                behavior_gain_.mute_gain_db);
  policy_action_reporter_.ReportPolicyAction(Usage2::WithCaptureUsage(std::move(usage)),
                                             fuchsia::media::Behavior::MUTE);
}

void AudioAdmin::SetUsageDuck(AudioRenderUsage2 usage) {
  TRACE_DURATION("audio", "AudioAdmin::SetUsageDuck(Render)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  stream_volume_manager_.SetUsageGainAdjustment(Usage2::WithRenderUsage(fidl::Clone(usage)),
                                                behavior_gain_.duck_gain_db);
  policy_action_reporter_.ReportPolicyAction(Usage2::WithRenderUsage(std::move(usage)),
                                             fuchsia::media::Behavior::DUCK);
}

void AudioAdmin::SetUsageDuck(AudioCaptureUsage2 usage) {
  TRACE_DURATION("audio", "AudioAdmin::SetUsageDuck(Capture)");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  stream_volume_manager_.SetUsageGainAdjustment(Usage2::WithCaptureUsage(fidl::Clone(usage)),
                                                behavior_gain_.duck_gain_db);
  policy_action_reporter_.ReportPolicyAction(Usage2::WithCaptureUsage(std::move(usage)),
                                             fuchsia::media::Behavior::DUCK);
}

void AudioAdmin::ApplyNewPolicies(const RendererPolicies& new_renderer_policies,
                                  const CapturerPolicies& new_capturer_policies) {
  TRACE_DURATION("audio", "AudioAdmin::ApplyNewPolicies");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  for (int i = 0; i < fuchsia::media::RENDER_USAGE2_COUNT; ++i) {
    auto usage = AudioRenderUsage2(i);
    switch (new_renderer_policies[i]) {
      case fuchsia::media::Behavior::NONE:
        SetUsageNone(usage);
        break;
      case fuchsia::media::Behavior::DUCK:
        SetUsageDuck(usage);
        break;
      case fuchsia::media::Behavior::MUTE:
        SetUsageMute(usage);
        break;
    }
  }
  for (int i = 0; i < fuchsia::media::CAPTURE_USAGE2_COUNT; ++i) {
    auto usage = AudioCaptureUsage2(i);
    switch (new_capturer_policies[i]) {
      case fuchsia::media::Behavior::NONE:
        SetUsageNone(usage);
        break;
      case fuchsia::media::Behavior::DUCK:
        SetUsageDuck(usage);
        break;
      case fuchsia::media::Behavior::MUTE:
        SetUsageMute(usage);
        break;
    }
  }
}

void AudioAdmin::UpdatePolicy() {
  TRACE_DURATION("audio", "AudioAdmin::UpdatePolicy");
  // Hold |fidl_thread_checker_| for the duration of this method to prevent applying incorrect
  // policy in the space between IsActive() and set_new_policies() calls below.
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
  // Initialize new policies to `None`.
  RendererPolicies new_renderer_policies;
  CapturerPolicies new_capturer_policies;
  new_renderer_policies.fill(fuchsia::media::Behavior::NONE);
  new_capturer_policies.fill(fuchsia::media::Behavior::NONE);
  // Lambda to set new renderer and capturer policies based on an active usage.
  auto set_new_policies = [this, &new_renderer_policies, &new_capturer_policies](auto active) {
    std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);
    for (int i = 0; i < fuchsia::media::RENDER_USAGE2_COUNT; ++i) {
      auto affected = AudioRenderUsage2(i);
      new_renderer_policies[i] =
          std::max(new_renderer_policies[i], active_rules_.GetPolicy(active, affected));
    }
    for (int i = 0; i < fuchsia::media::CAPTURE_USAGE2_COUNT; ++i) {
      auto affected = AudioCaptureUsage2(i);
      new_capturer_policies[i] =
          std::max(new_capturer_policies[i], active_rules_.GetPolicy(active, affected));
    }
  };
  // Loop through active usages and apply policies.
  // Store |active_usages| for Reporter logging.
  std::vector<Usage2> active_usages;
  for (int i = 0; i < fuchsia::media::RENDER_USAGE2_COUNT; ++i) {
    if (IsActive(static_cast<RenderUsage>(i))) {
      active_usages.push_back(Usage2::WithRenderUsage(AudioRenderUsage2(i)));
      set_new_policies(AudioRenderUsage2(i));
    }
  }
  for (int i = 0; i < fuchsia::media::CAPTURE_USAGE2_COUNT; ++i) {
    if (IsActive(static_cast<CaptureUsage>(i))) {
      active_usages.push_back(Usage2::WithCaptureUsage(AudioCaptureUsage2(i)));
      set_new_policies(AudioCaptureUsage2(i));
    }
  }
  ApplyNewPolicies(new_renderer_policies, new_capturer_policies);
  Reporter::Singleton().UpdateActiveUsagePolicy(active_usages, new_renderer_policies,
                                                new_capturer_policies);
}

// As needed by the ActivityReporter, "activity" counts FIDL usages (not ultrasound).
void AudioAdmin::UpdateRenderActivity() {
  TRACE_DURATION("audio", "AudioAdmin::UpdateRenderActivity");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);

  ActivityDispatcher::RenderActivity fidl_render_activity;
  for (int i = 0; i < fuchsia::media::RENDER_USAGE2_COUNT; i++) {
    if (IsActive(kRenderUsages[i])) {
      fidl_render_activity.set(i);
    }
  }

  activity_dispatcher_.OnRenderActivityChanged(fidl_render_activity);
}

// As needed by ActivityReporter, "activity" counts FIDL usages (not loopback or ultrasound).
void AudioAdmin::UpdateCaptureActivity() {
  TRACE_DURATION("audio", "AudioAdmin::UpdateCaptureActivity");
  std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);

  ActivityDispatcher::CaptureActivity capture_activity;
  for (int i = 0; i < fuchsia::media::CAPTURE_USAGE2_COUNT; i++) {
    if (IsActive(kCaptureUsages[i])) {
      capture_activity.set(i);
    }
  }
  activity_dispatcher_.OnCaptureActivityChanged(capture_activity);
}

void AudioAdmin::UpdateActiveStreamCount(StreamUsage stream_usage) {
  if (active_stream_count_reporter_) {
    if (stream_usage.is_capture_usage()) {
      auto usage = stream_usage.capture_usage();
      auto usage_index = static_cast<std::underlying_type_t<CaptureUsage>>(usage);
      active_stream_count_reporter_->OnActiveCaptureCountChanged(
          usage, static_cast<uint32_t>(active_streams_capture_[usage_index].size()));
    } else {
      auto usage = stream_usage.render_usage();
      auto usage_index = static_cast<std::underlying_type_t<RenderUsage>>(usage);
      active_stream_count_reporter_->OnActiveRenderCountChanged(
          usage, static_cast<uint32_t>(active_streams_playback_[usage_index].size()));
    }
  }
}

void AudioAdmin::UpdateRendererState(RenderUsage usage, bool active,
                                     fuchsia::media::AudioRenderer* renderer) {
  async::PostTask(fidl_dispatcher_, [this, usage = usage, active = active, renderer = renderer] {
    TRACE_DURATION("audio", "AudioAdmin::UpdateRendererState");
    std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);

    auto usage_index = static_cast<std::underlying_type_t<RenderUsage>>(usage);
    if (active) {
      auto result = active_streams_playback_[usage_index].insert(renderer);
      if (!result.second) {
        FX_LOGS(ERROR) << "Renderer " << renderer << " NOT inserted:  " << ToString(usage)
                       << "; prevented by renderer " << *(result.first);
      }
    } else {
      if (!active_streams_playback_[usage_index].erase(renderer)) {
        // Unrecognized renderer, or it was already destroyed. This is generally a logic error.
        FX_LOGS(ERROR) << "Unrecognized renderer " << renderer
                       << " NOT removed :  " << ToString(usage);
      }
    }

    UpdateActiveStreamCount(StreamUsage::WithRenderUsage(usage));
    UpdatePolicy();
    UpdateRenderActivity();
  });
}

void AudioAdmin::UpdateCapturerState(CaptureUsage usage, bool active,
                                     fuchsia::media::AudioCapturer* capturer) {
  async::PostTask(fidl_dispatcher_, [this, usage = usage, active = active, capturer = capturer] {
    TRACE_DURATION("audio", "AudioAdmin::UpdateCapturerState");
    std::lock_guard<fit::thread_checker> lock(fidl_thread_checker_);

    auto usage_index = static_cast<std::underlying_type_t<CaptureUsage>>(usage);
    if (active) {
      auto result = active_streams_capture_[usage_index].insert(capturer);
      if (!result.second) {
        FX_LOGS(ERROR) << "Capturer " << capturer << " NOT inserted: " << ToString(usage);
      }
    } else {
      if (!active_streams_capture_[usage_index].erase(capturer)) {
        FX_LOGS(ERROR) << "Unrecognized capturer " << capturer
                       << " NOT removed:  " << ToString(usage);
      }
    }

    UpdateActiveStreamCount(StreamUsage::WithCaptureUsage(usage));
    UpdatePolicy();
    UpdateCaptureActivity();
  });
}

void AudioAdmin::PolicyRules::ResetInteractions() {
  TRACE_DURATION("audio", "AudioAdmin::ResetInteractions");
  for (int ac = 0; ac < fuchsia::media::RENDER_USAGE2_COUNT; ac++) {
    auto active = AudioRenderUsage2(ac);
    for (int aff = 0; aff < fuchsia::media::RENDER_USAGE2_COUNT; aff++) {
      auto affected = AudioRenderUsage2(aff);
      SetRule(active, affected, fuchsia::media::Behavior::NONE);
    }
    for (int aff = 0; aff < fuchsia::media::CAPTURE_USAGE2_COUNT; aff++) {
      auto affected = AudioCaptureUsage2(aff);
      SetRule(active, affected, fuchsia::media::Behavior::NONE);
    }
  }
  for (int ac = 0; ac < fuchsia::media::CAPTURE_USAGE2_COUNT; ac++) {
    auto active = AudioCaptureUsage2(ac);
    for (int aff = 0; aff < fuchsia::media::RENDER_USAGE2_COUNT; aff++) {
      auto affected = AudioRenderUsage2(aff);
      SetRule(active, affected, fuchsia::media::Behavior::NONE);
    }
    for (int aff = 0; aff < fuchsia::media::CAPTURE_USAGE2_COUNT; aff++) {
      auto affected = AudioCaptureUsage2(aff);
      SetRule(active, affected, fuchsia::media::Behavior::NONE);
    }
  }
}

void AudioAdmin::SetInteractionsFromAudioPolicy(AudioPolicy policy) {
  async::PostTask(fidl_dispatcher_, [this, policy = std::move(policy)] {
    ResetInteractions();
    for (auto& rule : policy.rules()) {
      SetInteraction(fidl::Clone(rule.active), fidl::Clone(rule.affected), rule.behavior);
    }
  });
}

}  // namespace media::audio
