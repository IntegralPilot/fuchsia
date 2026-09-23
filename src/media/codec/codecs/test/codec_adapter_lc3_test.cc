// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/cpp/fidl.h>

#include <cstdarg>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

#include "src/media/codec/codecs/sw/lc3/codec_adapter_lc3_decoder.h"
#include "src/media/codec/codecs/sw/lc3/codec_adapter_lc3_encoder.h"

namespace {

class FakeCodecAdapterEvents : public CodecAdapterEvents {
 public:
  void onCoreCodecFailCodec(const char* format, ...) override {
    fail_codec_count_++;
    char buffer[256];
    va_list args;
    va_start(args, format);
    std::vsnprintf(buffer, sizeof(buffer), format, args);
    va_end(args);
    last_fail_message_ = buffer;
  }

  void onCoreCodecFailStream(fuchsia::media::StreamError error) override {}
  void onCoreCodecResetStreamAfterCurrentFrame() override {}
  void onCoreCodecMidStreamOutputConstraintsChange(bool output_re_config_required) override {}
  void onCoreCodecOutputFormatChange() override {}
  void onCoreCodecInputPacketDone(CodecPacket* packet) override {}
  void onCoreCodecOutputPacket(CodecPacket* packet, bool error_detected_before,
                               bool error_detected_during) override {}
  void onCoreCodecOutputTimestampHasNoOutput(uint64_t timestamp_ish) override {}
  void onCoreCodecOutputEndOfStream(bool error_detected_before) override {}
  void onCoreCodecLogEvent(
      media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code) override {}

  size_t fail_codec_count() const { return fail_codec_count_; }
  const std::string& last_fail_message() const { return last_fail_message_; }

 private:
  size_t fail_codec_count_ = 0;
  std::string last_fail_message_;
};

class TestCodecAdapterLc3Decoder : public CodecAdapterLc3Decoder {
 public:
  TestCodecAdapterLc3Decoder(std::mutex& lock, CodecAdapterEvents* events)
      : CodecAdapterLc3Decoder(lock, events) {}

  using CodecAdapterLc3Decoder::InputChunkSize;
  using CodecAdapterLc3Decoder::InputLoopStatus;
  using CodecAdapterLc3Decoder::MinOutputBufferSize;
  using CodecAdapterLc3Decoder::OutputFormatDetails;
  using CodecAdapterLc3Decoder::ProcessFormatDetails;
  using CodecAdapterLc3Decoder::ProcessInputChunkData;
};

class TestCodecAdapterLc3Encoder : public CodecAdapterLc3Encoder {
 public:
  TestCodecAdapterLc3Encoder(std::mutex& lock, CodecAdapterEvents* events)
      : CodecAdapterLc3Encoder(lock, events) {}

  using CodecAdapterLc3Encoder::CreateTimestampExtrapolator;
  using CodecAdapterLc3Encoder::InputChunkSize;
  using CodecAdapterLc3Encoder::InputLoopStatus;
  using CodecAdapterLc3Encoder::MinOutputBufferSize;
  using CodecAdapterLc3Encoder::ProcessFormatDetails;
  using CodecAdapterLc3Encoder::ProcessInputChunkData;
};

fuchsia::media::FormatDetails MakeLc3FormatDetails(std::vector<uint8_t> oob_bytes) {
  fuchsia::media::FormatDetails format_details;
  format_details.set_mime_type(kLc3MimeType);
  format_details.set_oob_bytes(std::move(oob_bytes));
  return format_details;
}

fuchsia::media::FormatDetails MakeValidLc3EncoderFormatDetails() {
  fuchsia::media::PcmFormat pcm;
  pcm.pcm_mode = fuchsia::media::AudioPcmMode::LINEAR;
  pcm.bits_per_sample = 16;
  pcm.frames_per_second = 48000;
  pcm.channel_map = {fuchsia::media::AudioChannelId::LF};

  fuchsia::media::AudioUncompressedFormat uncompressed;
  uncompressed.set_pcm(std::move(pcm));

  fuchsia::media::AudioFormat audio;
  audio.set_uncompressed(std::move(uncompressed));

  fuchsia::media::DomainFormat domain;
  domain.set_audio(std::move(audio));

  fuchsia::media::Lc3EncoderSettings lc3_settings;
  lc3_settings.set_nbytes(40);
  lc3_settings.set_frame_duration(fuchsia::media::Lc3FrameDuration::D10_MS);

  fuchsia::media::EncoderSettings encoder_settings;
  encoder_settings.set_lc3(std::move(lc3_settings));

  fuchsia::media::FormatDetails format_details;
  format_details.set_domain(std::move(domain));
  format_details.set_encoder_settings(std::move(encoder_settings));
  return format_details;
}

TEST(CodecAdapterLc3DecoderTest, ValidOobBytesSucceeds) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Valid 16-byte LTV configuration:
  // Sampling_Frequency: 48 kHz (0x08)
  // Frame_Duration: 10 ms (0x01)
  // Audio_Channel_Allocation: LF (0x00000001)
  // Octets_Per_Codec_Frame: 40 (0x0028)
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x28               // Octets_Per_Codec_Frame
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kOk);
  EXPECT_EQ(events.fail_codec_count(), 0u);
  EXPECT_EQ(decoder.InputChunkSize(), 40u);
  EXPECT_EQ(decoder.MinOutputBufferSize(), 960u);
}

