// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/validate.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <unordered_set>

#include "src/media/audio/services/device_registry/format_utils.h"
#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/signal_processing_utils.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

namespace {

/////////////////////////////////////////////////////
// Utility functions

// In the enclosed vector<ChannelSet>, how many num_channels equal 'channel_count_to_match'?
size_t CountChannelMatches(const std::vector<fha::ChannelSet>& channel_sets,
                           size_t channel_count_to_match) {
  return std::count_if(channel_sets.begin(), channel_sets.end(),
                       [channel_count_to_match](const fha::ChannelSet& channel_set) {
                         return channel_set.attributes()->size() == channel_count_to_match;
                       });
}

}  // namespace

bool ClientIsValidForDeviceType(const fad::DeviceType& device_type,
                                const fad::DriverClient& driver_client) {
  switch (driver_client.Which()) {
    case fad::DriverClient::Tag::kCodec:
      return (device_type == fad::DeviceType::kCodec);
    case fad::DriverClient::Tag::kComposite:
      return (device_type == fad::DeviceType::kComposite);
    default:
      return false;
  }
}

bool ValidatePcmSupportedFormats(const fha::PcmSupportedFormats& pcm_format_set,
                                 bool is_packet_stream) {
  const uint32_t kMinFrameRate =
      is_packet_stream ? kMinSupportedPacketStreamFrameRate : kMinSupportedRingBufferFrameRate;
  const uint32_t kMaxFrameRate =
      is_packet_stream ? kMaxSupportedPacketStreamFrameRate : kMaxSupportedRingBufferFrameRate;

  // Frame rates
  if (!pcm_format_set.frame_rates().has_value() || pcm_format_set.frame_rates()->empty()) {
    FX_LOGS(WARNING) << "frame_rates[] is " << (pcm_format_set.frame_rates() ? "empty" : "absent");
    return false;
  }
  uint32_t prev_frame_rate = 0, max_supported_frame_rate = 0;
  for (const auto& rate : *pcm_format_set.frame_rates()) {
    if (rate < kMinFrameRate || rate > kMaxFrameRate) {
      FX_LOGS(WARNING) << "frame_rate (" << rate << ") out of range [" << kMinFrameRate << ","
                       << kMaxFrameRate << "] ";
      return false;
    }
    if (rate <= prev_frame_rate) {
      FX_LOGS(WARNING) << "frame_rate must be in ascending order: " << prev_frame_rate
                       << " was listed before " << rate;
      return false;
    }
    prev_frame_rate = rate;
    max_supported_frame_rate = std::max(max_supported_frame_rate, rate);
  }

  // Channel sets
  if (!pcm_format_set.channel_sets().has_value() || pcm_format_set.channel_sets()->empty()) {
    FX_LOGS(WARNING) << "channel_sets[] is "
                     << (pcm_format_set.channel_sets() ? "empty" : "absent");
    return false;
  }
  auto max_allowed_frequency = max_supported_frame_rate / 2;
  for (const fha::ChannelSet& chan_set : *pcm_format_set.channel_sets()) {
    if (!chan_set.attributes().has_value() || chan_set.attributes()->empty()) {
      FX_LOGS(WARNING) << "ChannelSet.attributes[] is "
                       << (chan_set.attributes() ? "empty" : "absent");
      return false;
    }
    if (CountChannelMatches(*pcm_format_set.channel_sets(), chan_set.attributes()->size()) > 1) {
      FX_LOGS(WARNING) << "channel-count must be unique across channel_sets: "
                       << chan_set.attributes()->size();
      return false;
    }
    for (const auto& attrib : *chan_set.attributes()) {
      if (attrib.min_frequency().has_value()) {
        if (*attrib.min_frequency() > max_allowed_frequency) {
          FX_LOGS(WARNING) << "ChannelAttributes.min_frequency (" << *attrib.min_frequency()
                           << ") out of range: " << "[0, " << max_allowed_frequency << "]";
          return false;
        }
        if (attrib.max_frequency().has_value() &&
            *attrib.min_frequency() > *attrib.max_frequency()) {
          FX_LOGS(WARNING) << "min_frequency (" << *attrib.min_frequency()
                           << ") cannot exceed max_frequency (" << *attrib.max_frequency() << ")";
          return false;
        }
      }
      if (attrib.max_frequency().has_value() && *attrib.max_frequency() > max_allowed_frequency) {
        FX_LOGS(WARNING) << "ChannelAttrib.max_frequency " << *attrib.max_frequency()
                         << " will be limited to " << max_allowed_frequency;
      }
    }
  }

  // Sample format
  if (!pcm_format_set.sample_formats().has_value() || pcm_format_set.sample_formats()->empty()) {
    FX_LOGS(WARNING) << "sample_formats[] is "
                     << (pcm_format_set.sample_formats() ? "empty" : "absent");
    return false;
  }
  const auto& sample_formats = *pcm_format_set.sample_formats();
  for (const auto& format : sample_formats) {
    if (CountFormatMatches(sample_formats, format) > 1) {
      FX_LOGS(WARNING) << "no duplicate SampleFormat values allowed: " << format;
      return false;
    }
  }

  // Bytes per sample
  if (!pcm_format_set.bytes_per_sample() || pcm_format_set.bytes_per_sample()->empty()) {
    FX_LOGS(WARNING) << "bytes_per_sample[] is "
                     << (pcm_format_set.bytes_per_sample() ? "empty" : "absent");
    return false;
  }
  uint8_t prev_bytes_per_sample = 0, max_bytes_per_sample = 0;
  for (const auto& bytes : *pcm_format_set.bytes_per_sample()) {
    if (CountFormatMatches(sample_formats, fha::SampleFormat::kPcmSigned) &&
        (bytes != 2 && bytes != 4)) {
      FX_LOGS(WARNING) << "bytes_per_sample (" << static_cast<uint16_t>(bytes)
                       << ") must be 2 or 4 for PCM_SIGNED format";
      return false;
    }
    if (CountFormatMatches(sample_formats, fha::SampleFormat::kPcmFloat) &&
        (bytes != 4 && bytes != 8)) {
      FX_LOGS(WARNING) << "bytes_per_sample (" << static_cast<uint16_t>(bytes)
                       << ") must be 4 or 8 for PCM_FLOAT format";
      return false;
    }
    if (CountFormatMatches(sample_formats, fha::SampleFormat::kPcmUnsigned) && bytes != 1) {
      FX_LOGS(WARNING) << "bytes_per_sample (" << static_cast<uint16_t>(bytes)
                       << ") must be 1 for PCM_UNSIGNED format";
      return false;
    }
    if (bytes <= prev_bytes_per_sample) {
      FX_LOGS(WARNING) << "bytes_per_sample must be in ascending order: "
                       << static_cast<uint16_t>(prev_bytes_per_sample) << " was listed before "
                       << static_cast<uint16_t>(bytes);
      return false;
    }
    prev_bytes_per_sample = bytes;
    // While testing sample-size, we determine max_bytes_per_sample for valid-bits checks.
    max_bytes_per_sample = std::max(max_bytes_per_sample, bytes);
  }

  // Valid bits per sample
  if (!pcm_format_set.valid_bits_per_sample().has_value() ||
      pcm_format_set.valid_bits_per_sample()->empty()) {
    FX_LOGS(WARNING) << "valid_bits_per_sample[] is "
                     << (pcm_format_set.valid_bits_per_sample() ? "empty" : "absent");
    return false;
  }
  uint8_t prev_valid_bits = 0;
  for (const auto& valid_bits : *pcm_format_set.valid_bits_per_sample()) {
    if (valid_bits == 0 || valid_bits > max_bytes_per_sample * 8) {
      FX_LOGS(WARNING) << "valid_bits_per_sample (" << static_cast<uint16_t>(valid_bits)
                       << ") out of range [1, " << max_bytes_per_sample * 8 << "]";
      return false;
    }
    if (valid_bits <= prev_valid_bits) {
      FX_LOGS(WARNING) << "valid_bits_per_sample must be in ascending order: "
                       << static_cast<uint16_t>(prev_valid_bits) << " was listed before "
                       << static_cast<uint16_t>(valid_bits);
      return false;
    }
    prev_valid_bits = valid_bits;
  }
  return true;
}

bool ValidateSupportedEncodings(const fha::SupportedEncodings& encoding_format_set) {
  // Frame rates
  if (!encoding_format_set.decoded_frame_rates().has_value() ||
      encoding_format_set.decoded_frame_rates()->empty()) {
    FX_LOGS(WARNING) << "ValidateSupportedEncodings: decoded_frame_rates[] is "
                     << (encoding_format_set.decoded_frame_rates() ? "empty" : "absent");
    return false;
  }
  // While testing frame_rates, we can determine max_supported_frame_rate.
  uint32_t prev_frame_rate = 0, max_supported_frame_rate = 0;
  for (const auto& rate : *encoding_format_set.decoded_frame_rates()) {
    if (rate < kMinSupportedPacketStreamFrameRate || rate > kMaxSupportedPacketStreamFrameRate) {
      FX_LOGS(WARNING) << "ValidateSupportedEncodings: frame_rate (" << rate << ") out of range ["
                       << kMinSupportedPacketStreamFrameRate << ","
                       << kMaxSupportedPacketStreamFrameRate << "] ";
      return false;
    }
    // Checking for "strictly ascending" also eliminates duplicate entries.
    if (rate <= prev_frame_rate) {
      FX_LOGS(WARNING) << "ValidateSupportedEncodings: frame_rate must be in ascending order: "
                       << prev_frame_rate << " was listed before " << rate;
      return false;
    }
    prev_frame_rate = rate;
    max_supported_frame_rate = std::max(max_supported_frame_rate, rate);
  }

  // Channel sets
  if (!encoding_format_set.decoded_channel_sets().has_value() ||
      encoding_format_set.decoded_channel_sets()->empty()) {
    FX_LOGS(WARNING) << "ValidateSupportedEncodings: decoded_channel_sets[] is "
                     << (encoding_format_set.decoded_channel_sets() ? "empty" : "absent");
    return false;
  }
  auto max_allowed_frequency = max_supported_frame_rate / 2;
  for (const fha::ChannelSet& chan_set : *encoding_format_set.decoded_channel_sets()) {
    if (!chan_set.attributes().has_value() || chan_set.attributes()->empty()) {
      FX_LOGS(WARNING) << "ValidateSupportedEncodings: ChannelSet.attributes[] is "
                       << (chan_set.attributes() ? "empty" : "absent");
      return false;
    }
    if (CountChannelMatches(*encoding_format_set.decoded_channel_sets(),
                            chan_set.attributes()->size()) > 1) {
      FX_LOGS(WARNING)
          << "ValidateSupportedEncodings: channel-count must be unique across channel_sets: "
          << chan_set.attributes()->size();
      return false;
    }
    for (const auto& attrib : *chan_set.attributes()) {
      if (attrib.min_frequency().has_value()) {
        if (*attrib.min_frequency() > max_allowed_frequency) {
          FX_LOGS(WARNING) << "ValidateSupportedEncodings: ChannelAttributes.min_frequency ("
                           << *attrib.min_frequency() << ") out of range: "
                           << "[0, " << max_allowed_frequency << "]";
          return false;
        }
        if (attrib.max_frequency().has_value() &&
            *attrib.min_frequency() > *attrib.max_frequency()) {
          FX_LOGS(WARNING) << "ValidateSupportedEncodings: min_frequency ("
                           << *attrib.min_frequency() << ") cannot exceed max_frequency ("
                           << *attrib.max_frequency() << ")";
          return false;
        }
      }

      if (attrib.max_frequency().has_value()) {
        if (*attrib.max_frequency() > max_allowed_frequency) {
          FX_LOGS(WARNING) << "ValidateSupportedEncodings: ChannelAttrib.max_frequency "
                           << *attrib.max_frequency() << " will be limited to "
                           << max_allowed_frequency;
        }
      }
    }
  }
  if (!encoding_format_set.encoding_types().has_value() ||
      encoding_format_set.encoding_types()->empty()) {
    FX_LOGS(WARNING) << "ValidateSupportedEncodings: encoding_types[] is "
                     << (encoding_format_set.encoding_types() ? "empty" : "absent");
    return false;
  }
  const auto& encodings = *encoding_format_set.encoding_types();
  return std::ranges::all_of(encodings, [&encodings](const auto& encoding) {
    if (std::ranges::count(encodings, encoding) > 1) {
      FX_LOGS(WARNING) << "ValidateSupportedEncodings: duplicate encoding_type found: " << encoding;
      return false;
    }
    return true;
  });
}

