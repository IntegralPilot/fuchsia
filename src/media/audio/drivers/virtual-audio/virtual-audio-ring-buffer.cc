// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio/virtual-audio-ring-buffer.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/clock.h>

#include <algorithm>

#include <fbl/algorithm.h>

namespace virtual_audio {

VirtualAudioRingBuffer::VirtualAudioRingBuffer(
    fuchsia_hardware_audio::Format2 format, fuchsia_virtualaudio::RingBuffer& config,
    bool is_outgoing, async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer> server, OnVmoCreated on_vmo_created,
    OnStart on_start, OnStop on_stop,
    fit::callback<void(VirtualAudioRingBuffer*, fidl::UnbindInfo)> on_close)
    : format_(std::move(format)),
      config_(config),
      is_outgoing_(is_outgoing),
      binding_(dispatcher, std::move(server), this,
               [this, on_close = std::move(on_close)](fidl::UnbindInfo info) mutable {
                 if (on_close) {
                   on_close(this, info);
                 }
               }),
      on_vmo_created_(std::move(on_vmo_created)),
      on_start_(std::move(on_start)),
      on_stop_(std::move(on_stop)) {
  if (format_.pcm_format().has_value()) {
    frame_size_ =
        format_.pcm_format()->bytes_per_sample() * format_.pcm_format()->number_of_channels();
    active_channel_mask_ = (1UL << format_.pcm_format()->number_of_channels()) - 1;
  }
}

void VirtualAudioRingBuffer::GetProperties(GetPropertiesCompleter::Sync& completer) {
  fuchsia_hardware_audio::RingBufferProperties properties;
  properties.needs_cache_flush_or_invalidate(false).driver_transfer_bytes(
      config_.driver_transfer_bytes());
  completer.Reply(std::move(properties));
}

void VirtualAudioRingBuffer::GetVmo(GetVmoRequest& request, GetVmoCompleter::Sync& completer) {
  if (ring_buffer_mapper_.start() != nullptr) {
    ring_buffer_mapper_.Unmap();
  }

  uint32_t min_frames = 0;
  uint32_t modulo_frames = 1;
  if (config_.ring_buffer_constraints().has_value()) {
    min_frames = config_.ring_buffer_constraints()->min_frames();
    modulo_frames = config_.ring_buffer_constraints()->modulo_frames();
  }
  // The ring buffer must be at least min_frames + fifo_frames.
  num_ring_buffer_frames_ =
      request.min_frames() +
      ((config_.driver_transfer_bytes().value() + frame_size_ - 1) / frame_size_);

  num_ring_buffer_frames_ = std::max(
      min_frames, fbl::round_up<uint32_t, uint32_t>(num_ring_buffer_frames_, modulo_frames));

  zx_status_t status = ring_buffer_mapper_.CreateAndMap(
      static_cast<uint64_t>(num_ring_buffer_frames_) * frame_size_,
      ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &ring_buffer_vmo_,
      ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER);

  ZX_ASSERT_MSG(status == ZX_OK, "failed to create ring buffer VMO: %s",
                zx_status_get_string(status));

  zx::vmo out_vmo;
  zx_rights_t required_rights = ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_MAP;
  if (is_outgoing_) {
    required_rights |= ZX_RIGHT_WRITE;
  }
  status = ring_buffer_vmo_.duplicate(required_rights, &out_vmo);
  ZX_ASSERT_MSG(status == ZX_OK, "failed to duplicate VMO handle for out param: %s",
                zx_status_get_string(status));

  notifications_per_ring_ = request.clock_recovery_notifications_per_ring();

  zx::vmo duplicate_vmo_for_va;
  status = ring_buffer_vmo_.duplicate(
      ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP, &duplicate_vmo_for_va);
  ZX_ASSERT_MSG(status == ZX_OK, "failed to duplicate VMO handle for VA client: %s",
                zx_status_get_string(status));

  if (on_vmo_created_) {
    on_vmo_created_(std::move(duplicate_vmo_for_va), num_ring_buffer_frames_,
                    notifications_per_ring_);
  }

  fuchsia_hardware_audio::RingBufferGetVmoResponse response;
  response.num_frames(num_ring_buffer_frames_);
  response.ring_buffer(std::move(out_vmo));
  completer.Reply(zx::ok(std::move(response)));
  ring_buffer_vmo_fetched_ = true;
}

void VirtualAudioRingBuffer::Start(StartCompleter::Sync& completer) {
  if (!ring_buffer_vmo_fetched_) {
    fdf::error("Cannot start the ring buffer before retrieving the VMO");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (ring_buffer_started_) {
    fdf::error("Cannot start the ring buffer if already started");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  zx_time_t now = zx::clock::get_monotonic().get();

  if (on_start_) {
    on_start_(now);
  }

  ring_buffer_started_ = true;
  completer.Reply(now);
}

void VirtualAudioRingBuffer::Stop(StopCompleter::Sync& completer) {
  if (!ring_buffer_vmo_fetched_) {
    fdf::error("Cannot start the ring buffer before retrieving the VMO");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (!ring_buffer_started_) {
    fdf::info("Stop called while stopped; doing nothing");
    completer.Reply();
    return;
  }
  zx_time_t now = zx::clock::get_monotonic().get();

  if (on_stop_) {
    // TODO(https://fxbug.dev/42075676): Add support for 'stop' position, now we always report 0.
    on_stop_(now, 0);
  }

  ring_buffer_started_ = false;
  completer.Reply();
}

void VirtualAudioRingBuffer::WatchClockRecoveryPositionInfo(
    WatchClockRecoveryPositionInfoCompleter::Sync& completer) {
  if (should_reply_to_position_request_ && ring_buffer_started_ && notifications_per_ring_ > 0) {
    fuchsia_hardware_audio::RingBufferPositionInfo position_info;
    position_info.timestamp(zx::clock::get_monotonic().get());
    // TODO(https://fxbug.dev/42075676): Add support for current position; now we always report 0.
    position_info.position(0);
    should_reply_to_position_request_ = false;
    completer.Reply(std::move(position_info));
    return;
  }

  if (position_info_completer_.has_value()) {
    fdf::error("WatchClockRecoveryPositionInfo called while previous call was pending. Unbinding");
    should_reply_to_position_request_ = true;
    position_info_completer_.reset();
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  position_info_completer_.emplace(completer.ToAsync());
}

void VirtualAudioRingBuffer::WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) {
  if (should_reply_to_delay_request_) {
    fuchsia_hardware_audio::DelayInfo delay_info;
    delay_info.internal_delay(config_.internal_delay());
    delay_info.external_delay(config_.external_delay());
    should_reply_to_delay_request_ = false;
    completer.Reply(std::move(delay_info));
  } else if (!delay_info_completer_.has_value()) {
    delay_info_completer_.emplace(completer.ToAsync());
  } else {
    fdf::error("WatchDelayInfo called while previous call was pending. Unbinding");
    should_reply_to_delay_request_ = true;
    delay_info_completer_.reset();
    completer.Close(ZX_ERR_BAD_STATE);
  }
}

void VirtualAudioRingBuffer::SetActiveChannels(SetActiveChannelsRequest& request,
                                               SetActiveChannelsCompleter::Sync& completer) {
  const uint64_t max_channel_bitmask = (1UL << format_.pcm_format()->number_of_channels()) - 1;
  if (request.active_channels_bitmask() > max_channel_bitmask) {
    fdf::warn("SetActiveChannels({:#016x}) is out-of-range", request.active_channels_bitmask());
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (active_channel_mask_ != request.active_channels_bitmask()) {
    active_channel_set_time_ = zx::clock::get_monotonic();
    active_channel_mask_ = request.active_channels_bitmask();
  }
  completer.Reply(zx::ok(active_channel_set_time_.get()));
}

void VirtualAudioRingBuffer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::RingBuffer> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("RingBuffer: unknown method ordinal {}", metadata.method_ordinal);
}

}  // namespace virtual_audio
