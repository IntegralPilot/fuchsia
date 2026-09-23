// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/common_unittest.h"

#include <fidl/fuchsia.audio.device/cpp/fidl.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

namespace {

fha::PcmFormat SafePcmFormatFromSupportedFormats(
    const std::vector<fha::SupportedFormats2>& format_sets) {
  for (const auto& format_set : format_sets) {
    if (format_set.pcm_supported_formats()) {
      auto pcm_supported_formats = format_set.pcm_supported_formats().value();
      fha::PcmFormat format{{
          .number_of_channels = static_cast<uint8_t>(
              pcm_supported_formats.channel_sets()->front().attributes()->size()),
          .sample_format = pcm_supported_formats.sample_formats()->front(),
          .bytes_per_sample = pcm_supported_formats.bytes_per_sample()->front(),
          .valid_bits_per_sample = pcm_supported_formats.valid_bits_per_sample()->front(),
          .frame_rate = pcm_supported_formats.frame_rates()->front(),
      }};
      if (ValidatePcmFormat(format)) {
        return format;
      }
    }
  }
  ADD_FAILURE() << "No valid PCM format found in supported format sets";
  return fha::PcmFormat();
}

fha::Encoding SafeEncodingFromSupportedFormats(
    const std::vector<fha::SupportedFormats2>& format_sets) {
  for (const auto& format_set : format_sets) {
    if (format_set.supported_encodings()) {
      auto supported_encodings = format_set.supported_encodings().value();
      auto min_bitrate = supported_encodings.min_encoding_bitrate().value_or(0);
      auto max_bitrate = supported_encodings.max_encoding_bitrate().value_or(0);
      const auto decoded_channel_count = static_cast<uint32_t>(
          supported_encodings.decoded_channel_sets()->front().attributes()->size());
      const auto decoded_frame_rate = supported_encodings.decoded_frame_rates()->front();
      // Choose a bitrate based on a reasonable compression ratio relative to standard PCM.
      // Since uncompressed PCM bitrate is (channels * rate * bits_per_sample) bits/sec,
      // a factor of 2 results in an 8x compression ratio for 16-bit PCM (16/2=8)
      // or 4x for 8-bit PCM (8/2=4) or 16x for 32-bit PCM (32/2=16) and so on.
      uint32_t bitrate = decoded_channel_count * decoded_frame_rate * 2;

      if (max_bitrate > 0) {
        bitrate = std::clamp(bitrate, min_bitrate, max_bitrate);
      } else if (min_bitrate > 0) {
        bitrate = std::max(bitrate, min_bitrate);
      }
      fha::Encoding format{{
          .decoded_channel_count = decoded_channel_count,
          .decoded_frame_rate = decoded_frame_rate,
          .average_encoding_bitrate = bitrate,
          .encoding_type = supported_encodings.encoding_types()->front(),
      }};
      if (ValidateEncoding(format)) {
        return format;
      }
    }
  }
  ADD_FAILURE() << "No valid encoding format found in supported format sets";
  return fha::Encoding();
}

// From many fad::PcmFormatSet, get a fuchsia_audio::Format.
fuchsia_audio::Format SafeRingBufferFormatFromRingBufferFormatSets(
    const std::vector<fad::PcmFormatSet>& ring_buffer_format_sets) {
  return {{
      .sample_type = ring_buffer_format_sets.front().sample_types()->front(),
      .channel_count = ring_buffer_format_sets.front().channel_sets()->front().attributes()->size(),
      .frames_per_second = ring_buffer_format_sets.front().frame_rates()->front(),
  }};
}

// From many SupportedFormats, get a Format.
fha::Format2 SafeDriverRingBufferFormatFromDriverRingBufferFormatSets(
    const std::vector<fha::SupportedFormats2>& driver_ring_buffer_format_sets) {
  return fha::Format2::WithPcmFormat(
      SafePcmFormatFromSupportedFormats(driver_ring_buffer_format_sets));
}

}  // namespace

////////////////////////////////////////////////////////////////////////////////////////////////
// Helper functions that are useful for both low- (Device) and high-level (AdrServer) unittests.
//