bool ValidateRingBufferFormatSets(
    const std::vector<fha::SupportedFormats2>& ring_buffer_format_sets) {
  LogRingBufferFormatSets(ring_buffer_format_sets);

  if (ring_buffer_format_sets.empty()) {
    FX_LOGS(WARNING) << "ValidateRingBufferFormatSets: ring_buffer_format_sets[] is empty";
    return false;
  }

  for (auto i = 0u; i < ring_buffer_format_sets.size(); ++i) {
    const auto& rb_format_set = ring_buffer_format_sets[i];
    if (rb_format_set.pcm_supported_formats()) {
      if (!ValidatePcmSupportedFormats(rb_format_set.pcm_supported_formats().value(),
                                       /*is_packet_stream=*/false)) {
        FX_LOGS(WARNING) << "ValidateRingBufferFormatSets: pcm_supported_formats at index " << i
                         << " is invalid";
        return false;
      }
    } else {
      FX_LOGS(WARNING)
          << "ValidateRingBufferFormatSets: unknown union variant (flexible union) at index " << i
          << " (tag " << static_cast<uint32_t>(rb_format_set.Which()) << ")";
      return false;
    }
  }

  return true;
}

bool ValidatePacketStreamFormatSets(const std::vector<fha::SupportedFormats2>& format_sets) {
  LogPacketStreamFormatSets(format_sets);
  if (format_sets.empty()) {
    FX_LOGS(WARNING) << "ValidatePacketStreamFormatSets: format_sets[] is empty";
    return false;
  }

  for (auto i = 0u; i < format_sets.size(); ++i) {
    const auto& ps_format_set = format_sets[i];
    if (ps_format_set.Which() == fha::SupportedFormats2::Tag::kPcmSupportedFormats) {
      bool valid = ValidatePcmSupportedFormats(ps_format_set.pcm_supported_formats().value(),
                                               /*is_packet_stream=*/true);
      if (!valid) {
        FX_LOGS(WARNING) << "ValidatePcmSupportedFormats: pcm_supported_formats at index " << i
                         << " is invalid";
        return false;
      }
    } else if (ps_format_set.Which() == fha::SupportedFormats2::Tag::kSupportedEncodings) {
      bool valid = ValidateSupportedEncodings(ps_format_set.supported_encodings().value());
      if (!valid) {
        FX_LOGS(WARNING) << "ValidateSupportedEncodings: supported_encodings at index " << i
                         << " is invalid";
        return false;
      }
    } else {
      FX_LOGS(WARNING)
          << "ValidatePacketStreamFormatSets: unknown union variant (flexible union) at index " << i
          << " (tag " << static_cast<uint32_t>(ps_format_set.Which()) << ")";
      return false;
    }
  }

  return true;
}

bool ValidatePacketStreamFormat(const fha::Format2& format) {
  if (format.Which() == fha::Format2::Tag::kPcmFormat) {
    return ValidatePcmFormat(format.pcm_format().value());
  }
  if (format.Which() == fha::Format2::Tag::kEncoding) {
    return ValidateEncoding(format.encoding().value());
  }
  return false;
}

bool ValidateCodecProperties(const fha::CodecProperties& codec_props,
                             std::optional<const fha::PlugState> plug_state) {
  ADR_LOG(kLogDeviceMethods);
  LogCodecProperties(codec_props);

  if ((codec_props.manufacturer().has_value() && codec_props.manufacturer()->empty()) ||
      (codec_props.product().has_value() && codec_props.product()->empty())) {
    FX_LOGS(WARNING) << __func__
                     << ": manufacturer and product, if present, must not be empty strings";
    return false;
  }

  if (!codec_props.plug_detect_capabilities()) {
    FX_LOGS(WARNING) << "Incomplete Codec/GetProperties response";
    return false;
  }

  // If we already have this device's PlugState, double-check against that.
  if (plug_state.has_value() && !*plug_state->plugged() &&
      *codec_props.plug_detect_capabilities() == fha::PlugDetectCapabilities::kHardwired) {
    FX_LOGS(WARNING) << "GetProperties reports HARDWIRED, but Codec reports as UNPLUGGED";
    return false;
  }

  return true;
}

bool ValidateDaiFormatSets(const std::vector<fha::DaiSupportedFormats>& dai_format_sets) {
  LogDaiFormatSets(dai_format_sets);

  if (dai_format_sets.empty()) {
    FX_LOGS(WARNING) << "GetDaiSupportedFormats: response is empty";
    return false;
  }

  for (auto i = 0u; i < dai_format_sets.size(); ++i) {
    const auto& dai_format_set = dai_format_sets[i];
    if (dai_format_set.number_of_channels().empty()) {
      FX_LOGS(WARNING) << "DaiSupportedFormats[" << i << "]: empty number_of_channels vector";
      return false;
    }
    uint32_t previous_chans = 0;
    for (const auto& chans : dai_format_set.number_of_channels()) {
      if (chans <= previous_chans || chans > fha::kMaxCountDaiSupportedNumberOfChannels) {
        FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats number_of_channels: " << chans;
        return false;
      }
      previous_chans = chans;
    }
    if (dai_format_set.sample_formats().empty()) {
      FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats: empty sample_formats vector";
      return false;
    }
    for (const auto& dai_sample_format : dai_format_set.sample_formats()) {
      if (std::count(dai_format_set.sample_formats().begin(), dai_format_set.sample_formats().end(),
                     dai_sample_format) > 1) {
        FX_LOGS(WARNING) << "Duplicate DaiSupportedFormats sample_format: " << dai_sample_format;
        return false;
      }
    }
    if (dai_format_set.frame_formats().empty()) {
      FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats: empty frame_formats vector";
      return false;
    }
    for (const auto& dai_frame_format : dai_format_set.frame_formats()) {
      if (std::count(dai_format_set.frame_formats().begin(), dai_format_set.frame_formats().end(),
                     dai_frame_format) > 1) {
        FX_LOGS(WARNING) << "Duplicate DaiSupportedFormats frame_format: " << dai_frame_format;
        return false;
      }
    }
    if (dai_format_set.frame_rates().empty()) {
      FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats: empty frame_rates vector";
      return false;
    }
    uint32_t previous_rate = 0;
    for (const auto& rate : dai_format_set.frame_rates()) {
      if (rate <= previous_rate || rate < kMinSupportedDaiFrameRate ||
          rate > kMaxSupportedDaiFrameRate) {
        FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats frame_rate: " << rate;
        return false;
      }
      previous_rate = rate;
    }
    if (dai_format_set.bits_per_slot().empty()) {
      FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats: empty bits_per_slot vector";
      return false;
    }
    uint32_t previous_bits_per_slot = 0;
    uint32_t max_bits_per_slot = 0;
    for (const auto& bits : dai_format_set.bits_per_slot()) {
      if (bits <= previous_bits_per_slot || bits == 0 || bits > kMaxSupportedDaiFormatBitsPerSlot) {
        FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats bits_per_slot: "
                         << static_cast<uint16_t>(bits);
        return false;
      }
      max_bits_per_slot = std::max<uint32_t>(bits, max_bits_per_slot);
      previous_bits_per_slot = bits;
    }
    if (dai_format_set.bits_per_sample().empty()) {
      FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats: empty bits_per_sample vector";
      return false;
    }
    uint32_t previous_bits_per_sample = 0;
    for (const auto& bits : dai_format_set.bits_per_sample()) {
      if (bits <= previous_bits_per_sample || bits > max_bits_per_slot || bits == 0) {
        FX_LOGS(WARNING) << "Non-compliant DaiSupportedFormats bits_per_sample: "
                         << static_cast<uint16_t>(bits);
        return false;
      }
      previous_bits_per_sample = bits;
    }
  }

  return true;
}

