// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-dai.h"

#include <lib/ddk/debug.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <audio-proto-utils/format-utils.h>
#include <fbl/algorithm.h>

namespace virtual_audio {

// static
int VirtualAudioDai::instance_count_ = 0;

// static
fuchsia_virtualaudio::Configuration VirtualAudioDai::GetDefaultConfig(bool is_input) {
  fuchsia_virtualaudio::Configuration config = {};
  config.device_name(std::string("Virtual Audio DAI Device") +
                     (is_input ? " (input)" : " (output)"));
  config.manufacturer_name("Fuchsia Virtual Audio Group");
  config.product_name("Virgil v2, a Virtual Volume Vessel");
  config.unique_id(std::array<uint8_t, 16>({1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0}));

  // Driver type is DAI.
  fuchsia_virtualaudio::Dai dai = {};
  dai.is_input(is_input);

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

  // No ring_buffer_constraints specified (so we use the client's notifications_per_ring).
  dai.ring_buffer(std::move(ring_buffer));

  // DAI interconnect.
  fuchsia_virtualaudio::DaiInterconnect dai_interconnect = {};

  // By default, expose a single DAI format: 48kHz I2S (2 channels, 16-in-32, 8-byte frames).
  fuchsia_hardware_audio::DaiSupportedFormats item = {};
  item.number_of_channels(std::vector<uint32_t>{2});
  item.sample_formats(std::vector{fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned});
  item.frame_formats(std::vector{fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
      fuchsia_hardware_audio::DaiFrameFormatStandard::kI2S)});
  item.frame_rates(std::vector<uint32_t>{48'000});
  item.bits_per_slot(std::vector<uint8_t>{32});
  item.bits_per_sample(std::vector<uint8_t>{16});

  dai_interconnect.dai_supported_formats(
      std::optional<std::vector<fuchsia_hardware_audio::DaiSupportedFormats>>{
          std::in_place,
          {
              item,
          },
      });

  dai.dai_interconnect(std::move(dai_interconnect));

  // Clock properties with no rate_adjustment_ppm specified (defaults to 0).
  fuchsia_virtualaudio::ClockProperties clock_properties = {};
  clock_properties.domain(0);
  dai.clock_properties(std::move(clock_properties));

  config.device_specific() = fuchsia_virtualaudio::DeviceSpecific::WithDai(std::move(dai));

  return config;
}

VirtualAudioDai::VirtualAudioDai(fuchsia_virtualaudio::Configuration config,
                                 std::weak_ptr<VirtualAudioDevice> owner, zx_device_t* parent,
                                 fit::closure on_shutdown)
    : VirtualAudioDaiDeviceType(parent),
      VirtualAudioDriver(std::move(on_shutdown)),
      parent_(std::move(owner)),
      config_(std::move(config)) {
  ddk_proto_id_ = ZX_PROTOCOL_DAI;
  sprintf(instance_name_, "virtual-audio-dai-%d", instance_count_++);
  zx_status_t status = DdkAdd(ddk::DeviceAddArgs(instance_name_));
  ZX_ASSERT_MSG(status == ZX_OK, "DdkAdd failed");
}

void VirtualAudioDai::GetFormatForVA(
    fit::callback<void(fit::result<ErrorT, CurrentFormat>)> callback) {
  if (!ring_buffer_format_.has_value() || !ring_buffer_format_->pcm_format().has_value()) {
    zxlogf(WARNING, "%p ring buffer not initialized yet", this);
    callback(fit::error(ErrorT::kNoRingBuffer));
    return;
  }
  auto& pcm_format = ring_buffer_format_->pcm_format();
  if (!config_.device_specific() || !config_.device_specific()->dai() ||
      !config_.device_specific()->dai()->ring_buffer()) {
    callback(fit::error(ErrorT::kNoRingBuffer));
    return;
  }
  auto& ring_buffer = *config_.device_specific()->dai()->ring_buffer();
  int64_t external_delay = ring_buffer.external_delay().value_or(0);
  callback(fit::ok(CurrentFormat{
      .frames_per_second = pcm_format->frame_rate(),
      .sample_format = audio::utils::GetSampleFormat(pcm_format->valid_bits_per_sample(),
                                                     pcm_format->bytes_per_sample() * 8),
      .num_channels = pcm_format->number_of_channels(),
      .external_delay = zx::nsec(external_delay),
  }));
}

void VirtualAudioDai::GetBufferForVA(
    fit::callback<void(fit::result<ErrorT, CurrentBuffer>)> callback) {
  if (!ring_buffer_vmo_.is_valid()) {
    zxlogf(WARNING, "%p ring buffer not initialized yet", this);
    callback(fit::error(ErrorT::kNoRingBuffer));
    return;
  }

  zx::vmo dup_vmo;
  zx_status_t status = ring_buffer_vmo_.duplicate(
      ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP, &dup_vmo);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%p ring buffer creation failed: %s", this, zx_status_get_string(status));
    callback(fit::error(ErrorT::kInternal));
    return;
  }

  callback(fit::ok(CurrentBuffer{
      .vmo = std::move(dup_vmo),
      .num_frames = num_ring_buffer_frames_,
      .notifications_per_ring = notifications_per_ring_,
  }));
}

VirtualAudioDai::~VirtualAudioDai() {
  if (ring_buffer_binding_.has_value()) {
    ring_buffer_binding_->Unbind();
  }
  dai_bindings_.RemoveAll();
}

void VirtualAudioDai::GetProperties(
    fidl::Server<fuchsia_hardware_audio::Dai>::GetPropertiesCompleter::Sync& completer) {
  auto parent = parent_.lock();
  ZX_ASSERT(parent);

  fuchsia_hardware_audio::DaiProperties properties;
  if (config_.device_specific() && config_.device_specific()->dai() &&
      config_.device_specific()->dai()->is_input()) {
    properties.is_input(*config_.device_specific()->dai()->is_input());
  }
  properties.unique_id(config_.unique_id());
  properties.product_name(config_.product_name());
  properties.manufacturer(config_.manufacturer_name());
  if (config_.device_specific() && config_.device_specific()->dai() &&
      config_.device_specific()->dai()->clock_properties() &&
      config_.device_specific()->dai()->clock_properties()->domain()) {
    properties.clock_domain(*config_.device_specific()->dai()->clock_properties()->domain());
  }
  completer.Reply(std::move(properties));
}

void VirtualAudioDai::GetDaiFormats(GetDaiFormatsCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_audio::DaiSupportedFormats> formats;
  if (config_.device_specific() && config_.device_specific()->dai() &&
      config_.device_specific()->dai()->dai_interconnect() &&
      config_.device_specific()->dai()->dai_interconnect()->dai_supported_formats()) {
    formats = *config_.device_specific()->dai()->dai_interconnect()->dai_supported_formats();
  }
  completer.Reply(zx::ok(std::move(formats)));
}

void VirtualAudioDai::GetRingBufferFormats(GetRingBufferFormatsCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_audio::SupportedFormats> all_formats;
  if (config_.device_specific() && config_.device_specific()->dai() &&
      config_.device_specific()->dai()->ring_buffer() &&
      config_.device_specific()->dai()->ring_buffer()->supported_formats()) {
    for (auto& formats : *config_.device_specific()->dai()->ring_buffer()->supported_formats()) {
      fuchsia_hardware_audio::PcmSupportedFormats pcm_formats;
      std::vector<fuchsia_hardware_audio::ChannelSet> channel_sets;
      for (uint8_t number_of_channels = formats.min_channels();
           number_of_channels <= formats.max_channels(); ++number_of_channels) {
        // Vector with number_of_channels empty attributes.
        std::vector<fuchsia_hardware_audio::ChannelAttributes> attributes(number_of_channels);
        fuchsia_hardware_audio::ChannelSet channel_set;
        channel_set.attributes(std::move(attributes));
        channel_sets.push_back(std::move(channel_set));
      }
      pcm_formats.channel_sets(std::move(channel_sets));

      std::vector<uint32_t> frame_rates;
      audio_stream_format_range_t range;
      range.sample_formats = formats.sample_format_flags();
      range.min_frames_per_second = formats.min_frame_rate();
      range.max_frames_per_second = formats.max_frame_rate();
      range.min_channels = formats.min_channels();
      range.max_channels = formats.max_channels();
      range.flags = formats.rate_family_flags();
      audio::utils::FrameRateEnumerator enumerator(range);
      for (uint32_t frame_rate : enumerator) {
        frame_rates.push_back(frame_rate);
      }
      pcm_formats.frame_rates(std::move(frame_rates));

      std::vector<audio::utils::Format> formats2 =
          audio::utils::GetAllFormats(formats.sample_format_flags());
      for (audio::utils::Format& format : formats2) {
        std::vector<fuchsia_hardware_audio::SampleFormat> sample_formats{format.format};
        std::vector<uint8_t> bytes_per_sample{format.bytes_per_sample};
        std::vector<uint8_t> valid_bits_per_sample{format.valid_bits_per_sample};
        auto pcm_formats2 = pcm_formats;
        pcm_formats2.sample_formats(std::move(sample_formats));
        pcm_formats2.bytes_per_sample(std::move(bytes_per_sample));
        pcm_formats2.valid_bits_per_sample(std::move(valid_bits_per_sample));
        fuchsia_hardware_audio::SupportedFormats formats_entry;
        formats_entry.pcm_supported_formats(std::move(pcm_formats2));
        all_formats.push_back(std::move(formats_entry));
      }
    }
  }
  completer.Reply(zx::ok(std::move(all_formats)));
}

void VirtualAudioDai::CreateRingBuffer(CreateRingBufferRequest& request,
                                       CreateRingBufferCompleter::Sync& completer) {
  if (!request.ring_buffer_format().pcm_format() ||
      !request.ring_buffer_format().pcm_format()->number_of_channels()) {
    request.ring_buffer().Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  ring_buffer_format_.emplace(request.ring_buffer_format());
  ring_buffer_active_channel_mask_ =
      (1 << ring_buffer_format_->pcm_format()->number_of_channels()) - 1;
  active_channel_set_time_ = zx::clock::get_monotonic();
  dai_format_.emplace(request.dai_format());
  fidl::OnUnboundFn<fidl::Server<fuchsia_hardware_audio::RingBuffer>> on_unbound =
      [weak = weak_from_this()](fidl::Server<fuchsia_hardware_audio::RingBuffer>*,
                                fidl::UnbindInfo info,
                                fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer>) {
        if (auto self = weak.lock()) {
          auto* dai = static_cast<VirtualAudioDai*>(self.get());
          // Do not log canceled cases: happens too often in specific test cases.
          if (info.status() != ZX_ERR_CANCELED && !info.is_peer_closed() &&
              !info.is_user_initiated()) {
            zxlogf(INFO, "Ring buffer channel closing: %s", info.FormatDescription().c_str());
          }
          dai->ResetRingBuffer();
        }
      };
  ring_buffer_binding_ =
      fidl::BindServer(dispatcher_, std::move(request.ring_buffer()), this, std::move(on_unbound));
}

void VirtualAudioDai::ResetRingBuffer() {
  should_reply_to_delay_request_ = true;
  should_reply_to_position_request_ = true;
  ring_buffer_vmo_fetched_ = false;
  ring_buffer_started_ = false;
  notifications_per_ring_ = 0;
  position_info_completer_.reset();
  delay_info_completer_.reset();
  ring_buffer_binding_.reset();
  // We don't reset ring_buffer_format_ and dai_format_ to allow for retrieval for observability.
}

void VirtualAudioDai::GetProperties(
    fidl::Server<fuchsia_hardware_audio::RingBuffer>::GetPropertiesCompleter::Sync& completer) {
  fidl::Arena arena;
  fuchsia_hardware_audio::RingBufferProperties properties;
  uint32_t transfer_bytes = 48;
  if (config_.device_specific() && config_.device_specific()->dai() &&
      config_.device_specific()->dai()->ring_buffer() &&
      config_.device_specific()->dai()->ring_buffer()->driver_transfer_bytes()) {
    transfer_bytes = *config_.device_specific()->dai()->ring_buffer()->driver_transfer_bytes();
  }
  properties.needs_cache_flush_or_invalidate(false).driver_transfer_bytes(transfer_bytes);
  completer.Reply(std::move(properties));
}

void VirtualAudioDai::GetVmo(GetVmoRequest& request, GetVmoCompleter::Sync& completer) {
  if (!config_.device_specific() || !config_.device_specific()->dai() ||
      !config_.device_specific()->dai()->ring_buffer()) {
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (ring_buffer_mapper_.start() != nullptr) {
    ring_buffer_mapper_.Unmap();
  }

  uint32_t min_frames = 0;
  uint32_t modulo_frames = 1;
  auto& ring_buffer = *config_.device_specific()->dai()->ring_buffer();
  if (ring_buffer.ring_buffer_constraints().has_value()) {
    min_frames = ring_buffer.ring_buffer_constraints()->min_frames();
    modulo_frames = ring_buffer.ring_buffer_constraints()->modulo_frames();
  }
  uint32_t driver_transfer_bytes = ring_buffer.driver_transfer_bytes().value_or(48);
  // The ring buffer must be at least min_frames + (rounded up) "driver transfer frames".
  num_ring_buffer_frames_ =
      request.min_frames() + ((driver_transfer_bytes + frame_size_ - 1) / frame_size_);

  num_ring_buffer_frames_ = std::max(
      min_frames, fbl::round_up<uint32_t, uint32_t>(num_ring_buffer_frames_, modulo_frames));

  zx_status_t status = ring_buffer_mapper_.CreateAndMap(
      static_cast<uint64_t>(num_ring_buffer_frames_) * frame_size_,
      ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &ring_buffer_vmo_,
      ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP);

  ZX_ASSERT_MSG(status == ZX_OK, "failed to create ring buffer VMO: %s",
                zx_status_get_string(status));

  zx::vmo out_vmo;
  zx_rights_t required_rights = ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_MAP;
  if (dai_config().is_input().has_value() && !*dai_config().is_input()) {
    required_rights |= ZX_RIGHT_WRITE;
  }
  status = ring_buffer_vmo_.duplicate(required_rights, &out_vmo);
  ZX_ASSERT_MSG(status == ZX_OK, "failed to duplicate VMO handle for out param: %s",
                zx_status_get_string(status));

  notifications_per_ring_ = request.clock_recovery_notifications_per_ring();

  zx::vmo duplicate_vmo;
  status = ring_buffer_vmo_.duplicate(
      ZX_RIGHT_TRANSFER | ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP, &duplicate_vmo);
  ZX_ASSERT_MSG(status == ZX_OK, "failed to duplicate VMO handle for VA client: %s",
                zx_status_get_string(status));

  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  parent->NotifyBufferCreated(std::move(duplicate_vmo), num_ring_buffer_frames_,
                              notifications_per_ring_);
  fuchsia_hardware_audio::RingBufferGetVmoResponse response;
  response.num_frames(num_ring_buffer_frames_);
  response.ring_buffer(std::move(out_vmo));
  completer.Reply(zx::ok(std::move(response)));
  ring_buffer_vmo_fetched_ = true;
}

void VirtualAudioDai::Start(StartCompleter::Sync& completer) {
  if (!ring_buffer_vmo_fetched_) {
    zxlogf(ERROR, "Cannot start the ring buffer before retrieving the VMO");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (ring_buffer_started_) {
    zxlogf(ERROR, "Cannot start the ring buffer if already started");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  zx_time_t now = zx::clock::get_monotonic().get();
  parent->NotifyStart(now);
  completer.Reply(now);
  ring_buffer_started_ = true;
}

void VirtualAudioDai::Stop(StopCompleter::Sync& completer) {
  if (!ring_buffer_vmo_fetched_) {
    zxlogf(ERROR, "Cannot start the ring buffer before retrieving the VMO");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (!ring_buffer_started_) {
    zxlogf(INFO, "Stop called while stopped; doing nothing");
    completer.Reply();
    return;
  }
  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  zx_time_t now = zx::clock::get_monotonic().get();
  // TODO(https://fxbug.dev/42075676): Add support for position, now we always report 0.
  parent->NotifyStop(now, 0);
  completer.Reply();
  ring_buffer_started_ = false;
}

void VirtualAudioDai::WatchClockRecoveryPositionInfo(
    WatchClockRecoveryPositionInfoCompleter::Sync& completer) {
  if (should_reply_to_position_request_ && ring_buffer_started_ && notifications_per_ring_ > 0) {
    fuchsia_hardware_audio::RingBufferPositionInfo position_info;
    position_info.timestamp(zx::clock::get_monotonic().get());
    // TODO(https://fxbug.dev/42075676): Add support for position, now we always report 0.
    position_info.position(0);
    completer.Reply(std::move(position_info));
    should_reply_to_position_request_ = false;
    return;
  }

  if (position_info_completer_.has_value()) {
    // The client called WatchClockRecoveryPositionInfo when another hanging get was pending.
    // This is an error condition and hence we unbind the channel.
    zxlogf(ERROR,
           "WatchClockRecoveryPositionInfo called when another hanging get was pending, unbinding");
    completer.Close(ZX_ERR_BAD_STATE);
    position_info_completer_.reset();
    should_reply_to_position_request_ = true;
    return;
  }

  position_info_completer_.emplace(completer.ToAsync());
}

void VirtualAudioDai::WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) {
  if (should_reply_to_delay_request_) {
    fuchsia_hardware_audio::DelayInfo delay_info;
    if (config_.device_specific() && config_.device_specific()->dai() &&
        config_.device_specific()->dai()->ring_buffer()) {
      auto& ring_buffer = *config_.device_specific()->dai()->ring_buffer();
      delay_info.internal_delay(ring_buffer.internal_delay());
      delay_info.external_delay(ring_buffer.external_delay());
    }
    completer.Reply(std::move(delay_info));
    should_reply_to_delay_request_ = false;
    return;
  }

  if (delay_info_completer_.has_value()) {
    // The client called WatchDelayInfo when another hanging get was pending.
    // This is an error condition and hence we unbind the channel.
    zxlogf(ERROR, "WatchDelayInfo called when another hanging get was pending, unbinding");
    completer.Close(ZX_ERR_BAD_STATE);
    delay_info_completer_.reset();
    should_reply_to_delay_request_ = true;
    return;
  }

  delay_info_completer_.emplace(completer.ToAsync());
}

void VirtualAudioDai::SetActiveChannels(
    fuchsia_hardware_audio::RingBufferSetActiveChannelsRequest& request,
    SetActiveChannelsCompleter::Sync& completer) {
  if (!ring_buffer_format_ || !ring_buffer_format_->pcm_format() ||
      !ring_buffer_format_->pcm_format()->number_of_channels()) {
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  uint64_t max_channel_bitmask = (1 << ring_buffer_format_->pcm_format()->number_of_channels()) - 1;
  if (request.active_channels_bitmask() > max_channel_bitmask) {
    zxlogf(WARNING, "%p: SetActiveChannels(0x%04zx) is out-of-range", this,
           request.active_channels_bitmask());
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (ring_buffer_active_channel_mask_ != request.active_channels_bitmask()) {
    active_channel_set_time_ = zx::clock::get_monotonic();
    ring_buffer_active_channel_mask_ = request.active_channels_bitmask();
  }
  completer.Reply(zx::ok(active_channel_set_time_.get()));
}

// Complain loudly but don't close the connection, since it is possible for this test fixture to
// be used with a client that is built with a newer SDK version.
void VirtualAudioDai::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::RingBuffer> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  zxlogf(ERROR, "VirtualAudioDai::handle_unknown_method (RingBuffer) ordinal %zu",
         metadata.method_ordinal);
}

void VirtualAudioDai::ShutdownAsync() {
  if (ring_buffer_binding_.has_value()) {
    ring_buffer_binding_->Unbind();
  }
  dai_bindings_.RemoveAll();
  DdkAsyncRemove();
}

void VirtualAudioDai::DdkRelease() {
  if (ring_buffer_binding_.has_value()) {
    ring_buffer_binding_->Unbind();
  }
  dai_bindings_.RemoveAll();
  OnShutdown();
}

}  // namespace virtual_audio
