// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-codec.h"

#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <fidl/fuchsia.virtualaudio/cpp/wire.h>
#include <lib/ddk/debug.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <audio-proto-utils/format-utils.h>
#include <fbl/algorithm.h>

namespace virtual_audio {

// static
int VirtualAudioCodec::instance_count_ = 0;

// static
fuchsia_virtualaudio::Configuration VirtualAudioCodec::GetDefaultConfig(
    std::optional<bool> is_input) {
  fuchsia_virtualaudio::Configuration config = {};
  std::string device_name = "Virtual Audio Codec Device ";
  if (is_input.has_value()) {
    device_name += (*is_input ? "(input)" : "(output)");
  } else {
    device_name += "(no direction)";
  }
  config.device_name(device_name);
  config.manufacturer_name("Fuchsia Virtual Audio Group");
  config.product_name("Virgil v2, a Virtual Volume Vessel");
  config.unique_id({{
      1,
      2,
      3,
      4,
      5,
      6,
      7,
      8,
      9,
      10,
      11,
      12,
      13,
      14,
      15,
      0,
  }});

  // Driver type is Codec.
  fuchsia_virtualaudio::Codec codec = {};
  codec.is_input(is_input);
  codec.plug_properties() = {{
      .plug_state = fuchsia_hardware_audio::PlugState{{
          .plugged = true,
          .plug_state_time = 0,
      }},
      .plug_detect_capabilities = fuchsia_hardware_audio::PlugDetectCapabilities::kHardwired,
  }};

  // Codec interconnect (to/from a DAI).
  fuchsia_virtualaudio::DaiInterconnect dai_interconnect = {};

  // By default, expose a single Codec format: 48kHz I2S (2 channels, 16-in-32, 8-byte frames).
  fuchsia_hardware_audio::DaiSupportedFormats dai_format_set = {};
  dai_format_set.number_of_channels(std::vector<uint32_t>{2});
  dai_format_set.sample_formats(std::vector{fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned});
  dai_format_set.frame_formats(
      std::vector{fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
          fuchsia_hardware_audio::DaiFrameFormatStandard::kI2S)});
  dai_format_set.frame_rates(std::vector<uint32_t>{48'000});
  dai_format_set.bits_per_slot(std::vector<uint8_t>{32});
  dai_format_set.bits_per_sample(std::vector<uint8_t>{16});

  dai_interconnect.dai_supported_formats(
      std::optional<std::vector<fuchsia_hardware_audio::DaiSupportedFormats>>{
          std::in_place,
          {
              dai_format_set,
          },
      });
  codec.dai_interconnect(std::move(dai_interconnect));
  config.device_specific() = fuchsia_virtualaudio::DeviceSpecific::WithCodec(std::move(codec));

  return config;
}

VirtualAudioCodec::VirtualAudioCodec(fuchsia_virtualaudio::Configuration config,
                                     std::weak_ptr<VirtualAudioDevice> owner, zx_device_t* parent,
                                     fit::closure on_shutdown)
    : VirtualAudioCodecDeviceType(parent),
      VirtualAudioDriver(std::move(on_shutdown)),
      parent_(std::move(owner)),
      config_(std::move(config)) {
  ddk_proto_id_ = ZX_PROTOCOL_CODEC;
  sprintf(instance_name_, "virtual-audio-codec-%d", instance_count_++);
  zx_status_t status = DdkAdd(ddk::DeviceAddArgs(instance_name_));
  ZX_ASSERT_MSG(status == ZX_OK, "DdkAdd failed");

  if (config.device_specific() && config.device_specific()->codec() &&
      config.device_specific()->codec()->plug_properties() &&
      config.device_specific()->codec()->plug_properties()->plug_state()) {
    plug_state_ = *config.device_specific()->codec()->plug_properties()->plug_state();
  }
}

VirtualAudioCodec::~VirtualAudioCodec() {
  if (codec_binding_.has_value()) {
    codec_binding_->Unbind();
  }
}

void VirtualAudioCodec::ResetCodecState() {
  should_return_plug_state_ = true;
  watch_plug_state_completer_.reset();
  connected_ = false;
  codec_binding_.reset();
}

void VirtualAudioCodec::Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) {
  if (connected_) {
    request->codec_protocol.Close(ZX_ERR_ALREADY_BOUND);
    return;
  }
  connected_ = true;
  codec_binding_ = fidl::BindServer(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(request->codec_protocol), this,
      [weak = weak_from_this()](VirtualAudioCodec*, fidl::UnbindInfo,
                                fidl::ServerEnd<fuchsia_hardware_audio::Codec>) {
        if (auto self = weak.lock()) {
          static_cast<VirtualAudioCodec*>(self.get())->ResetCodecState();
        }
      });
}

// FIDL natural C++ methods for fuchsia.hardware.audio.Codec.
void VirtualAudioCodec::GetHealthState(GetHealthStateCompleter::Sync& completer) {
  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  completer.Reply(fuchsia_hardware_audio::HealthState{}.healthy(true));
}

void VirtualAudioCodec::SignalProcessingConnect(SignalProcessingConnectRequest& request,
                                                SignalProcessingConnectCompleter::Sync& completer) {
  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  request.protocol().Close(ZX_ERR_NOT_SUPPORTED);
}

void VirtualAudioCodec::GetProperties(
    fidl::Server<fuchsia_hardware_audio::Codec>::GetPropertiesCompleter::Sync& completer) {
  auto parent = parent_.lock();
  ZX_ASSERT(parent);

  fidl::Arena arena;
  fuchsia_hardware_audio::CodecProperties properties;
  if (config_.device_specific() && config_.device_specific()->codec() &&
      config_.device_specific()->codec()->is_input()) {
    properties.is_input(*config_.device_specific()->codec()->is_input());
  }
  if (config_.manufacturer_name()) {
    properties.manufacturer(*config_.manufacturer_name());
  }
  if (config_.product_name()) {
    properties.product(*config_.product_name());
  }
  if (config_.unique_id()) {
    properties.unique_id() = *config_.unique_id();
  }
  if (config_.device_specific() && config_.device_specific()->codec() &&
      config_.device_specific()->codec()->plug_properties() &&
      config_.device_specific()->codec()->plug_properties()->plug_detect_capabilities()) {
    properties.plug_detect_capabilities() =
        config_.device_specific()->codec()->plug_properties()->plug_detect_capabilities();
  }
  completer.Reply(std::move(properties));
}

void VirtualAudioCodec::GetDaiFormats(GetDaiFormatsCompleter::Sync& completer) {
  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  std::vector<fuchsia_hardware_audio::DaiSupportedFormats> formats;
  if (config_.device_specific() && config_.device_specific()->codec() &&
      config_.device_specific()->codec()->dai_interconnect() &&
      config_.device_specific()->codec()->dai_interconnect()->dai_supported_formats()) {
    formats = *config_.device_specific()->codec()->dai_interconnect()->dai_supported_formats();
  }
  completer.Reply(zx::ok(std::move(formats)));
}

void VirtualAudioCodec::SetDaiFormat(SetDaiFormatRequest& request,
                                     SetDaiFormatCompleter::Sync& completer) {
  auto parent = parent_.lock();
  ZX_ASSERT(parent);
  fuchsia_hardware_audio::DaiFormat format = request.format();
  if (format.frame_rate() > 192000) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  std::vector<fuchsia_hardware_audio::DaiSupportedFormats> supported_formats;
  if (config_.device_specific() && config_.device_specific()->codec() &&
      config_.device_specific()->codec()->dai_interconnect() &&
      config_.device_specific()->codec()->dai_interconnect()->dai_supported_formats()) {
    supported_formats =
        *config_.device_specific()->codec()->dai_interconnect()->dai_supported_formats();
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
      fuchsia_hardware_audio::CodecFormatInfo codec_info{{
          .external_delay = 123,
          .turn_on_delay = 234,
          .turn_off_delay = 345,
      }};
      completer.Reply(zx::ok(codec_info));
      return;
    }
  }
  completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
}

