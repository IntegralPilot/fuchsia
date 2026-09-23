// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_ENCODER_H_
#define SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_ENCODER_H_

#include "codec_adapter_cvsd.h"
#include "codec_adapter_sw_impl.h"

class CodecAdapterCvsdEncoder : public CodecAdapterSWImpl<CvsdParams> {
 public:
  explicit CodecAdapterCvsdEncoder(std::mutex& lock, CodecAdapterEvents* codec_adapter_events);
  ~CodecAdapterCvsdEncoder() = default;

 protected:
  std::pair<fuchsia::media::FormatDetails, size_t> OutputFormatDetails() override;
  CodecAdapterCvsdEncoder::InputLoopStatus ProcessFormatDetails(
      const fuchsia::media::FormatDetails& format_details) override;
  int ProcessInputChunkData(const uint8_t* input_data, size_t input_data_size,
                            uint8_t* output_buffer, size_t output_buffer_size) override;

  // The minimum frame size (number of input data bytes that can be processed) is 16 bytes.
  // For convenience and intuition, we make the `ChunkInputStream::InputBlock` size the same.
  size_t InputChunkSize() override { return kInputFrameSize; }

  size_t MinOutputBufferSize() override { return kOutputFrameSize; }
  fuchsia::sysmem::BufferCollectionConstraints BufferCollectionConstraints(
      const CodecPort port) override;

  TimestampExtrapolator CreateTimestampExtrapolator(
      const fuchsia::media::FormatDetails& format_details) override;

 private:
  // Each audio sample is signed 16 bits (2 byte).
  static constexpr uint32_t kInputBytesPerSample = 2;

  // CVSD encoder performs 16-1 compression of data where 2 bytes (16 bits) input
  // produces 1 bit of output. To produce 1 whole output byte, we need
  // 2 byte * 8 = 16 bytes of input.
  static constexpr uint32_t kInputFrameSize = kInputBytesPerSample * 8;

  // Number of output bytes from processing kInputFrameSize of input data.
  // 16-1 compression means output frame size is 16 / 1 = 1 byte.
  static constexpr uint32_t kOutputFrameSize = 1;
};

#endif  // SRC_MEDIA_CODEC_CODECS_SW_CVSD_CODEC_ADAPTER_CVSD_ENCODER_H_
