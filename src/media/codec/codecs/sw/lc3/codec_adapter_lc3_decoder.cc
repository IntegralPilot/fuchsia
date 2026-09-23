// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "codec_adapter_lc3_decoder.h"

#include <lib/media/codec_impl/codec_port.h>
#include <lib/media/codec_impl/log.h>
#include <netinet/in.h>

#include <cstring>
#include <unordered_set>

#include <safemath/safe_math.h>

#include "codec_adapter_sw.h"

namespace {

// Note: According to LC3 Specification v1.0 section 2.3,
// bits per audio sample for decoder's output PCM may differ
// from encoder input PCM setting's bits per audio sample.
// For simplicity, we always decode the output as 16-bit PCM audio data.
// It is possible for encoder to use 24-bit PCM input audio, in which case
// it will pad with additional 8-bits.
constexpr uint8_t kNumBitsPerPcmSample = 16;
constexpr size_t kNumBytesPerPcmSample = 2;

constexpr char kPcmMimeType[] = "audio/pcm";

// Parameter type numbers as defined by Bluetooth Assigned Numbers
// section 6.12.5 Codec_Specific_Configuration LTV structures.
constexpr uint8_t kSamplingFreqParamType = 0x01;
constexpr uint8_t kFrameDurationParamType = 0x02;
constexpr uint8_t kAudioChannelAllocParamType = 0x03;
constexpr uint8_t kOctetsPerCodecFrameParamType = 0x04;

// Minimum oob_bytes size for containing params for sampling freq, frame duration, audio channel
// alloc, and octets per codec frame.
constexpr size_t kMinOobBytesSize = 3 + 3 + 6 + 4;

const std::unordered_set<uint8_t> kRequiredLTVParams{
    kSamplingFreqParamType, kFrameDurationParamType, kAudioChannelAllocParamType,
    kOctetsPerCodecFrameParamType};

std::pair<uint8_t, std::vector<uint8_t>> ProcessLTVParam(const std::vector<uint8_t>& oob_bytes,
                                                         size_t idx, size_t len) {
  ZX_ASSERT(len >= 1);
  ZX_ASSERT(len <= oob_bytes.size() - idx);
  uint8_t key = oob_bytes[idx];  // Type is always 1 byte long.
  std::vector<uint8_t> value(oob_bytes.begin() + idx + 1, oob_bytes.begin() + idx + len);
  return std::make_pair(key, std::move(value));
}

// According to LC3 Specification v1.0 section 2.1, when the sampling frequency of the
// signal is 44.1 kHz, liblc3 uses the 48 kHz internal rate and frame length (480 samples
// for 10 ms frame interval, 360 samples for 7.5 ms frame interval).
constexpr int LibLc3SampleRateHz(int sr_hz) { return (sr_hz == 44100) ? 48000 : sr_hz; }

// Assigned Numbers section 6.12.5.1 Sampling_Frequency.
// Get the sampling frequency in Hz from the raw LTV parameter value.
// If the sampling frequency is not one of the acceptable values, return nullopt.
std::optional<int> GetSamplingFrequencyHz(const std::vector<uint8_t>& raw_bytes) {
  if (raw_bytes.size() != 1) {
    LOG(DEBUG, "Invalid Sampling_Frequency LTV length %zu", raw_bytes.size());
    return std::nullopt;
  }
  uint8_t value = raw_bytes[0];
  int sr_hz;
  if (value == 0x01) {
    sr_hz = 8000;
  } else if (value == 0x03) {
    sr_hz = 16000;
  } else if (value == 0x05) {
    sr_hz = 24000;
  } else if (value == 0x06) {
    sr_hz = 32000;
  } else if (value == 0x07) {
    sr_hz = 44100;
  } else if (value == 0x08) {
    sr_hz = 48000;
  } else {
    // Any other values are not acceptable for LC3 codec.
    LOG(DEBUG, "Invalid Sampling_Frequency LTV value %u", value);
    return std::nullopt;
  }
  return sr_hz;
}

// Assigned Numbers section 6.12.5.2 Frame_Duration.
// Get the frame duration in microseconds from the raw LTV parameter value.
// If the frame duration is not one of the acceptable values, return nullopt.
std::optional<int> GetFrameDurationUs(const std::vector<uint8_t>& raw_bytes) {
  if (raw_bytes.size() != 1) {
    LOG(DEBUG, "Invalid Frame_Duration LTV length %zu", raw_bytes.size());
    return std::nullopt;
  }
  uint8_t value = raw_bytes[0];
  int dt_us;
  if (value == 0x00) {
    dt_us = 7500;
  } else if (value == 0x01) {
    dt_us = 10000;
  } else {
    LOG(DEBUG, "Invalid Frame_Duration LTV value %u", value);
    return std::nullopt;
  }
  return dt_us;
}

// Assigned Numbers section 6.12.5.3 Audio_Channel_Allocation.
// Get the channel allocation from the raw LTV parameter value.
// If any of the channels is not one of the acceptable values, return nullopt.
std::optional<std::vector<fuchsia::media::AudioChannelId>> GetAudioChannelMap(
    const std::vector<uint8_t>& raw_bytes) {
  if (raw_bytes.size() != 4) {
    LOG(DEBUG, "Invalid Audio_Channel_Allocation LTV length %zu", raw_bytes.size());
    return std::nullopt;
  }

  // oob_bytes data assumes big endian encoding. Convert from big endian to host endian.
  uint32_t raw_value;
  std::memcpy(&raw_value, raw_bytes.data(), sizeof(raw_value));
  uint32_t value = ntohl(raw_value);

  // Fuchsia media currently supports:
  // - left front (LF)
  // - right front (RF)
  // - center front (CF)
  // - left surround (LS)
  // - right surround (RS)
  // - low frequency effects (LFE)
  // - back surround (CS)
  // - left rear (LR)
  // - right rear (RR)
  //
  // Values from Assigned Numbers section 6.12.1 Audio Location Definitions
  // that map to the above supported values are acceptable; other values are
  // not. Note that Assigned Numbers does not have a value that maps to CS.
  const uint32_t LF_FLAG = 0x00000001;
  const uint32_t RF_FLAG = 0x00000002;
  const uint32_t CF_FLAG = 0x00000004;
  const uint32_t LFE_FLAG = 0x00000008;
  const uint32_t LR_FLAG = 0x00000010;
  const uint32_t RR_FLAG = 0x00000020;
  const uint32_t LS_FLAG = 0x04000000;
  const uint32_t RS_FLAG = 0x08000000;

  const uint32_t ACCEPTABLE_MASK =
      LF_FLAG | RF_FLAG | CF_FLAG | LFE_FLAG | LR_FLAG | RR_FLAG | LS_FLAG | RS_FLAG;
  if (value == 0 || (value | ACCEPTABLE_MASK) != ACCEPTABLE_MASK) {
    // If the channel contains channel ID value that's not supported by fuchsia,
    // or if no channels are allocated, we shouldn't process.
    LOG(DEBUG, "Invalid Audio_Channel_Allocation LTV value %u", value);
    return std::nullopt;
  }

  std::vector<fuchsia::media::AudioChannelId> channels;
  // If present, channels are added in the following order:
  // LF -> RF -> CF -> LFE -> LR -> RR -> LS -> RS.
  if ((value & LF_FLAG) == LF_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::LF);
  }
  if ((value & RF_FLAG) == RF_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::RF);
  }
  if ((value & CF_FLAG) == CF_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::CF);
  }
  if ((value & LFE_FLAG) == LFE_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::LFE);
  }
  if ((value & LR_FLAG) == LR_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::LR);
  }
  if ((value & RR_FLAG) == RR_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::RR);
  }
  if ((value & LS_FLAG) == LS_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::LS);
  }
  if ((value & RS_FLAG) == RS_FLAG) {
    channels.push_back(fuchsia::media::AudioChannelId::RS);
  }

  ZX_DEBUG_ASSERT(channels.size() <= kMaxChannelCount);
  return channels;
}