bool ValidateDaiFormat(const fha::DaiFormat& dai_format) {
  ADR_LOG(kLogDeviceMethods);
  LogDaiFormat(dai_format);

  if (dai_format.number_of_channels() == 0 ||
      dai_format.number_of_channels() > fha::kMaxCountDaiSupportedNumberOfChannels) {
    FX_LOGS(WARNING) << "Non-compliant DaiFormat number_of_channels: "
                     << dai_format.number_of_channels();
    return false;
  }

  if (dai_format.channels_to_use_bitmask() == 0) {
    FX_LOGS(WARNING) << "Non-compliant DaiFormat channels_to_use_bitmask: 0";
    return false;
  }
  if (dai_format.number_of_channels() < 64 &&
      (dai_format.channels_to_use_bitmask() >> dai_format.number_of_channels()) > 0) {
    FX_LOGS(WARNING) << "Non-compliant DaiFormat channels_to_use_bitmask: 0x" << std::hex
                     << dai_format.channels_to_use_bitmask() << " is too large for " << std::dec
                     << dai_format.number_of_channels() << " channels";
    return false;
  }

  switch (dai_format.sample_format()) {
    case fha::DaiSampleFormat::kPdm:
    case fha::DaiSampleFormat::kPcmSigned:
    case fha::DaiSampleFormat::kPcmUnsigned:
    case fha::DaiSampleFormat::kPcmFloat:
      break;
    default:
      FX_LOGS(WARNING) << "Non-compliant DaiFormat sample_format: UNKNOWN enum";
      return false;
  }

  if (!dai_format.frame_format().frame_format_custom().has_value() &&
      !dai_format.frame_format().frame_format_standard().has_value()) {
    FX_LOGS(WARNING) << "Non-compliant DaiFormat frame_format: UNKNOWN union enum";
    return false;
  }
  switch (dai_format.frame_format().Which()) {
    case fha::DaiFrameFormat::Tag::kFrameFormatStandard:
    case fha::DaiFrameFormat::Tag::kFrameFormatCustom:
      break;
    default:
      FX_LOGS(WARNING) << "Non-compliant DaiFormat frame_format: UNKNOWN union tag";
      return false;
  }

  if (dai_format.frame_rate() < kMinSupportedDaiFrameRate ||
      dai_format.frame_rate() > kMaxSupportedDaiFrameRate) {
    FX_LOGS(WARNING) << "Non-compliant DaiFormat frame_rate: " << dai_format.frame_rate();
    return false;
  }

  if (dai_format.bits_per_slot() == 0 ||
      dai_format.bits_per_slot() > kMaxSupportedDaiFormatBitsPerSlot) {
    FX_LOGS(WARNING) << "Non-compliant DaiFormat bits_per_slot: "
                     << static_cast<uint16_t>(dai_format.bits_per_slot());
    return false;
  }

  if (dai_format.bits_per_sample() == 0 ||
      dai_format.bits_per_sample() > dai_format.bits_per_slot()) {
    FX_LOGS(WARNING) << "Non-compliant DaiFormat bits_per_sample: "
                     << static_cast<uint16_t>(dai_format.bits_per_sample());
    return false;
  }
  return true;
}

bool ValidateCodecFormatInfo(const fha::CodecFormatInfo& format_info) {
  ADR_LOG(kLogDeviceMethods);
  LogCodecFormatInfo(format_info);

  if (format_info.external_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "Invalid Codec::SetDaiFormat response - external_delay cannot be negative";
    return false;
  }
  if (format_info.turn_on_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "Invalid Codec::SetDaiFormat response - turn_on_delay cannot be negative";
    return false;
  }
  if (format_info.turn_off_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "Invalid Codec::SetDaiFormat response - turn_off_delay cannot be negative";
    return false;
  }
  return true;
}

bool ValidateCompositeProperties(const fha::CompositeProperties& composite_props) {
  LogCompositeProperties(composite_props);

  if (!composite_props.clock_domain().has_value()) {
    FX_LOGS(WARNING) << "Incomplete Composite/GetProperties response";
    return false;
  }

  if ((composite_props.manufacturer().has_value() && composite_props.manufacturer()->empty()) ||
      (composite_props.product().has_value() && composite_props.product()->empty())) {
    FX_LOGS(WARNING) << __func__ << ": manufacturer and product, if present, must not be empty";
    return false;
  }

  return true;
}

bool ValidatePlugState(const fha::PlugState& plug_state,
                       std::optional<fha::PlugDetectCapabilities> plug_detect_capabilities) {
  LogPlugState(plug_state);

  if (!plug_state.plugged().has_value() || !plug_state.plug_state_time()) {
    FX_LOGS(WARNING) << "Incomplete Codec/WatchPlugState response: required field missing";
    return false;
  }

  int64_t now = zx::clock::get_monotonic().get();
  if (*plug_state.plug_state_time() > now) {
    FX_LOGS(WARNING) << "PlugState.plug_state_time (" << *plug_state.plug_state_time()
                     << ") is in the future";
    return false;
  }

  // If we already have this device's PlugDetectCapabilities, double-check against those.
  if (plug_detect_capabilities.has_value()) {
    if (*plug_detect_capabilities == fha::PlugDetectCapabilities::kHardwired &&
        !plug_state.plugged().value_or(true)) {
      FX_LOGS(WARNING) << "Device reports as HARDWIRED, but PlugState.plugged is false";
      return false;
    }
  }

  return true;
}

// Validate only DeviceInfo-specific aspects. For example, don't re-validate format correctness.
bool ValidateDeviceInfo(const fad::Info& device_info) {
  LogDeviceInfo(device_info);

  // Validate top-level required members.
  if (!device_info.token_id().has_value() || !device_info.device_type().has_value() ||
      !device_info.device_name().has_value() || device_info.device_name()->empty()) {
    FX_LOGS(WARNING) << __func__ << ": incomplete DeviceInfo instance";
    return false;
  }
  // These strings must not be empty, if present.
  if ((device_info.manufacturer().has_value() && device_info.manufacturer()->empty()) ||
      (device_info.product().has_value() && device_info.product()->empty())) {
    FX_LOGS(WARNING) << __func__
                     << ": manufacturer and product, if present, must not be empty strings";
    return false;
  }
  // These vectors must not be empty, if present.
  if ((device_info.signal_processing_elements().has_value() &&
       device_info.signal_processing_elements()->empty()) ||
      (device_info.signal_processing_topologies().has_value() &&
       device_info.signal_processing_topologies()->empty())) {
    FX_LOGS(WARNING)
        << __func__
        << ": signal_processing elements/topologies, if present, must have at least one entry";
    return false;
  }
  switch (*device_info.device_type()) {
    case fad::DeviceType::kCodec:
      if (!device_info.dai_format_sets().has_value() || device_info.dai_format_sets()->empty() ||
          !device_info.plug_detect_caps().has_value()) {
        FX_LOGS(WARNING) << __func__ << ": incomplete DeviceInfo instance";
        return false;
      }
      if (device_info.ring_buffer_format_sets().has_value() ||
          device_info.clock_domain().has_value()) {
        FX_LOGS(WARNING) << __func__ << ": invalid DeviceInfo fields are populated";
        return false;
      }
      break;
    case fad::DeviceType::kComposite:
      if (!device_info.clock_domain().has_value() ||
          !device_info.signal_processing_elements().has_value() ||
          !device_info.signal_processing_topologies().has_value()) {
        FX_LOGS(WARNING) << __func__ << ": incomplete DeviceInfo instance";
        return false;
      }
      if (device_info.is_input().has_value() || device_info.plug_detect_caps().has_value()) {
        FX_LOGS(WARNING) << __func__ << ": invalid DeviceInfo fields are populated";
        return false;
      }
      break;
    default:
      FX_LOGS(WARNING) << __func__ << ": unsupported DeviceType: " << device_info.device_type();
      return false;
  }

  return true;
}

bool ValidateRingBufferProperties(const fha::RingBufferProperties& rb_props) {
  ADR_LOG(kLogDeviceMethods);
  LogRingBufferProperties(rb_props);

  if (!rb_props.needs_cache_flush_or_invalidate().has_value()) {
    FX_LOGS(WARNING) << "RingBufferProperties.needs_cache_flush_or_invalidate is missing";
    return false;
  }
  if (rb_props.turn_on_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "RingBufferProperties.turn_on_delay (" << *rb_props.turn_on_delay()
                     << ") is negative";
    return false;
  }
  if (!rb_props.driver_transfer_bytes().has_value()) {
    FX_LOGS(WARNING) << "RingBufferProperties.driver_transfer_bytes is missing";
    return false;
  }
  return true;
}

bool ValidatePcmFormat(const fha::PcmFormat& pcm_format) {
  LogPcmFormat(pcm_format);
  if (pcm_format.number_of_channels() == 0) {
    FX_LOGS(WARNING) << "PcmFormat number_of_channels is too low";
    return false;
  }
  // Is there an upper limit on channels?

  if (pcm_format.bytes_per_sample() == 0) {
    FX_LOGS(WARNING) << "PcmFormat bytes_per_sample is too low";
    return false;
  }
  if (pcm_format.sample_format() == fha::SampleFormat::kPcmUnsigned &&
      pcm_format.bytes_per_sample() > sizeof(uint8_t)) {
    FX_LOGS(WARNING) << "PcmFormat bytes_per_sample is too high";
    return false;
  }
  if (pcm_format.sample_format() == fha::SampleFormat::kPcmSigned &&
      pcm_format.bytes_per_sample() > sizeof(uint32_t)) {
    FX_LOGS(WARNING) << "PcmFormat bytes_per_sample is too high";
    return false;
  }
  if (pcm_format.sample_format() == fha::SampleFormat::kPcmFloat &&
      pcm_format.bytes_per_sample() > sizeof(double)) {
    FX_LOGS(WARNING) << "PcmFormat bytes_per_sample is too high";
    return false;
  }

  if (pcm_format.valid_bits_per_sample() == 0) {
    FX_LOGS(WARNING) << "PcmFormat valid_bits_per_sample is too low";
    return false;
  }
  auto bytes_per_sample = pcm_format.bytes_per_sample();
  if (pcm_format.valid_bits_per_sample() > bytes_per_sample * 8) {
    FX_LOGS(WARNING) << "PcmFormat valid_bits_per_sample ("
                     << static_cast<uint16_t>(pcm_format.valid_bits_per_sample())
                     << ") cannot exceed bytes_per_sample ("
                     << static_cast<uint16_t>(bytes_per_sample) << ") * 8";
    return false;
  }

  if (pcm_format.frame_rate() > kMaxSupportedRingBufferFrameRate ||
      pcm_format.frame_rate() < kMinSupportedRingBufferFrameRate) {
    FX_LOGS(WARNING) << "PcmFormat frame rate (" << pcm_format.frame_rate()
                     << ") must be within range [" << kMinSupportedRingBufferFrameRate << ", "
                     << kMaxSupportedRingBufferFrameRate << "]";
    return false;
  }

  return true;
}

