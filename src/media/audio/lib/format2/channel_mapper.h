// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_LIB_FORMAT2_CHANNEL_MAPPER_H_
#define SRC_MEDIA_AUDIO_LIB_FORMAT2_CHANNEL_MAPPER_H_

#include <array>
#include <numbers>
#include <type_traits>
#include <utility>

#include "src/media/audio/lib/format2/sample_converter.h"

namespace media_audio {

// TODO(https://fxbug.dev/42166075): Remove this workaround, once the device properly maps channels.
inline constexpr bool kEnable4ChannelWorkaround = true;

// Template to map a source frame of `SourceSampleType` with `SourceChannelCount` into each
// destination sample with `DestChannelCount` in a normalized 32-bit float format.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount,
          bool Customizable = false, typename Trait = void>
class ChannelMapper;

// N -> N channel mapper (passthrough).
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == DestChannelCount)>> {
 public:
  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    // Each source channel is directly mapped to the one corresponding dest channel, without change.
    return SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel]);
  }
};

// 1 -> N channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 1 && DestChannelCount > 1)>> {
 public:
  float Map(const SourceSampleType* source_frame, [[maybe_unused]] size_t dest_channel) {
    // The one source channel is directly mapped to every dest channel, without change.
    return SampleConverter<SourceSampleType>::ToFloat(source_frame[0]);
  }
};

// 2 -> 1 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 2 && DestChannelCount == 1)>> {
 public:
  float Map(const SourceSampleType* source_frame, [[maybe_unused]] size_t dest_channel) {
    // The two source channels are equally weighted, in the one dest channel.
    return 0.5f * (SampleConverter<SourceSampleType>::ToFloat(source_frame[0]) +
                   SampleConverter<SourceSampleType>::ToFloat(source_frame[1]));
  }
};

// 2 -> 3 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 2 && DestChannelCount == 3)>> {
 public:
  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    // Each source channel is directly mapped to the corresponding dest channel, without change.
    // For the remaining dest channel 2, source channels 0 and 1 are equally weighted.
    return (dest_channel < 2)
               ? SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel])
               : 0.5f * (SampleConverter<SourceSampleType>::ToFloat(source_frame[0]) +
                         SampleConverter<SourceSampleType>::ToFloat(source_frame[1]));
  }
};

// 2 -> 4 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 2 && DestChannelCount == 4)>> {
 public:
  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    // Source channel 0 is directly mapped to dest channels 0 and 2 both, without change.
    // Source channel 1 is directly mapped to dest channels 1 and 3 both, without change.
    return SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel % 2]);
  }
};

// 3 -> 1 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 3 && DestChannelCount == 1)>> {
 public:
  // The three source channels are equally weighted, in the one dest channel.
  float Map(const SourceSampleType* source_frame, [[maybe_unused]] size_t dest_channel) {
    return kInverseThree * (SampleConverter<SourceSampleType>::ToFloat(source_frame[0]) +
                            SampleConverter<SourceSampleType>::ToFloat(source_frame[1]) +
                            SampleConverter<SourceSampleType>::ToFloat(source_frame[2]));
  }

 private:
  static constexpr float kInverseThree = 1.0f / 3.0f;
};

// 3 -> 2 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 3 && DestChannelCount == 2)>> {
 public:
  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    // Conceptually we map source channel 0 directly to dest channel 0, and source channel 1
    // directly to dest channel 1, and split source channel 2 equally between dest channels 0 & 1.
    // When splitting source channel 2 across multiple dest channels, we should conserve power. This
    // means we contribute .7071 (M_SQRT1_2) of source channel 2 into each dest channel. Because our
    /// max amplitude is 1.7071 (1 + M_SQRT1_2), we must normalize accordingly to reduce this to 1.
    return (SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel]) *
            kInverseOnePlusRootHalf) +
           (SampleConverter<SourceSampleType>::ToFloat(source_frame[2]) * kInverseRootTwoPlusOne);
  }

 private:
  static constexpr float kInverseOnePlusRootHalf = static_cast<float>(1.0 / (1.0 + M_SQRT1_2));
  static constexpr float kInverseRootTwoPlusOne = static_cast<float>(1.0 / (M_SQRT2 + 1.0));
};

// 3 -> 4 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 3 && DestChannelCount == 4)>> {
 public:
  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    // Each source channel is directly mapped to the corresponding dest channel, without change.
    // For the remaining dest channel 3, source channels 0, 1 and 2 are equally weighted.
    return (dest_channel < 3)
               ? SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel])
               : kInverseThree * (SampleConverter<SourceSampleType>::ToFloat(source_frame[0]) +
                                  SampleConverter<SourceSampleType>::ToFloat(source_frame[1]) +
                                  SampleConverter<SourceSampleType>::ToFloat(source_frame[2]));
  }

 private:
  static constexpr float kInverseThree = 1.0f / 3.0f;
};

