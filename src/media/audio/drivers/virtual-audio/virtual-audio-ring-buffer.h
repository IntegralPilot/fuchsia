// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_RING_BUFFER_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_RING_BUFFER_H_

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/fzl/vmo-mapper.h>

namespace virtual_audio {

class VirtualAudioRingBuffer : public fidl::Server<fuchsia_hardware_audio::RingBuffer> {
 public:
  using OnVmoCreated = fit::function<void(zx::vmo, uint32_t, uint32_t)>;
  using OnStart = fit::function<void(zx_time_t)>;
  using OnStop = fit::function<void(zx_time_t, uint32_t)>;

  VirtualAudioRingBuffer(fuchsia_hardware_audio::Format2 format,
                         fuchsia_virtualaudio::RingBuffer& config, bool is_outgoing,
                         async_dispatcher_t* dispatcher,
                         fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer> server,
                         OnVmoCreated on_vmo_created, OnStart on_start, OnStop on_stop,
                         fit::callback<void(VirtualAudioRingBuffer*, fidl::UnbindInfo)> on_close);
  virtual ~VirtualAudioRingBuffer() = default;

  // fuchsia.hardware.audio.RingBuffer implementation.
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void GetVmo(GetVmoRequest& request, GetVmoCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void WatchClockRecoveryPositionInfo(
      WatchClockRecoveryPositionInfoCompleter::Sync& completer) override;
  void WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) override;
  void SetActiveChannels(SetActiveChannelsRequest& request,
                         SetActiveChannelsCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::RingBuffer> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  uint32_t num_frames() const { return num_ring_buffer_frames_; }
  uint32_t notifications_per_ring() const { return notifications_per_ring_; }
  const fuchsia_hardware_audio::Format2& format() const { return format_; }

  zx::result<zx::vmo> DuplicateVmo() {
    if (!ring_buffer_vmo_.is_valid()) {
      return zx::error(ZX_ERR_BAD_STATE);
    }
    zx::vmo dup;
    zx_status_t status = ring_buffer_vmo_.duplicate(
        ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP, &dup);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(dup));
  }

 private:
  fuchsia_hardware_audio::Format2 format_;
  fuchsia_virtualaudio::RingBuffer& config_;
  bool is_outgoing_;

  fidl::ServerBinding<fuchsia_hardware_audio::RingBuffer> binding_;

  OnVmoCreated on_vmo_created_;
  OnStart on_start_;
  OnStop on_stop_;

  fzl::VmoMapper ring_buffer_mapper_;
  zx::vmo ring_buffer_vmo_;
  uint32_t num_ring_buffer_frames_ = 0;
  uint32_t notifications_per_ring_ = 0;
  uint32_t frame_size_ = 0;
  bool ring_buffer_vmo_fetched_ = false;
  bool ring_buffer_started_ = false;

  uint64_t active_channel_mask_;
  zx::time active_channel_set_time_;

  // Hanging gets
  bool should_reply_to_delay_request_ = true;
  std::optional<WatchDelayInfoCompleter::Async> delay_info_completer_;
  bool should_reply_to_position_request_ = true;
  std::optional<WatchClockRecoveryPositionInfoCompleter::Async> position_info_completer_;
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_RING_BUFFER_H_
