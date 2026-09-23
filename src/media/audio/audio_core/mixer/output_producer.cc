// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/mixer/output_producer.h"

#include <lib/fidl/cpp/clone.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <algorithm>
#include <cmath>
#include <limits>
#include <type_traits>

#include "src/media/audio/lib/format2/format.h"
#include "src/media/audio/lib/format2/sample_converter.h"
#include "src/media/audio/lib/format2/stream_converter.h"

namespace media::audio {

OutputProducer::OutputProducer(const ::media_audio::Format& dest_format, int32_t bytes_per_sample)
    : converter_(::media_audio::StreamConverter::CreateFromFloatSource(dest_format)),
      channels_(static_cast<int32_t>(dest_format.channels())),
      bytes_per_sample_(bytes_per_sample),
      bytes_per_frame_(bytes_per_sample * static_cast<int32_t>(dest_format.channels())) {}

void OutputProducer::ProduceOutput(const float* source_ptr, void* dest_void_ptr,
                                   int64_t frames) const {
  converter_.CopyAndClip(source_ptr, dest_void_ptr, frames);
}

void OutputProducer::FillWithSilence(void* dest_void_ptr, int64_t frames) const {
  converter_.WriteSilence(dest_void_ptr, frames);
}

std::unique_ptr<OutputProducer> OutputProducer::Select(
    const fuchsia::media::AudioStreamType& output_format) {
  TRACE_DURATION("audio", "OutputProducer::Select");
  if (output_format.channels == 0u) {
    FX_LOGS(ERROR) << "Invalid output format";
    return nullptr;
  }

  fuchsia_audio::SampleType dest_sample_type;
  int32_t bytes_per_sample;

  switch (output_format.sample_format) {
    case fuchsia::media::AudioSampleFormat::UNSIGNED_8:
      dest_sample_type = fuchsia_audio::SampleType::kUint8;
      bytes_per_sample = sizeof(uint8_t);
      break;
    case fuchsia::media::AudioSampleFormat::SIGNED_16:
      dest_sample_type = fuchsia_audio::SampleType::kInt16;
      bytes_per_sample = sizeof(int16_t);
      break;
    case fuchsia::media::AudioSampleFormat::SIGNED_24_IN_32:
      dest_sample_type = fuchsia_audio::SampleType::kInt32;
      bytes_per_sample = sizeof(int32_t);
      break;
    case fuchsia::media::AudioSampleFormat::FLOAT:
      dest_sample_type = fuchsia_audio::SampleType::kFloat32;
      bytes_per_sample = sizeof(float);
      break;
    default:
      FX_LOGS(ERROR) << "Unsupported output format "
                     << static_cast<int64_t>(output_format.sample_format);
      return nullptr;
  }

  auto dest_format = ::media_audio::Format::CreateOrDie({
      .sample_type = dest_sample_type,
      .channels = output_format.channels,
      .frames_per_second = output_format.frames_per_second,
  });

  return std::make_unique<OutputProducer>(dest_format, bytes_per_sample);
}

}  // namespace media::audio
