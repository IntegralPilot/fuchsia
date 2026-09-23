// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "codec_adapter_lc3_encoder.h"

#include <lib/media/codec_impl/codec_port.h>
#include <lib/media/codec_impl/log.h>

#include <cstring>
#include <unordered_set>

#include <safemath/safe_math.h>

#include "codec_adapter_sw.h"
#include "lc3.h"

namespace {

bool IsAcceptableSamplingFrequency(const uint32_t freq) {
  // See LC3 specification v1.0 section 2.2 and 2.4 for acceptable sampling
  // frequency values for the uncompressed audio.
  static const std::unordered_set<uint32_t> supported{8000, 16000, 24000, 32000, 44100, 48000};
  return supported.find(freq) != supported.end();
}

bool IsAcceptableBitsPerUncompAudioSample(const uint32_t bits_per_sample) {
  // See LC3 specification v1.0 section 2.2 and 2.4 for acceptable bits per
  // audio sample.
  // Fuchsia LC3 codec does not support 32 bit.
  static const std::unordered_set<uint32_t> supported{16, 24};
  return supported.find(bits_per_sample) != supported.end();
}

Lc3EncoderParams CreateEncoderParams(const fuchsia::media::FormatDetails& format_details) {
  auto& input_format = format_details.domain().audio().uncompressed().pcm();
  auto& settings = format_details.encoder_settings().lc3();

  auto num_channels = static_cast<int>(input_format.channel_map.size());
  std::vector<Lc3CodecContainer<lc3_encoder_t>> encoders;

  int frame_us =
      (settings.frame_duration() == fuchsia::media::Lc3FrameDuration::D10_MS) ? 10000 : 7500;

  // According to LC3 Specification v1.0 section 2.1, when the sampling frequency of the
  // input signal is 44.1 kHz, the same frame length is used as for 48 kHz.
  int sampling_freq = (input_format.frames_per_second == 44100)
                          ? 48000
                          : static_cast<int>(input_format.frames_per_second);
  encoders.reserve(num_channels);
  auto encoder_size = lc3_encoder_size(frame_us, sampling_freq);

  // Set up an encoder for each channel to account for multi-channeled interleaved audio.
  for (int i = 0; i < num_channels; ++i) {
    encoders.emplace_back(
        [&frame_us, &sampling_freq](void* mem) {
          // Sets up and returns the pointer to the encoder struct. The pointer has the same value
          // as `mem`.
          return lc3_setup_encoder(frame_us, sampling_freq, 0, mem);
        },
        encoder_size);
  }

  return Lc3EncoderParams{
      .encoders = std::move(encoders),
      .num_channels = num_channels,
      .dt_us = frame_us,
      .sr_hz = sampling_freq,
      .nbytes = static_cast<int>(settings.nbytes()),
      .fmt = (input_format.bits_per_sample == 16) ? LC3_PCM_FORMAT_S16 : LC3_PCM_FORMAT_S24,
  };
}

}  // namespace

CodecAdapterLc3Encoder::CodecAdapterLc3Encoder(std::mutex& lock,
                                               CodecAdapterEvents* codec_adapter_events)
    : CodecAdapterSWImpl(lock, codec_adapter_events) {}

std::pair<fuchsia::media::FormatDetails, size_t> CodecAdapterLc3Encoder::OutputFormatDetails() {
  ZX_DEBUG_ASSERT(codec_params_);
  fuchsia::media::AudioCompressedFormatLc3 lc3;
  fuchsia::media::AudioCompressedFormat compressed_format;
  compressed_format.set_lc3(std::move(lc3));

  fuchsia::media::AudioFormat audio_format;
  audio_format.set_compressed(std::move(compressed_format));

  fuchsia::media::FormatDetails format_details;
  format_details.set_mime_type(kLc3MimeType);
  format_details.mutable_domain()->set_audio(std::move(audio_format));

  return {std::move(format_details), MinOutputBufferSize()};
}