// Assigned Numbers section 6.12.5.4 Octets_Per_Codec_Frame.
// Get the number of octets per codec frame from the raw LTV parameter value.
// If the octet count is not one of the acceptable values, return nullopt.
std::optional<int> GetNBytes(const std::vector<uint8_t>& raw_bytes) {
  if (raw_bytes.size() != 2) {
    LOG(DEBUG, "Invalid Octets_Per_Codec_Frame LTV length %zu", raw_bytes.size());
    return std::nullopt;
  }

  // oob_bytes data assumes big endian encoding. Convert from big endian to host endian.
  uint16_t raw_value;
  std::memcpy(&raw_value, raw_bytes.data(), sizeof(raw_value));
  uint16_t value = ntohs(raw_value);

  if (value < kMinExternalByteCount || value > kMaxExternalByteCount) {
    LOG(DEBUG, "Invalid Octets_Per_Codec_Frame %u. Acceptable values are between [20 .. 400].",
        value);
    return std::nullopt;
  }
  return static_cast<int>(value);
}
}  // namespace

CodecAdapterLc3Decoder::CodecAdapterLc3Decoder(std::mutex& lock,
                                               CodecAdapterEvents* codec_adapter_events)
    : CodecAdapterSWImpl(lock, codec_adapter_events) {}