// 4 -> 1 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 4 && DestChannelCount == 1)>> {
 public:
  float Map(const SourceSampleType* source_frame, [[maybe_unused]] size_t dest_channel) {
    if constexpr (kEnable4ChannelWorkaround) {
      // TODO(https://fxbug.dev/42166075): Temporarily ignore the third and fourth channels.
      //
      // The first two source channels are equally weighted, in the one dest channel.
      return 0.5f * (SampleConverter<SourceSampleType>::ToFloat(source_frame[0]) +
                     SampleConverter<SourceSampleType>::ToFloat(source_frame[1]));
    }
    // All four source channels are equally weighted, in the one dest channel.
    return 0.25f * (SampleConverter<SourceSampleType>::ToFloat(source_frame[0]) +
                    SampleConverter<SourceSampleType>::ToFloat(source_frame[1]) +
                    SampleConverter<SourceSampleType>::ToFloat(source_frame[2]) +
                    SampleConverter<SourceSampleType>::ToFloat(source_frame[3]));
  }
};

// 4 -> 2 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 4 && DestChannelCount == 2)>> {
 public:
  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    if constexpr (kEnable4ChannelWorkaround) {
      // TODO(https://fxbug.dev/42166075): Temporarily ignore the third and fourth channels.
      //
      // Source channels 0 and 1 are directly mapped to dest channels 0 and 1, without change.
      return SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel]);
    }
    // Source channels 0 & 2 are equally weighted in dest channel 0. Ditto source 1 & 3 in dest 1.
    return 0.5f * (SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel]) +
                   SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel + 2]));
  }
};

// 4 -> 3 channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/false,
                    typename std::enable_if_t<(SourceChannelCount == 4 && DestChannelCount == 3)>> {
 public:
  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    if constexpr (kEnable4ChannelWorkaround) {
      // TODO(https://fxbug.dev/42166075): Temporarily ignore the third and fourth channels.
      //
      // The first two source channels are directly mapped to dest channels 0 and 1, without change.
      // For the remaining dest channel 2, source channels 0 and 1 are equally weighted.
      return (dest_channel < 2)
                 ? SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel])
                 : 0.5f * (SampleConverter<SourceSampleType>::ToFloat(source_frame[0]) +
                           SampleConverter<SourceSampleType>::ToFloat(source_frame[1]));
    }
    // Conceptually we map source channels 0, 1, 2 directly to dest channels 0, 1, 2 respectively,
    // and then split source channel 3 equally between dest channels 0, 1 & 2. When splitting source
    // channel 3 in this way, we should conserve power. This means we contribute 0.57735
    // (kRootThird) of source channel 3 into each dest channel, leading to a max amplitude
    // of 1.57735 (1 + kRootThird). This exceeds 1.0, so we must normalize while combining them.
    return (kInverseOnePlusRootThird *
            SampleConverter<SourceSampleType>::ToFloat(source_frame[dest_channel])) +
           (kInverseRootThreePlusOne * SampleConverter<SourceSampleType>::ToFloat(source_frame[3]));
  }

 private:
  static constexpr double kRootThree = std::numbers::sqrt3;
  static constexpr double kRootThird = 1.0 / kRootThree;
  static constexpr float kInverseOnePlusRootThird = static_cast<float>(1.0 / (1.0 + kRootThird));
  static constexpr float kInverseRootThreePlusOne = static_cast<float>(1.0 / (kRootThree + 1.0));
};

// M -> N customizable channel mapper.
template <typename SourceSampleType, size_t SourceChannelCount, size_t DestChannelCount>
class ChannelMapper<SourceSampleType, SourceChannelCount, DestChannelCount, /*Customizable=*/true> {
 public:
  explicit ChannelMapper(
      std::array<std::array<float, SourceChannelCount>, DestChannelCount> coefficients)
      : coefficients_(std::move(coefficients)) {}

  float Map(const SourceSampleType* source_frame, size_t dest_channel) {
    float dest_sample = 0.0f;
    // Produce each dest channel based on the source-channel weightings specified in coefficients_.
    for (size_t source_channel = 0; source_channel < SourceChannelCount; ++source_channel) {
      dest_sample += coefficients_[dest_channel][source_channel] *
                     SampleConverter<SourceSampleType>::ToFloat(source_frame[source_channel]);
    }
    return dest_sample;
  }

 private:
  // Normalized channel coefficients.
  std::array<std::array<float, SourceChannelCount>, DestChannelCount> coefficients_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_LIB_FORMAT2_CHANNEL_MAPPER_H_
