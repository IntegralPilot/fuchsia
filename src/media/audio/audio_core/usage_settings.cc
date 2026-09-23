// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/usage_settings.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "src/media/audio/audio_core/mixer/gain.h"
#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/lib/processing/gain.h"

namespace media::audio {

float UsageGainSettings::GetAdjustedUsageGain(const fuchsia::media::Usage2& usage) const {
  TRACE_DURATION("audio", "UsageGainSettings::GetUsageGain");
  if (usage.is_render_usage()) {
    const auto usage_index = ToIndex(usage.render_usage());
    return std::min(Gain::CombineGains(render_usage_gain_[usage_index],
                                       render_usage_gain_adjustment_[usage_index]),
                    media_audio::kUnityGainDb);
  }
  FX_DCHECK(!usage.has_invalid_tag());
  const auto usage_index = ToIndex(usage.capture_usage());
  return std::min(Gain::CombineGains(capture_usage_gain_[usage_index],
                                     capture_usage_gain_adjustment_[usage_index]),
                  media_audio::kUnityGainDb);
}

float UsageGainSettings::GetUnadjustedUsageGain(const fuchsia::media::Usage2& usage) const {
  TRACE_DURATION("audio", "UsageGainSettings::GetUnadjustedUsageGain");
  if (usage.is_render_usage()) {
    const auto usage_index = ToIndex(usage.render_usage());
    return render_usage_gain_[usage_index];
  }
  FX_DCHECK(!usage.has_invalid_tag());
  const auto usage_index = ToIndex(usage.capture_usage());
  return capture_usage_gain_[usage_index];
}

float UsageGainSettings::GetUsageGainAdjustment(const fuchsia::media::Usage2& usage) const {
  TRACE_DURATION("audio", "UsageGainSettings::GetUsageGainAdjustment");
  if (usage.is_render_usage()) {
    const auto usage_index = ToIndex(usage.render_usage());
    return render_usage_gain_adjustment_[usage_index];
  }
  FX_DCHECK(!usage.has_invalid_tag());
  const auto usage_index = ToIndex(usage.capture_usage());
  return capture_usage_gain_adjustment_[usage_index];
}

void UsageGainSettings::SetUsageGain(fuchsia::media::Usage2 usage, float gain_db) {
  TRACE_DURATION("audio", "UsageGainSettings::SetUsageGain");
  if (usage.is_render_usage()) {
    render_usage_gain_[ToIndex(usage.render_usage())] = gain_db;
  } else {
    capture_usage_gain_[ToIndex(usage.capture_usage())] = gain_db;
  }
}

void UsageGainSettings::SetUsageGainAdjustment(fuchsia::media::Usage2 usage, float gain_db) {
  TRACE_DURATION("audio", "UsageGainSettings::SetUsageGainAdjustment");
  if (usage.is_render_usage()) {
    render_usage_gain_adjustment_[ToIndex(usage.render_usage())] = gain_db;
  } else {
    capture_usage_gain_adjustment_[ToIndex(usage.capture_usage())] = gain_db;
  }
}

UsageVolumeSettings::UsageVolumeSettings() {
  for (auto& volume : render_usage_volume_) {
    volume = fuchsia::media::audio::MAX_VOLUME;
  }

  for (auto& volume : capture_usage_volume_) {
    volume = fuchsia::media::audio::MAX_VOLUME;
  }
}

float UsageVolumeSettings::GetUsageVolume(const fuchsia::media::Usage2& usage) const {
  TRACE_DURATION("audio", "UsageVolumeSettings::GetUsageVolume");
  if (usage.is_render_usage()) {
    return render_usage_volume_[ToIndex(usage.render_usage())];
  }
  return capture_usage_volume_[ToIndex(usage.capture_usage())];
}

void UsageVolumeSettings::SetUsageVolume(fuchsia::media::Usage2 usage, float volume) {
  TRACE_DURATION("audio", "UsageVolumeSettings::SetUsageVolume");
  if (usage.is_render_usage()) {
    render_usage_volume_[ToIndex(usage.render_usage())] = volume;
  } else {
    capture_usage_volume_[ToIndex(usage.capture_usage())] = volume;
  }
}

}  // namespace media::audio