TEST(CodecAdapterLc3DecoderTest, MissingMimeTypeOrOobBytesRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  fuchsia::media::FormatDetails missing_mime;
  missing_mime.set_oob_bytes(std::vector<uint8_t>(16, 0));
  EXPECT_EQ(decoder.ProcessFormatDetails(missing_mime),
            TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);

  fuchsia::media::FormatDetails short_oob;
  short_oob.set_mime_type(kLc3MimeType);
  short_oob.set_oob_bytes(std::vector<uint8_t>(15, 0));
  EXPECT_EQ(decoder.ProcessFormatDetails(short_oob), TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 2u);
}

TEST(CodecAdapterLc3DecoderTest, OutOfRangeOctetsPerCodecFrameRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Octets_Per_Codec_Frame = 19 (< kMinExternalByteCount = 20).
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x13               // Octets_Per_Codec_Frame = 19
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Octets_Per_Codec_Frame"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, ValidOobBytesWithUnknownLtvParamSucceeds) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Valid 16-byte LTV configuration plus optional/unknown LTV parameter (e.g.
  // Codec_Frame_Blocks_Per_SDU = 0x05).
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x28,              // Octets_Per_Codec_Frame
      0x02, 0x05, 0x01                     // Optional Codec_Frame_Blocks_Per_SDU
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kOk);
  EXPECT_EQ(events.fail_codec_count(), 0u);
}

