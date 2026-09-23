// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_H_
#define SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_H_

#include <lib/fit/function.h>
#include <lib/media/codec_impl/codec_buffer.h>
#include <lib/media/codec_impl/codec_packet.h>

#include <cmath>
#include <optional>

// These are default values recommended in Bluetooth Core spec v5.3 section 9.2.
static constexpr uint32_t kK = 4;
static constexpr uint32_t kJ = 4;
static constexpr int16_t kStepDecaySizeMin = 10;    // delta min
static constexpr int16_t kStepDecaySizeMax = 1280;  // delta max
// Step decay (beta) = 1-1/1024 = 1023/1024.
static constexpr int16_t kStepDecayNumerator = 1023;
static constexpr int16_t kStepDecayDenominator = 1024;
// Accumulator decay (h) = 1-1/32 = 31/32.
static constexpr int16_t kAccumDecayNumerator = 31;
static constexpr int16_t kAccumDecayDenominator = 32;
static constexpr int16_t kAccumulatorPosSaturation = 32767;  // y_max = 2^15 - 1
// Table 9.2 allows -2^15 or -2^15+1; we choose -2^15 to match 2's complement,
// we may as well allow using -32768, and because table 9.2 lists -2^15 first.
static constexpr int16_t kAccumulatorNegSaturation = -32768;  // y_min = -2^15

static constexpr char kCvsdMimeType[] = "audio/cvsd";
static constexpr uint64_t kInputBufferConstraintsVersionOrdinal = 1;
static constexpr uint32_t kMinBufferCountForCamping = 1;

// This is an arbitrary cap for now.
constexpr uint32_t kInputPerPacketBufferBytesMax = 4 * 1024 * 1024;

// The input to the CVSD encode shall be 64000 samples per second linear PCM.
static constexpr uint32_t kExpectedSamplingFreq = 64000;

static_assert(kStepDecaySizeMax <= std::numeric_limits<int16_t>::max());
static_assert(kStepDecaySizeMin >= std::numeric_limits<int16_t>::min());
static_assert(kStepDecayNumerator < kStepDecayDenominator);

static_assert(kAccumulatorPosSaturation <= std::numeric_limits<int16_t>::max());
static_assert(kAccumulatorNegSaturation >= std::numeric_limits<int16_t>::min());
static_assert(kAccumDecayNumerator < kAccumDecayDenominator);

// See Bluetooth Core v5.3 section 9.2 CVSD CODEC.
struct CvsdParams {
  const uint32_t k;
  const uint32_t j;
  const uint32_t equal_bit_mask;
  const uint32_t historic_bit_mask;
  uint32_t historic_bits;  // Historic record of outputs bits.
  // Accumulator and step size are kept as double values to keep more
  // precisions for smaller values. Also makes the codec more
  // similar to other CVSD codec implementations.
  double accumulator;  // Referred to as x^ in the algorithm.
  double step_size;    // Referred to as delta in the algorithm.
};

struct OutputItem {
  CodecPacket* packet;
  const CodecBuffer* buffer;

  // Number of valid data bytes currently in the `buffer`.
  uint32_t data_length;
  // Timestamp of the last processed input item.
  std::optional<uint64_t> timestamp;
};

bool AreJbitsEqual(CvsdParams* params);

static inline int16_t Round(const double value) {
  return static_cast<int16_t>(std::floor(value + 0.5));
}

double GetNewStepSize(const CvsdParams& params);

double GetNewAccumulator(const CvsdParams& params, const uint8_t& bit);

void UpdateCvsdParams(CvsdParams* params, uint8_t bit);

void InitCvsdParams(std::optional<CvsdParams>& codec_params);

void SetOutputItem(std::optional<OutputItem>& output_item, CodecPacket* packet,
                   const CodecBuffer* buffer);

#endif  // SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_H_