///////////////////////////////
// Codec-related functions
//
// From a multi-element collection, each with many DaiSupportedFormats, get a DaiFormat.
fha::DaiFormat SafeDaiFormatFromElementDaiFormatSets(
    ElementId element_id, const std::vector<fad::ElementDaiFormatSet>& element_dai_format_sets) {
  std::vector<fha::DaiSupportedFormats> dai_format_sets;
  for (const auto& element_entry : element_dai_format_sets) {
    if (element_entry.element_id() && *element_entry.element_id() == element_id) {
      return SafeDaiFormatFromDaiFormatSets(*element_entry.format_sets());
    }
  }
  ADD_FAILURE()
      << "SafeDaiFormatFromDaiFormatSets: No element_dai_format_sets entry found with specified element_id "
      << element_id;
  return {{}};
}

// From a multi-element collection, each with many DaiSupportedFormats, get a DIFFERENT DaiFormat.
fha::DaiFormat SecondDaiFormatFromElementDaiFormatSets(
    ElementId element_id, const std::vector<fad::ElementDaiFormatSet>& element_dai_format_sets) {
  std::vector<fha::DaiSupportedFormats> dai_format_sets;
  for (const auto& element_entry : element_dai_format_sets) {
    if (element_entry.element_id() && *element_entry.element_id() == element_id) {
      return SecondDaiFormatFromDaiFormatSets(*element_entry.format_sets());
    }
  }
  ADD_FAILURE() << "Could not create a second valid DaiFormat for specified element_id "
                << element_id;
  return {{}};
}

// From a multi-element collection, each with many DaiSupportedFormats,
// get a DaiFormat that is UNSUPPORTED (but still a valid format).
fha::DaiFormat UnsupportedDaiFormatFromElementDaiFormatSets(
    ElementId element_id, const std::vector<fad::ElementDaiFormatSet>& element_dai_format_sets) {
  std::vector<fha::DaiSupportedFormats> dai_format_sets;
  for (const auto& element_entry : element_dai_format_sets) {
    if (element_entry.element_id() && *element_entry.element_id() == element_id) {
      return UnsupportedDaiFormatFromDaiFormatSets(*element_entry.format_sets());
    }
  }
  ADD_FAILURE()
      << "UnsupportedDaiFormatFromDaiFormatSets could not find an invalid dai_format for element_id "
      << element_id;
  return {{}};
}

// From many DaiSupportedFormats, get a DaiFormat.
fha::DaiFormat SafeDaiFormatFromDaiFormatSets(
    const std::vector<fha::DaiSupportedFormats>& dai_format_sets) {
  fha::DaiFormat dai_format{{
      .number_of_channels = dai_format_sets[0].number_of_channels()[0],
      .channels_to_use_bitmask = (dai_format_sets[0].number_of_channels()[0] < 64
                                      ? (1ull << dai_format_sets[0].number_of_channels()[0]) - 1ull
                                      : 0xFFFFFFFFFFFFFFFFull),
      .sample_format = dai_format_sets[0].sample_formats()[0],
      .frame_format = dai_format_sets[0].frame_formats()[0],
      .frame_rate = dai_format_sets[0].frame_rates()[0],
      .bits_per_slot = dai_format_sets[0].bits_per_slot()[0],
      .bits_per_sample = dai_format_sets[0].bits_per_sample()[0],
  }};
  if (!ValidateDaiFormat(dai_format)) {
    ADD_FAILURE() << "first entries did not create a valid DaiFormat";
  }

  return dai_format;
}