TEST(CodecAdapterLc3DecoderTest, TruncatedAudioChannelAllocationRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Audio_Channel_Allocation with LTV length 2 (1 byte value instead of 4).
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,             // Sampling_Frequency
      0x02, 0x02, 0x01,             // Frame_Duration
      0x02, 0x03, 0x00,             // Truncated Audio_Channel_Allocation (len=2)
      0x03, 0x04, 0x00, 0x28,       // Valid Octets_Per_Codec_Frame
      0x04, 0x99, 0x00, 0x00, 0x00  // Unknown LTV param to pad to >= 16B
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Audio_Channel_Allocation"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, TruncatedOctetsPerCodecFrameRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Octets_Per_Codec_Frame with LTV length 2 (1 byte value instead of 2).
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x02, 0x04, 0x28,                    // Truncated Octets_Per_Codec_Frame (len=2)
      0x01, 0x99                           // Unknown param to pad to 16B
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Octets_Per_Codec_Frame"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, TruncatedSamplingFrequencyRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Sampling_Frequency with LTV length 1 (0 value bytes).
  std::vector<uint8_t> oob_bytes = {
      0x01, 0x01,                          // Truncated Sampling_Frequency (len=1)
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x28,              // Octets_Per_Codec_Frame
      0x01, 0x99                           // Padding to >= 16B
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Sampling_Frequency"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, TruncatedFrameDurationRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Frame_Duration with LTV length 1 (0 value bytes).
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x01, 0x02,                          // Truncated Frame_Duration (len=1)
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x28,              // Octets_Per_Codec_Frame
      0x01, 0x99                           // Padding to >= 16B
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Frame_Duration"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, ZeroLengthLtvEntryAtEndRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Valid LTVs followed by a trailing 0x00 length byte at the very end of oob_bytes.
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x28,              // Octets_Per_Codec_Frame
      0x00                                 // len == 0 at final byte
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("invalid LTV length"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, LtvLengthExceedsRemainingBufferRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Final LTV claims length 10 when only 3 bytes remain.
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x0a, 0x04, 0x00, 0x28               // len=10 exceeds buffer
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("invalid LTV length"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, DuplicateLtvParameterKeyRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Duplicate Sampling_Frequency (key 0x01) entries.
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency #1
      0x02, 0x01, 0x08,                    // Sampling_Frequency #2 (duplicate)
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x28               // Octets_Per_Codec_Frame
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("duplicate LTV parameter key"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, ZeroAudioChannelAllocationRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Audio_Channel_Allocation = 0x00000000 (0 channels).
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x00,  // Audio_Channel_Allocation = 0
      0x03, 0x04, 0x00, 0x28               // Octets_Per_Codec_Frame
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Audio_Channel_Allocation"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, IncompleteRequiredLtvParamsRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // oob_bytes >= 16 bytes (padded with unknown LTV key 0x05), missing Octets_Per_Codec_Frame
  // (0x04).
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x04, 0x05, 0x01, 0x00, 0x00         // Unknown param to pad to >= 16B
  };

  auto status = decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes)));
  EXPECT_EQ(status, TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("incomplete"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, MidstreamFormatChangeRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency
      0x02, 0x02, 0x01,                    // Frame_Duration
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation
      0x03, 0x04, 0x00, 0x28               // Octets_Per_Codec_Frame
  };

  ASSERT_EQ(decoder.ProcessFormatDetails(MakeLc3FormatDetails(oob_bytes)),
            TestCodecAdapterLc3Decoder::kOk);
  EXPECT_EQ(events.fail_codec_count(), 0u);

  // A second ProcessFormatDetails call on an active stream should fail the codec.
  EXPECT_EQ(decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes))),
            TestCodecAdapterLc3Decoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Midstream input format change"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3DecoderTest, ProcessInputChunkDataBufferSizeValidation) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x08,                    // Sampling_Frequency: 48 kHz
      0x02, 0x02, 0x01,                    // Frame_Duration: 10 ms
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation: 1 ch
      0x03, 0x04, 0x00, 0x28               // Octets_Per_Codec_Frame: 40 bytes
  };
  ASSERT_EQ(decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes))),
            TestCodecAdapterLc3Decoder::kOk);

  const size_t input_size = decoder.InputChunkSize();
  const size_t min_output_size = decoder.MinOutputBufferSize();
  std::vector<uint8_t> input(input_size, 0);
  std::vector<uint8_t> output(min_output_size, 0);

  // Mismatched input_data_size should return -1.
  EXPECT_EQ(
      decoder.ProcessInputChunkData(input.data(), input_size - 1, output.data(), output.size()),
      -1);
  // Undersized output_buffer_size should return -1.
  EXPECT_EQ(
      decoder.ProcessInputChunkData(input.data(), input_size, output.data(), min_output_size - 1),
      -1);
  // Zeroed input triggers liblc3 Packet Loss Concealment (return code 1) and succeeds.
  EXPECT_EQ(decoder.ProcessInputChunkData(input.data(), input_size, output.data(), output.size()),
            static_cast<int>(min_output_size));
}

TEST(CodecAdapterLc3EncoderTest, ValidFormatDetailsSucceeds) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto status = encoder.ProcessFormatDetails(MakeValidLc3EncoderFormatDetails());
  EXPECT_EQ(status, TestCodecAdapterLc3Encoder::kOk);
  EXPECT_EQ(events.fail_codec_count(), 0u);
  EXPECT_EQ(encoder.InputChunkSize(), 960u);
  EXPECT_EQ(encoder.MinOutputBufferSize(), 40u);
}

