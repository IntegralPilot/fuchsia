// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio/virtual-audio-composite.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/device/audio.h>

#include <cmath>

#include <fbl/algorithm.h>

#include "src/media/audio/drivers/lib/audio-proto-utils/include/audio-proto-utils/format-utils.h"

namespace fhasp = fuchsia_hardware_audio_signalprocessing;

namespace virtual_audio {

fuchsia_virtualaudio::Configuration VirtualAudioComposite::GetDefaultConfig() {
  constexpr fuchsia_hardware_audio::ElementId kDefaultRingBufferId = kRingBufferId;
  constexpr fuchsia_hardware_audio::ElementId kDefaultDaiId = kDaiId;
  constexpr fuchsia_hardware_audio::ElementId kDefaultPacketStreamId = kPacketStreamId;
  constexpr fuchsia_hardware_audio::TopologyId kDefaultTopologyId = kPlaybackTopologyId;

  fuchsia_virtualaudio::Configuration config;
  config.device_name("Virtual Audio Composite Device");
  config.manufacturer_name("Fuchsia");
  config.product_name("Virgil v2, a Virtual Volume Vessel");
  config.unique_id(std::array<uint8_t, 16>({1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0}));

  fuchsia_virtualaudio::Composite composite = {};

  // Composite ring buffer.
  fuchsia_virtualaudio::CompositeRingBuffer composite_ring_buffer = {};

  // Ring Buffer.
  fuchsia_virtualaudio::RingBuffer ring_buffer = {};

  // By default we expose a single ring buffer format: 48kHz stereo 16bit.
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

  // Default FIFO is 250 usec, at 48k stereo 16, no external delay specified.
  ring_buffer.driver_transfer_bytes(48);
  ring_buffer.internal_delay(0);

  // No ring_buffer_constraints specified.
  // No notifications_per_ring specified.

  composite_ring_buffer.id(kDefaultRingBufferId);
  composite_ring_buffer.ring_buffer(std::move(ring_buffer));

  std::vector<fuchsia_virtualaudio::CompositeRingBuffer> composite_ring_buffers = {};
  composite_ring_buffers.push_back(std::move(composite_ring_buffer));
  composite.ring_buffers(std::move(composite_ring_buffers));

  fuchsia_hardware_audio::DaiSupportedFormats format_set = {};
  format_set.number_of_channels(std::vector<uint32_t>{2});
  format_set.sample_formats(std::vector{fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned});
  format_set.frame_formats(std::vector{
      fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
          fuchsia_hardware_audio::DaiFrameFormatStandard::kI2S),
  });
  format_set.frame_rates(std::vector<uint32_t>{48'000});
  format_set.bits_per_slot(std::vector<uint8_t>{32});
  format_set.bits_per_sample(std::vector<uint8_t>{16});

  std::vector<fuchsia_virtualaudio::CompositeDaiInterconnect> composite_dai_interconnects = {};
  fuchsia_virtualaudio::CompositeDaiInterconnect composite_dai_interconnect = {};
  fuchsia_virtualaudio::CompositeDaiInterconnect composite_single_dai_interconnect = {};
  fuchsia_virtualaudio::DaiInterconnect dai_interconnect = {};
  fuchsia_virtualaudio::DaiInterconnect single_dai_interconnect = {};

  // By default we expose one DAI format: 48kHz I2S (stereo 16-in-32, 8 bytes/frame total).
  dai_interconnect.dai_supported_formats(
      std::optional<std::vector<fuchsia_hardware_audio::DaiSupportedFormats>>{
          std::in_place,
          {
              format_set,
          },
      });
  composite_dai_interconnect.id(kDaiId);
  composite_dai_interconnect.dai_interconnect(std::move(dai_interconnect));
  composite_dai_interconnects.push_back(std::move(composite_dai_interconnect));

  single_dai_interconnect.dai_supported_formats(
      std::optional<std::vector<fuchsia_hardware_audio::DaiSupportedFormats>>{
          std::in_place,
          {
              format_set,
          },
      });
  composite_single_dai_interconnect.id(kSingleDaiId);
  composite_single_dai_interconnect.dai_interconnect(std::move(single_dai_interconnect));
  composite_dai_interconnects.push_back(std::move(composite_single_dai_interconnect));
  composite.dai_interconnects(std::move(composite_dai_interconnects));

  // Topology with one ring buffer (through Gain) and one packet stream into one DAI interconnect.
  fhasp::Topology topology;
  topology.id(kDefaultTopologyId);
  fhasp::EdgePair edge_rb_to_vendor_specific;
  fhasp::EdgePair edge_vendor_specific_to_dynamics;
  fhasp::EdgePair edge_dynamics_to_equalizer;
  fhasp::EdgePair edge_equalizer_to_gain;
  fhasp::EdgePair edge_gain_to_dai;
  fhasp::EdgePair edge_ps_to_dai;

  edge_rb_to_vendor_specific.processing_element_id_from(kDefaultRingBufferId)
      .processing_element_id_to(kVendorSpecificId);
  edge_vendor_specific_to_dynamics.processing_element_id_from(kVendorSpecificId)
      .processing_element_id_to(kDynamicsId);
  edge_dynamics_to_equalizer.processing_element_id_from(kDynamicsId)
      .processing_element_id_to(kEqualizerId);
  edge_equalizer_to_gain.processing_element_id_from(kEqualizerId).processing_element_id_to(kGainId);
  edge_gain_to_dai.processing_element_id_from(kGainId).processing_element_id_to(kDefaultDaiId);
  edge_ps_to_dai.processing_element_id_from(kDefaultPacketStreamId)
      .processing_element_id_to(kDefaultDaiId);
  topology.processing_elements_edge_pairs(std::vector({
      std::move(edge_rb_to_vendor_specific),
      std::move(edge_vendor_specific_to_dynamics),
      std::move(edge_dynamics_to_equalizer),
      std::move(edge_equalizer_to_gain),
      std::move(edge_gain_to_dai),
      std::move(edge_ps_to_dai),
  }));
  composite.topologies(std::optional<std::vector<fhasp::Topology>>{
      std::in_place,
      {
          std::move(topology),
      },
  });

  fuchsia_virtualaudio::CompositePacketStream composite_packet_stream = {};
  fuchsia_virtualaudio::PacketStream packet_stream = {};
  packet_stream.supported_buffer_types(fuchsia_hardware_audio::BufferType::kClientOwned |
                                       fuchsia_hardware_audio::BufferType::kDriverOwned);
  packet_stream.needs_cache_flush_or_invalidate(true);

  composite_packet_stream.id(kDefaultPacketStreamId);
  composite_packet_stream.packet_stream(std::move(packet_stream));

  std::vector<fuchsia_virtualaudio::CompositePacketStream> composite_packet_streams = {};
  composite_packet_streams.push_back(std::move(composite_packet_stream));
  composite.packet_streams(std::move(composite_packet_streams));

  // Clock properties with no rate_adjustment_ppm specified (defaults to 0).
  fuchsia_virtualaudio::ClockProperties clock_properties = {};
  clock_properties.domain(0);
  composite.clock_properties(std::move(clock_properties));

  config.device_specific() =
      fuchsia_virtualaudio::DeviceSpecific::WithComposite(std::move(composite));

  return config;
}

zx::result<std::unique_ptr<VirtualAudioComposite>> VirtualAudioComposite::Create(
    InstanceId instance_id, fuchsia_virtualaudio::Configuration config,
    async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_virtualaudio::Device> server,
    OnDeviceBindingClosed on_binding_closed,
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent) {
  auto device = std::make_unique<VirtualAudioComposite>(
      instance_id, std::move(config), dispatcher, std::move(server), std::move(on_binding_closed));
  if (zx::result result = device->Init(parent); result.is_error()) {
    fdf::error("Failed to initialize virtual audio composite device: {}", result);
    return result.take_error();
  }
  return zx::ok(std::move(device));
}

zx::result<> VirtualAudioComposite::Init(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent) {
  std::string child_node_name = "virtual-audio-composite-" + std::to_string(instance_id_);

  zx::result connector = devfs_connector_.Bind(dispatcher_);
  if (connector.is_error()) {
    fdf::error("Failed to bind devfs connector: {}", connector);
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args{{
      .connector = std::move(connector.value()),
      .class_name{
          kClassName,
      },
  }};

  zx::result child =
      fdf::AddOwnedChild(parent, *fdf::Logger::GlobalInstance(), child_node_name, devfs_args);
  if (child.is_error()) {
    fdf::error("Failed to add owned child: {}", child);
    return child.take_error();
  }
  child_.emplace(std::move(child.value()));

  return zx::ok();
}

fuchsia_virtualaudio::RingBuffer& VirtualAudioComposite::GetRingBuffer(uint64_t id) {
  // TODO(https://fxbug.dev/42075676): Add support for a variable number of ring buffers (incl. 0).
  ZX_ASSERT(id == kRingBufferId);
  auto& ring_buffers = config_.device_specific()->composite()->ring_buffers().value();
  ZX_ASSERT(ring_buffers.size() == 1);
  ZX_ASSERT(ring_buffers[0].ring_buffer().has_value());
  return *ring_buffers[0].ring_buffer();
}

void VirtualAudioComposite::GetFormat(GetFormatCompleter::Sync& completer) {
  if (!ring_buffer_ || !ring_buffer_->format().pcm_format().has_value()) {
    fdf::warn("Ring buffer not initialized");
    completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNoRingBuffer));
    return;
  }

  auto pcm_format = ring_buffer_->format().pcm_format();
  auto& ring_buffer = GetRingBuffer(kRingBufferId);
  int64_t external_delay = 0;
  if (ring_buffer.external_delay().has_value()) {
    external_delay = *ring_buffer.external_delay();
  };

  auto sample_format = audio::utils::GetSampleFormat(pcm_format->valid_bits_per_sample(),
                                                     pcm_format->bytes_per_sample() * 8);
  fuchsia_virtualaudio::DeviceGetFormatResponse response{{
      .frames_per_second = pcm_format->frame_rate(),
      .sample_format = sample_format,
      .num_channels = pcm_format->number_of_channels(),
      .external_delay = external_delay,
  }};
  completer.Reply(fit::ok(std::move(response)));
}

void VirtualAudioComposite::GetBuffer(GetBufferCompleter::Sync& completer) {
  if (!ring_buffer_) {
    fdf::warn("Ring buffer not initialized");
    completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNoRingBuffer));
    return;
  }

  auto dup_result = ring_buffer_->DuplicateVmo();
  if (dup_result.is_error()) {
    fdf::error("Failed to duplicate ring buffer VMO: {}", dup_result.status_string());
    completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNoRingBuffer));
    return;
  }

  fuchsia_virtualaudio::DeviceGetBufferResponse response{{
      .ring_buffer = std::move(dup_result.value()),
      .num_ring_buffer_frames = ring_buffer_->num_frames(),
      .notifications_per_ring = ring_buffer_->notifications_per_ring(),
  }};
  completer.Reply(fit::ok(std::move(response)));
}