void VirtualAudioCodec::Start(StartCompleter::Sync& completer) {
  completer.Reply(zx::clock::get_monotonic().get());
}

void VirtualAudioCodec::Stop(StopCompleter::Sync& completer) {
  completer.Reply(zx::clock::get_monotonic().get());
}

void VirtualAudioCodec::Reset(ResetCompleter::Sync& completer) { completer.Reply(); }

void VirtualAudioCodec::WatchPlugState(WatchPlugStateCompleter::Sync& completer) {
  if (should_return_plug_state_) {
    should_return_plug_state_ = false;
    completer.Reply(plug_state_);
    return;
  }

  if (watch_plug_state_completer_) {
    completer.Close(ZX_ERR_INVALID_ARGS);
    watch_plug_state_completer_.reset();
    return;
  }
  watch_plug_state_completer_ = completer.ToAsync();
}

void VirtualAudioCodec::PlugStateChanged(const fuchsia_hardware_audio::PlugState& plug_state) {
  plug_state_ = plug_state;

  if (watch_plug_state_completer_) {
    should_return_plug_state_ = false;
    watch_plug_state_completer_->Reply(plug_state_);
    return;
  }
  should_return_plug_state_ = true;
}

void VirtualAudioCodec::ShutdownAsync() {
  if (codec_binding_.has_value()) {
    codec_binding_->Unbind();
  }
  DdkAsyncRemove();
}

void VirtualAudioCodec::DdkRelease() {
  if (codec_binding_.has_value()) {
    codec_binding_->Unbind();
  }
  OnShutdown();
}

}  // namespace virtual_audio
