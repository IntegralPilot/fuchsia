// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/audio_capturer.h"

#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>

#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/lib/clock/clone_mono.h"

namespace media::audio {

AudioCapturer::AudioCapturer(fuchsia::media::AudioCapturerConfiguration configuration,
                             std::optional<Format> format,
                             fidl::InterfaceRequest<fuchsia::media::AudioCapturer> request,
                             Context* context)
    : BaseCapturer(format, std::move(request), context), loopback_(configuration.is_loopback()) {
  FX_DCHECK(context);
  if (loopback_) {
    usage_ = CaptureUsage::LOOPBACK;
  } else {
    context->volume_manager().AddStream(this);
    if (configuration.input().has_usage2()) {
      usage_ = ToCaptureUsage(configuration.input().usage2());
    } else if (configuration.input().has_usage()) {
      usage_ = ToCaptureUsage(configuration.input().usage());
    }
  }
  if constexpr (kLogAudioCapturerCtorDtor) {
    FX_LOGS(INFO) << __FUNCTION__ << " (" << this << ") usage:" << ToString(usage_);
  }
  reporter().SetUsage(usage_);
}

AudioCapturer::~AudioCapturer() {
  if constexpr (kLogAudioCapturerCtorDtor) {
    FX_LOGS(INFO) << __FUNCTION__ << " (" << this << ") usage:" << ToString(usage_);
  }
  if (!loopback_) {
    context().volume_manager().RemoveStream(this);
  }
}

void AudioCapturer::BeginShutdown() {
  if (!loopback_) {
    context().volume_manager().RemoveStream(this);
  }
  BaseCapturer::BeginShutdown();
}

void AudioCapturer::ReportStart() {
  BaseCapturer::ReportStart();
  if (!loopback_) {
    context().audio_admin().UpdateCapturerState(usage_, true, this);
  }
}

void AudioCapturer::ReportStop() {
  BaseCapturer::ReportStop();
  if (!loopback_) {
    context().audio_admin().UpdateCapturerState(usage_, false, this);
  }
}

void AudioCapturer::OnStateChanged(State old_state, State new_state) {
  BaseCapturer::OnStateChanged(old_state, new_state);
  if (!loopback_ && new_state == State::WaitingForRequest) {
    context().volume_manager().NotifyStreamChanged(this);
  }
}

void AudioCapturer::SetRoutingProfile(bool routable) {
  auto profile = RoutingProfile{
      .routable = routable,
      .usage = StreamUsage::WithCaptureUsage(usage_),
  };
  context().route_graph().SetCapturerRoutingProfile(*this, profile);

  // Once we route the capturer, we accept the default reference clock if one hasn't yet been set.
  if (routable) {
    std::scoped_lock lock(mutex_);
    reference_clock_is_set_ = true;
  }
}

void AudioCapturer::OnLinkAdded() {
  BaseCapturer::OnLinkAdded();
  if (!loopback_) {
    context().volume_manager().NotifyStreamChanged(this);
  }
}

constexpr auto kRequiredClockRights = ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_READ;
// If received clock is null, use our adjustable clock. Else, use this new clock. Fail/disconnect,
// if the client-submitted clock has insufficient rights. Strip off other rights such as WRITE.
void AudioCapturer::SetReferenceClock(zx::clock ref_clock) {
  TRACE_DURATION("audio", "AudioCapturer::SetReferenceClock");
  auto cleanup = fit::defer([this]() { BeginShutdown(); });

  // Lock after storing |cleanup| to ensure |mutex_| is released upon function return, rather than
  // |cleanup| completion.
  std::scoped_lock lock(mutex_);

  // We cannot change the reference clock, once set. Also, once the capturer is routed to a device
  // (which occurs upon AddPayloadBuffer), we set the default clock if one has not yet been set.
  if (reference_clock_is_set_) {
    FX_LOGS(ERROR) << "Cannot change reference clock once it is set!";
    return;
  }

  if (ref_clock.is_valid()) {
    // If ref_clock doesn't have DUPLICATE or READ or TRANSFER rights, return (i.e. shutdown).
    zx_status_t status = ref_clock.replace(kRequiredClockRights, &ref_clock);
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Could not set rights on client-submitted reference clock";
      return;
    }
    SetClock(context().clock_factory()->CreateClientFixed(std::move(ref_clock)));
  } else {
    // To achieve "no-SRC", we will rate-adjust this clock to match the device clock.
    SetClock(context().clock_factory()->CreateClientAdjustable(
        audio::clock::AdjustableCloneOfMonotonic()));
  }

  reference_clock_is_set_ = true;

  cleanup.cancel();
}

void AudioCapturer::SetPcmStreamType(fuchsia::media::AudioStreamType stream_type) {
  TRACE_DURATION("audio", "AudioCapturer::SetPcmStreamType");
  // If something goes wrong, hang up the phone and shutdown.
  auto cleanup = fit::defer([this]() { BeginShutdown(); });

  // If our shared buffer has been assigned, we are operating and our mode can no longer be changed.
  State state = capture_state();
  if (state != State::WaitingForVmo) {
    FX_LOGS(ERROR) << "Cannot change format after payload buffer has been added"
                   << "(state = " << static_cast<uint32_t>(state) << ")";
    return;
  }

  auto format_result = Format::Create(stream_type);
  if (format_result.is_error()) {
    FX_LOGS(ERROR) << "AudioCapturer: PcmStreamType is invalid";
    return;
  }

  if (stream_type.channels > 4) {
    FX_LOGS(ERROR) << "AudioCapturer::PcmStreamType specified channels (" << stream_type.channels
                   << ") is too large";
    return;
  }

  // Success, record our new format.
  UpdateFormat(format_result.take_value());

  cleanup.cancel();
}