// From many DaiSupportedFormats, get a DIFFERENT DaiFormat.
fha::DaiFormat SecondDaiFormatFromDaiFormatSets(
    const std::vector<fha::DaiSupportedFormats>& dai_format_sets) {
  auto safe_format_2 = SafeDaiFormatFromDaiFormatSets(dai_format_sets);

  if (safe_format_2.channels_to_use_bitmask() > 1) {
    safe_format_2.channels_to_use_bitmask() -= 1;
  } else if (dai_format_sets[0].number_of_channels().size() > 1) {
    safe_format_2.number_of_channels() = dai_format_sets[0].number_of_channels()[1];
  } else if (dai_format_sets[0].sample_formats().size() > 1) {
    safe_format_2.sample_format() = dai_format_sets[0].sample_formats()[1];
  } else if (dai_format_sets[0].frame_formats().size() > 1) {
    safe_format_2.frame_format() = dai_format_sets[0].frame_formats()[1];
  } else if (dai_format_sets[0].frame_rates().size() > 1) {
    safe_format_2.frame_rate() = dai_format_sets[0].frame_rates()[1];
  } else if (dai_format_sets[0].bits_per_slot().size() > 1) {
    safe_format_2.bits_per_slot() = dai_format_sets[0].bits_per_slot()[1];
  } else if (dai_format_sets[0].bits_per_sample().size() > 1) {
    safe_format_2.bits_per_sample() = dai_format_sets[0].bits_per_sample()[1];
  } else if (dai_format_sets.size() > 1) {
    return fha::DaiFormat{{
        .number_of_channels = dai_format_sets[1].number_of_channels()[0],
        .channels_to_use_bitmask =
            (dai_format_sets[1].number_of_channels()[0] < 64
                 ? (1ull << dai_format_sets[1].number_of_channels()[0]) - 1ull
                 : 0xFFFFFFFFFFFFFFFFull),
        .sample_format = dai_format_sets[1].sample_formats()[0],
        .frame_format = dai_format_sets[1].frame_formats()[0],
        .frame_rate = dai_format_sets[1].frame_rates()[0],
        .bits_per_slot = dai_format_sets[1].bits_per_slot()[0],
        .bits_per_sample = dai_format_sets[1].bits_per_sample()[0],
    }};

  } else {
    ADD_FAILURE() << "Dai format set has only one possible valid format";
    return {{}};
  }
  if (!ValidateDaiFormat(safe_format_2)) {
    ADD_FAILURE() << "Could not create a second valid DaiFormat";
  }
  return safe_format_2;
}

// From many DaiSupportedFormats, get a DaiFormat that is UNSUPPORTED (but still valid).
fha::DaiFormat UnsupportedDaiFormatFromDaiFormatSets(
    const std::vector<fha::DaiSupportedFormats>& dai_format_sets) {
  auto dai_format = SafeDaiFormatFromDaiFormatSets(dai_format_sets);
  if (dai_format.number_of_channels() > 1) {
    dai_format.number_of_channels() -= 1;
    dai_format.channels_to_use_bitmask() = (1ull << dai_format.number_of_channels()) - 1ull;
  } else if (dai_format.frame_rate() > kMinSupportedDaiFrameRate) {
    dai_format.frame_rate() -= 1;
  } else if (dai_format.bits_per_slot() > 1) {
    dai_format.bits_per_slot() -= 1;
  } else if (dai_format.bits_per_sample() > 1) {
    dai_format.bits_per_sample() -= 1;
  } else {
    ADD_FAILURE() << "No unsupported DaiFormat found for these format_sets";
    return {{}};
  }
  return dai_format;
}

///////////////////////////////
// RingBuffer-related functions
//
// From a multi-element collection, each with many fad::PcmFormatSet, get a fa::Format.
fuchsia_audio::Format SafeRingBufferFormatFromElementRingBufferFormatSets(
    ElementId element_id,
    const std::vector<fad::ElementRingBufferFormatSet>& element_ring_buffer_format_sets) {
  std::vector<fuchsia_audio::Format> ring_buffer_format_sets;
  for (const auto& element_entry : element_ring_buffer_format_sets) {
    if (element_entry.element_id() && *element_entry.element_id() == element_id) {
      return SafeRingBufferFormatFromRingBufferFormatSets(*element_entry.format_sets());
    }
  }
  ADD_FAILURE()
      << "SafeRingBufferFormatFromElementRingBufferFormatSets: No element_ring_buffer_format_sets entry found with specified element_id "
      << element_id;
  return {};
}

// From a multi-element collection, each with many SupportedFormats, get a Format.
fha::Format2 SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets(
    ElementId element_id,
    const std::vector<std::pair<ElementId, std::vector<fha::SupportedFormats2>>>&
        element_driver_ring_buffer_format_sets) {
  for (const auto& element_entry : element_driver_ring_buffer_format_sets) {
    if (element_entry.first == element_id) {
      return SafeDriverRingBufferFormatFromDriverRingBufferFormatSets(element_entry.second);
    }
  }
  ADD_FAILURE()
      << "SafeDriverRingBufferFormatFromElementDriverRingBufferFormatSets: No element_driver_ring_buffer_format_sets entry found with specified element_id "
      << element_id;
  return fha::Format2::WithPcmFormat(fha::PcmFormat());
}