bool ValidateEncoding(const fha::Encoding& encoding) {
  LogEncoding(encoding);
  if (!encoding.decoded_channel_count().has_value() || *encoding.decoded_channel_count() == 0) {
    FX_LOGS(WARNING) << "EncodedFormat decoded_channel_count is missing or zero";
    return false;
  }
  if (encoding.decoded_frame_rate().has_value()) {
    if (*encoding.decoded_frame_rate() > kMaxSupportedPacketStreamFrameRate ||
        *encoding.decoded_frame_rate() < kMinSupportedPacketStreamFrameRate) {
      FX_LOGS(WARNING) << "EncodedFormat decoded_frame_rate (" << *encoding.decoded_frame_rate()
                       << ") is out of range";
      return false;
    }
  }
  if (!encoding.average_encoding_bitrate().has_value()) {
    FX_LOGS(WARNING) << "EncodedFormat average_encoding_bitrate is missing";
    return false;
  }

  // Calculate max possible bitrate based on uncompressed stream assuming worst-case of 8
  // bytes/sample.
  uint32_t frame_rate = encoding.decoded_frame_rate().value_or(kMaxSupportedPacketStreamFrameRate);
  uint32_t channels = *encoding.decoded_channel_count();
  uint32_t max_bitrate = channels * frame_rate * 8 /* bytes/sample */ * 8 /* bits/byte */;

  if (*encoding.average_encoding_bitrate() == 0 ||
      *encoding.average_encoding_bitrate() > max_bitrate) {
    FX_LOGS(WARNING) << "EncodedFormat average_encoding_bitrate ("
                     << *encoding.average_encoding_bitrate() << ") is out of range";
    return false;
  }
  if (!encoding.encoding_type().has_value()) {
    FX_LOGS(WARNING) << "EncodedFormat encoding_type is missing";
    return false;
  }
  return true;
}

bool ValidatePacketStreamProperties(const fha::PacketStreamProperties& packet_stream_properties) {
  ADR_LOG(kLogDeviceMethods);
  LogPacketStreamProperties(packet_stream_properties);
  if (!packet_stream_properties.needs_cache_flush_or_invalidate().has_value()) {
    FX_LOGS(WARNING) << "PacketStreamProperties must set needs_cache_flush_or_invalidate";
    return false;
  }

  if (!packet_stream_properties.supported_buffer_types().has_value() ||
      static_cast<uint64_t>(*packet_stream_properties.supported_buffer_types()) == 0) {
    FX_LOGS(WARNING)
        << "PacketStreamProperties must set supported_buffer_types to a non-zero value";
    return false;
  }
  return true;
}

bool ValidateSampleFormatCompatibility(uint8_t bytes_per_sample, fha::SampleFormat sample_format) {
  // Explicitly check for fuchsia_audio::SampleType kUint8, kInt16, kInt32, kFloat32, kFloat64
  if ((sample_format == fha::SampleFormat::kPcmUnsigned && bytes_per_sample == 1) ||
      (sample_format == fha::SampleFormat::kPcmSigned && bytes_per_sample == 2) ||
      (sample_format == fha::SampleFormat::kPcmSigned && bytes_per_sample == 4) ||
      (sample_format == fha::SampleFormat::kPcmFloat && bytes_per_sample == 4) ||
      (sample_format == fha::SampleFormat::kPcmFloat && bytes_per_sample == 8)) {
    return true;
  }

  FX_LOGS(WARNING) << "No valid fuchsia_audio::SampleType exists, for "
                   << static_cast<uint16_t>(bytes_per_sample) << "-byte " << sample_format;
  return false;
}

bool ValidateRingBufferVmo(const zx::vmo& vmo, uint32_t num_frames, const fha::Format2& rb_format,
                           zx_rights_t required_rights) {
  ADR_LOG(kLogDeviceMethods);
  LogRingBufferVmo(vmo, num_frames, rb_format);

  uint64_t size;
  if (rb_format.Which() != fha::Format2::Tag::kPcmFormat ||
      !ValidatePcmFormat(rb_format.pcm_format().value())) {
    return false;
  }
  if (!ValidateSampleFormatCompatibility(rb_format.pcm_format()->bytes_per_sample(),
                                         rb_format.pcm_format()->sample_format())) {
    return false;
  }

  zx_info_handle_basic_t info;
  auto status = vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "vmo.get_info returned error";
    return false;
  }
  if ((info.rights & required_rights) != required_rights) {
    FX_LOGS(WARNING) << "VMO rights 0x" << std::hex << info.rights << " are insufficient (0x"
                     << required_rights << " are required)";
    return false;
  }

  status = vmo.get_size(&size);
  if (status != ZX_OK) {
    FX_LOGS(WARNING) << "get_size returned size " << size << " and error " << status;
    return false;
  }
  if (size < static_cast<uint64_t>(num_frames) * rb_format.pcm_format()->number_of_channels() *
                 rb_format.pcm_format()->bytes_per_sample()) {
    FX_LOGS(WARNING) << "RingBuffer.GetVmo num_frames (" << num_frames << ", "
                     << static_cast<uint16_t>(rb_format.pcm_format()->number_of_channels())
                     << " channels, "
                     << static_cast<uint16_t>(rb_format.pcm_format()->bytes_per_sample())
                     << " bytes_per_sample) does not match VMO size (" << size << " bytes)";
    return false;
  }
  return true;
}

zx_status_t ValidatePacketStreamVmo(const fha::VmoInfo& vmo_info, zx_rights_t required_rights,
                                    std::optional<uint64_t> min_size) {
  ADR_LOG(kLogDeviceMethods);
  LogPacketStreamVmo(vmo_info);

  if (!vmo_info.id().has_value()) {
    FX_LOGS(WARNING) << "VmoInfo.id is missing";
    return ZX_ERR_INVALID_ARGS;
  }
  if (!vmo_info.vmo().has_value() || !vmo_info.vmo()->is_valid()) {
    FX_LOGS(WARNING) << "VmoInfo.vmo is missing or invalid";
    return ZX_ERR_INVALID_ARGS;
  }

  zx_info_handle_basic_t info;
  auto status =
      vmo_info.vmo()->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "vmo.get_info returned error";
    return status;
  }
  if ((info.rights & required_rights) != required_rights) {
    FX_LOGS(WARNING) << "VmoInfo rights 0x" << std::hex << info.rights << " are insufficient (0x"
                     << required_rights << " are required)";
    return ZX_ERR_ACCESS_DENIED;
  }

  uint64_t size;
  status = vmo_info.vmo()->get_size(&size);
  if (status != ZX_OK) {
    FX_LOGS(WARNING) << "get_size returned size " << size << " and error " << status;
    return status;
  }
  if (min_size.has_value() && size < *min_size) {
    FX_LOGS(WARNING) << "VmoInfo.vmo size (" << size << " bytes) is less than required size ("
                     << *min_size << " bytes)";
    return ZX_ERR_BUFFER_TOO_SMALL;
  }

  return ZX_OK;
}

bool ValidateDelayInfo(const fha::DelayInfo& delay_info) {
  ADR_LOG(kLogDeviceMethods);
  LogDelayInfo(delay_info);

  if (!delay_info.internal_delay().has_value()) {
    FX_LOGS(WARNING) << "DelayInfo.internal_delay is missing";
    return false;
  }
  if (*delay_info.internal_delay() < 0) {
    FX_LOGS(WARNING) << "WatchDelayInfo: DelayInfo.internal_delay (" << *delay_info.internal_delay()
                     << " ns) cannot be negative";
    return false;
  }

  if (delay_info.external_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "WatchDelayInfo: DelayInfo.external_delay (" << *delay_info.external_delay()
                     << " ns) cannot be negative";
    return false;
  }

  return true;
}

// Validate the type-specific state for this element type.
bool ValidateDaiInterconnectElementState(const fhasp::ElementState& element_state,
                                         const fhasp::Element& element) {
  if (  // This must be an element of type DaiInterconnect
      element.type() != fhasp::ElementType::kDaiInterconnect ||
      // ... with appropriate type_specific info
      !element.type_specific().has_value() ||
      // A type_specific element description of type dai_interconnect() must exist
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kDaiInterconnect ||
      !element.type_specific()->dai_interconnect().has_value() ||
      // ... specifically including plug_detect_capabilities()
      !element.type_specific()->dai_interconnect()->plug_detect_capabilities().has_value() ||
      // ElementState must be type DaiInterconnect (type_specific dai_interconnect() must exist)
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() !=
          fhasp::TypeSpecificElementState::Tag::kDaiInterconnect ||
      // A type_specific element state dai_interconnect() must exist
      !element_state.type_specific()->dai_interconnect().has_value() ||
      // plug_state must be present
      !element_state.type_specific()->dai_interconnect()->plug_state().has_value() ||
      // plugged and plug_state_time are required
      !element_state.type_specific()->dai_interconnect()->plug_state()->plugged().has_value() ||
      !element_state.type_specific()
           ->dai_interconnect()
           ->plug_state()
           ->plug_state_time()
           .has_value() ||
      // plug_state_time (which is on the MONOTONIC timeline) cannot be negative
      *element_state.type_specific()->dai_interconnect()->plug_state()->plug_state_time() < 0 ||
      // if unplugged...
      (!*element_state.type_specific()->dai_interconnect()->plug_state()->plugged() &&
       // ... then the Element's capabilities must allow for that.
       *element.type_specific()->dai_interconnect()->plug_detect_capabilities() ==
           fhasp::PlugDetectCapabilities::kHardwired)) {
    FX_LOGS(WARNING) << "Invalid DaiInterconnect-specific fields in ElementState";
    return false;
  }

  // external_delay, if present, must not be negative
  if (element_state.type_specific()->dai_interconnect()->external_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "WatchElementState: ElementState.external_delay ("
                     << *element_state.type_specific()->dai_interconnect()->external_delay()
                     << " ns) cannot be negative";
    return false;
  }

  return true;
}

