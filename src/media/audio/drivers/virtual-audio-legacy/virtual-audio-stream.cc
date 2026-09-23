// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-stream.h"

#include <lib/affine/transform.h>
#include <lib/ddk/debug.h>
#include <lib/zx/clock.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

namespace virtual_audio {

// static
fbl::RefPtr<VirtualAudioStream> VirtualAudioStream::Create(
    const fuchsia_virtualaudio::Configuration& cfg, std::weak_ptr<VirtualAudioDevice> owner,
    zx_device_t* dev_node, fit::closure on_shutdown) {
  return audio::SimpleAudioStream::Create<VirtualAudioStream>(cfg, std::move(owner), dev_node,
                                                              std::move(on_shutdown));
}

zx::time VirtualAudioStream::MonoTimeFromRefTime(const zx::clock& clock, zx::time ref_time) {
  zx_clock_details_v1_t clock_details;
  zx_status_t status = clock.get_details(&clock_details);
  ZX_ASSERT_MSG(status == ZX_OK, "Could not get_details on this clock");

  zx_time_t mono_time = affine::Transform::ApplyInverse(
      clock_details.reference_to_synthetic.reference_offset,
      clock_details.reference_to_synthetic.synthetic_offset,
      affine::Ratio(clock_details.reference_to_synthetic.rate.synthetic_ticks,
                    clock_details.reference_to_synthetic.rate.reference_ticks),
      ref_time.get());

  return zx::time{mono_time};
}

// static
fuchsia_virtualaudio::Configuration VirtualAudioStream::GetDefaultConfig(bool is_input) {
  fuchsia_virtualaudio::Configuration config = {};
  config.device_name(std::string("Virtual Audio Device") + (is_input ? " (input)" : " (output)"));
  config.manufacturer_name("Fuchsia Virtual Audio Group");
  config.product_name("Virgil v1, a Virtual Volume Vessel");
  config.unique_id(std::array<uint8_t, 16>({1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0}));

  // Driver type is StreamConfig.
  fuchsia_virtualaudio::StreamConfig stream_config = {};
  stream_config.is_input(is_input);

  // Ring Buffer.
  fuchsia_virtualaudio::RingBuffer ring_buffer = {};

  // By default, expose a single ring buffer format: 48kHz stereo 16bit.
  fuchsia_virtualaudio::FormatRange format = {};
  format.sample_format_flags(AUDIO_SAMPLE_FORMAT_16BIT);
  format.min_frame_rate(48'000);
  format.max_frame_rate(48'000);
  format.min_channels(2);
  format.max_channels(2);
  format.rate_family_flags(ASF_RANGE_FLAG_FPS_48000_FAMILY);
  ring_buffer.supported_formats(std::optional<std::vector<fuchsia_virtualaudio::FormatRange>>{
      std::in_place,
      {
          format,
      },
  });

  // Default FIFO is 250 usec (at 48k stereo 16). No internal delay; external delay unspecified.
  ring_buffer.driver_transfer_bytes(48);
  ring_buffer.internal_delay(0);

  // Default ring buffer size is at least 250msec (assuming default rate 48k).
  fuchsia_virtualaudio::RingBufferConstraints ring_buffer_constraints = {};
  ring_buffer_constraints.min_frames(12000);
  ring_buffer_constraints.max_frames(1 << 19);  // (10+ sec, at default 48k!)
  ring_buffer_constraints.modulo_frames(1);
  ring_buffer.ring_buffer_constraints(std::move(ring_buffer_constraints));

  stream_config.ring_buffer(std::move(ring_buffer));

  // Clock properties with no rate_adjustment_ppm specified (defaults to 0).
  fuchsia_virtualaudio::ClockProperties clock_properties = {};
  clock_properties.domain(0);
  stream_config.clock_properties(std::move(clock_properties));

  // By default, support a wide gain range with good precision.
  fuchsia_virtualaudio::GainProperties gain_properties = {};
  gain_properties.can_mute(true);
  gain_properties.can_agc(false);
  gain_properties.min_gain_db(-160.f);
  gain_properties.max_gain_db(24.f);
  gain_properties.gain_step_db(0.25f);
  fuchsia_hardware_audio::GainState gain_state;
  gain_state.gain_db(-0.75f);
  gain_state.muted(false);
  gain_state.agc_enabled(false);
  gain_properties.gain_state(std::move(gain_state));
  stream_config.gain_properties(std::move(gain_properties));

  // By default, device is hot-pluggable
  fuchsia_virtualaudio::PlugProperties plug_properties = {};
  fuchsia_hardware_audio::PlugState plug_state = {};
  plug_state.plugged(true).plug_state_time(zx::clock::get_monotonic().get());
  plug_properties.plug_state(std::move(plug_state));
  plug_properties.plug_detect_capabilities(
      fuchsia_hardware_audio::PlugDetectCapabilities::kCanAsyncNotify);
  stream_config.plug_properties(std::move(plug_properties));

  config.device_specific() =
      fuchsia_virtualaudio::DeviceSpecific::WithStreamConfig(std::move(stream_config));
  return config;
}

void VirtualAudioStream::PostToDispatcher(fit::closure task_to_post) {
  async::PostTask(dispatcher(), [task_to_post = std::move(task_to_post)]() { task_to_post(); });
}

zx_status_t VirtualAudioStream::Init() {
  if (config_.device_name().has_value()) {
    strncpy(device_name_, config_.device_name()->c_str(), sizeof(device_name_));
  }
  if (config_.manufacturer_name().has_value()) {
    strncpy(mfr_name_, config_.manufacturer_name()->c_str(), sizeof(mfr_name_));
  }
  if (config_.product_name().has_value()) {
    strncpy(prod_name_, config_.product_name()->c_str(), sizeof(prod_name_));
  }
  if (config_.unique_id().has_value()) {
    memcpy(unique_id_.data, config_.unique_id().value().data(), sizeof(unique_id_.data));
  }

  InitStreamConfigSpecific();

  zx_status_t status = EstablishReferenceClock();
  ZX_ASSERT_MSG(status == ZX_OK, "EstablishReferenceClock failed: %d", status);

  return ZX_OK;
}

void VirtualAudioStream::InitStreamConfigSpecific() {
  ZX_ASSERT(config_.device_specific()->Which() ==
            fuchsia_virtualaudio::DeviceSpecific::Tag::kStreamConfig);
  auto& stream_config = config_.device_specific()->stream_config();

  supported_formats_.reset();
  ZX_ASSERT(stream_config->ring_buffer().has_value());
  auto& ring_buffer = stream_config->ring_buffer();

  ZX_ASSERT(ring_buffer->supported_formats().has_value());
  auto& supported_formats = ring_buffer->supported_formats().value();
  for (auto& range : supported_formats) {
    SimpleAudioStream::SupportedFormat format = {};
    format.range.sample_formats = range.sample_format_flags();
    format.range.min_frames_per_second = range.min_frame_rate();
    format.range.max_frames_per_second = range.max_frame_rate();
    format.range.min_channels = range.min_channels();
    format.range.max_channels = range.max_channels();
    format.range.flags = range.rate_family_flags();
    supported_formats_.push_back(std::move(format));
  }
  if (ring_buffer->driver_transfer_bytes().has_value()) {
    driver_transfer_bytes_ = ring_buffer->driver_transfer_bytes().value();
  }
  if (ring_buffer->external_delay().has_value()) {
    external_delay_nsec_ = ring_buffer->external_delay().value();
  }

  if (ring_buffer->ring_buffer_constraints().has_value()) {
    ZX_ASSERT_MSG(ring_buffer->ring_buffer_constraints()->min_frames(),
                  "ring buffer min_frames cannot be zero");
    ZX_ASSERT_MSG(ring_buffer->ring_buffer_constraints()->min_frames() <=
                      ring_buffer->ring_buffer_constraints()->max_frames(),
                  "ring buffer min_frames cannot exceed max_frames");
    max_buffer_frames_ = ring_buffer->ring_buffer_constraints()->max_frames();
    min_buffer_frames_ = ring_buffer->ring_buffer_constraints()->min_frames();
    modulo_buffer_frames_ = ring_buffer->ring_buffer_constraints()->modulo_frames();
  }

  if (ring_buffer->notifications_per_ring().has_value()) {
    va_client_notifications_per_ring_ = ring_buffer->notifications_per_ring();
  }

  ZX_ASSERT(stream_config->clock_properties().has_value());
  if (stream_config->clock_properties()->domain().has_value()) {
    clock_domain_ = stream_config->clock_properties()->domain().value();
  }
  if (stream_config->clock_properties()->rate_adjustment_ppm().has_value()) {
    clock_rate_adjustment_ = stream_config->clock_properties()->rate_adjustment_ppm().value();
  }

  ZX_ASSERT(stream_config->gain_properties().has_value());
  auto& gain_properties = stream_config->gain_properties();
  ZX_ASSERT(gain_properties->gain_state().has_value());
  auto& gain_state = gain_properties->gain_state().value();
  if (gain_state.muted().has_value()) {
    cur_gain_state_.cur_mute = gain_state.muted().value();
  }
  if (gain_state.agc_enabled().has_value()) {
    cur_gain_state_.cur_agc = gain_state.agc_enabled().value();
  }
  if (gain_state.gain_db().has_value()) {
    cur_gain_state_.cur_gain = gain_state.gain_db().value();
  }
  if (gain_properties->can_mute().has_value()) {
    cur_gain_state_.can_mute = gain_properties->can_mute().value();
  }
  if (gain_properties->can_agc().has_value()) {
    cur_gain_state_.can_agc = gain_properties->can_agc().value();
  }
  if (gain_properties->min_gain_db().has_value()) {
    cur_gain_state_.min_gain = gain_properties->min_gain_db().value();
  }
  if (gain_properties->max_gain_db().has_value()) {
    cur_gain_state_.max_gain = gain_properties->max_gain_db().value();
  }
  if (gain_properties->gain_step_db().has_value()) {
    cur_gain_state_.gain_step = gain_properties->gain_step_db().value();
  }

  ZX_ASSERT(stream_config->plug_properties().has_value());
  audio_pd_notify_flags_t plug_flags = {};
  auto& plug_properties = stream_config->plug_properties();
  if (plug_properties->plug_detect_capabilities().has_value()) {
    switch (plug_properties->plug_detect_capabilities().value()) {
      case fuchsia_hardware_audio::PlugDetectCapabilities::kHardwired:
        plug_flags |= AUDIO_PDNF_HARDWIRED;
        break;
      case fuchsia_hardware_audio::PlugDetectCapabilities::kCanAsyncNotify:
        plug_flags |= AUDIO_PDNF_CAN_NOTIFY;
        break;
    }
  }
  ZX_ASSERT(plug_properties->plug_state().has_value());
  if (plug_properties->plug_state().value().plugged().has_value() &&
      plug_properties->plug_state().value().plugged().value()) {
    plug_flags |= AUDIO_PDNF_PLUGGED;
  }
  SetInitialPlugState(plug_flags);
}

// We use this clock to emulate a real hardware time source. It is not exposed outside the driver.
zx_status_t VirtualAudioStream::EstablishReferenceClock() {
  zx_status_t status =
      zx::clock::create(ZX_CLOCK_OPT_MONOTONIC | ZX_CLOCK_OPT_CONTINUOUS | ZX_CLOCK_OPT_AUTO_START,
                        nullptr, &reference_clock_);
  ZX_ASSERT_MSG(status == ZX_OK, "Could not create driver clock");

  if (clock_rate_adjustment_ != 0) {
    status = AdjustClockRate();
  }
  return ZX_OK;
}

// Update the internal clock object that manages our variance from the local system timebase.
zx_status_t VirtualAudioStream::AdjustClockRate() {
  zx::clock::update_args args;
  args.reset().set_rate_adjust(clock_rate_adjustment_);

  zx_status_t status = reference_clock_.update(args);
  ZX_ASSERT_MSG(status == ZX_OK, "Could not rate-adjust driver clock");

  return ZX_OK;
}

fit::result<VirtualAudioStream::ErrorT, CurrentFormat> VirtualAudioStream::GetFormatForVA() {
  if (!frame_rate_) {
    zxlogf(WARNING, "%s: %p ring buffer not initialized yet", __func__, this);
    return fit::error(ErrorT::kNoRingBuffer);
  }

  return fit::ok(CurrentFormat{
      .frames_per_second = frame_rate_,
      .sample_format = sample_format_,
      .num_channels = num_channels_,
      .external_delay = zx::nsec(static_cast<int64_t>(external_delay_nsec_)),
  });
}

fit::result<VirtualAudioStream::ErrorT, CurrentGain> VirtualAudioStream::GetGainForVA() {
  return fit::ok(CurrentGain{
      .mute = cur_gain_state_.cur_mute,
      .agc = cur_gain_state_.cur_agc,
      .gain_db = cur_gain_state_.cur_gain,
  });
}

fit::result<VirtualAudioStream::ErrorT, CurrentBuffer> VirtualAudioStream::GetBufferForVA() {
  if (!ring_buffer_vmo_.is_valid()) {
    zxlogf(WARNING, "%s: %p ring buffer not initialized yet", __func__, this);
    return fit::error(ErrorT::kNoRingBuffer);
  }

  zx::vmo dup_vmo;
  zx_status_t status = ring_buffer_vmo_.duplicate(
      ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP, &dup_vmo);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: %p ring buffer creation failed with status %d", __func__, this, status);
    return fit::error(ErrorT::kInternal);
  }

  return fit::ok(CurrentBuffer{
      .vmo = std::move(dup_vmo),
      .num_frames = num_ring_buffer_frames_,
      .notifications_per_ring = notifications_per_ring_,
  });
}

fit::result<VirtualAudioStream::ErrorT, CurrentPosition> VirtualAudioStream::GetPositionForVA() {
  if (!ref_start_time_.get()) {
    zxlogf(WARNING, "%s: %p stream is not started yet", __func__, this);
    return fit::error(ErrorT::kNotStarted);
  }

  zx::time ref_now;
  zx_status_t status = reference_clock_.read(ref_now.get_address());
  ZX_ASSERT_MSG(status == ZX_OK, "reference_clock::read returned error (%d)", status);

  auto mono_now = MonoTimeFromRefTime(reference_clock_, ref_now);
  auto frames = ref_time_to_running_frame_.Apply(ref_now.get());
  ZX_ASSERT_MSG(frames >= 0, "running frames should never be negative");

  // This cast is safe unless the ring-buffer exceeds 4GB, which even at max bit-rate (8-channel,
  // float32 format, 192kHz) would be a 700-second ring buffer (!).
  auto ring_position = static_cast<uint32_t>((frames % num_ring_buffer_frames_) * frame_size_);

  return fit::ok(CurrentPosition{
      .monotonic_time = mono_now,
      .ring_position = ring_position,
  });
}

fit::result<VirtualAudioStream::ErrorT> VirtualAudioStream::SetNotificationFrequencyFromVA(
    uint32_t notifications_per_ring) {
  va_client_notifications_per_ring_ = notifications_per_ring;

  // If our client requested the same notification cadence that AudioCore did, then just use the
  // "official" AudioCore notification timer and frequency instead of this alternate mechanism.
  if (va_client_notifications_per_ring_.value() == notifications_per_ring_) {
    va_client_notifications_per_ring_ = std::nullopt;
  }
  SetVaClientNotificationPeriods();

  if (va_client_notifications_per_ring_.has_value() &&
      (va_client_notifications_per_ring_.value() > 0)) {
    zx_status_t status =
        reference_clock_.read(target_va_client_ref_notification_time_.get_address());
    ZX_ASSERT_MSG(status == ZX_OK, "reference_clock::read returned error (%d)", status);

    PostForVaClientNotifyAt(target_va_client_ref_notification_time_);
  } else {
    target_va_client_mono_notification_time_ = zx::time(0);
    va_client_notify_timer_.Cancel();
  }
  return fit::ok();
}

fit::result<VirtualAudioStream::ErrorT> VirtualAudioStream::ChangePlugStateFromVA(bool plugged) {
  SetPlugState(plugged);
  return fit::ok();
}

fit::result<VirtualAudioStream::ErrorT> VirtualAudioStream::AdjustClockRateFromVA(
    int32_t ppm_from_monotonic) {
  clock_rate_adjustment_ = ppm_from_monotonic;
  ZX_ASSERT_MSG(AdjustClockRate() == ZX_OK, "AdjustClockRate failed in %s", __func__);
  return fit::ok();
}

void VirtualAudioStream::PostForNotify() {
  ZX_ASSERT(notifications_per_ring_);
  ZX_ASSERT(target_mono_notification_time_.get() > 0);

  notify_timer_.PostForTime(dispatcher(), target_mono_notification_time_);
}

void VirtualAudioStream::PostForNotifyAt(zx::time ref_notification_time) {
  target_ref_notification_time_ = ref_notification_time;
  target_mono_notification_time_ =
      MonoTimeFromRefTime(reference_clock_, target_ref_notification_time_);
  PostForNotify();
}

void VirtualAudioStream::PostForVaClientNotify() {
  ZX_ASSERT(va_client_notifications_per_ring_.value() > 0);
  ZX_ASSERT(target_va_client_mono_notification_time_.get() > 0);

  va_client_notify_timer_.PostForTime(dispatcher(), target_va_client_mono_notification_time_);
}

void VirtualAudioStream::PostForVaClientNotifyAt(zx::time va_client_ref_notification_time) {
  target_va_client_ref_notification_time_ = va_client_ref_notification_time;
  target_va_client_mono_notification_time_ =
      MonoTimeFromRefTime(reference_clock_, target_va_client_ref_notification_time_);
  PostForVaClientNotify();
}

// On success, driver should return a valid VMO with appropriate permissions (READ | MAP | TRANSFER
// for input, plus WRITE for output) and report the total number of usable frames in the ring.
//
// Format must already be set: a ring buffer channel (over which this command arrived) is provided
// as the return value from a successful SetFormat call.
zx_status_t VirtualAudioStream::GetBuffer(const audio::audio_proto::RingBufGetBufferReq& req,
                                          uint32_t* out_num_rb_frames, zx::vmo* out_buffer) {
  ZX_ASSERT_MSG(req.notifications_per_ring <= req.min_ring_buffer_frames,
                "req.notifications_per_ring too big");
  ZX_ASSERT_MSG(req.min_ring_buffer_frames <= max_buffer_frames_,
                "req.min_ring_buffer_frames too big");

  num_ring_buffer_frames_ = std::max(
      min_buffer_frames_,
      fbl::round_up<uint32_t, uint32_t>(req.min_ring_buffer_frames, modulo_buffer_frames_));
  // Using uint32_t (not size_t) is safe unless the ring-buffer exceeds 4GB, which even at max
  // bit-rate (8-channel, float32 format, 192kHz) would be a 700-second ring buffer (!).
  uint32_t ring_buffer_size = fbl::round_up<uint32_t, uint32_t>(
      num_ring_buffer_frames_ * frame_size_, zx_system_get_page_size());

  if (ring_buffer_mapper_.start() != nullptr) {
    ring_buffer_mapper_.Unmap();
  }

  zx_status_t status = ring_buffer_mapper_.CreateAndMap(
      ring_buffer_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &ring_buffer_vmo_,
      ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER);

  ZX_ASSERT_MSG(status == ZX_OK, "%s failed to create ring buffer vmo - %d", __func__, status);

  zx_rights_t required_rights = ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_MAP;
  if (config_.device_specific().has_value() &&
      config_.device_specific()->stream_config().has_value() &&
      config_.device_specific()->stream_config()->is_input().has_value() &&
      !*config_.device_specific()->stream_config()->is_input()) {
    required_rights |= ZX_RIGHT_WRITE;
  }
  status = ring_buffer_vmo_.duplicate(required_rights, out_buffer);
  ZX_ASSERT_MSG(status == ZX_OK, "%s failed to duplicate VMO handle for out param - %d", __func__,
                status);

  notifications_per_ring_ = req.notifications_per_ring;
  SetNotificationPeriods();

  *out_num_rb_frames = num_ring_buffer_frames_;

  zx::vmo duplicate_vmo;
  status = ring_buffer_vmo_.duplicate(
      ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP, &duplicate_vmo);
  ZX_ASSERT_MSG(status == ZX_OK, "%s failed to duplicate VMO handle for VA client - %d", __func__,
                status);

  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  parent->NotifyBufferCreated(std::move(duplicate_vmo), num_ring_buffer_frames_,
                              notifications_per_ring_);

  return ZX_OK;
}

void VirtualAudioStream::SetNotificationPeriods() {
  if (notifications_per_ring_ > 0) {
    ref_notification_period_ = zx::duration((zx::sec(1) * num_ring_buffer_frames_) / frame_rate_ /
                                            notifications_per_ring_);
  } else {
    ref_notification_period_ = zx::duration(0);
  }

  SetVaClientNotificationPeriods();
}

void VirtualAudioStream::SetVaClientNotificationPeriods() {
  if (va_client_notifications_per_ring_.has_value() &&
      va_client_notifications_per_ring_.value() > 0) {
    va_client_ref_notification_period_ =
        zx::duration((zx::sec(1) * num_ring_buffer_frames_) / frame_rate_ /
                     va_client_notifications_per_ring_.value());
  } else {
    va_client_ref_notification_period_ = zx::duration(0);
  }
}

zx_status_t VirtualAudioStream::ChangeFormat(const audio::audio_proto::StreamSetFmtReq& req) {
  // frame_size_ is already set, automatically
  ZX_ASSERT_MSG(frame_size_, "frame_size canot be zero");

  frame_rate_ = req.frames_per_second;
  ZX_ASSERT_MSG(frame_rate_, "frame_rate cannot be zero");

  sample_format_ = req.sample_format;

  num_channels_ = req.channels;
  ring_buffer_active_channel_mask_ = (1 << num_channels_) - 1;
  active_channels_set_time_ = zx::clock::get_monotonic();
  bytes_per_sec_ = frame_rate_ * frame_size_;

  // (Re)set external_delay_nsec_ and driver_transfer_bytes_ before leaving, if needed.

  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  parent->NotifySetFormat(frame_rate_, sample_format_, num_channels_,
                          static_cast<int64_t>(external_delay_nsec_));

  return ZX_OK;
}

zx_status_t VirtualAudioStream::SetGain(const audio::audio_proto::SetGainReq& req) {
  if (req.flags & AUDIO_SGF_GAIN_VALID) {
    // We avoid 'trunc'|'floor', to minimize C-runtime dependencies on new HW targets such as RISCV.
    int64_t gain_step_count = static_cast<int64_t>(req.gain / cur_gain_state_.gain_step);
    cur_gain_state_.cur_gain = static_cast<float>(gain_step_count) * cur_gain_state_.gain_step;
  }

  if (req.flags & AUDIO_SGF_MUTE_VALID) {
    cur_gain_state_.cur_mute = req.flags & AUDIO_SGF_MUTE;
  }

  if (req.flags & AUDIO_SGF_AGC_VALID) {
    cur_gain_state_.cur_agc = req.flags & AUDIO_SGF_AGC;
  }

  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  parent->NotifySetGain(cur_gain_state_.cur_mute, cur_gain_state_.cur_agc,
                        cur_gain_state_.cur_gain);

  return ZX_OK;
}

// Drivers *must* return the start_time (on CLOCK_MONOTONIC timeline) at which the ring buffer
// start to be written to or read from. Internal or external delays are unrelated to
// `start_time`, since they occur beyond the ring buffer.
zx_status_t VirtualAudioStream::Start(uint64_t* out_start_time) {
  zx_status_t status = reference_clock_.read(ref_start_time_.get_address());
  ZX_ASSERT_MSG(status == ZX_OK, "reference_clock::read returned error (%d)", status);

  ref_time_to_running_frame_ =
      affine::Transform(ref_start_time_.get(), 0, affine::Ratio(frame_rate_, ZX_SEC(1)));

  auto mono_start_time = MonoTimeFromRefTime(reference_clock_, ref_start_time_);

  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  parent->NotifyStart(mono_start_time.get());

  // Set the timer here (if notifications are enabled).
  if (ref_notification_period_.get() > 0) {
    PostForNotifyAt(ref_start_time_);
  }

  if (va_client_ref_notification_period_.get() > 0) {
    PostForVaClientNotifyAt(ref_start_time_);
  }

  *out_start_time = mono_start_time.get();

  return ZX_OK;
}

// Timer handler for sending position notifications: to AudioCore, to VAD clients that don't set the
// notification frequency, and to VAD clients that set it to the same value that AudioCore selects.
// This method handles tasks posted to notify_timer_
void VirtualAudioStream::ProcessRingNotification() {
  audio::ScopedToken t(domain_token());
  ZX_ASSERT(ref_notification_period_.get() > 0);
  ZX_ASSERT(notifications_per_ring_ > 0);
  ZX_ASSERT(target_mono_notification_time_.get() > 0);

  zx::time ref_now;
  zx_status_t status = reference_clock_.read(ref_now.get_address());
  ZX_ASSERT_MSG(status == ZX_OK, "reference_clock::read returned error (%d)", status);

  // We should wake up close to target_ref_notification_time_
  if (target_ref_notification_time_ > ref_now) {
    PostForNotify();  // Too soon, re-post this for later
    return;
  }

  auto running_frame_position =
      ref_time_to_running_frame_.Apply(target_ref_notification_time_.get());
  ZX_ASSERT_MSG(running_frame_position >= 0, "running_frame_position should never be negative");

  auto ring_buffer_position =
      static_cast<uint32_t>((running_frame_position % num_ring_buffer_frames_) * frame_size_);
  audio::audio_proto::RingBufPositionNotify resp = {};
  resp.hdr.cmd = AUDIO_RB_POSITION_NOTIFY;
  resp.monotonic_time = target_mono_notification_time_.get();
  resp.ring_buffer_pos = ring_buffer_position;

  status = NotifyPosition(resp);
  ZX_ASSERT_MSG(status == ZX_OK, "NotifyPosition returned error (%d)", status);

  // If virtual_audio client uses this notification cadence, notify them too
  if (!va_client_notifications_per_ring_.has_value()) {
    auto parent = parent_.lock();
    ZX_ASSERT(parent);
    parent->NotifyPosition(target_mono_notification_time_.get(), ring_buffer_position);
  }

  // Post the next position notification
  PostForNotifyAt(target_ref_notification_time_ + ref_notification_period_);
}

// Handler for sending alternate position notifications: those going to VAD clients that specified
// a different notification frequency. These are not sent to AudioCore.
// This method receives tasks that have been posted to va_client_notify_timer_
void VirtualAudioStream::ProcessVaClientRingNotification() {
  audio::ScopedToken t(domain_token());
  ZX_ASSERT(va_client_ref_notification_period_.get() > 0);
  ZX_ASSERT(va_client_notifications_per_ring_.has_value());
  ZX_ASSERT(va_client_notifications_per_ring_.value() > 0);
  ZX_ASSERT(target_va_client_mono_notification_time_.get() > 0);

  zx::time ref_now;
  zx_status_t status = reference_clock_.read(ref_now.get_address());
  ZX_ASSERT_MSG(status == ZX_OK, "reference_clock::read returned error (%d)", status);

  // We should wake up close to target_ref_notification_time_
  if (target_va_client_ref_notification_time_ > ref_now) {
    PostForVaClientNotify();  // Too soon, re-post this for later
    return;
  }

  auto running_frame_position =
      ref_time_to_running_frame_.Apply(target_va_client_ref_notification_time_.get());
  ZX_ASSERT_MSG(running_frame_position >= 0, "running_frame_position should never be negative");

  auto ring_buffer_position =
      static_cast<uint32_t>((running_frame_position % num_ring_buffer_frames_) * frame_size_);

  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  parent->NotifyPosition(target_va_client_mono_notification_time_.get(), ring_buffer_position);

  // Post the next alt position notification
  PostForVaClientNotifyAt(target_va_client_ref_notification_time_ +
                          va_client_ref_notification_period_);
}

zx_status_t VirtualAudioStream::Stop() {
  zx::time ref_stop_time;
  zx_status_t status = reference_clock_.read(ref_stop_time.get_address());

  notify_timer_.Cancel();
  va_client_notify_timer_.Cancel();

  ZX_ASSERT_MSG(status == ZX_OK, "reference_clock::read returned error (%d)", status);

  auto mono_stop_time = MonoTimeFromRefTime(reference_clock_, ref_stop_time);
  auto stop_frame = ref_time_to_running_frame_.Apply(ref_stop_time.get());
  ZX_ASSERT_MSG(stop_frame >= 0, "stop_frame should never be negative");

  auto ring_buf_position =
      static_cast<uint32_t>((stop_frame % num_ring_buffer_frames_) * frame_size_);

  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  parent->NotifyStop(mono_stop_time.get(), ring_buf_position);

  ref_start_time_ = zx::time(0);
  target_mono_notification_time_ = zx::time(0);
  target_va_client_mono_notification_time_ = zx::time(0);
  ref_notification_period_ = zx::duration(0);
  va_client_ref_notification_period_ = zx::duration(0);

  ref_time_to_running_frame_ = affine::Transform(affine::Ratio(0, 1));

  return ZX_OK;
}

zx_status_t VirtualAudioStream::ChangeActiveChannels(uint64_t active_channels_bitmask,
                                                     zx_time_t* set_time_out) {
  const uint64_t max_channel_mask = (1 << num_channels_) - 1;
  if (active_channels_bitmask > max_channel_mask) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!set_time_out) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (ring_buffer_active_channel_mask_ != active_channels_bitmask) {
    ring_buffer_active_channel_mask_ = active_channels_bitmask;
    active_channels_set_time_ = zx::clock::get_monotonic();
  }
  *set_time_out = active_channels_set_time_.get();
  return ZX_OK;
}

// Called by parent SimpleAudioStream::Shutdown, during DdkUnbind.
void VirtualAudioStream::ShutdownHook() { on_shutdown_(); }

}  // namespace virtual_audio