CodecAdapterLc3Encoder::InputLoopStatus CodecAdapterLc3Encoder::ProcessFormatDetails(
    const fuchsia::media::FormatDetails& format_details) {
  if (codec_params_.has_value()) {
    events_->onCoreCodecFailCodec("LC3 Encoder: Midstream input format change is not supported.");
    return kShouldTerminate;
  }
  if (!format_details.has_domain() || !format_details.domain().is_audio() ||
      !format_details.domain().audio().is_uncompressed() ||
      !format_details.domain().audio().uncompressed().is_pcm()) {
    events_->onCoreCodecFailCodec(
        "LC3 Encoder received input that was not uncompressed pcm audio.");
    return kShouldTerminate;
  }
  if (!format_details.has_encoder_settings() || !format_details.encoder_settings().is_lc3()) {
    events_->onCoreCodecFailCodec("LC3 Encoder did not receive encoder settings.");
    return kShouldTerminate;
  }
  auto& input_format = format_details.domain().audio().uncompressed().pcm();
  auto& settings = format_details.encoder_settings().lc3();

  if (input_format.channel_map.empty() || input_format.channel_map.size() > kMaxChannelCount) {
    events_->onCoreCodecFailCodec("Unsupported channel count for LC3 Encoder: %zu",
                                  input_format.channel_map.size());
    return kShouldTerminate;
  }

  if (input_format.pcm_mode != fuchsia::media::AudioPcmMode::LINEAR ||
      !IsAcceptableBitsPerUncompAudioSample(input_format.bits_per_sample)) {
    events_->onCoreCodecFailCodec("Unsupported bits per sample LC3 Encoder.");
    return kShouldTerminate;
  }

  if (!IsAcceptableSamplingFrequency(input_format.frames_per_second)) {
    events_->onCoreCodecFailCodec("Unsupported sampling frequency for LC3 Encoder.");
    return kShouldTerminate;
  }

  if (!settings.has_nbytes() || settings.nbytes() < kMinExternalByteCount ||
      settings.nbytes() > kMaxExternalByteCount) {
    events_->onCoreCodecFailCodec("Byte count should be between [20 ... 400] bytes per channel.");
    return kShouldTerminate;
  }

  if (!settings.has_frame_duration() ||
      (settings.frame_duration() != fuchsia::media::Lc3FrameDuration::D10_MS &&
       settings.frame_duration() != fuchsia::media::Lc3FrameDuration::D7P5_MS)) {
    events_->onCoreCodecFailCodec("Invalid or missing frame duration for LC3 Encoder.");
    return kShouldTerminate;
  }

  codec_params_.emplace(CreateEncoderParams(format_details));
  InitChunkInputStream(format_details);
  return kOk;
}

// Encode one frame of interleaved PCM input samples across all channels into LC3 output bytes.
int CodecAdapterLc3Encoder::ProcessInputChunkData(const uint8_t* input_data, size_t input_data_size,
                                                  uint8_t* output_buffer,
                                                  size_t output_buffer_size) {
  ZX_DEBUG_ASSERT(codec_params_);
  if (input_data_size != InputChunkSize() || output_buffer_size < MinOutputBufferSize()) {
    return -1;
  }

  const lc3_pcm_format pcm_fmt = codec_params_->fmt;
  const size_t required_alignment =
      (pcm_fmt == LC3_PCM_FORMAT_S16) ? alignof(int16_t) : alignof(int32_t);
  const uint8_t* aligned_input = input_data;
  if (reinterpret_cast<uintptr_t>(input_data) % required_alignment != 0) {
    // ChunkInputStream passes input_packet.data_at_offset() directly when a packet has at least
    // InputChunkSize() unread bytes. Because client-supplied packets can have arbitrary
    // start_offset or valid_length_bytes (which can leave subsequent chunks at unaligned byte
    // offsets within a packet buffer), input_data is not guaranteed to be aligned to
    // int16_t/int32_t. Copy unaligned input chunk data to an aligned scratch buffer to avoid
    // Undefined Behavior when liblc3 dereferences sample pointers in lc3_encode().
    const size_t num_words = (input_data_size + sizeof(int32_t) - 1) / sizeof(int32_t);
    if (codec_params_->aligned_input_scratch.size() < num_words) {
      codec_params_->aligned_input_scratch.resize(num_words);
    }
    std::memcpy(codec_params_->aligned_input_scratch.data(), input_data, input_data_size);
    aligned_input = reinterpret_cast<const uint8_t*>(codec_params_->aligned_input_scratch.data());
  }

  int nch = codec_params_->num_channels;
  uint8_t* buffer = output_buffer;

  int bytes_produced = 0;
  for (int ich = 0; ich < nch; ich++) {
    // Ensure we have enough space.
    ZX_DEBUG_ASSERT(output_buffer_size >=
                    static_cast<size_t>(bytes_produced + codec_params_->nbytes));

    // In the case of 24 bits per input sample, we pad each sample to 32 bits with low 8 bits zero.
    const void* pcm =
        (pcm_fmt == LC3_PCM_FORMAT_S16)
            ? static_cast<const void*>(reinterpret_cast<const int16_t*>(aligned_input) + ich)
            : static_cast<const void*>(reinterpret_cast<const int32_t*>(aligned_input) + ich);
    if (lc3_encode(codec_params_->encoders[ich].GetCodec(), pcm_fmt, pcm, nch,
                   codec_params_->nbytes, buffer) != 0) {
      return -1;
    }
    bytes_produced += codec_params_->nbytes;
    buffer += codec_params_->nbytes;
  }
  return bytes_produced;
}

