// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/lib/format2/stream_converter.h"

#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.audio/cpp/natural_ostream.h>
#include <lib/syslog/cpp/macros.h>

#include <type_traits>

#include "src/media/audio/lib/format2/sample_converter.h"

namespace media_audio {

namespace {

using ::fuchsia_audio::SampleType;

template <typename SourceSampleType, typename DestSampleType>
void CopyWithConversion(const void* source_samples, void* dest_samples, int64_t sample_count) {
  using SourceConverter = SampleConverter<SourceSampleType>;
  using DestConverter = SampleConverter<DestSampleType>;
  const auto* source_ptr = static_cast<const SourceSampleType*>(source_samples);
  auto* dest_ptr = static_cast<DestSampleType*>(dest_samples);
  for (int64_t i = 0; i < sample_count; ++i) {
    dest_ptr[i] = DestConverter::FromFloat(SourceConverter::ToFloat(source_ptr[i]));
  }
}

template <typename SourceSampleType>
void CopyWithConversion(const Format& dest_format, const void* source_samples, void* dest_samples,
                        int64_t sample_count) {
  switch (dest_format.sample_type()) {
    case SampleType::kUint8:
      CopyWithConversion<SourceSampleType, uint8_t>(source_samples, dest_samples, sample_count);
      break;
    case SampleType::kInt16:
      CopyWithConversion<SourceSampleType, int16_t>(source_samples, dest_samples, sample_count);
      break;
    case SampleType::kInt32:
      CopyWithConversion<SourceSampleType, int32_t>(source_samples, dest_samples, sample_count);
      break;
    case SampleType::kFloat32:
      CopyWithConversion<SourceSampleType, float>(source_samples, dest_samples, sample_count);
      break;
    default:
      FX_LOGS(FATAL) << dest_format.sample_type();
      __builtin_unreachable();
  }
}

void CopyWithConversion(const Format& source_format, const Format& dest_format,
                        const void* source_samples, void* dest_samples, int64_t sample_count) {
  switch (source_format.sample_type()) {
    case SampleType::kUint8:
      CopyWithConversion<uint8_t>(dest_format, source_samples, dest_samples, sample_count);
      break;
    case SampleType::kInt16:
      CopyWithConversion<int16_t>(dest_format, source_samples, dest_samples, sample_count);
      break;
    case SampleType::kInt32:
      CopyWithConversion<int32_t>(dest_format, source_samples, dest_samples, sample_count);
      break;
    case SampleType::kFloat32:
      CopyWithConversion<float>(dest_format, source_samples, dest_samples, sample_count);
      break;
    default:
      FX_LOGS(FATAL) << source_format.sample_type();
      __builtin_unreachable();
  }
}

template <typename SourceSampleType, typename DestSampleType>
void CopyAndClipWithConversion(const void* source_samples, void* dest_samples,
                               int64_t sample_count) {
  using SourceConverter = SampleConverter<SourceSampleType>;
  using DestConverter = SampleConverter<DestSampleType>;
  const auto* source_ptr = static_cast<const SourceSampleType*>(source_samples);
  auto* dest_ptr = static_cast<DestSampleType*>(dest_samples);
  for (int64_t i = 0; i < sample_count; ++i) {
    if constexpr (std::is_same_v<SourceSampleType, float> &&
                  std::is_same_v<DestSampleType, float>) {
      float source = DestConverter::FromFloat(SourceConverter::ToFloat(source_ptr[i]));
      // Clamp the sample for float -> float conversion. Attempts to handle NAN without actually
      // using isnan(), such as comparing the value to itself, were not fruitful.
      dest_ptr[i] = std::clamp<float>(source, -1.0f, 1.0f);
    } else {
      dest_ptr[i] = DestConverter::FromFloat(SourceConverter::ToFloat(source_ptr[i]));
    }
  }
}

template <typename SourceSampleType>
void CopyAndClipWithConversion(const Format& dest_format, const void* source_samples,
                               void* dest_samples, int64_t sample_count) {
  switch (dest_format.sample_type()) {
    case SampleType::kUint8:
      CopyAndClipWithConversion<SourceSampleType, uint8_t>(source_samples, dest_samples,
                                                           sample_count);
      break;
    case SampleType::kInt16:
      CopyAndClipWithConversion<SourceSampleType, int16_t>(source_samples, dest_samples,
                                                           sample_count);
      break;
    case SampleType::kInt32:
      CopyAndClipWithConversion<SourceSampleType, int32_t>(source_samples, dest_samples,
                                                           sample_count);
      break;
    case SampleType::kFloat32:
      CopyAndClipWithConversion<SourceSampleType, float>(source_samples, dest_samples,
                                                         sample_count);
      break;
    default:
      FX_LOGS(FATAL) << dest_format.sample_type();
      __builtin_unreachable();
  }
}

void CopyAndClipWithConversion(const Format& source_format, const Format& dest_format,
                               const void* source_samples, void* dest_samples,
                               int64_t sample_count) {
  switch (source_format.sample_type()) {
    case SampleType::kUint8:
      CopyAndClipWithConversion<uint8_t>(dest_format, source_samples, dest_samples, sample_count);
      break;
    case SampleType::kInt16:
      CopyAndClipWithConversion<int16_t>(dest_format, source_samples, dest_samples, sample_count);
      break;
    case SampleType::kInt32:
      CopyAndClipWithConversion<int32_t>(dest_format, source_samples, dest_samples, sample_count);
      break;
    case SampleType::kFloat32:
      CopyAndClipWithConversion<float>(dest_format, source_samples, dest_samples, sample_count);
      break;
    default:
      FX_LOGS(FATAL) << source_format.sample_type();
      __builtin_unreachable();
  }
}

}  // namespace

StreamConverter::StreamConverter(const Format& source_format, const Format& dest_format)
    : source_format_(source_format),
      dest_format_(dest_format),
      // Conversion needed iff the formats are different or the source samples need clamping.
      should_convert_(source_format_.sample_type() != dest_format_.sample_type() ||
                      source_format_.sample_type() == SampleType::kFloat32) {}

// static
StreamConverter StreamConverter::CreateFromFloatSource(const Format& dest_format) {
  const auto source_format = Format::CreateOrDie({
      .sample_type = SampleType::kFloat32,
      .channels = dest_format.channels(),
      .frames_per_second = dest_format.frames_per_second(),
  });
  return StreamConverter(source_format, dest_format);
}

void StreamConverter::Copy(const void* source_samples, void* dest_samples,
                           int64_t frame_count) const {
  if (should_convert_) {
    CopyWithConversion(source_format_, dest_format_, source_samples, dest_samples,
                       frame_count * dest_format_.channels());
  } else {
    std::memmove(dest_samples, source_samples, frame_count * source_format_.bytes_per_frame());
  }
}

void StreamConverter::CopyAndClip(const void* source_samples, void* dest_samples,
                                  int64_t frame_count) const {
  if (should_convert_) {
    CopyAndClipWithConversion(source_format_, dest_format_, source_samples, dest_samples,
                              frame_count * dest_format_.channels());
  } else {
    std::memmove(dest_samples, source_samples, frame_count * source_format_.bytes_per_frame());
  }
}

void StreamConverter::WriteSilence(void* dest_samples, int64_t frame_count) const {
  if (dest_format().sample_type() == SampleType::kUint8) {
    std::memset(dest_samples, kInt8ToUint8, frame_count * dest_format().channels());
  } else {
    // All other sample formats represent silence with zeroes.
    std::memset(dest_samples, 0, frame_count * dest_format().bytes_per_frame());
  }
}

}  // namespace media_audio