std::pair<fuchsia::media::FormatDetails, size_t> CodecAdapterLc3Decoder::OutputFormatDetails() {
  ZX_DEBUG_ASSERT(codec_params_);
  fuchsia::media::PcmFormat out;
  out.pcm_mode = fuchsia::media::AudioPcmMode::LINEAR;
  // For simplicity, we always decode the output as 16-bit PCM audio data.
  out.bits_per_sample = kNumBitsPerPcmSample;
  out.frames_per_second = static_cast<uint32_t>(codec_params_->sr_hz);
  out.channel_map = codec_params_->channels;

  fuchsia::media::AudioUncompressedFormat uncompressed;
  uncompressed.set_pcm(out);

  fuchsia::media::AudioFormat audio_format;
  audio_format.set_uncompressed(std::move(uncompressed));

  fuchsia::media::FormatDetails format_details;
  format_details.set_mime_type(kPcmMimeType);
  format_details.mutable_domain()->set_audio(std::move(audio_format));

  return {std::move(format_details), MinOutputBufferSize()};
}

// Decode one frame of LC3 compressed input bytes across all channels into interleaved PCM samples.
int CodecAdapterLc3Decoder::ProcessInputChunkData(const uint8_t* input_data, size_t input_data_size,
                                                  uint8_t* output_buffer,
                                                  size_t output_buffer_size) {
  ZX_DEBUG_ASSERT(codec_params_);
  if (input_data_size != InputChunkSize() || output_buffer_size < MinOutputBufferSize()) {
    return -1;
  }

  const uint8_t* input = input_data;
  ZX_DEBUG_ASSERT(reinterpret_cast<uintptr_t>(output_buffer) % alignof(int16_t) == 0);
  int16_t* out_pcm = reinterpret_cast<int16_t*>(output_buffer);

  int nch = static_cast<int>(codec_params_->channels.size());
  int bytes_produced = 0;
  const int num_expected_output_bytes =
      lc3_frame_samples(codec_params_->dt_us, LibLc3SampleRateHz(codec_params_->sr_hz)) *
      kNumBytesPerPcmSample;

  for (int ich = 0; ich < nch; ++ich) {
    ZX_DEBUG_ASSERT(output_buffer_size >=
                    static_cast<size_t>(bytes_produced + num_expected_output_bytes));

    // We always decode the output as 16-bit PCM audio data.
    // lc3_decode returns 0 on normal decode, 1 when Packet Loss Concealment (PLC) synthesized
    // concealed audio for a corrupted frame, and < 0 on invalid parameters.
    if (lc3_decode(codec_params_->decoders[ich].GetCodec(), input, codec_params_->nbytes,
                   LC3_PCM_FORMAT_S16, out_pcm + ich, nch) < 0) {
      return -1;
    }
    bytes_produced += num_expected_output_bytes;
    input += codec_params_->nbytes;
  }
  return bytes_produced;
}