fuchsia::sysmem::BufferCollectionConstraints CodecAdapterLc3Encoder::BufferCollectionConstraints(
    CodecPort port) {
  fuchsia::sysmem::BufferCollectionConstraints c;
  if (port == kInputPort) {
    c.min_buffer_count_for_camping = kMinInputBufferCountForCamping;

    c.buffer_memory_constraints.min_size_bytes = zx_system_get_page_size();
    c.buffer_memory_constraints.max_size_bytes = kInputPerPacketBufferBytesMax;
  } else {
    c.min_buffer_count_for_camping = kMinOutputBufferCountForCamping;

    ZX_ASSERT(codec_params_.has_value());
    c.buffer_memory_constraints.min_size_bytes = static_cast<uint32_t>(MinOutputBufferSize());
    c.buffer_memory_constraints.max_size_bytes = 0xFFFFFFFF;  // arbitrary value.
  }

  return c;
}

size_t CodecAdapterLc3Encoder::InputChunkSize() {
  ZX_DEBUG_ASSERT(codec_params_);
  // CreateEncoderParams supports LC3_PCM_FORMAT_S16 and LC3_PCM_FORMAT_S24,
  // which are 2 and 4 bytes per sample respectively.
  size_t num_bytes_per_sample = (codec_params_->fmt == LC3_PCM_FORMAT_S16) ? 2 : 4;

  // LC3 Spec v1.0 section 2.2. Encoder Interfaces.
  // The total size of an input frame is specified by the session
  // configured number of channels, the frame size in samples, and the
  // configured encoder PCM bits per audio sample.
  int frame_samples = lc3_frame_samples(codec_params_->dt_us, codec_params_->sr_hz);
  ZX_DEBUG_ASSERT(frame_samples > 0);
  return static_cast<size_t>(codec_params_->num_channels) * static_cast<size_t>(frame_samples) *
         num_bytes_per_sample;
}

size_t CodecAdapterLc3Encoder::MinOutputBufferSize() {
  ZX_DEBUG_ASSERT(codec_params_);

  // LC3 Spec v1.0 section 2.2. Encoder Interfaces.
  // Expected output frame size is combined byte_count for all the channels.
  return static_cast<size_t>(codec_params_->nbytes) *
         static_cast<size_t>(codec_params_->num_channels);
}

TimestampExtrapolator CodecAdapterLc3Encoder::CreateTimestampExtrapolator(
    const fuchsia::media::FormatDetails& format_details) {
  ZX_DEBUG_ASSERT(codec_params_);

  if (format_details.has_timebase()) {
    const auto& input_format = format_details.domain().audio().uncompressed().pcm();
    // CreateEncoderParams supports LC3_PCM_FORMAT_S16 and LC3_PCM_FORMAT_S24,
    // which are 2 and 4 bytes per sample respectively. We can't use
    // input_format.bits_per_sample / 8 as num_bytes_per_sample because 24 in
    // bits_per_sample means 4 bytes per sample not 3.
    size_t num_bytes_per_sample = (codec_params_->fmt == LC3_PCM_FORMAT_S16) ? 2 : 4;
    // Bytes per second is input PCM sampling frequency * number_of_channels * bytes_per_sample.
    // Note that when the input signal is 44.1 kHz, liblc3 uses the 48 kHz frame sample count
    // (480 samples for 10 ms nominal interval, 360 samples for 7.5 ms nominal interval) via
    // codec_params_->sr_hz in InputChunkSize(). Dividing that 480-sample chunk size by
    // input_format.frames_per_second (44.1 kHz) yields the actual frame duration of 10.884 ms
    // (or 8.16 ms for 7.5 ms interval) per LC3 specification v1.0 section 2.1.
    const size_t bytes_per_second = static_cast<size_t>(codec_params_->num_channels) *
                                    static_cast<size_t>(input_format.frames_per_second) *
                                    num_bytes_per_sample;
    return TimestampExtrapolator(format_details.timebase(), bytes_per_second);
  }
  return TimestampExtrapolator();
}