// Validate the type-specific state for this element type.
bool ValidateDynamicsElementState(const fhasp::ElementState& element_state,
                                  const fhasp::Element& element) {
  if (  // This must be an element of type Dynamics
      element.type() != fhasp::ElementType::kDynamics || !element.type_specific().has_value() ||
      // ... with appropriate type_specific info
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kDynamics ||
      !element.type_specific()->dynamics().has_value() ||
      // ... specifically including the bands() vector
      !element.type_specific()->dynamics()->bands().has_value() ||
      element.type_specific()->dynamics()->bands()->empty() ||
      // A type_specific element state of dynamics() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() != fhasp::TypeSpecificElementState::Tag::kDynamics ||
      !element_state.type_specific()->dynamics().has_value() ||
      // band_states() must not be absent or empty.
      !element_state.type_specific()->dynamics()->band_states().has_value() ||
      element_state.type_specific()->dynamics()->band_states()->empty()) {
    FX_LOGS(WARNING) << "Invalid Dynamics-specific fields in ElementState";
    return false;
  }

  // Additional type-specific checks on each DynamicsBandState.
  std::unordered_set<uint64_t> band_state_ids;
  for (const auto& band_state : *element_state.type_specific()->dynamics()->band_states()) {
    if (  // id is required
        !band_state.id().has_value() ||
        // id must be unique (not duplicated in another band_state).
        band_state_ids.contains(*band_state.id()) ||
        // id must be contained in the Element.bands vector.
        std::none_of(element.type_specific()->dynamics()->bands()->cbegin(),
                     element.type_specific()->dynamics()->bands()->cend(),
                     [id = *band_state.id()](const fhasp::DynamicsBand& band) {
                       return (*band.id() == id);
                     }) ||
        // min_frequency and max_frequency are required
        !band_state.min_frequency().has_value() || !band_state.max_frequency().has_value() ||
        // max_frequency must equal/exceed min_frequency
        *band_state.min_frequency() > *band_state.max_frequency() ||
        // threshold_db is required
        !band_state.threshold_db().has_value() ||
        // threshold_db must be finite
        !std::isfinite(*band_state.threshold_db()) ||
        // threshold_type is required for servers
        !band_state.threshold_type().has_value() ||
        // ratio is required and must be finite
        !band_state.ratio().has_value() || !std::isfinite(*band_state.ratio()) ||
        // knee_width_db, if present, must not be negative
        (band_state.knee_width_db().value_or(0) < 0.0f) ||
        // knee_width_db, if present, must be finite
        (!std::isfinite(band_state.knee_width_db().value_or(0))) ||
        // attack, if present, must not be negative
        (band_state.attack().value_or(0) < 0) ||
        // release, if present, must not be negative
        (band_state.release().value_or(0) < 0) ||
        // output_gain_db, if present, must be finite
        (!std::isfinite(band_state.output_gain_db().value_or(0))) ||
        // input_gain_db, if present, must be finite
        (!std::isfinite(band_state.input_gain_db().value_or(0))) ||
        // lookahead, if present, must not be negative
        (band_state.lookahead().value_or(0) < 0)) {
      FX_LOGS(WARNING) << "Invalid DynamicsBandState (id " << to_string(band_state.id(), "<none>")
                       << ")";
      return false;
    }
    band_state_ids.insert(*band_state.id());
  }

  return true;
}

// Validate the type-specific state for this element type.
bool ValidateSettableDynamicsElementState(const fhasp::SettableElementState& element_state,
                                          const fhasp::Element& element) {
  if (  // This must be an element of type Dynamics
      element.type() != fhasp::ElementType::kDynamics || !element.type_specific().has_value() ||
      // ... with appropriate type_specific info
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kDynamics ||
      !element.type_specific()->dynamics().has_value() ||
      // ... specifically including the bands() vector
      !element.type_specific()->dynamics()->bands().has_value() ||
      element.type_specific()->dynamics()->bands()->empty() ||
      // A type_specific element state of dynamics() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() !=
          fhasp::SettableTypeSpecificElementState::Tag::kDynamics ||
      !element_state.type_specific()->dynamics().has_value() ||
      // band_states() must not be absent or empty.
      !element_state.type_specific()->dynamics()->band_states().has_value() ||
      element_state.type_specific()->dynamics()->band_states()->empty()) {
    FX_LOGS(WARNING) << "Invalid Dynamics-specific fields in SettableElementState";
    return false;
  }

  // Additional type-specific checks on each DynamicsBandState.
  std::unordered_set<uint64_t> band_state_ids;
  for (const auto& band_state : *element_state.type_specific()->dynamics()->band_states()) {
    if (  // id is required
        !band_state.id().has_value() ||
        // id must be unique (not duplicated in another band_state).
        band_state_ids.contains(*band_state.id()) ||
        // id must be contained in the Element.bands vector.
        std::none_of(element.type_specific()->dynamics()->bands()->cbegin(),
                     element.type_specific()->dynamics()->bands()->cend(),
                     [id = *band_state.id()](const fhasp::DynamicsBand& band) {
                       return (*band.id() == id);
                     }) ||
        // min_frequency and max_frequency are required
        !band_state.min_frequency().has_value() || !band_state.max_frequency().has_value() ||
        // max_frequency must equal/exceed min_frequency
        *band_state.min_frequency() > *band_state.max_frequency() ||
        // threshold_db is required
        !band_state.threshold_db().has_value() ||
        // threshold_db must be finite
        !std::isfinite(*band_state.threshold_db()) ||
        // threshold_type from a CLIENT requires that supported_controls bit to be set
        (band_state.threshold_type().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kThresholdType)) ||
        // ratio is required and must be finite
        !band_state.ratio().has_value() || !std::isfinite(*band_state.ratio()) ||
        // knee_width_db from a CLIENT requires that supported_controls bit to be set
        (band_state.knee_width_db().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kKneeWidth)) ||
        // knee_width_db, if present, must not be negative
        (band_state.knee_width_db().value_or(0) < 0.0f) ||
        // knee_width_db, if present, must be finite
        (band_state.knee_width_db().has_value() && !std::isfinite(*band_state.knee_width_db())) ||
        // attack from a CLIENT requires that supported_controls bit to be set
        (band_state.attack().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kAttack)) ||
        // attack, if present, must not be negative
        (band_state.attack().has_value() && *band_state.attack() < 0) ||
        // release from a CLIENT requires that supported_controls bit to be set
        (band_state.release().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kRelease)) ||
        // release, if present, must not be negative
        (band_state.release().has_value() && *band_state.release() < 0) ||
        // output_gain_db from a CLIENT requires that supported_controls bit to be set
        (band_state.output_gain_db().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kOutputGain)) ||
        // output_gain_db, if present, must be finite
        (band_state.output_gain_db().has_value() && !std::isfinite(*band_state.output_gain_db())) ||
        // input_gain_db from a CLIENT requires that supported_controls bit to be set
        (band_state.input_gain_db().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kInputGain)) ||
        // input_gain_db, if present, must be finite
        (band_state.input_gain_db().has_value() && !std::isfinite(*band_state.input_gain_db())) ||
        // level_type from a CLIENT requires that supported_controls bit to be set
        (band_state.level_type().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kLevelType)) ||
        // lookahead from a CLIENT requires that supported_controls bit to be set
        (band_state.lookahead().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kLookahead)) ||
        // lookahead, if present, must not be negative
        (band_state.lookahead().has_value() && *band_state.lookahead() < 0) ||
        // linked_channels from a CLIENT requires that supported_controls bit to be set
        (band_state.linked_channels().has_value() &&
         !(*element.type_specific()->dynamics()->supported_controls() &
           fhasp::DynamicsSupportedControls::kLinkedChannels))) {
      FX_LOGS(WARNING) << "Invalid DynamicsBandState (id "
                       << (band_state.id().has_value() ? std::to_string(*band_state.id())
                                                       : "<none>")
                       << ")";
      return false;
    }
    band_state_ids.insert(*band_state.id());
  }

  return true;
}

// Validate the type-specific state for this element type.
bool ValidateEqualizerElementState(const fhasp::ElementState& element_state,
                                   const fhasp::Element& element) {
  if (  // This must be an element of type Equalizer
      element.type() != fhasp::ElementType::kEqualizer ||
      // ... with appropriate type_specific info
      !element.type_specific().has_value() ||
      // A type_specific element description of type equalizer() must exist
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kEqualizer ||
      !element.type_specific()->equalizer().has_value() ||
      // ... specifically including bands(), min_frequency() and max_frequency()
      !element.type_specific()->equalizer()->bands().has_value() ||
      element.type_specific()->equalizer()->bands()->empty() ||
      !element.type_specific()->equalizer()->min_frequency().has_value() ||
      !element.type_specific()->equalizer()->max_frequency().has_value() ||
      // A type_specific element state of equalizer() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() != fhasp::TypeSpecificElementState::Tag::kEqualizer ||
      !element_state.type_specific()->equalizer().has_value() ||
      // band_states() must not be absent or empty.
      !element_state.type_specific()->equalizer()->band_states().has_value() ||
      element_state.type_specific()->equalizer()->band_states()->empty()) {
    FX_LOGS(WARNING) << "Invalid EQ-specific fields in ElementState";
    return false;
  }

  // Additional type-specific checks on each EqualizerBandState.
  for (const auto& band_state : *element_state.type_specific()->equalizer()->band_states()) {
    if (  // id is required
        !band_state.id().has_value() ||
        // id must be contained in the Element.bands vector.
        std::none_of(element.type_specific()->equalizer()->bands()->cbegin(),
                     element.type_specific()->equalizer()->bands()->cend(),
                     [id = *band_state.id()](const fhasp::EqualizerBand& band) {
                       return (*band.id() == id);
                     }) ||
        // frequency, if present, can't be lower than min_frequency...
        (band_state.frequency().has_value() &&
         element.type_specific()->equalizer()->min_frequency() &&
         *band_state.frequency() < *element.type_specific()->equalizer()->min_frequency()) ||
        // ... or greater than max_frequency
        (band_state.frequency().has_value() &&
         element.type_specific()->equalizer()->max_frequency() &&
         *band_state.frequency() > *element.type_specific()->equalizer()->max_frequency()) ||
        // q, if present, must be positive and finite
        (band_state.q().has_value() &&
         (*band_state.q() <= 0.0f || !std::isfinite(*band_state.q()))) ||
        // gain_db, if present, must be finite
        (band_state.gain_db().has_value() && !std::isfinite(*band_state.gain_db())) ||
        // gain_db is required for PEAK, LOW_SHELF, HIGH_SHELF...
        (!band_state.gain_db().has_value() && band_state.type().has_value() &&
         (*band_state.type() == fhasp::EqualizerBandType::kPeak ||
          *band_state.type() == fhasp::EqualizerBandType::kLowShelf ||
          *band_state.type() == fhasp::EqualizerBandType::kHighShelf)) ||
        // ... but it is disallowed for NOTCH, LOW_CUT, HIGH_CUT.
        (band_state.gain_db().has_value() && band_state.type().has_value() &&
         (*band_state.type() == fhasp::EqualizerBandType::kNotch ||
          *band_state.type() == fhasp::EqualizerBandType::kLowCut ||
          *band_state.type() == fhasp::EqualizerBandType::kHighCut))) {
      FX_LOGS(WARNING) << "Invalid EqualizerBandState (id " << to_string(band_state.id(), "<none>")
                       << ")";
      return false;
    }
  }

  return true;
}