fuchsia::sysmem::BufferCollectionConstraints CodecAdapterLc3Decoder::BufferCollectionConstraints(
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

size_t CodecAdapterLc3Decoder::InputChunkSize() {
  ZX_DEBUG_ASSERT(codec_params_);

  // LC3 Spec v1.0 section 2.4. Decoder Interfaces.
  // Expected input frame size is combined byte_count for all the channels.
  return static_cast<size_t>(codec_params_->nbytes) * codec_params_->channels.size();
}

size_t CodecAdapterLc3Decoder::MinOutputBufferSize() {
  ZX_DEBUG_ASSERT(codec_params_);

  // LC3 Spec v1.0 section 2.4. Decoder Interfaces.
  // Total size of an output audio data frame is specified by:
  // The session configured number of channels, the frame size in samples, and
  // the configured decoder PCM bits per audio sample.
  int frame_samples =
      lc3_frame_samples(codec_params_->dt_us, LibLc3SampleRateHz(codec_params_->sr_hz));
  ZX_DEBUG_ASSERT(frame_samples > 0);
  return codec_params_->channels.size() * static_cast<size_t>(frame_samples) *
         kNumBytesPerPcmSample;
}

CodecAdapterLc3Decoder::InputLoopStatus CodecAdapterLc3Decoder::ProcessFormatDetails(
    const fuchsia::media::FormatDetails& format_details) {
  if (codec_params_.has_value()) {
    events_->onCoreCodecFailCodec("LC3 Decoder: Midstream input format change is not supported.");
    return kShouldTerminate;
  }

  if (!format_details.has_mime_type() || format_details.mime_type() != kLc3MimeType ||
      !format_details.has_oob_bytes() || format_details.oob_bytes().size() < kMinOobBytesSize) {
    events_->onCoreCodecFailCodec(
        "LC3 Decoder received input that was not valid compressed lc3 audio.");
    return kShouldTerminate;
  }

  const auto& oob_bytes = format_details.oob_bytes();

  int frame_us = 0;
  int sampling_freq = 0;
  int nbytes = 0;
  std::vector<fuchsia::media::AudioChannelId> channels;
  std::unordered_set<uint8_t> seen_params;

  size_t idx = 0;
  while (idx < oob_bytes.size()) {
    size_t len = oob_bytes[idx];
    idx += 1;
    if (len < 1 || len > oob_bytes.size() - idx) {
      events_->onCoreCodecFailCodec("LC3 Decoder received oob_bytes with invalid LTV length.");
      return kShouldTerminate;
    }

    const auto [param_type, param_value] = ProcessLTVParam(oob_bytes, idx, len);
    if (!seen_params.insert(param_type).second) {
      events_->onCoreCodecFailCodec(
          "LC3 Decoder received oob_bytes with duplicate LTV parameter key %u.", param_type);
      return kShouldTerminate;
    }

    switch (param_type) {
      case kSamplingFreqParamType: {
        auto freq = GetSamplingFrequencyHz(param_value);
        if (!freq.has_value()) {
          events_->onCoreCodecFailCodec(
              "LC3 Decoder received oob_bytes with invalid Sampling_Frequency LTV value");
          return kShouldTerminate;
        }
        sampling_freq = *freq;
        break;
      }
      case kFrameDurationParamType: {
        auto duration = GetFrameDurationUs(param_value);
        if (!duration.has_value()) {
          events_->onCoreCodecFailCodec(
              "LC3 Decoder received oob_bytes with invalid Frame_Duration LTV value");
          return kShouldTerminate;
        }
        frame_us = *duration;
        break;
      }
      case kAudioChannelAllocParamType: {
        auto channel_map = GetAudioChannelMap(param_value);
        if (!channel_map.has_value()) {
          events_->onCoreCodecFailCodec(
              "LC3 Decoder received oob_bytes with invalid Audio_Channel_Allocation LTV value");
          return kShouldTerminate;
        }
        channels = std::move(*channel_map);
        break;
      }
      case kOctetsPerCodecFrameParamType: {
        auto byte_count = GetNBytes(param_value);
        if (!byte_count.has_value()) {
          events_->onCoreCodecFailCodec(
              "LC3 Decoder received oob_bytes with invalid Octets_Per_Codec_Frame LTV value");
          return kShouldTerminate;
        }
        nbytes = *byte_count;
        break;
      }
      default:
        // Don't care about other parameters.
        LOG(DEBUG, "Received Codec_Specific_Configuration LTV param with key %u. Will be ignored.",
            param_type);
        break;
    }
    idx += len;
  }

  for (uint8_t required_key : kRequiredLTVParams) {
    if (seen_params.count(required_key) == 0) {
      events_->onCoreCodecFailCodec(
          "LC3 Decoder received oob_bytes with incomplete Codec_Specific_Configuration LTV "
          "structure. Requires Sampling_Frequency, Frame_Duration, Audio_Channel_Allocation, "
          "and Octets_Per_Codec_Frame");
      return kShouldTerminate;
    }
  }

  int num_channels = static_cast<int>(channels.size());

  std::vector<Lc3CodecContainer<lc3_decoder_t>> decoders;
  decoders.reserve(num_channels);
  auto decoder_size = lc3_decoder_size(frame_us, LibLc3SampleRateHz(sampling_freq));

  // Set up a decoder for each channel to account for multi-channeled interleaved audio.
  for (int i = 0; i < num_channels; ++i) {
    decoders.emplace_back(
        [&frame_us, &sampling_freq](void* mem) {
          // Sets up and returns the pointer to the decoder struct. The pointer has the same value
          // as `mem`.
          return lc3_setup_decoder(frame_us, LibLc3SampleRateHz(sampling_freq), 0, mem);
        },
        decoder_size);
  }

  codec_params_.emplace(Lc3DecoderParams{
      .decoders = std::move(decoders),
      .dt_us = frame_us,
      .sr_hz = sampling_freq,
      .nbytes = nbytes,
      .channels = std::move(channels),
  });

  InitChunkInputStream(format_details);
  return kOk;
}
