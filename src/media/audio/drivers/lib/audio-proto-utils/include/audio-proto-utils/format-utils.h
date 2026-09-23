// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_LIB_AUDIO_PROTO_UTILS_INCLUDE_AUDIO_PROTO_UTILS_FORMAT_UTILS_H_
#define SRC_MEDIA_AUDIO_DRIVERS_LIB_AUDIO_PROTO_UTILS_INCLUDE_AUDIO_PROTO_UTILS_FORMAT_UTILS_H_

#include <fidl/fuchsia.hardware.audio/cpp/wire.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/device/audio.h>
#include <zircon/types.h>

#include <set>
#include <utility>
#include <vector>

namespace audio {
namespace utils {

struct Format {
  fuchsia_hardware_audio::wire::SampleFormat format;
  uint8_t valid_bits_per_sample;
  uint8_t bytes_per_sample;
};

// Check to see if the specified frame rate is in either the 48 KHz or 44.1 KHz
// family.
bool FrameRateIn48kFamily(uint32_t rate);
bool FrameRateIn441kFamily(uint32_t rate);
size_t MaxFrameRatesIn48kFamily();
size_t MaxFrameRatesIn441kFamily();

// Figure out the size of an audio frame based on the sample format.  Returns 0
// in the case of an error (bad channel count, bad sample format)
uint32_t ComputeFrameSize(uint16_t channels, audio_sample_format_t sample_format);

// Expand a packed `audio_sample_format_t` into a vector of Formats.
std::vector<Format> GetAllFormats(audio_sample_format_t sample_format);

// Figure out the sample format based on the audio sample and channel sizes.  Returns 0
// in the case of an error (bad sample and channel sizes)
audio_sample_format_t GetSampleFormat(uint8_t bits_per_sample, uint8_t bits_per_channel);

// Check to see if the specified format (rate, chan, sample_format) is
// compatible with the given format range.  Returns true if it is, or
// false otherwise.
bool FormatIsCompatible(uint32_t frame_rate, uint16_t channels, audio_sample_format_t sample_format,
                        const audio_stream_format_range_t& format_range);

// A small helper class which allows code to use c++11 range-based for loop
// syntax for enumerating discrete frame rates supported by an
// audio_stream_format_range_t in ascending order.  Note that this enumerator will not enumerate
// anything if the frame rate range is continuous.
class FrameRateEnumerator {
 public:
  explicit FrameRateEnumerator(const audio_stream_format_range_t& range);
  std::set<uint32_t>::iterator begin() { return rates_.begin(); }
  std::set<uint32_t>::iterator end() { return rates_.end(); }

 private:
  void InsertRates(const audio_stream_format_range_t& range, cpp20::span<const uint32_t> rates);

  // Guaranteed to be ordered from low to high in the way that is desired.
  std::set<uint32_t> rates_;
};

}  // namespace utils
}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_LIB_AUDIO_PROTO_UTILS_INCLUDE_AUDIO_PROTO_UTILS_FORMAT_UTILS_H_