// Health implementation
//
void VirtualAudioComposite::GetHealthState(GetHealthStateCompleter::Sync& completer) {
  completer.Reply(fuchsia_hardware_audio::HealthState{}.healthy(true));
}

// Composite implementation
//
void VirtualAudioComposite::Reset(ResetCompleter::Sync& completer) {
  // Must clear all state for DAIs.
  // Must stop all RingBuffers, close connections and clear all state for RingBuffers elements.
  // Must clear all state for signalprocessing elements.
  // Must clear all signalprocessing topology state (presumably returning to a default topology?)

  completer.Reply(zx::ok());
}

void VirtualAudioComposite::GetProperties(
    fidl::Server<fuchsia_hardware_audio::Composite>::GetPropertiesCompleter::Sync& completer) {
  fuchsia_hardware_audio::CompositeProperties properties;
  properties.unique_id(config_.unique_id());
  properties.product(config_.product_name());
  properties.manufacturer(config_.manufacturer_name());
  ZX_ASSERT(composite_config().clock_properties().has_value());
  properties.clock_domain(composite_config().clock_properties()->domain());
  completer.Reply(std::move(properties));
}

void VirtualAudioComposite::GetDaiFormats(GetDaiFormatsRequest& request,
                                          GetDaiFormatsCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/42075676): Add better support for more DAI interconnects, enabling
  // configuration and observability in the virtual_audio FIDL API.
  if (request.processing_element_id() != kDaiId &&
      request.processing_element_id() != kSingleDaiId) {
    fdf::error("GetDaiFormats({}) bad element_id", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  ZX_ASSERT(composite_config().dai_interconnects().has_value());
  auto& dai_interconnects = *composite_config().dai_interconnects();
  ZX_ASSERT(dai_interconnects.size() == 2);  // Supports two DAI interconnects.
  ZX_ASSERT(dai_interconnects[0].dai_interconnect().has_value());
  ZX_ASSERT(dai_interconnects[0].dai_interconnect()->dai_supported_formats().has_value());
  ZX_ASSERT(dai_interconnects[1].dai_interconnect().has_value());
  ZX_ASSERT(dai_interconnects[1].dai_interconnect()->dai_supported_formats().has_value());

  if (request.processing_element_id() == dai_interconnects[0].id()) {
    completer.Reply(
        zx::ok(dai_interconnects[0].dai_interconnect()->dai_supported_formats().value()));
  } else {
    completer.Reply(
        zx::ok(dai_interconnects[1].dai_interconnect()->dai_supported_formats().value()));
  }
}

void VirtualAudioComposite::SetDaiFormat(SetDaiFormatRequest& request,
                                         SetDaiFormatCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/42075676): Add better support for more DAI interconnects, enabling
  // configuration and observability in the virtual_audio FIDL API.
  if (request.processing_element_id() != kDaiId &&
      request.processing_element_id() != kSingleDaiId) {
    fdf::error("SetDaiFormat({}) bad element_id", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  fuchsia_hardware_audio::DaiFormat format = request.format();
  if (format.frame_rate() > 192000) {
    fdf::error("SetDaiFormat frame_rate ({}) too high", format.frame_rate());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  ZX_ASSERT(composite_config().dai_interconnects().has_value());
  ZX_ASSERT(composite_config().dai_interconnects()->size() == 2);

  fuchsia_virtualaudio::CompositeDaiInterconnect& dai_interconnect =
      composite_config().dai_interconnects()->at(0).id() == request.processing_element_id()
          ? composite_config().dai_interconnects()->at(0)
          : composite_config().dai_interconnects()->at(1);

  std::vector<fuchsia_hardware_audio::DaiSupportedFormats> supported_formats{};
  if (dai_interconnect.dai_interconnect().has_value() &&
      dai_interconnect.dai_interconnect()->dai_supported_formats().has_value()) {
    // TODO(https://fxbug.dev/441351908): set the format for the specified DAI_INTERCONNECT element,
    // not exclusively the first DAI element in our collection.
    supported_formats = composite_config()
                            .dai_interconnects()
                            ->at(0)
                            .dai_interconnect()
                            ->dai_supported_formats()
                            .value();
  }

  for (auto dai_format_set : supported_formats) {
    std::optional<uint32_t> number_of_channels;
    for (auto channel_count : dai_format_set.number_of_channels()) {
      if (channel_count == format.number_of_channels()) {
        number_of_channels = format.number_of_channels();
        break;
      }
    }
    std::optional<uint64_t> channels_to_use_bitmask;
    if (format.channels_to_use_bitmask() <= (1u << format.number_of_channels()) - 1) {
      channels_to_use_bitmask = format.channels_to_use_bitmask();
    }
    std::optional<fuchsia_hardware_audio::DaiSampleFormat> sample_format;
    for (auto sample_fmt : dai_format_set.sample_formats()) {
      if (sample_fmt == format.sample_format()) {
        sample_format = format.sample_format();
        break;
      }
    }
    std::optional<fuchsia_hardware_audio::DaiFrameFormat> frame_format;
    for (auto& frame_fmt : dai_format_set.frame_formats()) {
      if (frame_fmt == format.frame_format()) {
        frame_format = format.frame_format();
        break;
      }
    }
    std::optional<uint32_t> frame_rate;
    for (auto rate : dai_format_set.frame_rates()) {
      if (rate == format.frame_rate()) {
        frame_rate = format.frame_rate();
        break;
      }
    }
    std::optional<uint8_t> bits_per_slot;
    for (auto bits : dai_format_set.bits_per_slot()) {
      if (bits == format.bits_per_slot()) {
        bits_per_slot = format.bits_per_slot();
        break;
      }
    }
    std::optional<uint8_t> bits_per_sample;
    for (auto bits : dai_format_set.bits_per_sample()) {
      if (bits == format.bits_per_sample()) {
        bits_per_sample = format.bits_per_sample();
        break;
      }
    }
    if (number_of_channels.has_value() && channels_to_use_bitmask.has_value() &&
        sample_format.has_value() && frame_format.has_value() && frame_rate.has_value() &&
        bits_per_slot.has_value() && bits_per_sample.has_value()) {
      // TODO(https://fxbug.dev/441351908): save the actual DaiFormat.
      fdf::info("SetDaiFormat for element %u", request.processing_element_id());
      completer.Reply(zx::ok());
      return;
    }
  }
  fdf::error("SetDaiFormat: unsupported format");
  completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
}

void VirtualAudioComposite::GetRingBufferFormats(GetRingBufferFormatsRequest& request,
                                                 GetRingBufferFormatsCompleter::Sync& completer) {
  // This driver is limited to a single ring buffer.
  // TODO(https://fxbug.dev/42075676): Add support for more ring buffers, enabling configuration and
  // observability in the virtual_audio FIDL API.
  if (request.processing_element_id() != kRingBufferId) {
    fdf::error("GetRingBufferFormats({}) bad element_id", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  std::vector<fuchsia_hardware_audio::SupportedFormats2> all_formats;
  auto& ring_buffer = GetRingBuffer(request.processing_element_id());
  for (auto& formats : ring_buffer.supported_formats().value()) {
    fuchsia_hardware_audio::PcmSupportedFormats pcm_formats;
    std::vector<fuchsia_hardware_audio::ChannelSet> channel_sets;
    for (uint8_t number_of_channels = formats.min_channels();
         number_of_channels <= formats.max_channels(); ++number_of_channels) {
      std::vector<fuchsia_hardware_audio::ChannelAttributes> attributes(number_of_channels);
      // For simplicity, only provide channel attributes (frequency ranges) first channel.
      // When unspecified (as with other channels, and in other channel sets), this conveys that
      // the channel supports the full range (down to 0, and up to FrameRate/2).
      if (number_of_channels == formats.min_channels()) {
        attributes[0].min_frequency() = 20;
        attributes[0].max_frequency() = 20000;
      } else {
        // Vector of [number_of_channels] attributes that do not set min_frequency or max_frequency.
      }
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
      auto formats_entry = fuchsia_hardware_audio::SupportedFormats2::WithPcmSupportedFormats(
          std::move(pcm_formats2));
      all_formats.push_back(std::move(formats_entry));
    }
  }
  completer.Reply(zx::ok(std::move(all_formats)));
}

void VirtualAudioComposite::CreateRingBuffer(CreateRingBufferRequest& request,
                                             CreateRingBufferCompleter::Sync& completer) {
  // One ring buffer is supported by this driver.
  // TODO(https://fxbug.dev/42075676): Add support for more ring buffers, enabling configuration and
  // observability in the virtual_audio FIDL API.
  if (request.processing_element_id() != kRingBufferId) {
    fdf::error("CreateRingBuffer({}) bad element_id", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  if (!request.format().pcm_format().has_value()) {
    fdf::error("RingBuffer format must be PCM");
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  // Create and bind the RingBuffer.
  // We don't support multiple RingBuffers yet, so we just overwrite the existing one if any.
  auto& ring_buffer_config = GetRingBuffer(request.processing_element_id());

  ring_buffer_.reset(new VirtualAudioRingBuffer(
      request.format(), ring_buffer_config, (current_topology_id_ == kPlaybackTopologyId),
      dispatcher_, std::move(request.ring_buffer()),
      [this](zx::vmo vmo, uint32_t num_frames, uint32_t notifications) {
        fidl::Status result = fidl::WireSendEvent(device_binding_)
                                  ->OnBufferCreated(std::move(vmo), num_frames, notifications);
        if (result.status() != ZX_OK) {
          fdf::warn("Failed to send OnBufferCreated event: {}", result);
        }
      },
      [this](zx_time_t start_time) {
        fidl::Status result = fidl::WireSendEvent(device_binding_)->OnStart(start_time);
        if (result.status() != ZX_OK) {
          fdf::warn("Failed to send OnStart event: {}", result);
        }
      },
      [this](zx_time_t stop_time, uint32_t position) {
        fidl::Status result = fidl::WireSendEvent(device_binding_)->OnStop(stop_time, position);
        if (result.status() != ZX_OK) {
          fdf::warn("Failed to send OnStop event: {}", result);
        }
      },
      [this](VirtualAudioRingBuffer* stream, fidl::UnbindInfo info) {
        fdf::info("RingBuffer unbound: {}", info.status_string());
        if (ring_buffer_.get() == stream) {
          ring_buffer_.reset();
        }
      }));

  completer.Reply(zx::ok());
}

void VirtualAudioComposite::ResetRingBuffer() { ring_buffer_.reset(); }

void VirtualAudioComposite::GetPacketStreamFormats(
    GetPacketStreamFormatsRequest& request, GetPacketStreamFormatsCompleter::Sync& completer) {
  if (request.processing_element_id() != kPacketStreamId) {
    fdf::error("GetPacketStreamFormats({}) bad element_id", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  std::vector<fuchsia_hardware_audio::SupportedFormats2> all_formats;

  fuchsia_hardware_audio::PcmSupportedFormats pcm_formats;

  // PCM formats.
  {
    fuchsia_hardware_audio::ChannelSet channel_set;
    std::vector<fuchsia_hardware_audio::ChannelAttributes> attributes(2);
    attributes[0].min_frequency() = 20;
    attributes[0].max_frequency() = 20000;
    attributes[1].min_frequency() = 20;
    attributes[1].max_frequency() = 20000;
    channel_set.attributes(std::move(attributes));

    std::vector<fuchsia_hardware_audio::ChannelSet> channel_sets;
    channel_sets.push_back(std::move(channel_set));
    pcm_formats.channel_sets(std::move(channel_sets));

    pcm_formats.frame_rates(std::vector<uint32_t>{48000});
    pcm_formats.bytes_per_sample(std::vector<uint8_t>{2});
    pcm_formats.valid_bits_per_sample(std::vector<uint8_t>{16});

    pcm_formats.sample_formats(std::vector<fuchsia_hardware_audio::SampleFormat>{
        fuchsia_hardware_audio::SampleFormat::kPcmSigned,
    });

    all_formats.push_back(
        fuchsia_hardware_audio::SupportedFormats2::WithPcmSupportedFormats(std::move(pcm_formats)));
  }

  // Encoded formats.
  {
    fuchsia_hardware_audio::SupportedEncodings encoded_formats;

    fuchsia_hardware_audio::ChannelSet channel_set;
    std::vector<fuchsia_hardware_audio::ChannelAttributes> attributes(2);
    attributes[0].min_frequency() = 20;
    attributes[0].max_frequency() = 20000;
    attributes[1].min_frequency() = 20;
    attributes[1].max_frequency() = 20000;
    channel_set.attributes(std::move(attributes));

    std::vector<fuchsia_hardware_audio::ChannelSet> channel_sets;
    channel_sets.push_back(std::move(channel_set));
    encoded_formats.decoded_channel_sets(std::move(channel_sets));

    encoded_formats.decoded_frame_rates(std::vector<uint32_t>{48000});
    encoded_formats.encoding_types(std::vector<fuchsia_hardware_audio::EncodingType>{
        fuchsia_hardware_audio::EncodingType::kAac,
    });

    all_formats.push_back(fuchsia_hardware_audio::SupportedFormats2::WithSupportedEncodings(
        std::move(encoded_formats)));
  }

  completer.Reply(zx::ok(std::move(all_formats)));
}

void VirtualAudioComposite::CreatePacketStream(CreatePacketStreamRequest& request,
                                               CreatePacketStreamCompleter::Sync& completer) {
  if (request.processing_element_id() != kPacketStreamId) {
    fdf::error("CreatePacketStream({}) bad element_id", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  // Find the PacketStream config for this element ID.
  // We assume only one exists for now, and it matches kPacketStreamId.
  ZX_ASSERT(composite_config().packet_streams().has_value());
  auto& packet_streams = *composite_config().packet_streams();
  ZX_ASSERT(!packet_streams.empty());

  // Assuming the first and only packet stream config is for this ID.
  // In a multi-stream world, we would iterate to find the matching ID.
  ZX_ASSERT(packet_streams[0].id() == kPacketStreamId);
  auto& composite_packet_stream = packet_streams[0];

  bool is_outgoing = true;  // kPacketStreamId is outgoing.
  auto on_close = [this](VirtualAudioPacketStream* stream, fidl::UnbindInfo info) {
    if (info.is_user_initiated()) {
      fdf::warn("PacketStream client closed channel");
    } else if (info.is_peer_closed()) {
      fdf::warn("PacketStream peer closed channel");
    } else {
      fdf::error("PacketStream channel closed: {}", info.status_string());
    }
    // Remove the stream from the list.
    std::erase_if(packet_streams_, [stream](const auto& p) { return p.get() == stream; });
  };

  auto packet_stream = std::make_unique<VirtualAudioPacketStream>(
      is_outgoing, std::move(request.format()), *composite_packet_stream.packet_stream(),
      dispatcher_, std::move(request.packet_stream_control()), std::move(on_close));

  packet_streams_.push_back(std::move(packet_stream));

  completer.Reply(zx::ok());
}

// RingBuffer implementation methods removed (moved to VirtualAudioRingBuffer).

// signalprocessing
//
void VirtualAudioComposite::SignalProcessingConnect(
    SignalProcessingConnectRequest& request, SignalProcessingConnectCompleter::Sync& completer) {
  if (signal_) {
    fdf::error("Signal processing already bound");
    request.protocol().Close(ZX_ERR_ALREADY_BOUND);
    return;
  }

  SetupSignalProcessing();
  signal_.emplace(dispatcher_, std::move(request.protocol()), this,
                  std::mem_fn(&VirtualAudioComposite::OnSignalProcessingClosed));
}

void VirtualAudioComposite::OnSignalProcessingClosed(fidl::UnbindInfo info) {
  if (!info.is_user_initiated() && !info.is_peer_closed()) {
    // Do not log canceled cases; these happen particularly frequently in certain test cases.
    if (info.status() != ZX_ERR_CANCELED) {
      fdf::error("Client connection unbound: {}", info.status_string());
    }
  }
  if (signal_) {
    signal_.reset();
  }
  for (auto& [_element_id, snapshot] : element_states_) {
    snapshot.completer.reset();
    snapshot.last_notified.reset();
  }
  last_reported_topology_id_.reset();
  watch_topology_completer_.reset();
}

void VirtualAudioComposite::SetupSignalProcessing() {
  SetupSignalProcessingElements();
  SetupSignalProcessingTopologies();
  SetupSignalProcessingElementStates();
}

// signalprocessing Element handling
//
// This driver exposes signalprocessing Elements of various types: ring buffers, a packet stream,
// DAI interconnects, and otherprocessing elements (Dynamics, Equalizer, Gain, VendorSpecific).
//
// TODO(https://fxbug.dev/42075676): Add support for more topologies and elements, so that
// everything is configurable and observable via the virtual_audio FIDL API.
void VirtualAudioComposite::SetupSignalProcessingElements() {
  element_map_.clear();
  elements_.clear();

  // Configure RING_BUFFER element
  fhasp::Element ring_buffer;
  ring_buffer.id(kRingBufferId).type(fhasp::ElementType::kRingBuffer);

  // Configure DAI element
  fhasp::Element dai;
  fhasp::DaiInterconnect dai_interconnect;
  // Connect this to the existing virtualaudio FIDL method for dynamic plug_state changes?
  dai_interconnect.plug_detect_capabilities(fhasp::PlugDetectCapabilities::kHardwired);
  dai.id(kDaiId)
      .type(fhasp::ElementType::kDaiInterconnect)
      .type_specific(fhasp::TypeSpecificElement::WithDaiInterconnect(std::move(dai_interconnect)));

  // Configure VENDOR_SPECIFIC element
  fhasp::Element vendor_specific;
  fhasp::VendorSpecific vendor_specific_type_specific;
  vendor_specific.id(kVendorSpecificId)
      .type(fhasp::ElementType::kVendorSpecific)
      .type_specific(
          fhasp::TypeSpecificElement::WithVendorSpecific(std::move(vendor_specific_type_specific)));

  // Configure DYNAMICS element
  fhasp::Element dynamics;
  fhasp::Dynamics dynamics_type_specific;
  std::vector<fhasp::DynamicsBand> dynamics_bands;
  dynamics_bands.emplace_back(fhasp::DynamicsBand{{.id = 42}});
  dynamics_bands.emplace_back(fhasp::DynamicsBand{{.id = 68}});
  dynamics_type_specific.bands(std::move(dynamics_bands))
      .supported_controls(fhasp::DynamicsSupportedControls::kKneeWidth |
                          fhasp::DynamicsSupportedControls::kAttack |
                          fhasp::DynamicsSupportedControls::kRelease |
                          fhasp::DynamicsSupportedControls::kOutputGain |
                          fhasp::DynamicsSupportedControls::kInputGain |
                          fhasp::DynamicsSupportedControls::kLookahead |
                          fhasp::DynamicsSupportedControls::kLevelType |
                          fhasp::DynamicsSupportedControls::kLinkedChannels |
                          fhasp::DynamicsSupportedControls::kThresholdType);
  dynamics
      .can_bypass(true)  // This element can be tested for bypass.
      .can_stop(false)   // This element cannot be tested for stop.
      .id(kDynamicsId)
      .type(fhasp::ElementType::kDynamics)
      .type_specific(fhasp::TypeSpecificElement::WithDynamics(std::move(dynamics_type_specific)));

  // Configure EQUALIZER element
  fhasp::Element equalizer;
  fhasp::Equalizer equalizer_type_specific;
  std::vector<fhasp::EqualizerBand> equalizer_bands;
  equalizer_bands.emplace_back(fhasp::EqualizerBand{{.id = 62}});
  equalizer_bands.emplace_back(fhasp::EqualizerBand{{.id = 48}});
  equalizer_type_specific.bands(std::move(equalizer_bands))
      .supported_controls(fhasp::EqualizerSupportedControls::kCanControlFrequency |
                          fhasp::EqualizerSupportedControls::kCanControlQ |
                          fhasp::EqualizerSupportedControls::kSupportsTypePeak |
                          fhasp::EqualizerSupportedControls::kSupportsTypeNotch |
                          fhasp::EqualizerSupportedControls::kSupportsTypeLowCut |
                          fhasp::EqualizerSupportedControls::kSupportsTypeHighCut |
                          fhasp::EqualizerSupportedControls::kSupportsTypeLowShelf |
                          fhasp::EqualizerSupportedControls::kSupportsTypeHighShelf)
      .can_disable_bands(true)
      .min_frequency(0)
      .max_frequency(19876)
      .max_q(2.5)
      .min_gain_db(-96.0)
      .max_gain_db(96.0);
  equalizer
      .can_bypass(true)  // This element can be tested for bypass.
      .can_stop(true)    // This element can be tested for stop.
      .id(kEqualizerId)
      .type(fhasp::ElementType::kEqualizer)
      .type_specific(fhasp::TypeSpecificElement::WithEqualizer(std::move(equalizer_type_specific)));

  // Configure GAIN element
  fhasp::Element gain;
  fhasp::Gain gain_type_specific;
  gain_type_specific.type(fhasp::GainType::kDecibels)
      .domain(fhasp::GainDomain::kDigital)
      .min_gain(-68.0)
      .max_gain(+6.0)
      .min_gain_step(0.5);
  gain.can_bypass(true)  // This element can be tested for bypass.
      .id(kGainId)
      .type(fhasp::ElementType::kGain)
      .type_specific(fhasp::TypeSpecificElement::WithGain(gain_type_specific));

  // Configure standalone (self-referential) DAI element
  fhasp::Element single_dai;
  fhasp::DaiInterconnect single_dai_interconnect;
  single_dai_interconnect.plug_detect_capabilities(fhasp::PlugDetectCapabilities::kHardwired);
  single_dai.id(kSingleDaiId)
      .type(fhasp::ElementType::kDaiInterconnect)
      .type_specific(
          fhasp::TypeSpecificElement::WithDaiInterconnect(std::move(single_dai_interconnect)))
      .description("Single-element DAI")
      .can_stop(false)
      .can_bypass(false);

  // Configure PACKET_STREAM element
  fhasp::Element packet_stream;
  packet_stream.id(kPacketStreamId)
      .type(fhasp::ElementType::kPacketStream)
      .description("Packet Stream Endpoint");

  elements_ = {{
      ring_buffer,
      dai,
      vendor_specific,
      dynamics,
      equalizer,
      gain,
      single_dai,
      packet_stream,
  }};
  for (auto& el : elements_) {
    element_map_.insert({*el.id(), &el});
  }
}

void VirtualAudioComposite::GetElements(GetElementsCompleter::Sync& completer) {
  completer.Reply(zx::ok(elements_));
}

// signalprocessing Topology handling
//
// We expose three topologies:
// - kPlaybackTopologyId: { Rb -> VendorSpecific -> Dynamics -> Equalizer -> Gain -> Dai }
//                    and { PacketStream -> Dai }
// - kCaptureTopologyId: { Dai -> Gain -> Rb }
// - kSingleElementTopologyId: { SingleDai -> SingleDai }
//
// TODO(https://fxbug.dev/42075676): Include signalprocessing configuration/observability in the
// virtual_audio FIDL API.
void VirtualAudioComposite::SetupSignalProcessingTopologies() {
  topologies_.clear();

  {
    fhasp::Topology topology;
    topology.id(kPlaybackTopologyId);
    fhasp::EdgePair edge1, edge2, edge3, edge4, edge5, edge6;
    edge1.processing_element_id_from(kRingBufferId).processing_element_id_to(kVendorSpecificId);
    edge2.processing_element_id_from(kVendorSpecificId).processing_element_id_to(kDynamicsId);
    edge3.processing_element_id_from(kDynamicsId).processing_element_id_to(kEqualizerId);
    edge4.processing_element_id_from(kEqualizerId).processing_element_id_to(kGainId);
    edge5.processing_element_id_from(kGainId).processing_element_id_to(kDaiId);
    edge6.processing_element_id_from(kPacketStreamId).processing_element_id_to(kDaiId);
    topology.processing_elements_edge_pairs(std::vector({
        std::move(edge1),
        std::move(edge2),
        std::move(edge3),
        std::move(edge4),
        std::move(edge5),
        std::move(edge6),
    }));

    // By default (in the topology of kPlaybackTopologyId), our ring buffer will be an outgoing one.
    current_topology_id_ = kPlaybackTopologyId;
    topologies_.emplace_back(std::move(topology));
  }

  {
    fhasp::Topology topology;
    topology.id(kCaptureTopologyId);
    fhasp::EdgePair edge1, edge2;
    edge1.processing_element_id_from(kDaiId).processing_element_id_to(kGainId);
    edge2.processing_element_id_from(kGainId).processing_element_id_to(kRingBufferId);
    topology.processing_elements_edge_pairs(std::vector({
        std::move(edge1),
        std::move(edge2),
    }));
    topologies_.emplace_back(std::move(topology));
  }

  {
    fhasp::Topology topology;
    topology.id(kSingleElementTopologyId);
    fhasp::EdgePair edge1;
    edge1.processing_element_id_from(kSingleDaiId).processing_element_id_to(kSingleDaiId);
    topology.processing_elements_edge_pairs(std::vector({
        std::move(edge1),
    }));
    topologies_.emplace_back(std::move(topology));
  }
}

void VirtualAudioComposite::GetTopologies(GetTopologiesCompleter::Sync& completer) {
  completer.Reply(zx::ok(topologies_));
}

void VirtualAudioComposite::SetTopology(SetTopologyRequest& request,
                                        SetTopologyCompleter::Sync& completer) {
  if (request.topology_id() != kPlaybackTopologyId && request.topology_id() != kCaptureTopologyId &&
      request.topology_id() != kSingleElementTopologyId) {
    fdf::error("SetTopology({}) unknown topology_id", request.topology_id());
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  current_topology_id_ = request.topology_id();
  completer.Reply(zx::ok());
  MaybeCompleteWatchTopology();
}

void VirtualAudioComposite::WatchTopology(WatchTopologyCompleter::Sync& completer) {
  // The client should not call WatchTopology when a previous WatchTopology is pending.
  if (watch_topology_completer_.has_value()) {
    fdf::error("WatchTopology called while previous call was pending. Unbinding");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  watch_topology_completer_ = completer.ToAsync();
  MaybeCompleteWatchTopology();
}

// If we should tell the client about the topology, and if there is a pending request, complete it.
void VirtualAudioComposite::MaybeCompleteWatchTopology() {
  if (watch_topology_completer_.has_value() &&
      (!last_reported_topology_id_.has_value() ||
       *last_reported_topology_id_ != current_topology_id_)) {
    last_reported_topology_id_ = current_topology_id_;
    auto completer = std::move(watch_topology_completer_);
    watch_topology_completer_.reset();
    completer->Reply(current_topology_id_);
  }
}

// signalprocessing ElementState handling
//
// This driver is limited to a ring buffer, a DAI interconnect and a gain element. Of these, DAI and
// Gain return type-specific ElementState; only the Gain element has _settable_ type-specific state.
// DaiInterconnect, Dynamics, Equalizer, Gain and VendorSpecific return type-specific ElementState;
// all but DaiInterconnect also support having this type-specific ElementState _set by a client_.
//
// TODO(https://fxbug.dev/42075676): Add support for more topologies and elements, so that
// everything is configurable and observable via the virtual_audio FIDL API. This includes the
// ability for a client to trigger 'unsolicited' state changes (e.g. un/plugging the DAI).
void VirtualAudioComposite::SetupSignalProcessingElementStates() {
  element_states_.clear();

  fhasp::DaiInterconnectElementState dai_state;
  fhasp::PlugState plug_state;
  plug_state.plugged(true).plug_state_time(0);
  dai_state.plug_state(std::move(plug_state));
  ElementSnapshot dai_snapshot;
  dai_snapshot.current.started(true).bypassed(false).type_specific(
      fhasp::TypeSpecificElementState::WithDaiInterconnect(std::move(dai_state)));
  dai_snapshot.last_notified.reset();
  dai_snapshot.completer.reset();
  element_states_.insert({
      kDaiId,
      std::move(dai_snapshot),
  });

  ElementSnapshot rb_snapshot;
  rb_snapshot.current.started(true).bypassed(false).processing_delay(0);
  rb_snapshot.last_notified.reset();
  rb_snapshot.completer.reset();
  element_states_.insert({
      kRingBufferId,
      std::move(rb_snapshot),
  });

  fhasp::VendorSpecificState vendor_specific_state;
  ElementSnapshot vendor_specific_snapshot;
  std::vector<uint8_t> vendor_specific_vec{1, 2, 3, 4, 5, 6, 7, 8};
  vendor_specific_snapshot.current.started(true)
      .bypassed(false)
      .turn_on_delay(0)
      .vendor_specific_data(std::move(vendor_specific_vec))
      .type_specific(
          fhasp::TypeSpecificElementState::WithVendorSpecific(std::move(vendor_specific_state)));
  vendor_specific_snapshot.last_notified.reset();
  vendor_specific_snapshot.completer.reset();
  element_states_.insert({
      kVendorSpecificId,
      std::move(vendor_specific_snapshot),
  });

  fhasp::DynamicsElementState dynamics_state;
  std::vector<fhasp::DynamicsBandState> dyn_band_states;
  dyn_band_states.emplace_back(fhasp::DynamicsBandState{{
      .id = 42,
      .min_frequency = 12,
      .max_frequency = 12000,
      .threshold_db = -4.0,
      .threshold_type = fhasp::ThresholdType::kBelow,
      .ratio = 0.333,
      .knee_width_db = 4.0,
      .attack = zx::msec(40).get(),
      .release = zx::msec(160).get(),
      .output_gain_db = 0.0,
      .input_gain_db = 0.0,
      .level_type = fhasp::LevelType::kPeak,
      .lookahead = zx::msec(200).get(),
      .linked_channels = true,
  }});
  dyn_band_states.emplace_back(fhasp::DynamicsBandState{{
      .id = 68,
      .min_frequency = 23,
      .max_frequency = 16000,
      .threshold_db = -48.0,
      .threshold_type = fhasp::ThresholdType::kAbove,
      .ratio = 0.888,
      .knee_width_db = 6.0,
      .attack = zx::usec(200).get(),
      .release = zx::usec(300).get(),
      .output_gain_db = -2.0,
      .input_gain_db = -4.0,
      .level_type = fhasp::LevelType::kRms,
      .lookahead = zx::msec(400).get(),
      .linked_channels = false,
  }});
  dynamics_state.band_states(std::move(dyn_band_states));
  ElementSnapshot dynamics_snapshot;
  dynamics_snapshot.current.started(true).bypassed(false).turn_on_delay(0).type_specific(
      fhasp::TypeSpecificElementState::WithDynamics(std::move(dynamics_state)));
  dynamics_snapshot.last_notified.reset();
  dynamics_snapshot.completer.reset();
  element_states_.insert({
      kDynamicsId,
      std::move(dynamics_snapshot),
  });

  fhasp::EqualizerElementState equalizer_state;
  std::vector<fhasp::EqualizerBandState> eq_band_states;
  eq_band_states.emplace_back(fhasp::EqualizerBandState{{
      .id = 62,
      .type = fhasp::EqualizerBandType::kPeak,
      .frequency = 2000,
      .q = 2,
      .gain_db = 3.0,
      .enabled = true,
  }});
  eq_band_states.emplace_back(fhasp::EqualizerBandState{{
      .id = 48,
      .type = fhasp::EqualizerBandType::kLowShelf,
      .frequency = 120,
      .q = 3.0,
      .gain_db = -6.0,
      .enabled = true,
  }});
  equalizer_state.band_states(std::move(eq_band_states));
  ElementSnapshot equalizer_snapshot;
  equalizer_snapshot.current.started(true).bypassed(false).turn_on_delay(0).type_specific(
      fhasp::TypeSpecificElementState::WithEqualizer(std::move(equalizer_state)));
  equalizer_snapshot.last_notified.reset();
  equalizer_snapshot.completer.reset();
  element_states_.insert({
      kEqualizerId,
      std::move(equalizer_snapshot),
  });

  fhasp::GainElementState gain_state;
  gain_state.gain(-6.0);
  ElementSnapshot gain_snapshot;
  gain_snapshot.current.started(true).bypassed(false).turn_on_delay(0).type_specific(
      fhasp::TypeSpecificElementState::WithGain(gain_state));
  gain_snapshot.last_notified.reset();
  gain_snapshot.completer.reset();
  element_states_.insert({
      kGainId,
      std::move(gain_snapshot),
  });

  ElementSnapshot ps_snapshot;
  ps_snapshot.current.started(true).bypassed(false).processing_delay(0);
  ps_snapshot.last_notified.reset();
  ps_snapshot.completer.reset();
  element_states_.insert({
      kPacketStreamId,
      std::move(ps_snapshot),
  });

  fhasp::DaiInterconnectElementState single_dai_state;
  fhasp::PlugState single_dai_plug_state;
  single_dai_plug_state.plugged(true).plug_state_time(0);
  single_dai_state.plug_state(std::move(single_dai_plug_state));
  ElementSnapshot single_dai_snapshot;
  single_dai_snapshot.current.started(true).bypassed(false).type_specific(
      fhasp::TypeSpecificElementState::WithDaiInterconnect(std::move(single_dai_state)));
  single_dai_snapshot.last_notified.reset();
  single_dai_snapshot.completer.reset();
  element_states_.insert({
      kSingleDaiId,
      std::move(single_dai_snapshot),
  });
}

// Note that the range of type-specific state for an element is greater than the range of
// type-specific state that can be changed by clients. This is why we define two distinct unions:
//
// TypeSpecificElementState is used by the method WatchElementState.
// This union defines variants for DAI, DYNAMICS, EQUALIZER, GAIN and VENDOR_SPECIFIC element types.
//
// SettableTypeSpecificElementState is used by the method SetElementState.
// This union defines variants for DYNAMICS, EQUALIZER, GAIN and VENDOR_SPECIFIC element types.
//
// To verify these modes, the driver supports 1 ring buffer, 1 gain element and 1 DAI interconnect.
// TODO(https://fxbug.dev/42075676): Add support for more elements specified in the Configuration,
// enabling dynamic behavior and observability via the virtual_audio FIDL API.
void VirtualAudioComposite::SetElementState(SetElementStateRequest& request,
                                            SetElementStateCompleter::Sync& completer) {
  fuchsia_hardware_audio::ElementId element_id = request.processing_element_id();
  fdf::info("SetElementState({})", element_id);

  // Reject all error cases BEFORE changing any state variables.

  // Error: unknown element_id
  if (!element_map_.contains(element_id)) {
    fdf::error("SetElementState({}): unknown element_id", element_id);
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  const auto& ele = element_map_[element_id];

  // Error: this element cannot Stop as requested.
  if (request.state().started().has_value() && !*request.state().started() &&
      !ele->can_stop().value_or(false)) {
    fdf::error("SetElementState({}): element cannot be stopped", element_id);
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  // Error: this element cannot Bypass as requested.
  if (request.state().bypassed().has_value() && *request.state().bypassed() &&
      !ele->can_bypass().value_or(false)) {
    fdf::error("SetElementState({}): element cannot be bypassed", element_id);
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  switch (element_id) {
    case kPacketStreamId:
      // PACKET_STREAM elements contain no type-specific state.
      // TypeSpecificElementState contains no variant for the PACKET_STREAM type.
      __FALLTHROUGH;
    case kRingBufferId:
      // RING_BUFFER elements contain no type-specific state.
      // TypeSpecificElementState contains no variant for the RING_BUFFER type.
      __FALLTHROUGH;
    case kDaiId:
    case kSingleDaiId:
      // DAI_INTERCONNECT elements can specify type-specific state, but clients cannot CHANGE it.
      // TypeSpecificElementState defines a DAI variant; SettableTypeSpecificElementState does not.
      if (request.state().type_specific().has_value()) {
        // Error: type_specific state in this request does not match this element type.
        fdf::error("SetElementState({}): TypeSpecificElementState does not match this element type",
                   element_id);
        completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
        return;
      }
      break;
    case kDynamicsId:
      if (request.state().type_specific().has_value()) {
        // For this element, clients can specify type-specific state but it must be dynamics.
        if (!request.state().type_specific()->dynamics().has_value()) {
          // Error: type_specific state in this request does not match this element type.
          fdf::error(
              "SetElementState({}): TypeSpecificElementState does not match this element type",
              element_id);
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
        // Error: SetElementState value is missing or non-finite.
        if (!request.state().type_specific()->dynamics()->band_states().has_value() ||
            request.state().type_specific()->dynamics()->band_states()->empty()) {
          fdf::error("SetElementState({}): Dynamics requires non-empty band_states", element_id);
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
        for (const auto& band_state : *request.state().type_specific()->dynamics()->band_states()) {
          if (!band_state.id().has_value()) {
            fdf::error(
                "SetElementState({}): DynamicsElementState band_state does not specify band_id",
                element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          if (!band_state.min_frequency().has_value()) {
            fdf::error(
                "SetElementState({}): DynamicsElementState band_state does not specify min_frequency",
                element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          if (!band_state.max_frequency().has_value()) {
            fdf::error(
                "SetElementState({}): DynamicsElementState band_state does not specify max_frequency",
                element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          if (!band_state.threshold_db().has_value() || !isfinite(*band_state.threshold_db())) {
            fdf::error(
                "SetElementState({}): Dynamics band_state does not specify a finite threshold_db {}",
                element_id, *band_state.threshold_db());
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          fhasp::DynamicsSupportedControls ctrls =
              ele->type_specific()->dynamics()->supported_controls().value_or(
                  static_cast<fhasp::DynamicsSupportedControls>(0));
          if (band_state.threshold_type().has_value() &&
              !(ctrls & fhasp::DynamicsSupportedControls::kThresholdType)) {
            fdf::error(
                "SetElementState({}): Dynamics band_state specifies threshold_type (unsupported)",
                element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          if (!band_state.ratio().has_value() || !isfinite(*band_state.ratio())) {
            fdf::error(
                "SetElementState({}): Dynamics band_state does not specify a finite ratio {}",
                element_id, *band_state.ratio());
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          if (band_state.knee_width_db().has_value()) {
            if (!(ctrls & fhasp::DynamicsSupportedControls::kKneeWidth)) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state specifies knee_width_db (unsupported)",
                  element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (!isfinite(*band_state.knee_width_db()) || *band_state.knee_width_db() < 0) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state does not specify a positive finite knee_width_db {}",
                  element_id, *band_state.knee_width_db());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }
          if (band_state.attack().has_value()) {
            if (!(ctrls & fhasp::DynamicsSupportedControls::kAttack)) {
              fdf::error("SetElementState({}): Dynamics band_state specifies attack (unsupported)",
                         element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (*band_state.attack() < 0) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state does not specify a non-negative attack time {}",
                  element_id, *band_state.attack());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }
          if (band_state.release().has_value()) {
            if (!(ctrls & fhasp::DynamicsSupportedControls::kRelease)) {
              fdf::error("SetElementState({}): Dynamics band_state specifies release (unsupported)",
                         element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (*band_state.release() < 0) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state does not specify a non-negative release time {}",
                  element_id, *band_state.release());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }
          if (band_state.output_gain_db().has_value()) {
            if (!(ctrls & fhasp::DynamicsSupportedControls::kOutputGain)) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state specifies output_gain_db (unsupported)",
                  element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (!isfinite(*band_state.output_gain_db())) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state does not specify a finite output_gain_db {}",
                  element_id, *band_state.output_gain_db());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }
          if (band_state.input_gain_db().has_value()) {
            if (!(ctrls & fhasp::DynamicsSupportedControls::kInputGain)) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state specifies input_gain_db (unsupported)",
                  element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (!isfinite(*band_state.input_gain_db())) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state does not specify a finite input_gain_db {}",
                  element_id, *band_state.input_gain_db());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }
          if (band_state.level_type().has_value() &&
              !(ctrls & fhasp::DynamicsSupportedControls::kLevelType)) {
            fdf::error(
                "SetElementState({}): Dynamics band_state specifies level_type (unsupported)",
                element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          if (band_state.lookahead().has_value()) {
            if (!(ctrls & fhasp::DynamicsSupportedControls::kLookahead)) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state specifies lookahead (unsupported)",
                  element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (*band_state.lookahead() < 0) {
              fdf::error(
                  "SetElementState({}): Dynamics band_state does not specify a non-negative lookahead time {}",
                  element_id, *band_state.lookahead());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }
          if (band_state.linked_channels().has_value() &&
              !(ctrls & fhasp::DynamicsSupportedControls::kLinkedChannels)) {
            fdf::error(
                "SetElementState({}): Dynamics band_state specifies linked_channels (unsupported)",
                element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
        }
        // We passed every check so we can record this state change. First: type-specific changes.
        element_states_.at(element_id)
            .current.type_specific(fhasp::TypeSpecificElementState::WithDynamics(
                request.state().type_specific()->dynamics().value()));
      }
      break;
    case kEqualizerId:
      if (request.state().type_specific().has_value()) {
        // For this element, clients can specify type-specific state but it must be equalizer.
        if (!request.state().type_specific()->equalizer().has_value()) {
          // Error: type_specific state in this request does not match this element type.
          fdf::error(
              "SetElementState({}): TypeSpecificElementState does not match this element type",
              element_id);
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
        if (!request.state().type_specific()->equalizer()->band_states().has_value() ||
            request.state().type_specific()->equalizer()->band_states()->empty()) {
          fdf::error("SetElementState({}): Equalizer requires non-empty band_states", element_id);
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }

        for (const auto& band_state :
             *request.state().type_specific()->equalizer()->band_states()) {
          // id
          if (!band_state.id().has_value()) {
            fdf::error("SetElementState({}): Equalizer band_state does not specify band id",
                       element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }

          // type
          if (!band_state.type().has_value()) {
            fdf::error("SetElementState({}): Equalizer band_state does not specify band type",
                       element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          const auto& controls = *ele->type_specific()->equalizer()->supported_controls();
          constexpr fhasp::EqualizerSupportedControls kIncapable =
              fhasp::EqualizerSupportedControls(0);
          bool capable = false;
          switch (*band_state.type()) {
            case fhasp::EqualizerBandType::kPeak:
              capable =
                  (controls & fhasp::EqualizerSupportedControls::kSupportsTypePeak) != kIncapable;
              break;
            case fhasp::EqualizerBandType::kLowShelf:
              capable = (controls & fhasp::EqualizerSupportedControls::kSupportsTypeLowShelf) !=
                        kIncapable;
              break;
            case fhasp::EqualizerBandType::kHighShelf:
              capable = (controls & fhasp::EqualizerSupportedControls::kSupportsTypeHighShelf) !=
                        kIncapable;
              break;
            case fhasp::EqualizerBandType::kNotch:
              capable =
                  (controls & fhasp::EqualizerSupportedControls::kSupportsTypeNotch) != kIncapable;
              break;
            case fhasp::EqualizerBandType::kLowCut:
              capable =
                  (controls & fhasp::EqualizerSupportedControls::kSupportsTypeLowCut) != kIncapable;
              break;
            case fhasp::EqualizerBandType::kHighCut:
              capable = (controls & fhasp::EqualizerSupportedControls::kSupportsTypeHighCut) !=
                        kIncapable;
              break;
            default:
              capable = false;
              fdf::error("SetElementState({}): Equalizer band_state unknown band_type enum {}",
                         element_id, static_cast<uint64_t>(*band_state.type()));
              break;
          }
          if (!capable) {
            fdf::error("SetElementState({}): Equalizer band_state type {} not supported",
                       element_id, static_cast<uint64_t>(*band_state.type()));
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }

          // frequency
          if (!band_state.frequency().has_value()) {
            fdf::error("SetElementState({}): Equalizer band_state does not specify frequency",
                       element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }
          uint32_t current_frequency = 0;
          if (!element_states_.at(element_id).current.IsEmpty()) {
            const auto& curr_band_states =
                *element_states_.at(element_id).current.type_specific()->equalizer()->band_states();
            for (const auto& curr_band_state : curr_band_states) {
              if (*curr_band_state.id() == *band_state.id()) {
                current_frequency = *curr_band_state.frequency();
                break;
              }
            }
          }
          if (*band_state.frequency() != current_frequency &&
              !(controls & fhasp::EqualizerSupportedControls::kCanControlFrequency)) {
            fdf::error(
                "SetElementState({}): Equalizer band_state specifies frequency change (unsupported)",
                element_id);
            completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
            return;
          }

          // Q
          if (band_state.q().has_value()) {
            if (!(controls & fhasp::EqualizerSupportedControls::kCanControlQ)) {
              fdf::error("SetElementState({}): Equalizer band_state specifies Q (unsupported)",
                         element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (!isfinite(*band_state.q())) {
              fdf::error("SetElementState({}): Equalizer band_state Q {} must be finite",
                         element_id, *band_state.q());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }

          // gain_db
          if (band_state.gain_db().has_value()) {
            if (*band_state.type() != fhasp::EqualizerBandType::kPeak &&
                *band_state.type() != fhasp::EqualizerBandType::kLowShelf &&
                *band_state.type() != fhasp::EqualizerBandType::kHighShelf) {
              fdf::error(
                  "SetElementState({}): Equalizer band_state specifies gain_db (unsupported)",
                  element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
            if (!isfinite(*band_state.gain_db())) {
              fdf::error("SetElementState({}): Equalizer band_state gain_db {} must be finite",
                         element_id, *band_state.gain_db());
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          } else {
            if (*band_state.type() != fhasp::EqualizerBandType::kPeak &&
                *band_state.type() != fhasp::EqualizerBandType::kLowShelf &&
                *band_state.type() != fhasp::EqualizerBandType::kHighShelf) {
              fdf::error(
                  "SetElementState({}): Equalizer band_state does not specify gain_db (required)",
                  element_id);
              completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            }
          }

          // enabled
          // Nothing to validate (can always be set/cleared/omitted)
        }

        // We passed every check so we can record the state change. First: type-specific changes.
        element_states_.at(element_id)
            .current.type_specific(
                fhasp::TypeSpecificElementState::WithEqualizer(fhasp::EqualizerElementState{{
                    .band_states =
                        std::move(*request.state().type_specific()->equalizer()->band_states()),
                }}));
      }
      break;
    case kVendorSpecificId:
      if (request.state().type_specific().has_value()) {
        // For this element, clients can specify type-specific state but it must be vendor-specific.
        if (!request.state().type_specific()->vendor_specific().has_value()) {
          // Error: type_specific state in this request does not match this element type.
          fdf::error(
              "SetElementState({}): TypeSpecificElementState does not match this element type",
              element_id);
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
      }
      break;
    case kGainId:
      if (request.state().type_specific().has_value()) {
        // For this element, clients can specify type-specific state but it must be gain-specific.
        if (!request.state().type_specific()->gain().has_value()) {
          // Error: type_specific state in this request does not match this element type.
          fdf::error(
              "SetElementState({}): TypeSpecificElementState does not match this element type",
              element_id);
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
        // Error: SetElementState value is missing or non-finite.
        if (!request.state().type_specific()->gain()->gain().has_value() ||
            !std::isfinite(*request.state().type_specific()->gain()->gain())) {
          fdf::error("SetElementState({}): Gain requires a finite gain", element_id);
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
        // Error: SetElementState value is outside the [min,max] bounds.
        if (*request.state().type_specific()->gain()->gain() <
                *element_map_[element_id]->type_specific()->gain()->min_gain() ||
            *request.state().type_specific()->gain()->gain() >
                *element_map_[element_id]->type_specific()->gain()->max_gain()) {
          fdf::error("SetElementState({}): Gain {} is outside the allowed range [{}, {}]",
                     element_id, *request.state().type_specific()->gain()->gain(),
                     *element_map_[element_id]->type_specific()->gain()->min_gain(),
                     *element_map_[element_id]->type_specific()->gain()->max_gain());
          completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }

        // We passed every check so we can record this state change. First: type-specific changes.
        element_states_.at(element_id)
            .current.type_specific(fhasp::TypeSpecificElementState::WithGain(
                request.state().type_specific()->gain().value()));
      }
      break;
    default:
      // Error: we don't recognize this element_id.
      fdf::error("SetElementState({}) unknown element_id", element_id);
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
  }

  // All error cases have exited. Record the non-type-specific state changes, if any.
  if (request.state().started().has_value()) {
    element_states_.at(element_id).current.started() = request.state().started();
  }
  if (request.state().bypassed().has_value()) {
    element_states_.at(element_id).current.bypassed() = request.state().bypassed();
  }
  if (request.state().vendor_specific_data().has_value()) {
    fdf::warn("SetElementState({}): ignoring {} bytes of vendor_specific_data (unsupported)",
              element_id, request.state().vendor_specific_data()->size());
  }

  completer.Reply(zx::ok());
  MaybeCompleteWatchElementState(element_id);
}

// Immediately return the state of this element, if it has changed since last time this was called.
// Otherwise, pend this call until the state DOES change.
void VirtualAudioComposite::WatchElementState(WatchElementStateRequest& request,
                                              WatchElementStateCompleter::Sync& completer) {
  if (!element_states_.contains(request.processing_element_id())) {
    fdf::error("WatchElementState({}) unknown element_id. Unbinding",
               request.processing_element_id());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (element_states_.at(request.processing_element_id()).completer.has_value()) {
    // The client called WatchElementState when another hanging get was pending for the same id.
    fdf::error("WatchElementState({}) called while previous call was pending. Unbinding",
               request.processing_element_id());
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  element_states_.at(request.processing_element_id()).completer = completer.ToAsync();
  MaybeCompleteWatchElementState(request.processing_element_id());
}

// WatchElementState or SetElementState were called for this element (or it changed state for some
// other reason). If there is a pending WatchElementState to complete, do so.
void VirtualAudioComposite::MaybeCompleteWatchElementState(fhasp::ElementId element_id) {
  if (!element_states_.at(element_id).completer.has_value()) {
    fdf::debug("We don't have a completer, so we can't complete this");
    return;
  }
  const auto& prev = element_states_.at(element_id).last_notified;
  const auto& curr = element_states_.at(element_id).current;
  if (prev.has_value() && prev->type_specific() == curr.type_specific() &&
      prev->started().value_or(true) == curr.started().value_or(true) &&
      prev->bypassed().value_or(false) == curr.bypassed().value_or(false)) {
    fdf::debug("The value is unchanged, so we won't complete this");
    return;
  }

  auto completer = std::move(element_states_.at(element_id).completer);
  element_states_.at(element_id).completer.reset();
  element_states_.at(element_id).last_notified = element_states_.at(element_id).current;
  completer->Reply(element_states_.at(element_id).current);
}

// Driver doesn't support a new SignalProcessing method. Complain loudly but don't disconnect, since
// this test fixture might be used with a client that is built with a newer SDK version.
void VirtualAudioComposite::handle_unknown_method(
    fidl::UnknownMethodMetadata<fhasp::SignalProcessing> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("VirtualAudioComposite::handle_unknown_method (SignalProcessing) ordinal {}",
             metadata.method_ordinal);
}

// Driver doesn't support a new Composite method. Complain loudly but don't disconnect, since
// this test fixture might be used with a client that is built with a newer SDK version.
void VirtualAudioComposite::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::Composite> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("VirtualAudioComposite::handle_unknown_method (Composite) ordinal {}",
             metadata.method_ordinal);
}

void VirtualAudioComposite::Serve(
    fidl::ServerEnd<fuchsia_hardware_audio::CompositeConnector> server) {
  connector_bindings_.AddBinding(dispatcher_, std::move(server), this, fidl::kIgnoreBindingClosure);
}

void VirtualAudioComposite::Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) {
  if (composite_binding_.has_value()) {
    fdf::error("Composite already bound");
    std::move(request.composite_protocol()).Close(ZX_ERR_ALREADY_BOUND);
    return;
  }
  composite_binding_.emplace(dispatcher_, std::move(request.composite_protocol()), this,
                             [this](auto info) { composite_binding_.reset(); });
}

void VirtualAudioComposite::GetGain(GetGainCompleter::Sync& completer) {
  completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNotSupported));
}

void VirtualAudioComposite::SetNotificationFrequency(
    SetNotificationFrequencyRequest& request, SetNotificationFrequencyCompleter::Sync& completer) {
  completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNotSupported));
}

void VirtualAudioComposite::GetPosition(GetPositionCompleter::Sync& completer) {
  completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNotSupported));
}

void VirtualAudioComposite::ChangePlugState(ChangePlugStateRequest& request,
                                            ChangePlugStateCompleter::Sync& completer) {
  completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNotSupported));
}

void VirtualAudioComposite::AdjustClockRate(AdjustClockRateRequest& request,
                                            AdjustClockRateCompleter::Sync& completer) {
  completer.Reply(fit::error(fuchsia_virtualaudio::Error::kNotSupported));
}

}  // namespace virtual_audio
