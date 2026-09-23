// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_CODEC_CODECS_SW_LC3_CODEC_ADAPTER_LC3_DECODER_H_
#define SRC_MEDIA_CODEC_CODECS_SW_LC3_CODEC_ADAPTER_LC3_DECODER_H_

#include <lc3.h>

#include "codec_adapter_lc3.h"
#include "codec_adapter_sw_impl.h"

// See LC3 Specifications v1.0 section 2.4 Decoder Interfaces.
struct Lc3DecoderParams {
  std::vector<Lc3CodecContainer<lc3_decoder_t>> decoders;
  // Frame duration in microseconds.
  const int dt_us;
  // Sample rate in Hz.
  const int sr_hz;
  const int nbytes;
  std::vector<fuchsia::media::AudioChannelId> channels;
};

class CodecAdapterLc3Decoder : public CodecAdapterSWImpl<Lc3DecoderParams> {
 public:
  explicit CodecAdapterLc3Decoder(std::mutex& lock, CodecAdapterEvents* codec_adapter_events);
  ~CodecAdapterLc3Decoder() = default;

 protected:
  std::pair<fuchsia::media::FormatDetails, size_t> OutputFormatDetails() override;
  CodecAdapterLc3Decoder::InputLoopStatus ProcessFormatDetails(
      const fuchsia::media::FormatDetails& format_details) override;
  int ProcessInputChunkData(const uint8_t* input_data, size_t input_data_size,
                            uint8_t* output_buffer, size_t output_buffer_size) override;
  size_t InputChunkSize() override;
  size_t MinOutputBufferSize() override;
  fuchsia::sysmem::BufferCollectionConstraints BufferCollectionConstraints(CodecPort port) override;

  TimestampExtrapolator CreateTimestampExtrapolator(
      const fuchsia::media::FormatDetails& format_details) override {
    return TimestampExtrapolator();
  }
};

#endif  // SRC_MEDIA_CODEC_CODECS_SW_LC3_CODEC_ADAPTER_LC3_DECODER_H_