///////////////////////////////
// PacketStream-related functions
//
// From many SupportedFormats, get a Format.
fha::Format2 SafeDriverPcmPacketStream(
    const std::vector<fha::SupportedFormats2>& driver_packet_stream_format_sets) {
  for (const auto& format_set : driver_packet_stream_format_sets) {
    if (format_set.Which() == fha::SupportedFormats2::Tag::kPcmSupportedFormats) {
      return fha::Format2::WithPcmFormat(
          SafePcmFormatFromSupportedFormats(driver_packet_stream_format_sets));
    }
  }
  ADD_FAILURE() << "SafeDriverPcmPacketStream: No valid format found";
  return fha::Format2::WithPcmFormat(fha::PcmFormat());
}

fha::Format2 SafeDriverEncodedPacketStream(
    const std::vector<fha::SupportedFormats2>& driver_packet_stream_format_sets) {
  for (const auto& format_set : driver_packet_stream_format_sets) {
    if (format_set.Which() == fha::SupportedFormats2::Tag::kSupportedEncodings) {
      return fha::Format2::WithEncoding(
          SafeEncodingFromSupportedFormats(driver_packet_stream_format_sets));
    }
  }
  ADD_FAILURE() << "SafeDriverEncodedPacketStream: No valid format found";
  return fha::Format2::WithPcmFormat(fha::PcmFormat());
}

fha::Format2 SafeDriverPcmPacketStream(
    ElementId element_id,
    const std::vector<std::pair<ElementId, std::vector<fha::SupportedFormats2>>>&
        element_driver_packet_stream_format_sets) {
  for (const auto& element_entry : element_driver_packet_stream_format_sets) {
    if (element_entry.first == element_id) {
      return SafeDriverPcmPacketStream(element_entry.second);
    }
  }
  ADD_FAILURE() << "SafeDriverPcmPacketStream: No element entry found for element_id "
                << element_id;
  return fha::Format2::WithPcmFormat(fha::PcmFormat());
}

fha::Format2 SafeDriverEncodedPacketStream(
    ElementId element_id,
    const std::vector<std::pair<ElementId, std::vector<fha::SupportedFormats2>>>&
        element_driver_packet_stream_format_sets) {
  for (const auto& element_entry : element_driver_packet_stream_format_sets) {
    if (element_entry.first == element_id) {
      return SafeDriverEncodedPacketStream(element_entry.second);
    }
  }
  ADD_FAILURE() << "SafeDriverEncodedPacketStream: No element entry found for element_id "
                << element_id;
  return fha::Format2::WithPcmFormat(fha::PcmFormat());
}

std::vector<fha::Format2> SafeDriverPacketStreamFormats(
    const std::vector<fha::SupportedFormats2>& driver_packet_stream_format_sets) {
  std::vector<fha::Format2> formats;
  for (const auto& format_set : driver_packet_stream_format_sets) {
    if (format_set.Which() == fha::SupportedFormats2::Tag::kPcmSupportedFormats) {
      formats.push_back(
          fha::Format2::WithPcmFormat(SafePcmFormatFromSupportedFormats(std::vector{format_set})));
    }
    if (format_set.Which() == fha::SupportedFormats2::Tag::kSupportedEncodings) {
      formats.push_back(
          fha::Format2::WithEncoding(SafeEncodingFromSupportedFormats(std::vector{format_set})));
    }
  }
  return formats;
}

std::vector<fha::Format2> SafeDriverPacketStreamFormats(
    ElementId element_id,
    const std::vector<std::pair<ElementId, std::vector<fha::SupportedFormats2>>>&
        element_driver_packet_stream_format_sets) {
  for (const auto& element_entry : element_driver_packet_stream_format_sets) {
    if (element_entry.first == element_id) {
      return SafeDriverPacketStreamFormats(element_entry.second);
    }
  }
  ADD_FAILURE() << "SafeDriverPacketStreamFormats: No element entry found for element_id "
                << element_id;
  return {};
}

fad::PacketStreamFormat SafePcmPacketStreamFormat(
    const std::vector<fad::PacketStreamSupportedFormats>& packet_stream_format_sets) {
  for (const auto& format_set : packet_stream_format_sets) {
    if (format_set.Which() == fad::PacketStreamSupportedFormats::Tag::kPcmFormat) {
      auto& pcm_set = format_set.pcm_format().value();
      return fad::PacketStreamFormat::WithPcmFormat(fuchsia_audio::Format{{
          .sample_type = pcm_set.sample_types()->front(),
          .channel_count = pcm_set.channel_sets()->front().attributes()->size(),
          .frames_per_second = pcm_set.frame_rates()->front(),
      }});
    }
  }
  ADD_FAILURE() << "SafePcmPacketStreamFormat: No PCM format found";
  return fad::PacketStreamFormat(::fidl::internal::DefaultConstructPossiblyInvalidObjectTag{});
}

