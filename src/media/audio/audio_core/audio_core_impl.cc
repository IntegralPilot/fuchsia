// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/audio_core_impl.h"

#include <fuchsia/media/audio/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>

#include "src/media/audio/audio_core/audio_admin.h"
#include "src/media/audio/audio_core/audio_capturer.h"
#include "src/media/audio/audio_core/audio_device_manager.h"
#include "src/media/audio/audio_core/audio_renderer.h"
#include "src/media/audio/audio_core/loudness_transform.h"
#include "src/media/audio/audio_core/process_config.h"
#include "src/media/audio/audio_core/profile_acquirer.h"
#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/lib/format/format.h"

namespace media::audio {

AudioCoreImpl::AudioCoreImpl(Context* context) : context_(*context) {
  FX_DCHECK(context);
  // The main async_t here is responsible for receiving audio payloads sent by applications, so it
  // has real time requirements just like mixing threads. Ideally, this task would not run on the
  // same thread that processes *all* non-mix audio service jobs (even non-realtime ones), but that
  // will take more significant restructuring, when we can deal with realtime requirements in place.
  auto result = AcquireSchedulerRole(zx::thread::self(), "fuchsia.media.audio.core.dispatch");
  if (result.is_error()) {
    // Failing to apply a Scheduler Profile is not fatal (e.g. it may happen in tests),
    // but we warn because performance may suffer.
    FX_PLOGS(WARNING, result.status_value())
        << "Unable to set scheduler role for the audio_core FIDL thread";
  }

  // Set up our audio policy.
  LoadDefaults();

  context_.component_context().outgoing()->AddPublicService(bindings_.GetHandler(this));
}

AudioCoreImpl::~AudioCoreImpl() { Shutdown(); }

void AudioCoreImpl::Shutdown() {
  TRACE_DURATION("audio", "AudioCoreImpl::Shutdown");
  context_.device_manager().Shutdown();
}

void AudioCoreImpl::CreateAudioRenderer(
    fidl::InterfaceRequest<fuchsia::media::AudioRenderer> audio_renderer_request) {
  TRACE_DURATION("audio", "AudioCoreImpl::CreateAudioRenderer");
  FX_LOGS(DEBUG);

  context_.route_graph().AddRenderer(
      AudioRenderer::Create(std::move(audio_renderer_request), &context_));
}

void AudioCoreImpl::CreateAudioCapturerWithConfiguration(
    fuchsia::media::AudioStreamType stream_type,
    fuchsia::media::AudioCapturerConfiguration configuration,
    fidl::InterfaceRequest<fuchsia::media::AudioCapturer> audio_capturer_request) {
  auto format = Format::Create(stream_type);
  if (format.is_error()) {
    FX_LOGS(WARNING) << "Attempted to create AudioCapturer with invalid stream type";
    return;
  }
  context_.route_graph().AddCapturer(AudioCapturer::Create(
      std::move(configuration), std::make_optional<Format>(format.take_value()),
      std::move(audio_capturer_request), &context_));
}

void AudioCoreImpl::CreateAudioCapturer(
    bool loopback, fidl::InterfaceRequest<fuchsia::media::AudioCapturer> audio_capturer_request) {
  TRACE_DURATION("audio", "AudioCoreImpl::CreateAudioCapturer");
  FX_LOGS(DEBUG);

  auto configuration = loopback ? fuchsia::media::AudioCapturerConfiguration::WithLoopback(
                                      fuchsia::media::LoopbackAudioCapturerConfiguration())
                                : fuchsia::media::AudioCapturerConfiguration::WithInput(
                                      fuchsia::media::InputAudioCapturerConfiguration());
  context_.route_graph().AddCapturer(
      AudioCapturer::Create(std::move(configuration), /*format= */ std::nullopt,
                            std::move(audio_capturer_request), &context_));
}

void AudioCoreImpl::SetRenderUsageGain(fuchsia::media::AudioRenderUsage render_usage,
                                       float gain_db) {
  TRACE_DURATION("audio", "AudioCoreImpl::SetRenderUsageGain");
  SetRenderUsageGain2(ToFidlRenderUsage2(render_usage), gain_db);
}

void AudioCoreImpl::SetRenderUsageGain2(fuchsia::media::AudioRenderUsage2 render_usage,
                                        float gain_db) {
  TRACE_DURATION("audio", "AudioCoreImpl::SetRenderUsageGain2");
  if (render_usage >= fuchsia::media::RENDER_USAGE2_COUNT) {
    FX_LOGS(WARNING) << "Invalid render usage for SetRenderUsageGain2";
    return;
  }
  context_.volume_manager().SetUsageGain(
      fuchsia::media::Usage2::WithRenderUsage(fidl::Clone(render_usage)), gain_db);
}

void AudioCoreImpl::SetCaptureUsageGain(fuchsia::media::AudioCaptureUsage capture_usage,
                                        float gain_db) {
  TRACE_DURATION("audio", "AudioCoreImpl::SetCaptureUsageGain");
  SetCaptureUsageGain2(ToFidlCaptureUsage2(capture_usage), gain_db);
}

void AudioCoreImpl::SetCaptureUsageGain2(fuchsia::media::AudioCaptureUsage2 capture_usage,
                                         float gain_db) {
  TRACE_DURATION("audio", "AudioCoreImpl::SetCaptureUsageGain2");
  if (capture_usage >= fuchsia::media::CAPTURE_USAGE2_COUNT) {
    FX_LOGS(WARNING) << "Invalid render usage for SetCaptureUsageGain2";
    return;
  }
  context_.volume_manager().SetUsageGain(
      fuchsia::media::Usage2::WithCaptureUsage(fidl::Clone(capture_usage)), gain_db);
}

void AudioCoreImpl::BindUsageVolumeControl(
    fuchsia::media::Usage usage,
    fidl::InterfaceRequest<fuchsia::media::audio::VolumeControl> volume_control) {
  TRACE_DURATION("audio", "AudioCoreImpl::BindUsageVolumeControl");
  BindUsageVolumeControl2(ToFidlUsage2(usage), std::move(volume_control));
}

void AudioCoreImpl::BindUsageVolumeControl2(
    fuchsia::media::Usage2 usage,
    fidl::InterfaceRequest<fuchsia::media::audio::VolumeControl> volume_control) {
  TRACE_DURATION("audio", "AudioCoreImpl::BindUsageVolumeControl2");

  if (usage.is_render_usage()) {
    if (usage.render_usage() >= fuchsia::media::RENDER_USAGE2_COUNT) {
      FX_LOGS(WARNING) << "Invalid render usage for BindUsageVolumeControl2";
      volume_control.Close(ZX_ERR_INVALID_ARGS);
    } else {
      context_.volume_manager().BindUsageVolumeClient(std::move(usage), std::move(volume_control));
    }
  } else {
    if (usage.capture_usage() >= fuchsia::media::CAPTURE_USAGE2_COUNT) {
      FX_LOGS(WARNING) << "Invalid capture usage for BindUsageVolumeControl2";
      volume_control.Close(ZX_ERR_INVALID_ARGS);
    } else {
      volume_control.Close(ZX_ERR_NOT_SUPPORTED);
    }
  }
}

void AudioCoreImpl::GetDbFromVolume(fuchsia::media::Usage usage, float volume,
                                    GetDbFromVolumeCallback callback) {
  callback(GetDbFromVolumeBase(ToFidlUsage2(usage), volume));
}

void AudioCoreImpl::GetDbFromVolume2(fuchsia::media::Usage2 usage, float volume,
                                     GetDbFromVolume2Callback callback) {
  if (usage.is_render_usage()) {
    if (usage.render_usage() >= fuchsia::media::RENDER_USAGE2_COUNT) {
      FX_LOGS(WARNING) << "Invalid render usage for GetDbFromVolume2";
      callback(fpromise::ok(fuchsia::media::audio::MUTED_GAIN_DB));
      return;
    }
  } else {
    if (usage.capture_usage() >= fuchsia::media::CAPTURE_USAGE2_COUNT) {
      FX_LOGS(WARNING) << "Invalid capture usage for GetDbFromVolume2";
      callback(fpromise::ok(fuchsia::media::audio::MUTED_GAIN_DB));
      return;
    }
  }

  callback(fpromise::ok(GetDbFromVolumeBase(usage, volume)));
}

float AudioCoreImpl::GetDbFromVolumeBase(const fuchsia::media::Usage2& usage, float volume) {
  auto loudness_transform = context_.route_graph().LoudnessTransformForUsage(ToStreamUsage(usage));
  if (loudness_transform) {
    return loudness_transform->Evaluate<1>({VolumeValue{volume}});
  }
  return context_.process_config().default_volume_curve().VolumeToDb(volume);
}

void AudioCoreImpl::GetVolumeFromDb(fuchsia::media::Usage usage, float gain_db,
                                    GetVolumeFromDbCallback callback) {
  callback(GetVolumeFromDbBase(ToFidlUsage2(usage), gain_db));
}

void AudioCoreImpl::GetVolumeFromDb2(fuchsia::media::Usage2 usage, float gain_db,
                                     GetVolumeFromDb2Callback callback) {
  if (usage.is_render_usage()) {
    if (usage.render_usage() >= fuchsia::media::RENDER_USAGE2_COUNT) {
      FX_LOGS(WARNING) << "Invalid render usage for GetVolumeFromDb2";
      callback(fpromise::ok(fuchsia::media::audio::MIN_VOLUME));
      return;
    }
  } else {
    if (usage.capture_usage() >= fuchsia::media::CAPTURE_USAGE2_COUNT) {
      FX_LOGS(WARNING) << "Invalid capture usage for GetVolumeFromDb2";
      callback(fpromise::ok(fuchsia::media::audio::MIN_VOLUME));
      return;
    }
  }
  callback(fpromise::ok(GetVolumeFromDbBase(usage, gain_db)));
}

float AudioCoreImpl::GetVolumeFromDbBase(const fuchsia::media::Usage2& usage, float gain_db) {
  auto loudness_transform = context_.route_graph().LoudnessTransformForUsage(ToStreamUsage(usage));
  if (loudness_transform) {
    return loudness_transform->Evaluate<1>({GainToVolumeValue{gain_db}});
  }
  return context_.process_config().default_volume_curve().DbToVolume(gain_db);
}

void AudioCoreImpl::SetInteraction(fuchsia::media::Usage active, fuchsia::media::Usage affected,
                                   fuchsia::media::Behavior behavior) {
  TRACE_DURATION("audio", "AudioCoreImpl::SetInteraction");
  SetInteraction2(ToFidlUsage2(active), ToFidlUsage2(affected), behavior);
}

void AudioCoreImpl::SetInteraction2(fuchsia::media::Usage2 active, fuchsia::media::Usage2 affected,
                                    fuchsia::media::Behavior behavior) {
  TRACE_DURATION("audio", "AudioCoreImpl::SetInteraction2");
  if ((active.is_render_usage() && active.render_usage() >= fuchsia::media::RENDER_USAGE2_COUNT) ||
      (affected.is_render_usage() &&
       affected.render_usage() >= fuchsia::media::RENDER_USAGE2_COUNT)) {
    FX_LOGS(WARNING) << "Invalid render usage for SetInteraction2";
    return;
  }
  if ((active.is_capture_usage() &&
       active.capture_usage() >= fuchsia::media::CAPTURE_USAGE2_COUNT) ||
      (affected.is_capture_usage() &&
       affected.capture_usage() >= fuchsia::media::CAPTURE_USAGE2_COUNT)) {
    FX_LOGS(WARNING) << "Invalid capture usage for SetInteraction2";
    return;
  }
  if (behavior >= fuchsia::media::BEHAVIOR_COUNT) {
    FX_LOGS(WARNING) << "Invalid behavior enum for SetInteraction2";
    return;
  }
  context_.audio_admin().SetInteraction(std::move(active), std::move(affected), behavior);
}

void AudioCoreImpl::LoadDefaults() {
  TRACE_DURATION("audio", "AudioCoreImpl::LoadDefaults");
  auto policy = PolicyLoader::LoadPolicy();
  context_.device_router().SetIdlePowerOptionsFromPolicy(policy.idle_power_options());
  context_.audio_admin().SetInteractionsFromAudioPolicy(std::move(policy));
}

void AudioCoreImpl::ResetInteractions() {
  TRACE_DURATION("audio", "AudioCoreImpl::ResetInteractions");
  context_.audio_admin().ResetInteractions();
}

}  // namespace media::audio
