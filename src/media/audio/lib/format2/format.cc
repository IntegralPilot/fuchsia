// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/lib/format2/format.h"

#include <fidl/fuchsia.audio/cpp/natural_ostream.h>
#include <fidl/fuchsia.audio/cpp/natural_types.h>
#include <fidl/fuchsia.audio/cpp/wire_types.h>
#include <fidl/fuchsia.mediastreams/cpp/wire_types.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>

#include <ostream>
#include <string>

#include "src/media/audio/lib/format2/fixed.h"

namespace media_audio {

using fuchsia_audio::SampleType;
using media::TimelineRate;

namespace {

std::optional<SampleType> ConvertHardwareFormatToSampleType(
    fuchsia_hardware_audio::wire::PcmFormat msg) {
  switch (msg.sample_format) {
    case fuchsia_hardware_audio::SampleFormat::kPcmSigned:
      switch (msg.bytes_per_sample) {
        case 2:
          return SampleType::kInt16;
        case 4:
          return SampleType::kInt32;
        default:
          return std::nullopt;
      }

    case fuchsia_hardware_audio::SampleFormat::kPcmUnsigned:
      if (msg.bytes_per_sample == 1) {
        return SampleType::kUint8;
      }
      return std::nullopt;

    case fuchsia_hardware_audio::SampleFormat::kPcmFloat:
      switch (msg.bytes_per_sample) {
        case 4:
          return SampleType::kFloat32;
        case 8:
          return SampleType::kFloat64;
        default:
          return std::nullopt;
      }

    default:
      return std::nullopt;
  }
}

fuchsia_hardware_audio::SampleFormat ToHardwareSampleFormat(SampleType sample_type) {
  switch (sample_type) {
    case SampleType::kUint8:
      return fuchsia_hardware_audio::SampleFormat::kPcmUnsigned;
    case SampleType::kInt16:
    case SampleType::kInt32:
      return fuchsia_hardware_audio::SampleFormat::kPcmSigned;
    case SampleType::kFloat32:
    case SampleType::kFloat64:
      return fuchsia_hardware_audio::SampleFormat::kPcmFloat;
    default:
      FX_LOGS(FATAL) << "unexpected sample_type '" << sample_type << "'";
      __builtin_unreachable();
  }
}

std::optional<SampleType> ConvertLegacySampleType(fuchsia_mediastreams::AudioSampleFormat fmt) {
  switch (fmt) {
    case fuchsia_mediastreams::AudioSampleFormat::kUnsigned8:
      return SampleType::kUint8;
    case fuchsia_mediastreams::AudioSampleFormat::kSigned16:
      return SampleType::kInt16;
    case fuchsia_mediastreams::AudioSampleFormat::kSigned24In32:
      return SampleType::kInt32;
    case fuchsia_mediastreams::AudioSampleFormat::kFloat:
      return SampleType::kFloat32;
    default:
      return std::nullopt;
  }
}

std::optional<SampleType> ConvertLegacySampleType(fuchsia_media::AudioSampleFormat fmt) {
  switch (fmt) {
    case fuchsia_media::AudioSampleFormat::kUnsigned8:
      return SampleType::kUint8;
    case fuchsia_media::AudioSampleFormat::kSigned16:
      return SampleType::kInt16;
    case fuchsia_media::AudioSampleFormat::kSigned24In32:
      return SampleType::kInt32;
    case fuchsia_media::AudioSampleFormat::kFloat:
      return SampleType::kFloat32;
    default:
      return std::nullopt;
  }
}

}  // namespace

fpromise::result<Format, std::string> Format::Create(fuchsia_audio::wire::Format msg) {
  if (!msg.has_sample_type()) {
    return fpromise::error("missing required field (sample_type)");
  }
  if (!msg.has_channel_count()) {
    return fpromise::error("missing required field (channel_count)");
  }
  if (!msg.has_frames_per_second()) {
    return fpromise::error("missing required field (frames_per_second)");
  }

  return Create(Args{
      .sample_type = msg.sample_type(),
      .channels = msg.channel_count(),
      .frames_per_second = msg.frames_per_second(),
  });
}

fpromise::result<Format, std::string> Format::Create(const fuchsia_audio::Format& msg) {
  if (!msg.sample_type().has_value()) {
    return fpromise::error("missing required field (sample_type)");
  }
  if (!msg.channel_count().has_value()) {
    return fpromise::error("missing required field (channel_count)");
  }
  if (!msg.frames_per_second().has_value()) {
    return fpromise::error("missing required field (frames_per_second)");
  }

  return Create(Args{
      .sample_type = *msg.sample_type(),
      .channels = *msg.channel_count(),
      .frames_per_second = *msg.frames_per_second(),
  });
}

fpromise::result<Format, std::string> Format::Create(fuchsia_hardware_audio::wire::Format msg) {
  if (!msg.has_pcm_format()) {
    return fpromise::error("missing required field (pcm_format)");
  }
  return Create(msg.pcm_format());
}

fpromise::result<Format, std::string> Format::Create(fuchsia_hardware_audio::wire::PcmFormat msg) {
  // The valid bits property is ignored, except to verify that it is not too large.
  if (msg.valid_bits_per_sample > 8 * msg.bytes_per_sample) {
    return fpromise::error("incompatible valid_bits_per_sample '" +
                           std::to_string(msg.valid_bits_per_sample) + "' and bytes_per_sample '" +
                           std::to_string(msg.bytes_per_sample));
  }

  auto sample_type = ConvertHardwareFormatToSampleType(msg);
  if (!sample_type) {
    return fpromise::error("incompatible sample_format '" +
                           std::to_string(fidl::ToUnderlying(msg.sample_format)) +
                           "' and bytes_per_sample '" + std::to_string(msg.bytes_per_sample));
  }

  return Create(Args{
      .sample_type = *sample_type,
      .channels = msg.number_of_channels,
      .frames_per_second = msg.frame_rate,
  });
}

fpromise::result<Format, std::string> Format::Create(const fuchsia_hardware_audio::Format& msg) {
  if (!msg.pcm_format()) {
    return fpromise::error("missing required field (pcm_format)");
  }
  return Create(*msg.pcm_format());
}

fpromise::result<Format, std::string> Format::Create(const fuchsia_hardware_audio::PcmFormat& msg) {
  return Create(fuchsia_hardware_audio::wire::PcmFormat{
      .number_of_channels = msg.number_of_channels(),
      .sample_format = msg.sample_format(),
      .bytes_per_sample = msg.bytes_per_sample(),
      .valid_bits_per_sample = msg.valid_bits_per_sample(),
      .frame_rate = msg.frame_rate(),
  });
}

fpromise::result<Format, std::string> Format::Create(Args args) {
  switch (args.sample_type) {
    case SampleType::kUint8:
    case SampleType::kInt16:
    case SampleType::kInt32:
    case SampleType::kFloat32:
    case SampleType::kFloat64:
      break;
    default:
      return fpromise::error("bad sample_type '" +
                             std::to_string(fidl::ToUnderlying(args.sample_type)) + "'");
  }

  // TODO(https://fxbug.dev/42066207): Validate channel and fps limits once those are defined.
  // For now just validate they are not zero.
  if (args.channels == 0) {
    return fpromise::error("bad channel_count '" + std::to_string(args.channels) + "'");
  }
  if (args.frames_per_second == 0) {
    return fpromise::error("bad frames_per_second '" + std::to_string(args.frames_per_second) +
                           "'");
  }

  return fpromise::ok(Format(args.sample_type, args.channels, args.frames_per_second));
}

Format Format::CreateOrDie(fuchsia_audio::wire::Format msg) {
  auto result = Create(msg);
  if (!result.is_ok()) {
    FX_CHECK(false) << "Format::CreateOrDie failed: " << result.error();
  }
  return result.take_value();
}

Format Format::CreateOrDie(const fuchsia_audio::Format& msg) {
  auto result = Create(msg);
  if (!result.is_ok()) {
    FX_CHECK(false) << "Format::CreateOrDie failed: " << result.error();
  }
  return result.take_value();
}

Format Format::CreateOrDie(Args args) {
  auto result = Create(args);
  if (!result.is_ok()) {
    FX_CHECK(false) << "Format::CreateOrDie failed: " << result.error();
  }
  return result.take_value();
}

fpromise::result<Format, std::string> Format::CreateLegacy(
    const fuchsia_mediastreams::AudioFormat& msg) {
  auto sample_type = ConvertLegacySampleType(msg.sample_format());
  if (!sample_type) {
    return fpromise::error("bad sample_format '" +
                           std::to_string(fidl::ToUnderlying(msg.sample_format())) + "'");
  }

  fidl::Arena<> arena;
  return Create(fuchsia_audio::wire::Format::Builder(arena)
                    .sample_type(*sample_type)
                    .channel_count(msg.channel_count())
                    .frames_per_second(msg.frames_per_second())
                    .Build());
}

fpromise::result<Format, std::string> Format::CreateLegacy(
    fuchsia_mediastreams::wire::AudioFormat msg) {
  auto sample_type = ConvertLegacySampleType(msg.sample_format);
  if (!sample_type) {
    return fpromise::error("bad sample_format '" +
                           std::to_string(fidl::ToUnderlying(msg.sample_format)) + "'");
  }

  fidl::Arena<> arena;
  return Create(fuchsia_audio::wire::Format::Builder(arena)
                    .sample_type(*sample_type)
                    .channel_count(msg.channel_count)
                    .frames_per_second(msg.frames_per_second)
                    .Build());
}

Format Format::CreateLegacyOrDie(const fuchsia_mediastreams::AudioFormat& msg) {
  auto result = CreateLegacy(msg);
  if (!result.is_ok()) {
    FX_CHECK(false) << "Format::CreateLegacyOrDie failed: " << result.error();
  }
  return result.take_value();
}

Format Format::CreateLegacyOrDie(fuchsia_mediastreams::wire::AudioFormat msg) {
  auto result = CreateLegacy(msg);
  if (!result.is_ok()) {
    FX_CHECK(false) << "Format::CreateLegacyOrDie failed: " << result.error();
  }
  return result.take_value();
}

fpromise::result<Format, std::string> Format::CreateLegacy(
    const fuchsia_media::AudioStreamType& msg) {
  auto sample_type = ConvertLegacySampleType(msg.sample_format());
  if (!sample_type) {
    return fpromise::error("bad sample_format '" +
                           std::to_string(fidl::ToUnderlying(msg.sample_format())) + "'");
  }

  fidl::Arena<> arena;
  return Create(fuchsia_audio::wire::Format::Builder(arena)
                    .sample_type(*sample_type)
                    .channel_count(msg.channels())
                    .frames_per_second(msg.frames_per_second())
                    .Build());
}

fpromise::result<Format, std::string> Format::CreateLegacy(
    fuchsia_media::wire::AudioStreamType msg) {
  auto sample_type = ConvertLegacySampleType(msg.sample_format);
  if (!sample_type) {
    return fpromise::error("bad sample_format '" +
                           std::to_string(fidl::ToUnderlying(msg.sample_format)) + "'");
  }

  fidl::Arena<> arena;
  return Create(fuchsia_audio::wire::Format::Builder(arena)
                    .sample_type(*sample_type)
                    .channel_count(msg.channels)
                    .frames_per_second(msg.frames_per_second)
                    .Build());
}

Format Format::CreateLegacyOrDie(const fuchsia_media::AudioStreamType& msg) {
  auto result = CreateLegacy(msg);
  if (!result.is_ok()) {
    FX_CHECK(false) << "Format::CreateLegacyOrDie failed: " << result.error();
  }
  return result.take_value();
}

Format Format::CreateLegacyOrDie(fuchsia_media::wire::AudioStreamType msg) {
  auto result = CreateLegacy(msg);
  if (!result.is_ok()) {
    FX_CHECK(false) << "Format::CreateLegacyOrDie failed: " << result.error();
  }
  return result.take_value();
}

Format::Format(fuchsia_audio::SampleType sample_type, int64_t channels, int64_t frames_per_second)
    : sample_type_(sample_type), channels_(channels), frames_per_second_(frames_per_second) {
  // Caller has validated that the parameters are valid, so just precompute the remaining fields.
  int32_t bytes_per_sample;
  switch (sample_type_) {
    case SampleType::kUint8:
      bytes_per_sample = 1;
      valid_bits_per_sample_ = 8;
      break;
    case SampleType::kInt16:
      bytes_per_sample = 2;
      valid_bits_per_sample_ = 16;
      break;
    case SampleType::kInt32:
    case SampleType::kFloat32:
      bytes_per_sample = 4;
      valid_bits_per_sample_ = 32;
      break;
    case SampleType::kFloat64:
      bytes_per_sample = 8;
      valid_bits_per_sample_ = 64;
      break;
    default:
      FX_LOGS(FATAL) << "unexpected sample format " << sample_type_;
      __builtin_unreachable();
  }

  bytes_per_frame_ = bytes_per_sample * channels_;
  frames_per_ns_ = TimelineRate(frames_per_second_, zx::sec(1).to_nsecs());
  frac_frames_per_ns_ = TimelineRate(Fixed(frames_per_second_).raw_value(), zx::sec(1).to_nsecs());
}

bool Format::operator==(const Format& rhs) const {
  // All other fields are derived from these types.
  return sample_type_ == rhs.sample_type_ && channels_ == rhs.channels_ &&
         frames_per_second_ == rhs.frames_per_second_;
}

fuchsia_audio::wire::Format Format::ToWireFidl(fidl::AnyArena& arena) const {
  return fuchsia_audio::wire::Format::Builder(arena)
      .sample_type(sample_type_)
      .channel_count(static_cast<uint32_t>(channels_))
      .frames_per_second(static_cast<uint32_t>(frames_per_second_))
      .Build();
}

fuchsia_audio::Format Format::ToNaturalFidl() const {
  fuchsia_audio::Format msg;
  msg.sample_type() = sample_type_;
  msg.channel_count() = static_cast<uint32_t>(channels_);
  msg.frames_per_second() = static_cast<uint32_t>(frames_per_second_);
  return msg;
}

fuchsia_hardware_audio::wire::Format Format::ToHardwareWireFidl(fidl::AnyArena& arena) const {
  return fuchsia_hardware_audio::wire::Format::Builder(arena)
      .pcm_format(fuchsia_hardware_audio::wire::PcmFormat{
          .number_of_channels = static_cast<uint8_t>(channels_),
          .sample_format = ToHardwareSampleFormat(sample_type_),
          .bytes_per_sample = static_cast<uint8_t>(bytes_per_sample()),
          .valid_bits_per_sample = static_cast<uint8_t>(8 * bytes_per_sample()),
          .frame_rate = static_cast<uint32_t>(frames_per_second_),
      })
      .Build();
}

fuchsia_hardware_audio::Format Format::ToHardwareNaturalFidl() const {
  return fuchsia_hardware_audio::Format({
      .pcm_format = fuchsia_hardware_audio::PcmFormat({
          .number_of_channels = static_cast<uint8_t>(channels_),
          .sample_format = ToHardwareSampleFormat(sample_type_),
          .bytes_per_sample = static_cast<uint8_t>(bytes_per_sample()),
          .valid_bits_per_sample = static_cast<uint8_t>(8 * bytes_per_sample()),
          .frame_rate = static_cast<uint32_t>(frames_per_second_),
      }),
  });
}

fuchsia_media::wire::AudioStreamType Format::ToLegacyMediaWireFidl() const {
  auto sample_format = fuchsia_media::AudioSampleFormat::kFloat;
  switch (sample_type_) {
    case SampleType::kUint8:
      sample_format = fuchsia_media::AudioSampleFormat::kUnsigned8;
      break;
    case SampleType::kInt16:
      sample_format = fuchsia_media::AudioSampleFormat::kSigned16;
      break;
    case SampleType::kInt32:
      sample_format = fuchsia_media::AudioSampleFormat::kSigned24In32;
      break;
    case SampleType::kFloat32:
      sample_format = fuchsia_media::AudioSampleFormat::kFloat;
      break;
    default:
      FX_LOGS(FATAL) << "unexpected sample format " << sample_type_;
      break;
  }
  return {
      .sample_format = sample_format,
      .channels = static_cast<uint32_t>(channels_),
      .frames_per_second = static_cast<uint32_t>(frames_per_second_),
  };
}

fuchsia_mediastreams::wire::AudioFormat Format::ToLegacyMediastreamsWireFidl() const {
  auto sample_format = fuchsia_mediastreams::AudioSampleFormat::kFloat;
  switch (sample_type_) {
    case SampleType::kUint8:
      sample_format = fuchsia_mediastreams::AudioSampleFormat::kUnsigned8;
      break;
    case SampleType::kInt16:
      sample_format = fuchsia_mediastreams::AudioSampleFormat::kSigned16;
      break;
    case SampleType::kInt32:
      sample_format = fuchsia_mediastreams::AudioSampleFormat::kSigned24In32;
      break;
    case SampleType::kFloat32:
      sample_format = fuchsia_mediastreams::AudioSampleFormat::kFloat;
      break;
    default:
      FX_LOGS(FATAL) << "unexpected sample format " << sample_type_;
      break;
  }
  return {
      .sample_format = sample_format,
      .channel_count = static_cast<uint32_t>(channels_),
      .frames_per_second = static_cast<uint32_t>(frames_per_second_),
      .channel_layout = fuchsia_mediastreams::wire::AudioChannelLayout::WithPlaceholder(0),
  };
}

int64_t Format::integer_frames_per(zx::duration duration, TimelineRate::RoundingMode mode) const {
  return frames_per_ns_.Scale(duration.to_nsecs(), mode);
}

Fixed Format::frac_frames_per(zx::duration duration, TimelineRate::RoundingMode mode) const {
  return Fixed::FromRaw(frac_frames_per_ns_.Scale(duration.to_nsecs(), mode));
}

int64_t Format::bytes_per(zx::duration duration, TimelineRate::RoundingMode mode) const {
  return bytes_per_frame_ * integer_frames_per(duration, mode);
}

zx::duration Format::duration_per(Fixed frames, media::TimelineRate::RoundingMode mode) const {
  return zx::duration(frac_frames_per_ns_.Inverse().Scale(frames.raw_value(), mode));
}

std::ostream& operator<<(std::ostream& out, const Format& format) {
  out << format.frames_per_second() << "hz-" << format.channels() << "ch-";

  switch (format.sample_type()) {
    case SampleType::kUint8:
      out << "u8";
      break;
    case SampleType::kInt16:
      out << "i16";
      break;
    case SampleType::kInt32:
      out << "i24";
      break;
    case SampleType::kFloat32:
      out << "f32";
      break;
    case SampleType::kFloat64:
      out << "f64";
      break;
    default:
      FX_CHECK(false) << format.sample_type();
  }

  return out;
}

}  // namespace media_audio
