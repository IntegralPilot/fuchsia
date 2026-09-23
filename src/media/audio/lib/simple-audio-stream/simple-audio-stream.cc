// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.audio/cpp/wire_types.h>
#include <lib/async/dispatcher.h>
#include <lib/ddk/debug.h>
#include <lib/fidl/cpp/wire/array.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/simple-audio-stream/simple-audio-stream.h>
#include <lib/zx/clock.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <zircon/device/audio.h>
#include <zircon/errors.h>

#include <cmath>
#include <utility>

#include <audio-proto-utils/format-utils.h>

namespace audio {

SimpleAudioStream::SimpleAudioStream(zx_device_t* parent, bool is_input)
    : SimpleAudioStreamBase(parent),
      SimpleAudioStreamProtocol(is_input),
      loop_(&kAsyncLoopConfigNeverAttachToThread) {
  simple_audio_ = inspect_.GetRoot().CreateChild("simple_audio_stream");
  state_ = simple_audio_.CreateString("state", "created");
  start_time_ = simple_audio_.CreateInt("start_time", 0);
  position_request_time_ = simple_audio_.CreateInt("position_request_time", 0);
  position_reply_time_ = simple_audio_.CreateInt("position_reply_time", 0);
  ring_buffer_size_ = simple_audio_.CreateUint("ring_buffer_size", 0);
  frames_requested_ = simple_audio_.CreateUint("frames_requested", 0);

  number_of_channels_ = simple_audio_.CreateUint("number_of_channels", 0);
  channels_to_use_bitmask_ = simple_audio_.CreateUint("channels_to_use_bitmask", 0);
  frame_rate_ = simple_audio_.CreateUint("frame_rate", 0);
  bits_per_slot_ = simple_audio_.CreateUint("bits_per_slot", 0);
  bits_per_sample_ = simple_audio_.CreateUint("bits_per_sample", 0);
  sample_format_ = simple_audio_.CreateString("sample_format", "not_set");
}

void SimpleAudioStream::Shutdown() {
  bool already_shutting_down;
  {
    fbl::AutoLock channel_lock(&channel_lock_);
    already_shutting_down = shutting_down_;
    shutting_down_ = true;
  }
  if (!already_shutting_down) {
    loop_.Shutdown();

    // We have shutdown our loop, it is now safe to assert we are holding the domain token.
    ScopedToken t(domain_token());

    {
      // Now we explicitly destroy the channels.
      fbl::AutoLock channel_lock(&channel_lock_);
      DeactivateRingBufferChannel(rb_channel_.get());

      stream_channels_.clear();
      stream_channel_ = nullptr;
    }

    ShutdownHook();
  }
}

zx_status_t SimpleAudioStream::CreateInternal() {
  zx_status_t res;

  {
    // We have not created the domain yet, it should be safe to pretend that
    // we have the token (since we know that no dispatches are going to be
    // invoked from the non-existent domain at this point)
    ScopedToken t(domain_token());
    res = Init();
    if (res != ZX_OK) {
      zxlogf(ERROR, "Failed during initialization (err %d)", res);
      return res;
    }
    // If no subclass has set this, we need to do so here.
    if (plug_time_ == 0) {
      plug_time_ = zx::clock::get_monotonic().get();
    }
  }

  // TODO(https://fxbug.dev/42113004): Add profile configuration.
  // This single threaded dispatcher serializes the FIDL server implementation by this class.
  loop_.StartThread("simple-audio-stream-loop");

  res = PublishInternal();
  if (res != ZX_OK) {
    zxlogf(ERROR, "Failed during publishing (err %d)", res);
    return res;
  }

  return ZX_OK;
}

zx_status_t SimpleAudioStream::PublishInternal() {
  device_name_[sizeof(device_name_) - 1] = 0;
  if (!strlen(device_name_)) {
    zxlogf(ERROR, "Zero-length device name");
    return ZX_ERR_BAD_STATE;
  }

  // If we succeed in adding our device, add an explicit reference to
  // ourselves to represent the reference now being held by the DDK.  We will
  // get this reference back when the DDK (eventually) calls release.
  zx_status_t res =
      DdkAdd(ddk::DeviceAddArgs(device_name_).set_inspect_vmo(inspect_.DuplicateVmo()));
  if (res == ZX_OK) {
    AddRef();
  }

  return res;
}

// Called by a child subclass during Init, to establish the properties of this plug.
// Caller must include only flags defined for audio_stream_cmd_plug_detect_resp_t.
void SimpleAudioStream::SetInitialPlugState(audio_pd_notify_flags_t initial_state) {
  audio_pd_notify_flags_t known_flags =
      AUDIO_PDNF_HARDWIRED | AUDIO_PDNF_CAN_NOTIFY | AUDIO_PDNF_PLUGGED;
  ZX_DEBUG_ASSERT((initial_state & known_flags) == initial_state);

  pd_flags_ = initial_state;
  plug_time_ = zx::clock::get_monotonic().get();
}

// Called by a child subclass when a dynamic plug state change occurs.
// Special behavior if this isn't actually a change, or if we should not be able to unplug.
zx_status_t SimpleAudioStream::SetPlugState(bool plugged) {
  if (plugged == ((pd_flags_ & AUDIO_PDNF_PLUGGED) != 0)) {
    return ZX_OK;
  }

  ZX_DEBUG_ASSERT(((pd_flags_ & AUDIO_PDNF_HARDWIRED) == 0) || plugged);

  if (plugged) {
    pd_flags_ |= AUDIO_PDNF_PLUGGED;
  } else {
    pd_flags_ &= ~AUDIO_PDNF_PLUGGED;
  }
  plug_time_ = zx::clock::get_monotonic().get();

  if (pd_flags_ & AUDIO_PDNF_CAN_NOTIFY) {
    return NotifyPlugDetect();
  }

  return ZX_OK;
}

// Asynchronously notify of plug state changes.
zx_status_t SimpleAudioStream::NotifyPlugDetect() {
  fbl::AutoLock channel_lock(&channel_lock_);
  for (auto& channel : stream_channels_) {
    if (channel.plug_completer_) {
      fidl::Arena allocator;
      auto plug_state = audio_fidl::wire::PlugState::Builder(allocator)
                            .plugged(pd_flags_ & AUDIO_PDNF_PLUGGED)
                            .plug_state_time(plug_time_)
                            .Build();

      channel.plug_completer_->Reply(plug_state);
      channel.plug_completer_.reset();
    }
  }
  return ZX_OK;
}

zx_status_t SimpleAudioStream::NotifyPosition(const audio_proto::RingBufPositionNotify& notif) {
  fbl::AutoLock channel_lock(&channel_lock_);
  if (!expected_notifications_per_ring_.load() || rb_channel_ == nullptr) {
    return ZX_ERR_BAD_STATE;
  }
  audio_fidl::wire::RingBufferPositionInfo position_info = {};
  position_info.position = notif.ring_buffer_pos;
  position_info.timestamp = notif.monotonic_time;

  fbl::AutoLock position_lock(&position_lock_);
  if (position_completer_) {
    position_completer_->Reply(position_info);
    position_completer_.reset();
    position_reply_time_.Set(zx::clock::get_monotonic().get());
  }
  return ZX_OK;
}

void SimpleAudioStream::DdkUnbind(ddk::UnbindTxn txn) {
  Shutdown();

  // TODO(johngro): We need to signal our SimpleAudioStream owner to let them
  // know that we have been unbound and are in the process of shutting down.

  // Unpublish our device node.
  txn.Reply();
}

void SimpleAudioStream::DdkRelease() {
  // Recover our ref from the DDK, then let it fall out of scope.
  auto thiz = fbl::ImportFromRawPtr(this);
}

void SimpleAudioStream::DdkSuspend(ddk::SuspendTxn txn) {
  // TODO(https://fxbug.dev/42118826): Implement proper power management based on the requested
  // state.
  Shutdown();
  txn.Reply(ZX_OK, txn.requested_state());
}

void SimpleAudioStream::Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) {
  fbl::AutoLock channel_lock(&channel_lock_);
  if (shutting_down_) {
    zxlogf(ERROR, "Can't retrieve the stream channel -- we are closing");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  // Attempt to allocate a new driver channel and bind it to us.  If we don't
  // already have an stream_channel_, flag this channel is the privileged
  // connection (The connection which is allowed to do things like change
  // formats).
  bool privileged = stream_channel_ == nullptr;

  auto stream_channel = StreamChannel::Create<StreamChannel>(this);
  // We keep alive all channels in stream_channels_ (protected by channel_lock_).
  stream_channels_.push_back(stream_channel);
  fidl::OnUnboundFn<fidl::WireServer<audio_fidl::StreamConfig>> on_unbound =
      [this, stream_channel](fidl::WireServer<audio_fidl::StreamConfig>*, fidl::UnbindInfo info,
                             fidl::ServerEnd<fuchsia_hardware_audio::StreamConfig>) {
        // Do not log canceled cases which happens too often in particular in test cases.
        if (info.status() != ZX_ERR_CANCELED && !info.is_peer_closed() &&
            !info.is_user_initiated()) {
          zxlogf(INFO, "StreamConf channel closing: %s", info.FormatDescription().c_str());
        }
        ScopedToken t(domain_token());
        fbl::AutoLock channel_lock(&channel_lock_);
        this->DeactivateStreamChannel(stream_channel.get());
      };

  fidl::BindServer(dispatcher(), std::move(request->protocol), stream_channel.get(),
                   std::move(on_unbound));

  if (privileged) {
    ZX_DEBUG_ASSERT(stream_channel_ == nullptr);
    stream_channel_ = std::move(stream_channel);
  }
}

void SimpleAudioStream::DeactivateStreamChannel(StreamChannel* channel) {
  if (stream_channel_.get() == channel) {
    stream_channel_ = nullptr;
  }

  // Any pending LLCPP completer must be either replied to or closed before we destroy it.
  if (channel->plug_completer_.has_value()) {
    zxlogf(INFO, "Plug completer is still open when deactivating stream channel");
    // This is expected/normal: clients may always have an outstanding Plug hanging-get, and it is
    // not automatically completed/canceled when either side closes the StreamConfig channel.
    channel->plug_completer_->Reply({});
  }
  channel->plug_completer_.reset();

  // Any pending LLCPP completer must be either replied to or closed before we destroy it.
  if (channel->gain_completer_.has_value()) {
    zxlogf(INFO, "Gain completer is still open when deactivating stream channel");
    // This is expected/normal: clients may always have an outstanding Gain hanging-get, and it is
    // not automatically completed/canceled when either side closes the StreamConfig channel.
    channel->gain_completer_->Reply({});
  }
  channel->gain_completer_.reset();

  stream_channels_.erase(*channel);  // Must be last since we may destruct *channel.
}

void SimpleAudioStream::DeactivateRingBufferChannel(const Channel* channel) {
  if (rb_channel_.get() == channel) {
    if (rb_started_) {
      Stop();
      rb_started_ = false;
      state_.Set("deactivated");
    }
    RingBufferShutdown();
    rb_vmo_fetched_ = false;
    delay_info_updated_ = false;
    // Any pending LLCPP completer must be either replied to or closed before we destroy it.
    if (delay_completer_.has_value()) {
      zxlogf(INFO, "Delay completer is still open when deactivating ring buffer channel");
      // This is expected/normal: clients may always have an outstanding Delay hanging-get while the
      // RingBuffer lives. No RingBuffer call (e.g. `Stop()`) automatically completes/cancels them.
      delay_completer_->Reply({});  // Don't close the channel with an error.
    }
    delay_completer_.reset();

    expected_notifications_per_ring_.store(0);
    {
      fbl::AutoLock position_lock(&position_lock_);
      // Any pending LLCPP completer must be either replied to or closed before we destroy it.
      if (position_completer_.has_value()) {
        zxlogf(INFO, "Position completer is still open when deactivating ring buffer channel");
        // This is expected/normal: clients may always have an outstanding Position hanging-get
        // while the RingBuffer lives. No RingBuffer call automatically completes/cancels them.
        position_completer_->Reply({});  // Don't close the channel with an error.
      }
      position_completer_.reset();
    }
    rb_channel_ = nullptr;
  }
}

void SimpleAudioStream::CreateRingBuffer(
    StreamChannel* channel, audio_fidl::wire::Format format,
    fidl::ServerEnd<audio_fidl::RingBuffer> ring_buffer,
    fidl::WireServer<audio_fidl::StreamConfig>::CreateRingBufferCompleter::Sync& completer) {
  ScopedToken t(domain_token());
  zx::channel rb_channel_local;
  zx::channel rb_channel_remote;
  bool found_one = false;

  // On errors we call Close() and ring_buffer goes out of scope closing the channel.

  // Only the privileged stream channel is allowed to change the format.
  {
    fbl::AutoLock channel_lock(&channel_lock_);
    if (channel != stream_channel_.get()) {
      zxlogf(ERROR, "Unprivileged channel cannot set the format");
      completer.Close(ZX_ERR_INVALID_ARGS);
      return;
    }
  }

  auto pcm_format = format.pcm_format();
  audio_sample_format_t sample_format = audio::utils::GetSampleFormat(
      pcm_format.valid_bits_per_sample, 8 * pcm_format.bytes_per_sample);

  if (sample_format == 0) {
    zxlogf(ERROR, "Unsupported format: Invalid bits per sample (%u/%u)\n",
           pcm_format.valid_bits_per_sample, 8 * pcm_format.bytes_per_sample);
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (pcm_format.sample_format == audio_fidl::wire::SampleFormat::kPcmFloat) {
    sample_format = AUDIO_SAMPLE_FORMAT_32BIT_FLOAT;
    if (pcm_format.valid_bits_per_sample != 32 || pcm_format.bytes_per_sample != 4) {
      zxlogf(ERROR, "Unsupported format: float format must be 4 byte, 32 valid-bits\n");
      completer.Close(ZX_ERR_INVALID_ARGS);
      return;
    }
  }

  if (pcm_format.sample_format == audio_fidl::wire::SampleFormat::kPcmUnsigned) {
    sample_format |= AUDIO_SAMPLE_FORMAT_FLAG_UNSIGNED;
  }

  // Check the format for compatibility
  for (const auto& fmt : supported_formats_) {
    if (audio::utils::FormatIsCompatible(pcm_format.frame_rate,
                                         static_cast<uint16_t>(pcm_format.number_of_channels),
                                         sample_format, fmt.range)) {
      found_one = true;
      break;
    }
  }

  if (!found_one) {
    zxlogf(ERROR, "Could not find a suitable format");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  // Determine the frame size.
  frame_size_ = audio::utils::ComputeFrameSize(static_cast<uint16_t>(pcm_format.number_of_channels),
                                               sample_format);
  if (!frame_size_) {
    zxlogf(ERROR, "Failed to compute frame size (ch %hu fmt 0x%08x)", pcm_format.number_of_channels,
           sample_format);
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  // Looks like we are going ahead with this format change.  Tear down any
  // exiting ring buffer interface before proceeding.
  {
    fbl::AutoLock channel_lock(&channel_lock_);
    if (rb_channel_ != nullptr) {
      DeactivateRingBufferChannel(rb_channel_.get());
      ZX_DEBUG_ASSERT(rb_channel_ == nullptr);
    }
  }

  audio_stream_cmd_set_format_req_t req = {};
  req.frames_per_second = pcm_format.frame_rate;
  req.sample_format = sample_format;
  req.channels = static_cast<uint16_t>(pcm_format.number_of_channels);

  // Actually attempt to change the format.
  auto result = ChangeFormat(req);
  if (result != ZX_OK) {
    zxlogf(ERROR, "Failed to change the format");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  uint32_t bytes_per_frame = static_cast<uint32_t>(pcm_format.bytes_per_sample) *
                             static_cast<uint32_t>(pcm_format.number_of_channels);
  if (pcm_format.frame_rate == 0) {
    zxlogf(ERROR, "Bad (zero) frame rate");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  if (bytes_per_frame == 0) {
    zxlogf(ERROR, "Bad (zero) frame size");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  internal_delay_nsec_ = 0;  // No internal delay known, so we report 0.

  number_of_channels_.Set(pcm_format.number_of_channels);
  frame_rate_.Set(pcm_format.frame_rate);
  bits_per_slot_.Set(pcm_format.bytes_per_sample * 8ul);
  bits_per_sample_.Set(pcm_format.valid_bits_per_sample);
  using FidlSampleFormat = audio_fidl::wire::SampleFormat;
  // clang-format off
  switch (pcm_format.sample_format) {
    case FidlSampleFormat::kPcmSigned:   sample_format_.Set("PCM_signed");   break;
    case FidlSampleFormat::kPcmUnsigned: sample_format_.Set("PCM_unsigned"); break;
    case FidlSampleFormat::kPcmFloat:    sample_format_.Set("PCM_float");    break;
  }
  // clang-format on

  {
    fbl::AutoLock channel_lock(&channel_lock_);
    if (shutting_down_) {
      zxlogf(ERROR, "Already shutting down when trying to create ring buffer");
      completer.Close(ZX_ERR_BAD_STATE);
      return;
    }

    rb_channel_ = Channel::Create<Channel>();

    fidl::OnUnboundFn<fidl::WireServer<audio_fidl::RingBuffer>> on_unbound =
        [this](fidl::WireServer<audio_fidl::RingBuffer>*, fidl::UnbindInfo info,
               fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer>) {
          // Do not log canceled cases which happens too often in particular in test cases.
          if (info.status() != ZX_ERR_CANCELED && !info.is_peer_closed() &&
              !info.is_user_initiated()) {
            zxlogf(INFO, "Ring buffer channel closing: %s", info.FormatDescription().c_str());
          }
          ScopedToken t(domain_token());
          fbl::AutoLock channel_lock(&channel_lock_);
          this->DeactivateRingBufferChannel(rb_channel_.get());
        };

    fidl::BindServer(dispatcher(), std::move(ring_buffer), this, std::move(on_unbound));
  }
}

void SimpleAudioStream::WatchGainState(StreamChannel* channel,
                                       StreamChannel::WatchGainStateCompleter::Sync& completer) {
  if (channel->gain_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    channel->gain_completer_.reset();
    return;
  }
  channel->gain_completer_ = completer.ToAsync();

  ScopedToken t(domain_token());
  // Reply is delayed if there is no change since the last reported gain state.
  if (channel->last_reported_gain_state_ != cur_gain_state_) {
    fidl::Arena allocator;
    auto gain_state = audio_fidl::wire::GainState::Builder(allocator);
    if (cur_gain_state_.can_mute) {
      gain_state.muted(cur_gain_state_.cur_mute);
    }
    if (cur_gain_state_.can_agc) {
      gain_state.agc_enabled(cur_gain_state_.cur_agc);
    }
    gain_state.gain_db(cur_gain_state_.cur_gain);
    channel->last_reported_gain_state_ = cur_gain_state_;
    channel->gain_completer_->Reply(gain_state.Build());
    channel->gain_completer_.reset();
  }
}

void SimpleAudioStream::WatchPlugState(StreamChannel* channel,
                                       StreamChannel::WatchPlugStateCompleter::Sync& completer) {
  if (channel->plug_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    channel->plug_completer_.reset();
    return;
  }
  channel->plug_completer_ = completer.ToAsync();

  ScopedToken t(domain_token());
  bool plugged = pd_flags_ & AUDIO_PDNF_PLUGGED;
  // Reply is delayed if there is no change since the last reported plugged state.
  if (channel->last_reported_plugged_state_ == StreamChannel::Plugged::kNotReported ||
      (channel->last_reported_plugged_state_ == StreamChannel::Plugged::kPlugged) != plugged) {
    fidl::Arena allocator;
    // plug_state_time can't be any later than "right now"
    auto plug_time = std::min(plug_time_, zx::clock::get_monotonic().get());
    auto plug_state = audio_fidl::wire::PlugState::Builder(allocator)
                          .plugged(plugged)
                          .plug_state_time(plug_time)
                          .Build();
    channel->last_reported_plugged_state_ =
        plugged ? StreamChannel::Plugged::kPlugged : StreamChannel::Plugged::kUnplugged;
    channel->plug_completer_->Reply(plug_state);
    channel->plug_completer_.reset();
  }
}

void SimpleAudioStream::WatchClockRecoveryPositionInfo(
    WatchClockRecoveryPositionInfoCompleter::Sync& completer) {
  fbl::AutoLock position_lock(&position_lock_);

  if (position_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    position_completer_.reset();
    return;
  }

  position_request_time_.Set(zx::clock::get_monotonic().get());
  position_completer_ = completer.ToAsync();
}

void SimpleAudioStream::WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) {
  ScopedToken t(domain_token());

  if (delay_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    delay_completer_.reset();
    return;
  }

  if (!delay_info_updated_) {
    delay_info_updated_ = true;
    fidl::Arena allocator;
    auto delay_info = audio_fidl::wire::DelayInfo::Builder(allocator);
    delay_info.internal_delay(internal_delay_nsec_);
    delay_info.external_delay(external_delay_nsec_);
    completer.Reply(delay_info.Build());
  } else {
    delay_completer_ = completer.ToAsync();
  }
}

void SimpleAudioStream::handle_unknown_method(fidl::UnknownMethodMetadata<audio_fidl::RingBuffer>,
                                              fidl::UnknownMethodCompleter::Sync&) {}

void SimpleAudioStream::SetGain(audio_fidl::wire::GainState target_state,
                                StreamChannel::SetGainCompleter::Sync& completer) {
  ScopedToken t(domain_token());
  audio_stream_cmd_set_gain_req_t req = {};

  // Sanity check the request before passing it along
  if (target_state.has_muted() && target_state.muted() && !cur_gain_state_.can_mute) {
    zxlogf(ERROR, "Can't mute\n");
    return;
  }

  if (target_state.has_agc_enabled() && target_state.agc_enabled() && !cur_gain_state_.can_agc) {
    zxlogf(ERROR, "Can't enable AGC\n");
    return;
  }

  if (target_state.has_gain_db()) {
    if (!std::isfinite(target_state.gain_db())) {
      zxlogf(ERROR, "Can't set gain for invalid float gain_db\n");
      return;
    }

    if (target_state.gain_db() < cur_gain_state_.min_gain ||
        target_state.gain_db() > cur_gain_state_.max_gain) {
      zxlogf(ERROR, "Can't set gain outside valid range\n");
      return;
    }
  }

  if (target_state.has_muted()) {
    req.flags |= AUDIO_SGF_MUTE_VALID;
    if (target_state.muted()) {
      req.flags |= AUDIO_SGF_MUTE;
    }
  }
  if (target_state.has_agc_enabled()) {
    req.flags |= AUDIO_SGF_AGC_VALID;
    if (target_state.agc_enabled()) {
      req.flags |= AUDIO_SGF_AGC;
    }
  }
  if (target_state.has_gain_db()) {
    req.flags |= AUDIO_SGF_GAIN_VALID;
    req.gain = target_state.gain_db();
  }
  auto status = SetGain(req);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Could not set gain state %d\n", status);
    return;
  }

  fbl::AutoLock channel_lock(&channel_lock_);
  for (auto& channel : stream_channels_) {
    if (channel.gain_completer_) {
      channel.gain_completer_->Reply(target_state);
      channel.gain_completer_.reset();
    }
  }
}

void SimpleAudioStream::SetActiveChannels(SetActiveChannelsRequestView request,
                                          SetActiveChannelsCompleter::Sync& completer) {
  ScopedToken t(domain_token());
  zx_time_t set_time;
  zx_status_t status = ChangeActiveChannels(request->active_channels_bitmask, &set_time);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Error while setting the active channels");
    completer.ReplyError(status);
    return;
  }
  channels_to_use_bitmask_.Set(request->active_channels_bitmask);
  completer.ReplySuccess(set_time);
}

void SimpleAudioStream::GetProperties(
    fidl::WireServer<audio_fidl::StreamConfig>::GetPropertiesCompleter::Sync& completer) {
  ScopedToken t(domain_token());
  fidl::Arena allocator;

  fidl::Array<uint8_t, audio_fidl::wire::kUniqueIdSize> uid_array;
  for (size_t i = 0; i < audio_fidl::wire::kUniqueIdSize; ++i) {
    uid_array[i] = unique_id_.data[i];
  }
  auto stream_properties =
      audio_fidl::wire::StreamProperties::Builder(allocator).unique_id(uid_array);

  auto product = fidl::StringView::FromExternal(prod_name_);
  auto manufacturer = fidl::StringView::FromExternal(mfr_name_);

  stream_properties.is_input(is_input())
      .can_mute(cur_gain_state_.can_mute)
      .can_agc(cur_gain_state_.can_agc)
      .min_gain_db(cur_gain_state_.min_gain)
      .max_gain_db(cur_gain_state_.max_gain)
      .gain_step_db(cur_gain_state_.gain_step)
      .product(product)
      .manufacturer(manufacturer)
      .clock_domain(clock_domain_);

  if (pd_flags_ & AUDIO_PDNF_CAN_NOTIFY) {
    stream_properties.plug_detect_capabilities(
        audio_fidl::wire::PlugDetectCapabilities::kCanAsyncNotify);
  } else if (pd_flags_ & AUDIO_PDNF_HARDWIRED) {
    stream_properties.plug_detect_capabilities(
        audio_fidl::wire::PlugDetectCapabilities::kHardwired);
  }

  completer.Reply(stream_properties.Build());
}

void SimpleAudioStream::GetSupportedFormats(
    fidl::WireServer<audio_fidl::StreamConfig>::GetSupportedFormatsCompleter::Sync& completer) {
  ScopedToken t(domain_token());

  bool ranges_with_one_number_of_channels = true;
  // Build formats compatible with FIDL from a vector of audio_stream_format_range_t.
  // Needs to be alive until the reply is sent.
  struct FidlCompatibleFormats {
    fbl::Vector<uint8_t> number_of_channels;
    fbl::Vector<audio_fidl::wire::SampleFormat> sample_formats;
    fbl::Vector<uint32_t> frame_rates;
    fbl::Vector<uint8_t> valid_bits_per_sample;
    fbl::Vector<uint8_t> bytes_per_sample;
    fbl::Vector<FrequencyRange> frequency_ranges;
  };
  fbl::Vector<FidlCompatibleFormats> fidl_compatible_formats;
  for (SupportedFormat& i : supported_formats_) {
    std::vector<utils::Format> formats = audio::utils::GetAllFormats(i.range.sample_formats);
    ZX_ASSERT(!formats.empty());
    for (utils::Format& j : formats) {
      fbl::Vector<uint32_t> rates;
      audio::utils::FrameRateEnumerator enumerator(i.range);
      for (uint32_t rate : enumerator) {
        rates.push_back(rate);
      }

      fbl::Vector<uint8_t> number_of_channels;
      for (uint8_t k = i.range.min_channels; k <= i.range.max_channels; ++k) {
        number_of_channels.push_back(k);
      }
      if (i.range.min_channels != i.range.max_channels) {
        ranges_with_one_number_of_channels = false;
      }

      const size_t n_frequencies = i.frequency_ranges.size();
      fbl::Vector<FrequencyRange> frequency_ranges;
      for (size_t k = 0; k < n_frequencies; ++k) {
        frequency_ranges.push_back({
            .min_frequency = i.frequency_ranges[k].min_frequency,
            .max_frequency = i.frequency_ranges[k].max_frequency,
        });
      }

      fidl_compatible_formats.push_back({
          .number_of_channels = std::move(number_of_channels),
          .sample_formats = {j.format},
          .frame_rates = std::move(rates),
          .valid_bits_per_sample = {j.valid_bits_per_sample},
          .bytes_per_sample = {j.bytes_per_sample},
          .frequency_ranges = std::move(frequency_ranges),
      });
    }
  }

  fidl::Arena allocator;
  fidl::VectorView<audio_fidl::wire::SupportedFormats> fidl_formats(allocator,
                                                                    fidl_compatible_formats.size());
  // Get FIDL PcmSupportedFormats from FIDL compatible vectors.
  // Needs to be alive until the reply is sent.
  for (size_t i = 0; i < fidl_compatible_formats.size(); ++i) {
    FidlCompatibleFormats& src = fidl_compatible_formats[i];
    auto formats = audio_fidl::wire::PcmSupportedFormats::Builder(allocator);
    fidl::VectorView<audio_fidl::wire::ChannelSet> channel_sets(allocator,
                                                                src.number_of_channels.size());

    for (size_t j = 0; j < src.number_of_channels.size(); ++j) {
      fidl::VectorView<audio_fidl::wire::ChannelAttributes> attributes(allocator,
                                                                       src.number_of_channels[j]);
      if (src.frequency_ranges.size()) {
        ZX_ASSERT_MSG(ranges_with_one_number_of_channels,
                      "must have only one number_of_channels for frequency ranges usage");
        for (uint8_t k = 0; k < src.number_of_channels[j]; ++k) {
          attributes[k] = audio_fidl::wire::ChannelAttributes::Builder(allocator)
                              .min_frequency(src.frequency_ranges[k].min_frequency)
                              .max_frequency(src.frequency_ranges[k].max_frequency)
                              .Build();
        }
      }
      channel_sets[j] =
          audio_fidl::wire::ChannelSet::Builder(allocator).attributes(attributes).Build();
    }
    formats.channel_sets(channel_sets);
    formats.sample_formats(::fidl::VectorView<audio_fidl::wire::SampleFormat>::FromExternal(
        src.sample_formats.data(), src.sample_formats.size()));
    formats.frame_rates(
        ::fidl::VectorView<uint32_t>::FromExternal(src.frame_rates.data(), src.frame_rates.size()));
    formats.bytes_per_sample(::fidl::VectorView<uint8_t>::FromExternal(
        src.bytes_per_sample.data(), src.bytes_per_sample.size()));
    formats.valid_bits_per_sample(::fidl::VectorView<uint8_t>::FromExternal(
        src.valid_bits_per_sample.data(), src.valid_bits_per_sample.size()));
    fidl_formats[i] = audio_fidl::wire::SupportedFormats::Builder(allocator)
                          .pcm_supported_formats(formats.Build())
                          .Build();
  }

  completer.Reply(fidl_formats);
}

// Ring Buffer GetProperties.
void SimpleAudioStream::GetProperties(GetPropertiesCompleter::Sync& completer) {
  ScopedToken t(domain_token());
  fidl::Arena allocator;
  auto ring_buffer_properties = audio_fidl::wire::RingBufferProperties::Builder(allocator)
                                    .driver_transfer_bytes(driver_transfer_bytes_)
                                    .needs_cache_flush_or_invalidate(true)
                                    .turn_on_delay(turn_on_delay_nsec_)
                                    .Build();

  completer.Reply(ring_buffer_properties);
}

void SimpleAudioStream::GetVmo(GetVmoRequestView request, GetVmoCompleter::Sync& completer) {
  ScopedToken t(domain_token());
  frames_requested_.Set(request->min_frames);

  if (rb_started_) {
    zxlogf(ERROR, "Cannot retrieve the buffer if already started");
    completer.ReplyError(audio_fidl::wire::GetVmoError::kInternalError);
    return;
  }
  expected_notifications_per_ring_.store(request->clock_recovery_notifications_per_ring);

  uint32_t num_ring_buffer_frames = 0;
  audio_proto::RingBufGetBufferReq req = {};
  // The ring buffer must be at least min_frames + fifo_frames.
  uint64_t req_frames = static_cast<uint64_t>(request->min_frames) +
                        ((driver_transfer_bytes_ + frame_size_ - 1) / frame_size_);
  if (req_frames > std::numeric_limits<uint32_t>::max() ||
      req_frames * frame_size_ > 4 * 1024 * 1024) {
    zxlogf(ERROR, "Requested ring buffer too large (min_frames %u, frame_size %u)",
           request->min_frames, frame_size_);
    completer.ReplyError(audio_fidl::wire::GetVmoError::kInvalidArgs);
    return;
  }
  req.min_ring_buffer_frames = static_cast<uint32_t>(req_frames);
  req.notifications_per_ring = request->clock_recovery_notifications_per_ring;
  zx::vmo buffer;
  rb_vmo_fetched_ = false;
  auto status = GetBuffer(req, &num_ring_buffer_frames, &buffer);
  if (status != ZX_OK) {
    expected_notifications_per_ring_.store(0);
    zxlogf(ERROR, "Failed to retrieve the buffer (err %u)", status);
    completer.ReplyError(audio_fidl::wire::GetVmoError::kInternalError);
    return;
  }

  uint64_t size = 0;
  status = buffer.get_size(&size);
  if (status != ZX_OK) {
    expected_notifications_per_ring_.store(0);
    zxlogf(ERROR, "Failed to get the size of the VMO (err %u)", status);
    completer.ReplyError(audio_fidl::wire::GetVmoError::kInternalError);
    return;
  }
  rb_vmo_fetched_ = true;
  ring_buffer_size_.Set(size);
  completer.ReplySuccess(num_ring_buffer_frames, std::move(buffer));
}

void SimpleAudioStream::Start(StartCompleter::Sync& completer) {
  ScopedToken t(domain_token());

  if (!rb_vmo_fetched_) {
    zxlogf(ERROR, "Cannot start the ring buffer before retrieving the VMO");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (rb_started_) {
    zxlogf(ERROR, "Cannot start the ring buffer if already started");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  uint64_t start_time = 0;
  auto result = Start(&start_time);
  if (result == ZX_OK) {
    rb_started_ = true;
    state_.Set("started");
    start_time_.Set(zx::clock::get_monotonic().get());
  }
  completer.Reply(static_cast<int64_t>(start_time));
}

void SimpleAudioStream::Stop(StopCompleter::Sync& completer) {
  ScopedToken t(domain_token());

  if (!rb_vmo_fetched_) {
    zxlogf(ERROR, "Cannot stop the ring buffer before retrieving the VMO");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (!rb_started_) {
    zxlogf(INFO, "Stop called while stopped; doing nothing");
    completer.Reply();
    return;
  }

  auto result = Stop();
  if (result == ZX_OK) {
    rb_started_ = false;
    state_.Set("stopped");
  }
  completer.Reply();
}

}  // namespace audio