// Validate the type-specific state for this element type.
bool ValidateSettableEqualizerElementState(const fhasp::SettableElementState& element_state,
                                           const fhasp::Element& element) {
  if (  // This must be an element of type Equalizer
      element.type() != fhasp::ElementType::kEqualizer ||
      // ... with appropriate type_specific info
      !element.type_specific().has_value() ||
      // A type_specific element description of type equalizer() must exist
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kEqualizer ||
      !element.type_specific()->equalizer().has_value() ||
      // ... specifically including bands(), min_frequency() and max_frequency()
      !element.type_specific()->equalizer()->bands().has_value() ||
      element.type_specific()->equalizer()->bands()->empty() ||
      !element.type_specific()->equalizer()->min_frequency().has_value() ||
      !element.type_specific()->equalizer()->max_frequency().has_value() ||
      // A type_specific element state of equalizer() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() !=
          fhasp::SettableTypeSpecificElementState::Tag::kEqualizer ||
      !element_state.type_specific()->equalizer().has_value() ||
      // band_states() must not be absent or empty.
      !element_state.type_specific()->equalizer()->band_states().has_value() ||
      element_state.type_specific()->equalizer()->band_states()->empty()) {
    FX_LOGS(WARNING) << "Invalid EQ-specific fields in SettableElementState";
    return false;
  }

  // Additional type-specific checks on each EqualizerBandState.
  for (const auto& band_state : *element_state.type_specific()->equalizer()->band_states()) {
    if (  // id is required
        !band_state.id().has_value() ||
        // id must be contained in the Element.bands vector.
        std::none_of(element.type_specific()->equalizer()->bands()->cbegin(),
                     element.type_specific()->equalizer()->bands()->cend(),
                     [id = *band_state.id()](const fhasp::EqualizerBand& band) {
                       return (*band.id() == id);
                     }) ||

        // frequency, if present, can't be lower than min_frequency...
        (band_state.frequency().has_value() &&
         element.type_specific()->equalizer()->min_frequency() &&
         *band_state.frequency() < *element.type_specific()->equalizer()->min_frequency()) ||
        // ... or greater than max_frequency
        (band_state.frequency().has_value() &&
         element.type_specific()->equalizer()->max_frequency() &&
         *band_state.frequency() > *element.type_specific()->equalizer()->max_frequency()) ||
        // CAN_CONTROL_FREQUENCY must be a property of this EQ element.
        (band_state.frequency().has_value() &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.frequency().has_value() &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kCanControlFrequency)) ||

        // q, if present, must be positive and finite
        (band_state.q().has_value() &&
         (*band_state.q() <= 0.0f || !std::isfinite(*band_state.q()))) ||
        // CAN_CONTROL_Q must be a property of this EQ element.
        (band_state.q().has_value() &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.q().has_value() &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kCanControlQ)) ||

        // gain_db, if present, must be finite
        (band_state.gain_db().has_value() && !std::isfinite(*band_state.gain_db())) ||
        // gain_db is required for PEAK, LOW_SHELF, HIGH_SHELF...
        (!band_state.gain_db().has_value() && band_state.type().has_value() &&
         (*band_state.type() == fhasp::EqualizerBandType::kPeak ||
          *band_state.type() == fhasp::EqualizerBandType::kLowShelf ||
          *band_state.type() == fhasp::EqualizerBandType::kHighShelf)) ||
        // ... but it is disallowed for NOTCH, LOW_CUT, HIGH_CUT.
        (band_state.gain_db().has_value() && band_state.type().has_value() &&
         (*band_state.type() == fhasp::EqualizerBandType::kNotch ||
          *band_state.type() == fhasp::EqualizerBandType::kLowCut ||
          *band_state.type() == fhasp::EqualizerBandType::kHighCut)) ||

        // SUPPORTS_TYPE_PEAK must be a property of this EQ element.
        (band_state.type().has_value() && *band_state.type() == fhasp::EqualizerBandType::kPeak &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.type().has_value() && *band_state.type() == fhasp::EqualizerBandType::kPeak &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kSupportsTypePeak)) ||

        // SUPPORTS_TYPE_NOTCH must be a property of this EQ element.
        (band_state.type().has_value() && *band_state.type() == fhasp::EqualizerBandType::kNotch &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.type().has_value() && *band_state.type() == fhasp::EqualizerBandType::kNotch &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kSupportsTypeNotch)) ||

        // SUPPORTS_TYPE_LOW_CUT must be a property of this EQ element.
        (band_state.type().has_value() && *band_state.type() == fhasp::EqualizerBandType::kLowCut &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.type().has_value() && *band_state.type() == fhasp::EqualizerBandType::kLowCut &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kSupportsTypeLowCut)) ||

        // SUPPORTS_TYPE_HIGH_CUT must be a property of this EQ element.
        (band_state.type().has_value() &&
         *band_state.type() == fhasp::EqualizerBandType::kHighCut &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.type().has_value() &&
         *band_state.type() == fhasp::EqualizerBandType::kHighCut &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kSupportsTypeHighCut)) ||

        // SUPPORTS_TYPE_LOW_SHELF must be a property of this EQ element.
        (band_state.type().has_value() &&
         *band_state.type() == fhasp::EqualizerBandType::kLowShelf &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.type().has_value() &&
         *band_state.type() == fhasp::EqualizerBandType::kLowShelf &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kSupportsTypeLowShelf)) ||

        // SUPPORTS_TYPE_HIGH_SHELF must be a property of this EQ element.
        (band_state.type().has_value() &&
         *band_state.type() == fhasp::EqualizerBandType::kHighShelf &&
         !element.type_specific()->equalizer()->supported_controls().has_value()) ||
        (band_state.type().has_value() &&
         *band_state.type() == fhasp::EqualizerBandType::kHighShelf &&
         !(*element.type_specific()->equalizer()->supported_controls() &
           fhasp::EqualizerSupportedControls::kSupportsTypeHighShelf))) {
      FX_LOGS(WARNING) << "Invalid EqualizerBandState (id " << to_string(band_state.id(), "<none>")
                       << ")";
      return false;
    }
  }

  return true;
}

// Validate the type-specific state for this element type.
bool ValidateGainElementState(const fhasp::ElementState& element_state,
                              const fhasp::Element& element) {
  if (  // This must be an element of type Gain
      element.type() != fhasp::ElementType::kGain ||
      // ... with appropriate type_specific info
      !element.type_specific().has_value() ||
      // A type_specific element description of type gain() must exist
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kGain ||
      !element.type_specific()->gain().has_value() ||
      // ... specifically including min_gain() and max_gain()
      !element.type_specific()->gain()->min_gain().has_value() ||
      !element.type_specific()->gain()->max_gain().has_value() ||
      // ... both of which must be finite
      !std::isfinite(*element.type_specific()->gain()->min_gain()) ||
      !std::isfinite(*element.type_specific()->gain()->max_gain()) ||
      // A type_specific element state of gain() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() != fhasp::TypeSpecificElementState::Tag::kGain ||
      !element_state.type_specific()->gain().has_value() ||
      // A gain value is required, and it must be finite
      !element_state.type_specific()->gain()->gain().has_value() ||
      !std::isfinite(*element_state.type_specific()->gain()->gain()) ||
      // ... and must be in the [min_gain, max_gain] range.
      *element_state.type_specific()->gain()->gain() <
          element.type_specific()->gain()->min_gain() ||
      *element_state.type_specific()->gain()->gain() >
          element.type_specific()->gain()->max_gain()) {
    FX_LOGS(WARNING) << "Invalid Gain-specific fields in ElementState";
    return false;
  }
  return true;
}

// Validate the type-specific state for this element type.
bool ValidateSettableGainElementState(const fhasp::SettableElementState& element_state,
                                      const fhasp::Element& element) {
  if (  // This must be an element of type Gain
      element.type() != fhasp::ElementType::kGain ||
      // ... with appropriate type_specific info
      !element.type_specific().has_value() ||
      // A type_specific element description of type gain() must exist
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kGain ||
      !element.type_specific()->gain().has_value() ||
      // ... specifically including min_gain() and max_gain()
      !element.type_specific()->gain()->min_gain().has_value() ||
      !element.type_specific()->gain()->max_gain().has_value() ||
      // ... both of which must be finite
      !std::isfinite(*element.type_specific()->gain()->min_gain()) ||
      !std::isfinite(*element.type_specific()->gain()->max_gain()) ||
      // A type_specific element state of gain() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() !=
          fhasp::SettableTypeSpecificElementState::Tag::kGain ||
      !element_state.type_specific()->gain().has_value() ||
      // A gain value is required, and it must be finite
      !element_state.type_specific()->gain()->gain().has_value() ||
      !std::isfinite(*element_state.type_specific()->gain()->gain()) ||
      // ... and must be in the [min_gain, max_gain] range.
      *element_state.type_specific()->gain()->gain() <
          element.type_specific()->gain()->min_gain() ||
      *element_state.type_specific()->gain()->gain() >
          element.type_specific()->gain()->max_gain()) {
    FX_LOGS(WARNING) << "Invalid Gain-specific fields in SettableElementState";
    return false;
  }
  return true;
}

// Validate the type-specific state for this element type.
bool ValidateVendorSpecificElementState(const fhasp::ElementState& element_state,
                                        const fhasp::Element& element) {
  if (  // This must be an element of type VendorSpecific
      element.type() != fhasp::ElementType::kVendorSpecific ||
      // ... with appropriate type_specific info
      !element.type_specific().has_value() ||
      // A type_specific element description of type vendor_specific() must exist
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kVendorSpecific ||
      !element.type_specific()->vendor_specific().has_value() ||
      // A type_specific element state of vendor_specific() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() !=
          fhasp::TypeSpecificElementState::Tag::kVendorSpecific ||
      !element_state.type_specific()->vendor_specific().has_value() ||
      // Overall vendor_specific_data is optional for ANY element type, but required for this one.
      !element_state.vendor_specific_data().has_value() ||
      // vendor_specific_data is opaque: we have no structured checks; just check for empty.
      element_state.vendor_specific_data()->empty()) {
    FX_LOGS(WARNING) << "Invalid VendorSpecific fields in ElementState";
    return false;
  }
  return true;
}

