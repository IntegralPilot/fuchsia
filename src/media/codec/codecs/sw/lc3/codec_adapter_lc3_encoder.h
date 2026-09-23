// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_CODEC_CODECS_SW_LC3_CODEC_ADAPTER_LC3_ENCODER_H_
#define SRC_MEDIA_CODEC_CODECS_SW_LC3_CODEC_ADAPTER_LC3_ENCODER_H_

#include <lc3.h>
#include <zircon/assert.h>

#include <cstdint>
#include <memory>
#include <vector>

#include "codec_adapter_lc3.h"
#include "codec_adapter_sw_impl.h"

// See LC3 Specifications v1.0 section 2.2 Encoder Interfaces.
struct Lc3EncoderParams {
  std::vector<Lc3CodecContainer<lc3_encoder_t>> encoders;

  const int num_channels;
  // Frame duration in microseconds.
  const int dt_us;
  // Sample rate in Hz.
  const int sr_hz;
  const int nbytes;
  // Number of bits per audio sample enc.
  const lc3_pcm_format fmt;
  // Scratch buffer used when input_data is not aligned to int16_t/int32_t.
  std::vector<int32_t> aligned_input_scratch;
};

class CodecAdapterLc3Encoder : public CodecAdapterSWImpl<Lc3EncoderParams> {
 public:
  explicit CodecAdapterLc3Encoder(std::mutex& lock, CodecAdapterEvents* codec_adapter_events);
  ~CodecAdapterLc3Encoder() = default;

 protected:
  std::pair<fuchsia::media::FormatDetails, size_t> OutputFormatDetails() override;
  CodecAdapterLc3Encoder::InputLoopStatus ProcessFormatDetails(
      const fuchsia::media::FormatDetails& format_details) override;
  int ProcessInputChunkData(const uint8_t* input_data, size_t input_data_size,
                            uint8_t* output_buffer, size_t output_buffer_size) override;
  size_t InputChunkSize() override;
  size_t MinOutputBufferSize() override;
  fuchsia::sysmem::BufferCollectionConstraints BufferCollectionConstraints(
      const CodecPort port) override;
  TimestampExtrapolator CreateTimestampExtrapolator(
      const fuchsia::media::FormatDetails& format_details) override;
};

#endif  // SRC_MEDIA_CODEC_CODECS_SW_LC3_CODEC_ADAPTER_LC3_ENCODER_H_
