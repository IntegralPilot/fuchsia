// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "codec_adapter_cvsd_decoder.h"

#include <safemath/safe_math.h>

namespace {

static constexpr char kPcmMimeType[] = "audio/pcm";
static constexpr uint8_t kPcmBitsPerSample = 16;

// The implementations are based off of the CVSD codec algorithm defined in
// Bluetooth Core Spec v5.3, Vol 2, Part B Sec 9.2.
int16_t SingleDecode(CvsdParams* params, const uint8_t& compressed_bit) {
  UpdateCvsdParams(params, compressed_bit);

  int16_t x = Round(params->accumulator);
  return x;
}

}  // namespace

CodecAdapterCvsdDecoder::CodecAdapterCvsdDecoder(std::mutex& lock,
                                                 CodecAdapterEvents* codec_adapter_events)
    : CodecAdapterSWImpl(lock, codec_adapter_events) {}

std::pair<fuchsia::media::FormatDetails, size_t> CodecAdapterCvsdDecoder::OutputFormatDetails() {
  ZX_DEBUG_ASSERT(codec_params_);
  fuchsia::media::PcmFormat out;
  out.pcm_mode = fuchsia::media::AudioPcmMode::LINEAR;
  out.bits_per_sample = kPcmBitsPerSample;
  out.frames_per_second = kExpectedSamplingFreq;
  // For CVSD, we assume MONO channel.
  out.channel_map = {fuchsia::media::AudioChannelId::LF};

  fuchsia::media::AudioUncompressedFormat uncompressed;
  uncompressed.set_pcm(out);

  fuchsia::media::AudioFormat audio_format;
  audio_format.set_uncompressed(std::move(uncompressed));

  fuchsia::media::FormatDetails format_details;
  format_details.set_mime_type(kPcmMimeType);
  format_details.mutable_domain()->set_audio(std::move(audio_format));

  return {std::move(format_details), kOutputFrameSize};
}

CodecAdapterCvsdDecoder::InputLoopStatus CodecAdapterCvsdDecoder::ProcessFormatDetails(
    const fuchsia::media::FormatDetails& format_details) {
  if (!format_details.has_mime_type() || format_details.mime_type() != kCvsdMimeType) {
    events_->onCoreCodecFailCodec(
        "CVSD Decoder received input that was not compressed cvsd audio.");
    return kShouldTerminate;
  }

  InitCvsdParams(codec_params_);
  InitChunkInputStream(format_details);
  return kOk;
}

// Decode input buffer of 1 byte size to produce 16 bytes of output data.
int CodecAdapterCvsdDecoder::ProcessInputChunkData(const uint8_t* input_data,
                                                   size_t input_data_size, uint8_t* output_buffer,
                                                   size_t output_buffer_size) {
  // Since decode does 1 bit to 2 byte (16 bits) decompression, we cast to int16_t
  // pointer.
  int16_t* output = reinterpret_cast<int16_t*>(output_buffer);
  auto num_output_bytes_produced = 0;
  for (int idx = 0; idx < static_cast<int>(input_data_size); idx += 1) {
    // Process one byte of input data.
    uint8_t input_byte = input_data[idx];
    uint8_t mask = 0b10000000;
    for (uint32_t idx = 0; idx < 8; idx++) {
      uint8_t encoded_bit = input_byte & mask ? 1 : 0;
      *output = SingleDecode(&(*codec_params_), encoded_bit);
      mask >>= 1;
      output++;
    }
    num_output_bytes_produced += kOutputFrameSize;
    ZX_DEBUG_ASSERT((int)output_buffer_size >= num_output_bytes_produced);
  }
  return num_output_bytes_produced;
}

fuchsia::sysmem::BufferCollectionConstraints CodecAdapterCvsdDecoder::BufferCollectionConstraints(
    CodecPort port) {
  fuchsia::sysmem::BufferCollectionConstraints c;
  c.min_buffer_count_for_camping = kMinBufferCountForCamping;

  // Actual minimum buffer size is 1 byte for input and 16 bytes for output since we're doing
  // 1:16 decompression for decoding. However, sysmem will basically allocate at least a
  // 4KiB page per buffer even if the client is also ok with less, so we default to
  // system page size for now.
  if (port == kInputPort) {
    c.buffer_memory_constraints.min_size_bytes =
        std::max(zx_system_get_page_size(), kInputFrameSize);
    c.buffer_memory_constraints.max_size_bytes = kInputPerPacketBufferBytesMax;
  } else {
    // TODO(dayeonglee): consider requiring a larger buffer, based on what
    // seems like a minimum reasonable time duration per buffer, to keep the
    // buffers per second <= ~120.
    c.buffer_memory_constraints.min_size_bytes =
        std::max(zx_system_get_page_size(), (uint32_t)MinOutputBufferSize());
    // Set to some arbitrary value.
    c.buffer_memory_constraints.max_size_bytes = 0xFFFFFFFF;
  }

  return c;
}
