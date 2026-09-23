// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_DECODER_H_
#define SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_DECODER_H_

#include <lib/fit/defer.h>
#include <lib/media/codec_impl/codec_adapter.h>

#include "codec_adapter_cvsd.h"
#include "codec_adapter_sw_impl.h"

class CodecAdapterCvsdDecoder : public CodecAdapterSWImpl<CvsdParams> {
 public:
  explicit CodecAdapterCvsdDecoder(std::mutex& lock, CodecAdapterEvents* codec_adapter_events);
  ~CodecAdapterCvsdDecoder() = default;

 protected:
  std::pair<fuchsia::media::FormatDetails, size_t> OutputFormatDetails() override;
  CodecAdapterCvsdDecoder::InputLoopStatus ProcessFormatDetails(
      const fuchsia::media::FormatDetails& format_details) override;
  int ProcessInputChunkData(const uint8_t* input_data, size_t input_data_size,
                            uint8_t* output_buffer, size_t output_buffer_size) override;

  // The minimum frame size (number of input data bytes that can be processed) is 1 byte.
  // For convenience and intuition, we enforce the same for `ChunkInputStream::InputBlock`.
  size_t InputChunkSize() override { return kInputFrameSize; }

  // Buffer size required to process one input chunk.
  size_t MinOutputBufferSize() override { return kOutputFrameSize; }

  fuchsia::sysmem::BufferCollectionConstraints BufferCollectionConstraints(CodecPort port) override;

  TimestampExtrapolator CreateTimestampExtrapolator(
      const fuchsia::media::FormatDetails& format_details) override {
    return TimestampExtrapolator();
  }

 private:
  // CVSD decoder performs 1-16 decompression of data, where each 1 bit
  // of input data is decoded into 2 bytes (16 bits). Therefore, minimum input
  // sample data size is 1 byte.
  static constexpr uint32_t kInputFrameSize = 1;

  // Number of output bytes from processing kInputFrameSize of input data.
  // 1-16 decompression means output frame size is 1 * 16 = 16 bytes.
  static constexpr uint32_t kOutputFrameSize = 16;
};

#endif  // SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_DECODER_H_