void AudioCapturer::BindGainControl(
    fidl::InterfaceRequest<fuchsia::media::audio::GainControl> request) {
  TRACE_DURATION("audio", "AudioCapturer::BindGainControl");
  gain_control_bindings_.AddBinding(this, std::move(request));
}

void AudioCapturer::SetUsage(fuchsia::media::AudioCaptureUsage usage) {
  TRACE_DURATION("audio", "AudioCapturer::SetUsage");
  SetUsage2(ToFidlCaptureUsage2(usage));
}

void AudioCapturer::SetUsage2(fuchsia::media::AudioCaptureUsage2 usage) {
  TRACE_DURATION("audio", "AudioCapturer::SetUsage2");
  if (usage >= fuchsia::media::CAPTURE_USAGE2_COUNT) {
    FX_LOGS(ERROR) << "Invalid capture usage for SetUsage2.";
    context().route_graph().RemoveCapturer(*this);
    return;
  }
  if (usage_ == ToCaptureUsage(usage)) {
    return;
  }
  if (loopback_) {
    FX_LOGS(WARNING) << "SetUsage on loopback capturer is not allowed - ignoring this command";
    return;
  }

  State state = capture_state();
  if (state == State::SyncOperating || state == State::AsyncOperating) {
    context().audio_admin().UpdateCapturerState(usage_, false, this);
  }

  if constexpr (kLogAudioCapturerSetUsage) {
    FX_LOGS(INFO) << __FUNCTION__ << " (" << this << ") changed from " << ToString(usage_) << " to "
                  << ToString(ToCaptureUsage(usage));
  }

  usage_ = ToCaptureUsage(usage);
  reporter().SetUsage(usage_);
  context().volume_manager().NotifyStreamChanged(this);
  SetRoutingProfile(StateIsRoutable(state));

  if (state == State::SyncOperating || state == State::AsyncOperating) {
    context().audio_admin().UpdateCapturerState(usage_, true, this);
  }
}

fuchsia::media::Usage2 AudioCapturer::GetStreamUsage() const {
  // We should only be calling these from the StreamVolumeManager. We don't register LOOPBACK
  // capturers with the StreamVolumeManager since those capturers do not have a compatible usage.
  FX_CHECK(!loopback_);
  fuchsia::media::Usage2 usage;
  usage.set_capture_usage(ToFidlCaptureUsage2(usage_));
  return usage;
}

void AudioCapturer::RealizeVolume(VolumeCommand volume_command) {
  if (volume_command.ramp.has_value()) {
    FX_LOGS(WARNING) << "Capturer gain ramping is not implemented - ignoring the ramp component";
  }

  context().link_matrix().ForEachSourceLink(*this, [this,
                                                    volume_command](LinkMatrix::LinkHandle link) {
    float gain_db = link.loudness_transform->Evaluate<3>({
        VolumeValue{volume_command.volume},
        GainDbFsValue{volume_command.gain_db_adjustment},
        GainDbFsValue{stream_gain_db_},
    });

    std::stringstream stream;
    stream << static_cast<const void*>(this) << " (link " << static_cast<const void*>(&link) << ") "
           << StreamUsage::WithCaptureUsage(usage_).ToString() << " Gain(" << gain_db
           << "db) = " << "Vol(" << volume_command.volume << ") + GainAdjustment("
           << volume_command.gain_db_adjustment << "db) + StreamGain(" << stream_gain_db_ << "db)";
    std::string log_string = stream.str();

    // log_string is only included for log-display of loudness changes
    mix_domain().PostTask([link, gain_db, log_string]() {
      if (gain_db != link.mixer->gain.GetGainDb()) {
        link.mixer->gain.SetDestGain(gain_db);

        if constexpr (kLogCaptureUsageVolumeGainActions) {
          // TODO(https://fxbug.dev/42128197) Logging should be removed upon creation of inspect
          // tool or other real-time method for gain observation
          FX_LOGS(INFO) << log_string;
        }
      }
    });
  });
}

void AudioCapturer::SetGain(float gain_db) {
  TRACE_DURATION("audio", "AudioCapturer::SetGain");
  // Before setting stream_gain_db_, we should always perform this range check.
  if ((gain_db < fuchsia::media::audio::MUTED_GAIN_DB) ||
      (gain_db > fuchsia::media::audio::MAX_GAIN_DB) || isnan(gain_db)) {
    FX_LOGS(ERROR) << "SetGain(" << gain_db << " dB) out of range.";
    BeginShutdown();
    return;
  }

  // If the incoming SetGain request represents no change, we're done
  // (once we add gain ramping, this type of check isn't workable).
  if (stream_gain_db_ == gain_db) {
    return;
  }

  stream_gain_db_ = gain_db;
  reporter().SetGain(gain_db);

  if (!loopback_) {
    context().volume_manager().NotifyStreamChanged(this);
  }

  NotifyGainMuteChanged();
}

void AudioCapturer::SetMute(bool mute) {
  TRACE_DURATION("audio", "AudioCapturer::SetMute");
  // If the incoming SetMute request represents no change, we're done.
  if (mute_ == mute) {
    return;
  }

  reporter().SetMute(mute);

  mute_ = mute;

  if (!loopback_) {
    context().volume_manager().NotifyStreamChanged(this);
  }
  NotifyGainMuteChanged();
}

void AudioCapturer::NotifyGainMuteChanged() {
  TRACE_DURATION("audio", "AudioCapturer::NotifyGainMuteChanged");
  // Consider making these events disable-able like MinLeadTime.
  for (auto& gain_binding : gain_control_bindings_.bindings()) {
    gain_binding->events().OnGainMuteChanged(stream_gain_db_, mute_);
  }
}

}  // namespace media::audio