TEST(CodecAdapterLc3EncoderTest, MissingDomainOrEncoderSettingsRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  fuchsia::media::FormatDetails missing_domain = MakeValidLc3EncoderFormatDetails();
  missing_domain.clear_domain();
  EXPECT_EQ(encoder.ProcessFormatDetails(missing_domain),
            TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);

  fuchsia::media::FormatDetails missing_settings = MakeValidLc3EncoderFormatDetails();
  missing_settings.clear_encoder_settings();
  EXPECT_EQ(encoder.ProcessFormatDetails(missing_settings),
            TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 2u);
}

TEST(CodecAdapterLc3EncoderTest, OutOfRangeEncoderNbytesRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_encoder_settings()->lc3().set_nbytes(401);

  auto status = encoder.ProcessFormatDetails(format_details);
  EXPECT_EQ(status, TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Byte count"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3EncoderTest, EmptyChannelMapRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_domain()->audio().uncompressed().pcm().channel_map.clear();

  auto status = encoder.ProcessFormatDetails(format_details);
  EXPECT_EQ(status, TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Unsupported channel count"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3EncoderTest, ExcessiveChannelMapRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_domain()->audio().uncompressed().pcm().channel_map.assign(
      kMaxChannelCount + 1, fuchsia::media::AudioChannelId::LF);

  auto status = encoder.ProcessFormatDetails(format_details);
  EXPECT_EQ(status, TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Unsupported channel count"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3EncoderTest, MissingNbytesRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_encoder_settings()->lc3().clear_nbytes();

  auto status = encoder.ProcessFormatDetails(format_details);
  EXPECT_EQ(status, TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Byte count"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3EncoderTest, MissingFrameDurationRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_encoder_settings()->lc3().clear_frame_duration();

  auto status = encoder.ProcessFormatDetails(format_details);
  EXPECT_EQ(status, TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("frame duration"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3EncoderTest, InvalidFrameDurationRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_encoder_settings()->lc3().set_frame_duration(
      static_cast<fuchsia::media::Lc3FrameDuration>(99));

  auto status = encoder.ProcessFormatDetails(format_details);
  EXPECT_EQ(status, TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("frame duration"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3EncoderTest, MidstreamFormatChangeRejected) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  ASSERT_EQ(encoder.ProcessFormatDetails(MakeValidLc3EncoderFormatDetails()),
            TestCodecAdapterLc3Encoder::kOk);
  EXPECT_EQ(events.fail_codec_count(), 0u);

  EXPECT_EQ(encoder.ProcessFormatDetails(MakeValidLc3EncoderFormatDetails()),
            TestCodecAdapterLc3Encoder::kShouldTerminate);
  EXPECT_EQ(events.fail_codec_count(), 1u);
  EXPECT_NE(events.last_fail_message().find("Midstream input format change"), std::string::npos)
      << "Actual message: " << events.last_fail_message();
}

TEST(CodecAdapterLc3EncoderTest, ProcessInputChunkDataValidationAndUnalignedInput) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_domain()->audio().uncompressed().pcm().bits_per_sample = 24;
  ASSERT_EQ(encoder.ProcessFormatDetails(format_details), TestCodecAdapterLc3Encoder::kOk);

  const size_t input_size = encoder.InputChunkSize();
  const size_t min_output_size = encoder.MinOutputBufferSize();

  // Allocate extra padding so input_storage.data() + 1 is guaranteed misaligned for int32_t.
  std::vector<uint8_t> input_storage(input_size + 4, 0);
  const uint8_t* unaligned_input = input_storage.data() + 1;
  std::vector<uint8_t> output(min_output_size, 0);

  // Mismatched input_data_size should return -1.
  EXPECT_EQ(
      encoder.ProcessInputChunkData(unaligned_input, input_size - 1, output.data(), output.size()),
      -1);
  // Undersized output_buffer_size should return -1.
  EXPECT_EQ(encoder.ProcessInputChunkData(unaligned_input, input_size, output.data(),
                                          min_output_size - 1),
            -1);
  // Valid unaligned input chunk should succeed via aligned scratch buffer copy.
  EXPECT_EQ(
      encoder.ProcessInputChunkData(unaligned_input, input_size, output.data(), output.size()),
      static_cast<int>(min_output_size));
}

TEST(CodecAdapterLc3DecoderTest, SamplingFrequency44100HzPreservedInOutputFormat) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Decoder decoder(lock, &events);

  // Sampling_Frequency 0x07 (44.1 kHz), Frame_Duration 0x01 (10 ms), 1 ch, 40 bytes.
  std::vector<uint8_t> oob_bytes = {
      0x02, 0x01, 0x07,                    // Sampling_Frequency: 44.1 kHz
      0x02, 0x02, 0x01,                    // Frame_Duration: 10 ms
      0x05, 0x03, 0x00, 0x00, 0x00, 0x01,  // Audio_Channel_Allocation: LF
      0x03, 0x04, 0x00, 0x28               // Octets_Per_Codec_Frame: 40
  };

  ASSERT_EQ(decoder.ProcessFormatDetails(MakeLc3FormatDetails(std::move(oob_bytes))),
            TestCodecAdapterLc3Decoder::kOk);
  EXPECT_EQ(events.fail_codec_count(), 0u);

  // liblc3 uses 480 samples per 10ms frame for 44.1 kHz, so MinOutputBufferSize is 480 * 2 = 960 B.
  EXPECT_EQ(decoder.MinOutputBufferSize(), 960u);
  // Output format details must report the actual PCM sample rate (44100 Hz), not coerced 48000 Hz.
  auto [out_format, min_size] = decoder.OutputFormatDetails();
  EXPECT_EQ(out_format.domain().audio().uncompressed().pcm().frames_per_second, 44100u);
}

TEST(CodecAdapterLc3EncoderTest, TimestampExtrapolatorUsesTrueSampleRateFor44100Hz) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_domain()->audio().uncompressed().pcm().frames_per_second = 44100;
  format_details.set_timebase(1'000'000'000ull);  // 1 second in ns
  ASSERT_EQ(encoder.ProcessFormatDetails(format_details), TestCodecAdapterLc3Encoder::kOk);

  // For 44.1 kHz 10ms nominal frame, liblc3 encodes 480 samples per frame (InputChunkSize = 960 B).
  // At 44,100 samples/sec, 480 samples lasts 480 / 44100 s = 10,884,353 ns (~10.884 ms).
  const size_t chunk_size = encoder.InputChunkSize();
  EXPECT_EQ(chunk_size, 960u);

  auto extrapolator = encoder.CreateTimestampExtrapolator(format_details);
  extrapolator.Inform(0, 0);
  auto extrapolated = extrapolator.Extrapolate(chunk_size);
  ASSERT_TRUE(extrapolated.has_value());
  EXPECT_EQ(*extrapolated, 480ull * 1'000'000'000ull / 44100ull);
}

TEST(CodecAdapterLc3EncoderTest, TimestampExtrapolatorUsesFourBytesPerSampleFor24BitAudio) {
  std::mutex lock;
  FakeCodecAdapterEvents events;
  TestCodecAdapterLc3Encoder encoder(lock, &events);

  auto format_details = MakeValidLc3EncoderFormatDetails();
  format_details.mutable_domain()->audio().uncompressed().pcm().bits_per_sample = 24;
  format_details.set_timebase(1'000'000'000ull);  // 1 second in ns
  ASSERT_EQ(encoder.ProcessFormatDetails(format_details), TestCodecAdapterLc3Encoder::kOk);

  // For 48 kHz 10ms frame, liblc3 encodes 480 samples per frame. Since 24-bit PCM
  // (LC3_PCM_FORMAT_S24) uses 4-byte (int32_t) words in memory, InputChunkSize is 480 * 4 = 1920 B.
  const size_t chunk_size = encoder.InputChunkSize();
  EXPECT_EQ(chunk_size, 1920u);

  // One 1920-byte chunk at 48 kHz (480 samples) must advance the timestamp by 10 ms (10,000,000
  // ns), not 13.333 ms (which would happen if bits_per_sample / 8 == 3 bytes/sample were used).
  auto extrapolator = encoder.CreateTimestampExtrapolator(format_details);
  extrapolator.Inform(0, 0);
  auto extrapolated = extrapolator.Extrapolate(chunk_size);
  ASSERT_TRUE(extrapolated.has_value());
  EXPECT_EQ(*extrapolated, 10'000'000ull);
}

}  // namespace
