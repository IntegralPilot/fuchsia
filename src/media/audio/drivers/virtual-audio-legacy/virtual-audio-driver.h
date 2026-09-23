// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DRIVER_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DRIVER_H_

#include <memory>

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-device.h"

namespace virtual_audio {

struct CurrentFormat {
  uint32_t frames_per_second;
  uint32_t sample_format;
  uint32_t num_channels;
  zx::duration external_delay;
};
struct CurrentGain {
  bool mute;
  bool agc;
  float gain_db;
};
struct CurrentBuffer {
  zx::vmo vmo;
  uint32_t num_frames;
  uint32_t notifications_per_ring;
};
struct CurrentPosition {
  zx::time monotonic_time;
  uint32_t ring_position;
};

class VirtualAudioDriver : public std::enable_shared_from_this<VirtualAudioDriver> {
 public:
  using ErrorT = fuchsia_virtualaudio::Error;

  explicit VirtualAudioDriver(fit::closure on_shutdown) : on_shutdown_(std::move(on_shutdown)) {
    ZX_ASSERT(on_shutdown_ != nullptr);
  }

  virtual ~VirtualAudioDriver() = default;

  // Shutdown the stream and request that it be unbound from the device tree.
  virtual void ShutdownAsync() = 0;

  //
  // The following methods implement getters and setters for fuchsia.virtualaudio.Device.
  // Default to not supported. Callback may be invoked on any dispatcher.
  // TODO(https://fxbug.dev/42077474): Add the ability to trigger dynamic delay changes.
  //
  virtual void GetFormatForVA(fit::callback<void(fit::result<ErrorT, CurrentFormat>)> callback) {
    callback(fit::error(ErrorT::kNotSupported));
  }
  virtual void GetGainForVA(fit::callback<void(fit::result<ErrorT, CurrentGain>)> callback) {
    callback(fit::error(ErrorT::kNotSupported));
  }
  virtual void GetBufferForVA(fit::callback<void(fit::result<ErrorT, CurrentBuffer>)> callback) {
    callback(fit::error(ErrorT::kNotSupported));
  }
  virtual void GetPositionForVA(
      fit::callback<void(fit::result<ErrorT, CurrentPosition>)> callback) {
    callback(fit::error(ErrorT::kNotSupported));
  }
  virtual void SetNotificationFrequencyFromVA(uint32_t notifications_per_ring,
                                              fit::callback<void(fit::result<ErrorT>)> callback) {
    callback(fit::error(ErrorT::kNotSupported));
  }
  virtual void ChangePlugStateFromVA(bool plugged,
                                     fit::callback<void(fit::result<ErrorT>)> callback) {
    callback(fit::error(ErrorT::kNotSupported));
  }
  virtual void AdjustClockRateFromVA(int32_t ppm_from_monotonic,
                                     fit::callback<void(fit::result<ErrorT>)> callback) {
    callback(fit::error(ErrorT::kNotSupported));
  }

 protected:
  // Call when the driver has completely shutdown.
  void OnShutdown() { on_shutdown_(); }

 private:
  fit::closure on_shutdown_;
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_DRIVER_H_