fad::PacketStreamFormat SafeEncodedPacketStreamFormat(
    const std::vector<fad::PacketStreamSupportedFormats>& packet_stream_format_sets) {
  for (const auto& format_set : packet_stream_format_sets) {
    if (format_set.Which() == fad::PacketStreamSupportedFormats::Tag::kSupportedEncodings) {
      auto& encoding_set = format_set.supported_encodings().value();
      return fad::PacketStreamFormat::WithEncoding(fuchsia_hardware_audio::Encoding{{
          .decoded_channel_count =
              encoding_set.decoded_channel_sets()->front().attributes()->size(),
          .decoded_frame_rate = encoding_set.decoded_frame_rates()
                                    ? encoding_set.decoded_frame_rates()->front()
                                    : 48000,
          .average_encoding_bitrate =
              encoding_set.max_encoding_bitrate() ? *encoding_set.max_encoding_bitrate() : 128000,
          .encoding_type = encoding_set.encoding_types()->front(),
      }});
    }
  }
  ADD_FAILURE() << "SafeEncodedPacketStreamFormat: No encoded format found";
  return fad::PacketStreamFormat(::fidl::internal::DefaultConstructPossiblyInvalidObjectTag{});
}

fad::PacketStreamFormat SafePcmPacketStreamFormat(
    ElementId element_id,
    const std::vector<fad::ElementPacketStreamFormatSet>& element_packet_stream_format_sets) {
  for (const auto& element_entry : element_packet_stream_format_sets) {
    if (element_entry.element_id() && *element_entry.element_id() == element_id) {
      return SafePcmPacketStreamFormat(*element_entry.format_sets());
    }
  }
  ADD_FAILURE() << "SafePcmPacketStreamFormat: No element entry found for element_id "
                << element_id;
  return fad::PacketStreamFormat(::fidl::internal::DefaultConstructPossiblyInvalidObjectTag{});
}

fad::PacketStreamFormat SafeEncodedPacketStreamFormat(
    ElementId element_id,
    const std::vector<fad::ElementPacketStreamFormatSet>& element_packet_stream_format_sets) {
  for (const auto& element_entry : element_packet_stream_format_sets) {
    if (element_entry.element_id() && *element_entry.element_id() == element_id) {
      return SafeEncodedPacketStreamFormat(*element_entry.format_sets());
    }
  }
  ADD_FAILURE() << "SafeEncodedPacketStreamFormat: No element entry found for element_id "
                << element_id;
  return fad::PacketStreamFormat(::fidl::internal::DefaultConstructPossiblyInvalidObjectTag{});
}

std::vector<fad::PacketStreamFormat> SafePacketStreamFormats(
    const std::vector<fad::PacketStreamSupportedFormats>& packet_stream_format_sets) {
  std::vector<fad::PacketStreamFormat> formats;
  bool pcm_found = false;
  bool encoded_found = false;
  for (const auto& format_set : packet_stream_format_sets) {
    if (format_set.Which() == fad::PacketStreamSupportedFormats::Tag::kPcmFormat && !pcm_found) {
      auto format = SafePcmPacketStreamFormat(packet_stream_format_sets);
      formats.push_back(std::move(format));
      pcm_found = true;
    }
    if (format_set.Which() == fad::PacketStreamSupportedFormats::Tag::kSupportedEncodings &&
        !encoded_found) {
      auto format = SafeEncodedPacketStreamFormat(packet_stream_format_sets);
      formats.push_back(std::move(format));
      encoded_found = true;
    }
  }
  if (formats.empty()) {
    ADD_FAILURE() << "SafePacketStreamFormats: No supported formats found";
  }
  return formats;
}

std::vector<fad::PacketStreamFormat> SafePacketStreamFormats(
    ElementId element_id,
    const std::vector<fad::ElementPacketStreamFormatSet>& element_packet_stream_format_sets) {
  for (const auto& element_entry : element_packet_stream_format_sets) {
    if (element_entry.element_id() && *element_entry.element_id() == element_id) {
      if (element_entry.format_sets()) {
        return SafePacketStreamFormats(*element_entry.format_sets());
      }
    }
  }
  ADD_FAILURE() << "SafePacketStreamFormats: No element entry found for element_id " << element_id;
  return {};
}
}  // namespace media_audio