// Validate the type-specific state for this element type.
bool ValidateSettableVendorSpecificElementState(const fhasp::SettableElementState& element_state,
                                                const fhasp::Element& element) {
  if (  // This must be an element of type VendorSpecific
      element.type() != fhasp::ElementType::kVendorSpecific ||
      // ... with appropriate type_specific info
      !element.type_specific().has_value() ||
      // A type_specific element description of type vendor_specific() must exist
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kVendorSpecific ||
      !element.type_specific()->vendor_specific().has_value() ||
      // A type_specific element state of vendor_specific() must not be absent or empty.
      !element_state.type_specific().has_value() ||
      element_state.type_specific()->Which() !=
          fhasp::SettableTypeSpecificElementState::Tag::kVendorSpecific ||
      !element_state.type_specific()->vendor_specific().has_value() ||
      // Overall vendor_specific_data is optional for ANY element type, but required for this one.
      !element_state.vendor_specific_data().has_value() ||
      // vendor_specific_data is opaque: we have no structured checks; just check for empty.
      element_state.vendor_specific_data()->empty()) {
    FX_LOGS(WARNING) << "Invalid VendorSpecific fields in SettableElementState";
    return false;
  }
  return true;
}

bool ValidateElementState(const fhasp::ElementState& element_state, const fhasp::Element& element) {
  LogElementState(element_state);

  if (!ValidateElement(element)) {
    return false;
  }

  switch (*element.type()) {
    case fhasp::ElementType::kDaiInterconnect:
      if (!ValidateDaiInterconnectElementState(element_state, element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kDynamics:
      if (!ValidateDynamicsElementState(element_state, element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kEqualizer:
      if (!ValidateEqualizerElementState(element_state, element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kGain:
      if (!ValidateGainElementState(element_state, element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kVendorSpecific:
      if (!ValidateVendorSpecificElementState(element_state, element)) {
        return false;
      }
      break;
    default:
      // If none of these, then .type_specific should not contain any value.
      if (element_state.type_specific().has_value()) {
        FX_LOGS(WARNING)
            << "WatchElementState: ElementState.type_specific should be empty for ElementType "
            << element.type();
        return false;
      }
      break;
  }

  if (element_state.vendor_specific_data().has_value()) {
    // vendor_specific_data is opaque to us, so we can't really perform other structured checks.
    if (element_state.vendor_specific_data()->empty()) {
      FX_LOGS(WARNING)
          << "WatchElementState: ElementState.vendor_specific_data, if present, must not be empty";
      return false;
    }
  }

  if (!element_state.started().has_value()) {
    FX_LOGS(WARNING) << "WatchElementState: ElementState.started is required but missing";
    return false;
  }
  if (!element.can_stop().value_or(false) && !*element_state.started()) {
    FX_LOGS(WARNING) << "WatchElementState: Element.can_stop is false, but ElementState is stopped";
    return false;
  }

  if (!element.can_bypass().value_or(false) && element_state.bypassed().value_or(false)) {
    FX_LOGS(WARNING)
        << "WatchElementState: Element.can_bypass is false, but ElementState is bypassed";
    return false;
  }

  if (element_state.turn_on_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "WatchElementState: ElementState.turn_on_delay ("
                     << *element_state.turn_on_delay() << " ns) cannot be negative";
    return false;
  }

  if (element_state.turn_off_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "WatchElementState: ElementState.turn_off_delay ("
                     << *element_state.turn_off_delay() << " ns) cannot be negative";
    return false;
  }

  if (element_state.processing_delay().value_or(0) < 0) {
    FX_LOGS(WARNING) << "WatchElementState: ElementState.processing_delay ("
                     << *element_state.processing_delay() << " ns) cannot be negative";
    return false;
  }

  return true;
}

bool ValidateSettableElementState(const fhasp::SettableElementState& element_state,
                                  const fhasp::Element& element) {
  LogSettableElementState(element_state);

  if (!ValidateElement(element)) {
    return false;
  }

  switch (*element.type()) {
    case fhasp::ElementType::kDynamics:
      if (!ValidateSettableDynamicsElementState(element_state, element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kEqualizer:
      if (!ValidateSettableEqualizerElementState(element_state, element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kGain:
      if (!ValidateSettableGainElementState(element_state, element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kVendorSpecific:
      if (!ValidateSettableVendorSpecificElementState(element_state, element)) {
        return false;
      }
      break;
    default:
      // If none of these, then .type_specific should not contain any value.
      if (element_state.type_specific().has_value()) {
        FX_LOGS(WARNING)
            << "WatchElementState: ElementState.type_specific should be empty for ElementType "
            << element.type();
        return false;
      }
      break;
  }

  if (element_state.vendor_specific_data().has_value()) {
    // vendor_specific_data is opaque to us, so we can't really perform other structured checks.
    if (element_state.vendor_specific_data()->empty()) {
      FX_LOGS(WARNING)
          << "WatchElementState: ElementState.vendor_specific_data, if present, must not be empty";
      return false;
    }
  }

  if (!element.can_stop().value_or(false) && !element_state.started().value_or(true)) {
    FX_LOGS(WARNING) << "WatchElementState: Element.can_stop is false, but ElementState is stopped";
    return false;
  }

  if (!element.can_bypass().value_or(false) && element_state.bypassed().value_or(false)) {
    FX_LOGS(WARNING)
        << "WatchElementState: Element.can_bypass is false, but ElementState is bypassed";
    return false;
  }

  return true;
}

// Validate the type-specific table for this element type.
bool ValidateDaiInterconnectElement(const fhasp::Element& element) {
  if (  // A type_specific info of dai_interconnect() must not be absent or empty.
      !element.type_specific().has_value() ||
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kDaiInterconnect ||
      !element.type_specific()->dai_interconnect().has_value() ||
      // plug_detect_capabilities must be present
      !element.type_specific()->dai_interconnect()->plug_detect_capabilities().has_value()) {
    FX_LOGS(WARNING) << "Invalid DaiInterconnect-specific fields";
    return false;
  }
  return true;
}

// Validate the type-specific table for this element type.
bool ValidateDynamicsElement(const fhasp::Element& element) {
  if (  // A type_specific info of dynamics() must not be absent or empty.
      !element.type_specific().has_value() ||
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kDynamics ||
      !element.type_specific()->dynamics().has_value() ||
      // bands must be present and non-empty
      !element.type_specific()->dynamics()->bands().has_value() ||
      element.type_specific()->dynamics()->bands()->empty() ||
      // if supported_controls is set, it must not be zero (otherwise, why set it)
      (element.type_specific()->dynamics()->supported_controls().has_value() &&
       !*element.type_specific()->dynamics()->supported_controls())) {
    FX_LOGS(WARNING) << "Invalid Dynamics-specific fields";
    return false;
  }
  // Band ids must be unique.
  std::unordered_set<uint64_t> band_ids;
  for (const auto& band : *element.type_specific()->dynamics()->bands()) {
    if (!band.id().has_value() || band_ids.contains(*band.id())) {
      FX_LOGS(WARNING) << "Missing or duplicate DynamicsBand.id";
      return false;
    }
    band_ids.insert(*band.id());
  }
  return true;
}

// Validate the type-specific table for this element type.
bool ValidateEqualizerElement(const fhasp::Element& element) {
  if (  // A type_specific info of equalizer() must not be absent or empty.
      !element.type_specific().has_value() ||
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kEqualizer ||
      !element.type_specific()->equalizer().has_value() ||
      // bands must be present and non-empty
      !element.type_specific()->equalizer()->bands().has_value() ||
      element.type_specific()->equalizer()->bands()->empty() ||
      // min_frequency must be present; max_frequency must be present and >= min_frequency
      !element.type_specific()->equalizer()->min_frequency().has_value() ||
      !element.type_specific()->equalizer()->max_frequency().has_value() ||
      (*element.type_specific()->equalizer()->min_frequency() >
       *element.type_specific()->equalizer()->max_frequency()) ||
      // max_q, if present, must be positive and finite
      (element.type_specific()->equalizer()->max_q().has_value() &&
       (*element.type_specific()->equalizer()->max_q() <= 0.0f ||
        !std::isfinite(*element.type_specific()->equalizer()->max_q()))) ||
      // min_gain_db, if present, must be positive and finite
      (element.type_specific()->equalizer()->min_gain_db().has_value() &&
       !std::isfinite(*element.type_specific()->equalizer()->min_gain_db())) ||
      // max_gain_db, if present, must be positive and finite
      (element.type_specific()->equalizer()->max_gain_db().has_value() &&
       !std::isfinite(*element.type_specific()->equalizer()->max_gain_db())) ||
      // min_gain_db <= max_gain_db, if both are present.
      (element.type_specific()->equalizer()->min_gain_db().has_value() &&
       element.type_specific()->equalizer()->max_gain_db().has_value() &&
       *element.type_specific()->equalizer()->min_gain_db() >
           *element.type_specific()->equalizer()->max_gain_db()) ||
      // If EQ supports PEAK/LOW_SHELF/HIGH_SHELF ...
      (element.type_specific()->equalizer()->supported_controls().has_value() &&
       (*element.type_specific()->equalizer()->supported_controls() &
        (fhasp::EqualizerSupportedControls::kSupportsTypePeak |
         fhasp::EqualizerSupportedControls::kSupportsTypeLowShelf |
         fhasp::EqualizerSupportedControls::kSupportsTypeHighShelf)) &&
       // ... then min_gain_db/max_gain_db are required.
       (!element.type_specific()->equalizer()->min_gain_db().has_value() ||
        !element.type_specific()->equalizer()->max_gain_db().has_value()))) {
    FX_LOGS(WARNING) << "Invalid EQ-specific fields";
    return false;
  }
  // Band ids must be unique.
  std::unordered_set<uint64_t> band_ids;
  for (const auto& band : *element.type_specific()->equalizer()->bands()) {
    if (!band.id().has_value() || band_ids.contains(*band.id())) {
      FX_LOGS(WARNING) << "Missing or duplicate EqualizerBand.id";
      return false;
    }
    band_ids.insert(*band.id());
  }
  return true;
}

// Validate the type-specific table for this element type.
bool ValidateGainElement(const fhasp::Element& element) {
  if (  // A type_specific info of gain() must not be absent or empty.
      !element.type_specific().has_value() ||
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kGain ||
      !element.type_specific()->gain().has_value() ||
      !element.type_specific()->gain()->type().has_value() ||
      // min_gain must be present and finite
      !element.type_specific()->gain()->min_gain().has_value() ||
      !std::isfinite(*element.type_specific()->gain()->min_gain()) ||
      // max_gain must be present and finite
      !element.type_specific()->gain()->max_gain().has_value() ||
      !std::isfinite(*element.type_specific()->gain()->max_gain()) ||
      // max_gain must equal or exceed min_gain;
      *element.type_specific()->gain()->min_gain() > *element.type_specific()->gain()->max_gain() ||
      // min_gain_step must be present and finite
      !element.type_specific()->gain()->min_gain_step().has_value() ||
      !std::isfinite(*element.type_specific()->gain()->min_gain_step()) ||
      // min_gain_step must not be negative ...
      *element.type_specific()->gain()->min_gain_step() < 0.0f ||
      // ... but it also must not exceed the gap between min_gain and max_gain.
      *element.type_specific()->gain()->min_gain_step() >
          *element.type_specific()->gain()->max_gain() -
              *element.type_specific()->gain()->min_gain()) {
    FX_LOGS(WARNING) << "Invalid Gain-specific fields";
    return false;
  }
  return true;
}

// Validate the type-specific table for this element type.
bool ValidateVendorSpecificElement(const fhasp::Element& element) {
  if (  // A type_specific info of vendor_specific() must not be absent or empty.
      !element.type_specific().has_value() ||
      element.type_specific()->Which() != fhasp::TypeSpecificElement::Tag::kVendorSpecific ||
      !element.type_specific()->vendor_specific().has_value()) {
    FX_LOGS(WARNING) << "Invalid VendorSpecific fields";
    return false;
  }
  return true;
}

bool ValidateElement(const fhasp::Element& element) {
  LogElement(element);

  if (!element.id().has_value()) {
    FX_LOGS(WARNING) << "SignalProcessing.element.id is missing";
    return false;
  }

  if (!element.type().has_value()) {
    FX_LOGS(WARNING) << "SignalProcessing.element.type is missing";
    return false;
  }

  switch (*element.type()) {
    case fhasp::ElementType::kDaiInterconnect:
      if (!ValidateDaiInterconnectElement(element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kDynamics: {
      if (!ValidateDynamicsElement(element)) {
        return false;
      }
      break;
    }
    case fhasp::ElementType::kEqualizer: {
      if (!ValidateEqualizerElement(element)) {
        return false;
      }
      break;
    }
    case fhasp::ElementType::kGain:
      if (!ValidateGainElement(element)) {
        return false;
      }
      break;
    case fhasp::ElementType::kVendorSpecific:
      if (!ValidateVendorSpecificElement(element)) {
        return false;
      }
      break;
    default:
      if (element.type_specific().has_value()) {
        FX_LOGS(WARNING) << "element(" << *element.id() << "): type " << element.type()
                         << " should not have a type_specific value";
        return false;
      }
      break;
  }

  if (element.description().has_value() && element.description()->empty()) {
    FX_LOGS(WARNING) << "SignalProcessing.element.description cannot be empty, if present";
    return false;
  }

  // can_stop is optional for all element types.
  // can_bypass is optional for all element types.

  return true;
}

bool ValidateElements(const std::vector<fhasp::Element>& elements) {
  LogElements(elements);

  // There should be at least one element.
  if (elements.empty()) {
    FX_LOGS(WARNING) << "SignalProcessing.elements[] is empty";
    return false;
  }

  for (const auto& element : elements) {
    // If any individual element is invalid, we're done.
    if (auto status = ValidateElement(element); !status) {
      return status;
    }
  }

  // Fail if the vector of elements cannot be mapped.
  return (!MapElements(elements).empty());
}

bool ValidateTopology(const fhasp::Topology& topology,
                      const std::unordered_map<ElementId, ElementRecord>& element_map) {
  LogTopology(topology);

  // Verify the TopologyId and vector<EdgePair> are present and non-empty.
  if (!topology.id().has_value()) {
    FX_LOGS(WARNING) << "SignalProcessing.topology.id is missing";
    return false;
  }
  if (!topology.processing_elements_edge_pairs().has_value()) {
    FX_LOGS(WARNING) << "SignalProcessing.topology.processing_elements_edge_pairs is missing";
    return false;
  }
  const auto& edge_pairs = *topology.processing_elements_edge_pairs();
  if (edge_pairs.empty()) {
    FX_LOGS(WARNING) << "SignalProcessing.topology.processing_elements_edge_pairs[] is empty";
    return false;
  }

  std::unordered_map<ElementId, uint64_t> edge_source_id_counts, edge_dest_id_counts;
  for (const auto& edge_pair : edge_pairs) {
    // Only succeeds if the ElementId is not yet present in the map.
    edge_source_id_counts.insert({
        edge_pair.processing_element_id_from(),
        0,
    });
    edge_dest_id_counts.insert({
        edge_pair.processing_element_id_to(),
        0,
    });

    // Always succeeds.
    edge_source_id_counts[edge_pair.processing_element_id_from()] += 1;
    edge_dest_id_counts[edge_pair.processing_element_id_to()] += 1;
  }

  for (const auto& edge_pair : edge_pairs) {
    // An EdgePair cannot refer to an unknown ElementId.
    if (!element_map.contains(edge_pair.processing_element_id_from())) {
      FX_LOGS(WARNING) << "Element_id_from " << edge_pair.processing_element_id_from()
                       << " not found in element list";
      return false;
    }
    if (!element_map.contains(edge_pair.processing_element_id_to())) {
      FX_LOGS(WARNING) << "Element_id_to " << edge_pair.processing_element_id_to()
                       << " not found in element list";
      return false;
    }

    // If an EdgePair refers to itself...
    if (edge_pair.processing_element_id_from() == edge_pair.processing_element_id_to()) {
      // ...then that ElementId cannot be mentioned in other EdgePairs.
      if (edge_source_id_counts[edge_pair.processing_element_id_from()] != 1 ||
          edge_dest_id_counts[edge_pair.processing_element_id_to()] != 1) {
        FX_LOGS(WARNING) << "Edge_pair connects element_id " << edge_pair.processing_element_id_to()
                         << " to itself, as well as OTHER elements";
        return false;
      }

      // ...and that Element must be of type DAI_INTERCONNECT.
      const ElementRecord& record = element_map.at(edge_pair.processing_element_id_from());
      if (*record.element.type() !=
          fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect) {
        FX_LOGS(WARNING) << "Edge_pair connects element_id " << edge_pair.processing_element_id_to()
                         << " to itself, but it is not DAI_INTERCONNECT (is "
                         << record.element.type() << ")";
        return false;
      }
    }
  }

  // Check for topology "violations" based on element type.
  return std::ranges::all_of(element_map, [edge_pairs](const auto& entry) {
    auto id = entry.first;

    if (!entry.second.element.type().has_value()) {
      FX_LOGS(WARNING) << "Element " << id << " has no type";
      return false;
    }

    const auto& element_type = *entry.second.element.type();
    bool has_outgoing_edges = ElementHasOutgoingEdges(edge_pairs, id);
    bool has_incoming_edges = ElementHasIncomingEdges(edge_pairs, id);

    if (has_outgoing_edges && has_incoming_edges) {
      // If an element has incoming AND outgoing edges, it cannot be RingBuffer or PacketStream.
      // Interestingly, this IS allowed for DAI elements.
      if (element_type == fhasp::ElementType::kRingBuffer ||
          element_type == fhasp::ElementType::kPacketStream) {
        FX_LOGS(WARNING) << element_type << " element " << id
                         << " has both incoming edges and outgoing edges!";
        return false;
      }
    }
    if (has_outgoing_edges && !has_incoming_edges) {
      // If the element has no incoming edges, then it must be a DAI, RingBuffer or PacketStream.
      if (element_type != fhasp::ElementType::kDaiInterconnect &&
          element_type != fhasp::ElementType::kRingBuffer &&
          element_type != fhasp::ElementType::kPacketStream) {
        FX_LOGS(WARNING) << "Element " << id << " (" << element_type << ") has no incoming edges!";
        return false;
      }
    }
    if (!has_outgoing_edges && has_incoming_edges) {
      // If the element has no outgoing edges, then it must be a DAI, RingBuffer or PacketStream.
      if (element_type != fhasp::ElementType::kDaiInterconnect &&
          element_type != fhasp::ElementType::kRingBuffer &&
          element_type != fhasp::ElementType::kPacketStream) {
        FX_LOGS(WARNING) << "Element " << id << " (" << element_type << ") has no outgoing edges!";
        return false;
      }
    }
    // if !has_outgoing_edges && !has_incoming_edges for this particular topology, that's OK.
    // (If an element isn't mentioned in ANY topology, ValidateTopologies will fail.)

    // No topology violations were found for this element.
    return true;
  });
}

bool ValidateTopologies(const std::vector<fhasp::Topology>& topologies,
                        const std::unordered_map<ElementId, ElementRecord>& element_map) {
  LogTopologies(topologies);

  if (topologies.empty()) {
    FX_LOGS(WARNING) << "SignalProcessing.topologies[] is empty";
    return false;
  }
  if (element_map.empty()) {
    FX_LOGS(WARNING) << "SignalProcessing.elements[] is empty";
    return false;
  }

  // Check each topology
  if (MapTopologies(topologies).empty()) {
    return false;
  }

  std::unordered_set<ElementId> elements_remaining;
  for (const auto& element_entry_pair : element_map) {
    elements_remaining.insert(element_entry_pair.first);
  }

  for (auto& topology : topologies) {
    if (!ValidateTopology(topology, element_map)) {
      return false;
    }
    for (const auto& edge_pair : *topology.processing_elements_edge_pairs()) {
      elements_remaining.erase(edge_pair.processing_element_id_from());
      elements_remaining.erase(edge_pair.processing_element_id_to());
    }
  }
  if (!elements_remaining.empty()) {
    FX_LOGS(WARNING) << "topologies did not cover all elements. " << elements_remaining.size()
                     << " elements were not included in any topology. Example: element_id "
                     << *elements_remaining.begin();
    return false;
  }

  return true;
}

}  // namespace media_audio
