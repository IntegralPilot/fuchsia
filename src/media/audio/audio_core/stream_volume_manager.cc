// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/stream_volume_manager.h"

#include <lib/syslog/cpp/macros.h>

#include "src/media/audio/audio_core/stream_usage.h"

namespace media::audio {
namespace {

using fuchsia::media::AudioCaptureUsage2;
using fuchsia::media::AudioRenderUsage2;
using fuchsia::media::Usage2;

const auto kRendererVolumeRamp = Ramp{
    .duration = zx::msec(5),
    .ramp_type = fuchsia::media::audio::RampType::SCALE_LINEAR,
};

}  // namespace

StreamVolumeManager::VolumeSettingImpl::VolumeSettingImpl(Usage2 usage, StreamVolumeManager* owner)
    : usage_(std::move(usage)), owner_(owner) {}

void StreamVolumeManager::VolumeSettingImpl::SetVolume(float volume) {
  owner_->SetUsageVolume(fidl::Clone(usage_), volume);
}

StreamVolumeManager::StreamVolumeManager(async_dispatcher_t* fidl_dispatcher)
    :  // These must be listed in the order of the fuchsia::media::AudioRenderUsage2 enum.
      render_usage_volume_setting_impls_{
          VolumeSettingImpl(ToFidlUsage2(RenderUsage::BACKGROUND), this),
          VolumeSettingImpl(ToFidlUsage2(RenderUsage::MEDIA), this),
          VolumeSettingImpl(ToFidlUsage2(RenderUsage::INTERRUPTION), this),
          VolumeSettingImpl(ToFidlUsage2(RenderUsage::SYSTEM_AGENT), this),
          VolumeSettingImpl(ToFidlUsage2(RenderUsage::COMMUNICATION), this),
          VolumeSettingImpl(ToFidlUsage2(RenderUsage::ACCESSIBILITY), this),
      },
      // These must be listed in the order of the fuchsia::media::AudioCaptureUsage2 enum.
      capture_usage_volume_setting_impls_{
          VolumeSettingImpl(ToFidlUsage2(CaptureUsage::BACKGROUND), this),
          VolumeSettingImpl(ToFidlUsage2(CaptureUsage::FOREGROUND), this),
          VolumeSettingImpl(ToFidlUsage2(CaptureUsage::SYSTEM_AGENT), this),
          VolumeSettingImpl(ToFidlUsage2(CaptureUsage::COMMUNICATION), this),
      },
      // These must be listed in the order of the fuchsia::media::AudioRenderUsage2 enum.
      render_usage_volume_controls_{
          VolumeControl(&render_usage_volume_setting_impls_[ToIndex(AudioRenderUsage2::BACKGROUND)],
                        fidl_dispatcher),
          VolumeControl(&render_usage_volume_setting_impls_[ToIndex(AudioRenderUsage2::MEDIA)],
                        fidl_dispatcher),
          VolumeControl(
              &render_usage_volume_setting_impls_[ToIndex(AudioRenderUsage2::INTERRUPTION)],
              fidl_dispatcher),
          VolumeControl(
              &render_usage_volume_setting_impls_[ToIndex(AudioRenderUsage2::SYSTEM_AGENT)],
              fidl_dispatcher),
          VolumeControl(
              &render_usage_volume_setting_impls_[ToIndex(AudioRenderUsage2::COMMUNICATION)],
              fidl_dispatcher),
          VolumeControl(
              &render_usage_volume_setting_impls_[ToIndex(AudioRenderUsage2::ACCESSIBILITY)],
              fidl_dispatcher),
      },
      // These must be listed in the order of the fuchsia::media::AudioCaptureUsage2 enum.
      capture_usage_volume_controls_{
          VolumeControl(
              &capture_usage_volume_setting_impls_[ToIndex(AudioCaptureUsage2::BACKGROUND)],
              fidl_dispatcher),
          VolumeControl(
              &capture_usage_volume_setting_impls_[ToIndex(AudioCaptureUsage2::FOREGROUND)],
              fidl_dispatcher),
          VolumeControl(
              &capture_usage_volume_setting_impls_[ToIndex(AudioCaptureUsage2::SYSTEM_AGENT)],
              fidl_dispatcher),
          VolumeControl(
              &capture_usage_volume_setting_impls_[ToIndex(AudioCaptureUsage2::COMMUNICATION)],
              fidl_dispatcher),
      } {
  FX_DCHECK(fidl_dispatcher);

  static_assert(ToIndex(AudioRenderUsage2::BACKGROUND) == 0);
  static_assert(ToIndex(AudioRenderUsage2::MEDIA) == 1);
  static_assert(ToIndex(AudioRenderUsage2::INTERRUPTION) == 2);
  static_assert(ToIndex(AudioRenderUsage2::SYSTEM_AGENT) == 3);
  static_assert(ToIndex(AudioRenderUsage2::COMMUNICATION) == 4);
  static_assert(ToIndex(AudioRenderUsage2::ACCESSIBILITY) == 5);

  static_assert(ToIndex(AudioCaptureUsage2::BACKGROUND) == 0);
  static_assert(ToIndex(AudioCaptureUsage2::FOREGROUND) == 1);
  static_assert(ToIndex(AudioCaptureUsage2::SYSTEM_AGENT) == 2);
  static_assert(ToIndex(AudioCaptureUsage2::COMMUNICATION) == 3);
}

const UsageGainSettings& StreamVolumeManager::GetUsageGainSettings() const {
  return usage_gain_settings_;
}

void StreamVolumeManager::SetUsageGain(Usage2 usage, float gain_db) {
  if (gain_db != usage_gain_settings_.GetUnadjustedUsageGain(usage)) {
    FX_LOGS(INFO) << "SetUsageGain(" << ToStreamUsage(usage).ToString() << ", " << gain_db << "db)";
    usage_gain_settings_.SetUsageGain(fidl::Clone(usage), gain_db);
    UpdateStreamsWithUsage(std::move(usage));
  }
}

void StreamVolumeManager::SetUsageGainAdjustment(Usage2 usage, float gain_db) {
  float gain_adjustment = usage_gain_settings_.GetUsageGainAdjustment(usage);
  if (gain_db != gain_adjustment) {
    usage_gain_settings_.SetUsageGainAdjustment(fidl::Clone(usage), gain_db);
    UpdateStreamsWithUsage(std::move(usage));
  }
}

void StreamVolumeManager::BindUsageVolumeClient(
    Usage2 usage, fidl::InterfaceRequest<fuchsia::media::audio::VolumeControl> request) {
  if (usage.is_render_usage()) {
    render_usage_volume_controls_[ToIndex(usage.render_usage())].AddBinding(
        std::move(request), ToStreamUsage(usage).ToString());
  } else {
    capture_usage_volume_controls_[ToIndex(usage.capture_usage())].AddBinding(
        std::move(request), ToStreamUsage(usage).ToString());
  }
}

void StreamVolumeManager::NotifyStreamChanged(StreamVolume* stream_volume) {
  UpdateStream(stream_volume, std::nullopt);
}

void StreamVolumeManager::NotifyStreamChanged(StreamVolume* stream_volume, Ramp ramp) {
  UpdateStream(stream_volume, ramp);
}

void StreamVolumeManager::AddStream(StreamVolume* stream_volume) {
  stream_volumes_.insert(stream_volume);
  UpdateStream(stream_volume, std::nullopt);
}

void StreamVolumeManager::RemoveStream(StreamVolume* stream_volume) {
  stream_volumes_.erase(stream_volume);
}

void StreamVolumeManager::SetUsageVolume(Usage2 usage, float volume) {
  if (volume != usage_volume_settings_.GetUsageVolume(usage)) {
    usage_volume_settings_.SetUsageVolume(fidl::Clone(usage), volume);
    UpdateStreamsWithUsage(std::move(usage));
  }
}

void StreamVolumeManager::UpdateStreamsWithUsage(Usage2 usage) {
  for (auto& stream : stream_volumes_) {
    if (fidl::Equals(stream->GetStreamUsage(), usage)) {
      if (usage.is_render_usage()) {
        UpdateStream(stream, kRendererVolumeRamp);
      } else {
        // Because destination gain ramping is not implemented, capturer volume ramping is
        // unsupported.
        UpdateStream(stream, std::nullopt);
      }
    }
  }
}

void StreamVolumeManager::UpdateStream(StreamVolume* stream, std::optional<Ramp> ramp) {
  const auto usage = stream->GetStreamUsage();
  const auto respects_policy_adjustments = stream->RespectsPolicyAdjustments();
  const auto usage_gain = respects_policy_adjustments
                              ? usage_gain_settings_.GetAdjustedUsageGain(fidl::Clone(usage))
                              : usage_gain_settings_.GetUnadjustedUsageGain(fidl::Clone(usage));
  const auto usage_volume = usage_volume_settings_.GetUsageVolume(std::move(usage));

  stream->RealizeVolume(
      VolumeCommand{.volume = usage_volume, .gain_db_adjustment = usage_gain, .ramp = ramp});
}

}  // namespace media::audio
