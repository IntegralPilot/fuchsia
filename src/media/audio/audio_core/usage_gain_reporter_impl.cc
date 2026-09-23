// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/usage_gain_reporter_impl.h"

#include <algorithm>

#include "src/media/audio/audio_core/device_id.h"

namespace media::audio {

fidl::InterfaceRequestHandler<fuchsia::media::UsageGainReporter>
UsageGainReporterImpl::GetFidlRequestHandler() {
  return bindings_.GetHandler(this);
}

void UsageGainReporterImpl::RegisterListener(
    std::string device_unique_id, fuchsia::media::Usage _usage,
    fidl::InterfaceHandle<fuchsia::media::UsageGainListener> usage_gain_listener_handler) {
  auto usage = ToFidlUsage2(_usage);
  RegisterListener2(device_unique_id, std::move(usage), std::move(usage_gain_listener_handler));
}

void UsageGainReporterImpl::RegisterListener2(
    std::string device_unique_id, fuchsia::media::Usage2 usage,
    fidl::InterfaceHandle<fuchsia::media::UsageGainListener> usage_gain_listener_handler) {
  if (usage.is_render_usage() && usage.render_usage() >= fuchsia::media::RENDER_USAGE2_COUNT) {
    FX_LOGS(WARNING) << "Invalid render usage for RegisterListener2; ignoring.";
    return;
  }
  if (usage.is_capture_usage() && usage.capture_usage() >= fuchsia::media::CAPTURE_USAGE2_COUNT) {
    FX_LOGS(WARNING) << "Invalid capture usage for RegisterListener2; ignoring.";
    return;
  }
  const auto deserialize_result = DeviceUniqueIdFromString(device_unique_id);
  if (deserialize_result.is_error()) {
    FX_LOGS(WARNING) << "UsageGainReporter client provided invalid device id";
    return;
  }
  const auto& deserialized_id = deserialize_result.value();

  auto devices = device_lister_.GetDeviceInfos();
  const auto it = std::ranges::find_if(devices, [&device_unique_id](const auto& candidate) {
    return candidate.unique_id == device_unique_id;
  });
  if (it == devices.end()) {
    FX_LOGS(WARNING) << "UsageGainReporter client cannot listen: device id not found";
    return;
  }

  auto usage_gain_listener = usage_gain_listener_handler.Bind();

  const auto& device_config = process_config_.device_config();
  const auto& output_device_profile = device_config.output_device_profile(deserialized_id);
  auto listener = std::make_unique<Listener>(*this, output_device_profile, std::move(usage),
                                             std::move(usage_gain_listener));
  stream_volume_manager_.AddStream(listener.get());

  listeners_[listener.get()] = std::move(listener);
}

UsageGainReporterImpl::Listener::Listener(
    UsageGainReporterImpl& parent, const DeviceConfig::OutputDeviceProfile& output_device_profile,
    fuchsia::media::Usage2 usage, fuchsia::media::UsageGainListenerPtr usage_gain_listener)
    : parent_(parent),
      loudness_transform_(output_device_profile.loudness_transform()),
      independent_volume_control_(output_device_profile.independent_volume_control()),
      usage_(std::move(usage)),
      usage_gain_listener_(std::move(usage_gain_listener)) {
  usage_gain_listener_.set_error_handler([this](zx_status_t) {
    parent_.stream_volume_manager_.RemoveStream(this);
    parent_.listeners_.erase(this);
  });
}

void UsageGainReporterImpl::Listener::RealizeVolume(VolumeCommand volume_command) {
  if (!independent_volume_control_) {
    const auto gain_db = loudness_transform_->Evaluate<2>(
        {VolumeValue{volume_command.volume}, GainDbFsValue{volume_command.gain_db_adjustment}});

    unacked_messages_++;
    usage_gain_listener_->OnGainMuteChanged(/*muted=*/false, gain_db,
                                            [this]() { unacked_messages_--; });
  }
}

}  // namespace media::audio
