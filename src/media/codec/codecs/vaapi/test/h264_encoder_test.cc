// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <stdio.h>

#include <limits>
#include <memory>
#include <thread>

#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/media/codec/codecs/test/test_codec_packets.h"
#include "src/media/codec/codecs/vaapi/codec_adapter_vaapi_encoder.h"
#include "src/media/codec/codecs/vaapi/codec_runner_app.h"
#include "src/media/codec/codecs/vaapi/vaapi_utils.h"
#include "src/media/third_party/chromium_media/geometry.h"
#include "src/media/third_party/chromium_media/media/gpu/gpu_video_encode_accelerator_helpers.h"
#include "src/media/third_party/chromium_media/media/parsers/h264_parser.h"
#include "vaapi_stubs.h"

namespace {

class FakeCodecAdapterEvents : public CodecAdapterEvents {
 public:
  void onCoreCodecFailCodec(const char *format, ...) override {
    va_list args;
    va_start(args, format);
    printf("Got onCoreCodecFailCodec: ");
    vprintf(format, args);
    printf("\n");
    fflush(stdout);
    va_end(args);

    fail_codec_count_++;
    cond_.notify_all();
  }

  void onCoreCodecFailStream(fuchsia::media::StreamError error) override {
    printf("Got onCoreCodecFailStream %d\n", static_cast<int>(error));
    fflush(stdout);
    fail_stream_count_++;
  }

  void onCoreCodecResetStreamAfterCurrentFrame() override {}

  void onCoreCodecMidStreamOutputConstraintsChange(bool output_re_config_required) override {
    // Test a representative value.
    auto output_constraints = codec_adapter_->CoreCodecGetBufferCollectionConstraints2(
        CodecPort::kOutputPort, fuchsia::media::StreamBufferConstraints(),
        fuchsia::media::StreamBufferPartialSettings());
    EXPECT_TRUE(*output_constraints.buffer_memory_constraints()->cpu_domain_supported());

    std::unique_lock<std::mutex> lock(lock_);
    // Wait for buffer initialization to complete to ensure all buffers are staged to be loaded.
    cond_.wait(lock, [&]() { return buffer_initialization_completed_; });

    // Fake out the client setting buffer constraints on sysmem
    fuchsia_sysmem2::BufferCollectionInfo buffer_collection;
    buffer_collection.settings().emplace();
    if (output_constraints.image_format_constraints().has_value()) {
      buffer_collection.settings()->image_format_constraints() =
          output_constraints.image_format_constraints()->at(0);
    }
    buffer_collection.buffers().emplace(*output_constraints.min_buffer_count_for_camping());
    codec_adapter_->CoreCodecSetBufferCollectionInfo(CodecPort::kOutputPort, buffer_collection);
    codec_adapter_->CoreCodecMidStreamOutputBufferReConfigFinish();
  }

  void onCoreCodecOutputFormatChange() override {}

  void onCoreCodecInputPacketDone(CodecPacket *packet) override {
    std::lock_guard lock(lock_);
    input_packets_done_.push_back(packet);
    cond_.notify_all();
  }

  void onCoreCodecOutputPacket(CodecPacket *packet, bool error_detected_before,
                               bool error_detected_during) override {
    auto output_format = codec_adapter_->CoreCodecGetOutputFormat(1u, 1u);
    // Test a representative value.
    EXPECT_TRUE(output_format.format_details().domain().video().is_compressed());

    std::lock_guard lock(lock_);
    output_packets_done_.push_back(packet);
    cond_.notify_all();
  }

  void onCoreCodecOutputTimestampHasNoOutput(uint64_t timestamp_ish) override {}

  void onCoreCodecOutputEndOfStream(bool error_detected_before) override {
    printf("Got onCoreCodecOutputEndOfStream\n");
    fflush(stdout);
  }

  void onCoreCodecLogEvent(
      media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code) override {}

  uint64_t fail_codec_count() const { return fail_codec_count_; }
  uint64_t fail_stream_count() const { return fail_stream_count_; }

  void WaitForInputPacketsDone() {
    std::unique_lock<std::mutex> lock(lock_);
    cond_.wait(lock, [this]() { return !input_packets_done_.empty(); });
  }

  void set_codec_adapter(CodecAdapter *codec_adapter) { codec_adapter_ = codec_adapter; }

  void WaitForOutputPacketCount(size_t output_packet_count) {
    std::unique_lock<std::mutex> lock(lock_);
    cond_.wait(lock, [&]() { return output_packets_done_.size() == output_packet_count; });
  }

  size_t output_packet_count() const { return output_packets_done_.size(); }

  void SetBufferInitializationCompleted() {
    std::lock_guard lock(lock_);
    buffer_initialization_completed_ = true;
    cond_.notify_all();
  }

  void WaitForCodecFailure(uint64_t failure_count) {
    std::unique_lock<std::mutex> lock(lock_);
    cond_.wait(lock, [&]() { return fail_codec_count_ == failure_count; });
  }

  void ReturnLastOutputPacket() {
    std::lock_guard lock(lock_);
    auto packet = output_packets_done_.back();
    output_packets_done_.pop_back();
    codec_adapter_->CoreCodecRecycleOutputPacket(packet);
  }

 private:
  CodecAdapter *codec_adapter_ = nullptr;
  uint64_t fail_codec_count_{};
  uint64_t fail_stream_count_{};

  std::mutex lock_;
  std::condition_variable cond_;

  std::vector<const CodecPacket *> input_packets_done_;
  std::vector<CodecPacket *> output_packets_done_;
  bool buffer_initialization_completed_ = false;
};

class H264EncoderTestFixture : public ::testing::Test {
 protected:
  H264EncoderTestFixture() = default;
  ~H264EncoderTestFixture() override { encoder_.reset(); }

  void SetUp() override {
    EXPECT_TRUE(VADisplayWrapper::InitializeSingletonForTesting());

    vaDefaultStubSetReturn();

    // Have to defer the construction of encoder_ until
    // VADisplayWrapper::InitializeSingletonForTesting is called
    encoder_ = std::make_unique<CodecAdapterVaApiEncoder>(lock_, &events_);
    events_.set_codec_adapter(encoder_.get());
  }

  void TearDown() override { vaDefaultStubSetReturn(); }

  void CodecAndStreamInit() {
    fuchsia::media::FormatDetails format_details;
    format_details.set_format_details_version_ordinal(1);
    format_details.set_mime_type("video/h264");

    fuchsia::media::DomainFormat domain_format;
    domain_format.video().uncompressed().image_format.display_width = 10;
    domain_format.video().uncompressed().image_format.display_height = 10;
    domain_format.video().uncompressed().image_format.coded_width = 10;
    domain_format.video().uncompressed().image_format.coded_height = 10;
    format_details.set_domain(std::move(domain_format));
    encoder_->CoreCodecInit(format_details);

    auto input_constraints = encoder_->CoreCodecGetBufferCollectionConstraints2(
        CodecPort::kInputPort, fuchsia::media::StreamBufferConstraints(),
        fuchsia::media::StreamBufferPartialSettings());
    EXPECT_TRUE(*input_constraints.buffer_memory_constraints()->cpu_domain_supported());

    encoder_->CoreCodecStartStream();
    encoder_->CoreCodecQueueInputFormatDetails(format_details);
  }

  void CodecStreamStop() {
    encoder_->CoreCodecStopStream();
    encoder_->CoreCodecEnsureBuffersNotConfigured(CodecPort::kOutputPort);
  }

  void ConfigureOutputBuffers(uint32_t output_packet_count, size_t output_packet_size) {
    auto test_packets = Packets(output_packet_count);
    test_buffers_ = Buffers(std::vector<size_t>(output_packet_count, output_packet_size));

    test_packets_ = std::vector<std::unique_ptr<CodecPacket>>(output_packet_count);
    for (size_t i = 0; i < output_packet_count; i++) {
      auto &packet = test_packets.packets[i];
      test_packets_[i] = std::move(packet);
      encoder_->CoreCodecAddBuffer(CodecPort::kOutputPort, test_buffers_.buffers[i].get());
    }

    encoder_->CoreCodecConfigureBuffers(CodecPort::kOutputPort, test_packets_);
    for (size_t i = 0; i < output_packet_count; i++) {
      encoder_->CoreCodecRecycleOutputPacket(test_packets_[i].get());
    }

    encoder_->CoreCodecConfigureBuffers(CodecPort::kOutputPort, test_packets_);
  }

  std::mutex lock_;
  FakeCodecAdapterEvents events_;
  std::unique_ptr<CodecAdapterVaApiEncoder> encoder_;
  std::unique_ptr<CodecPacketForTest> input_packet_;
  std::unique_ptr<CodecBufferForTest> input_buffer_;
  TestBuffers test_buffers_;
  std::vector<std::unique_ptr<CodecPacket>> test_packets_;
};

TEST_F(H264EncoderTestFixture, InvalidFormat) {
  constexpr uint64_t kExpectedNumOfCodecFailures = 1u;

  fuchsia::media::FormatDetails format_details;
  format_details.set_format_details_version_ordinal(1);
  format_details.set_mime_type("video/h264");
  encoder_->CoreCodecInit(format_details);
  events_.WaitForCodecFailure(kExpectedNumOfCodecFailures);

  EXPECT_EQ(kExpectedNumOfCodecFailures, events_.fail_codec_count());
  EXPECT_EQ(0u, events_.fail_stream_count());
}

TEST_F(H264EncoderTestFixture, Resize) {
  constexpr uint32_t kExpectedOutputPackets = 2;

  CodecAndStreamInit();

  // Should be enough to handle a large fraction of bear.h264 output without recycling.
  constexpr uint32_t kOutputPacketCount = 35;
  // Nothing writes to the output packet so its size doesn't matter.
  constexpr size_t kOutputPacketSize = 4096;
  {
    auto input_constraints = encoder_->CoreCodecGetBufferCollectionConstraints2(
        CodecPort::kInputPort, fuchsia::media::StreamBufferConstraints(),
        fuchsia::media::StreamBufferPartialSettings());
    EXPECT_TRUE(*input_constraints.buffer_memory_constraints()->cpu_domain_supported());

    // Fake out the client setting buffer constraints on sysmem
    fuchsia_sysmem2::BufferCollectionInfo buffer_collection;
    buffer_collection.settings().emplace().image_format_constraints() =
        input_constraints.image_format_constraints()->at(0);
    encoder_->CoreCodecSetBufferCollectionInfo(CodecPort::kInputPort, buffer_collection);
  }

  constexpr uint32_t kInputStride = 16;
  constexpr uint32_t kInputBufferSize = kInputStride * 12 * 3 / 2;

  input_buffer_ = std::make_unique<CodecBufferForTest>(kInputBufferSize, 0, false);

  std::vector<std::unique_ptr<CodecPacketForTest>> input_packets;
  {
    auto input_packet = std::make_unique<CodecPacketForTest>(0);
    input_packet->SetStartOffset(0);
    input_packet->SetValidLengthBytes(kInputBufferSize);
    input_packet->SetBuffer(input_buffer_.get());
    encoder_->CoreCodecQueueInputPacket(input_packet.get());
    input_packets.push_back(std::move(input_packet));
  }
  {
    fuchsia::media::FormatDetails format_details;
    format_details.set_format_details_version_ordinal(2);
    format_details.set_mime_type("video/h264");

    fuchsia::media::DomainFormat domain_format;
    domain_format.video().uncompressed().image_format.display_width = 12;
    domain_format.video().uncompressed().image_format.display_height = 10;
    domain_format.video().uncompressed().image_format.coded_width = 12;
    domain_format.video().uncompressed().image_format.coded_height = 10;
    format_details.set_domain(std::move(domain_format));
    encoder_->CoreCodecQueueInputFormatDetails(format_details);
  }
  {
    auto input_packet = std::make_unique<CodecPacketForTest>(0);
    input_packet->SetStartOffset(0);
    input_packet->SetValidLengthBytes(kInputBufferSize);
    input_packet->SetBuffer(input_buffer_.get());
    encoder_->CoreCodecQueueInputPacket(input_packet.get());
    input_packets.push_back(std::move(input_packet));
  }
  ConfigureOutputBuffers(kOutputPacketCount, kOutputPacketSize);

  events_.SetBufferInitializationCompleted();
  events_.WaitForInputPacketsDone();
  events_.WaitForOutputPacketCount(kExpectedOutputPackets);
  events_.ReturnLastOutputPacket();

  CodecStreamStop();

  // One packet was returned, so it was already removed from the list.
  EXPECT_EQ(kExpectedOutputPackets - 1u, events_.output_packet_count());

  EXPECT_EQ(0u, events_.fail_codec_count());
  EXPECT_EQ(0u, events_.fail_stream_count());
}

TEST_F(H264EncoderTestFixture, EncodeBasic) {
  constexpr uint32_t kExpectedOutputPackets = 29;

  CodecAndStreamInit();

  // Should be enough to handle a large fraction of bear.h264 output without recycling.
  constexpr uint32_t kOutputPacketCount = 35;
  // Nothing writes to the output packet so its size doesn't matter.
  constexpr size_t kOutputPacketSize = 4096;
  {
    auto input_constraints = encoder_->CoreCodecGetBufferCollectionConstraints2(
        CodecPort::kInputPort, fuchsia::media::StreamBufferConstraints(),
        fuchsia::media::StreamBufferPartialSettings());
    EXPECT_TRUE(*input_constraints.buffer_memory_constraints()->cpu_domain_supported());

    // Fake out the client setting buffer constraints on sysmem
    fuchsia_sysmem2::BufferCollectionInfo buffer_collection;
    buffer_collection.settings().emplace().image_format_constraints() =
        input_constraints.image_format_constraints()->at(0);
    encoder_->CoreCodecSetBufferCollectionInfo(CodecPort::kInputPort, buffer_collection);
  }

  constexpr uint32_t kInputStride = 16;
  constexpr uint32_t kInputBufferSize = kInputStride * 10 * 3 / 2;

  input_buffer_ = std::make_unique<CodecBufferForTest>(kInputBufferSize, 0, false);

  std::vector<std::unique_ptr<CodecPacketForTest>> input_packets;
  for (size_t i = 0; i < 29; i++) {
    auto input_packet = std::make_unique<CodecPacketForTest>(0);
    input_packet->SetStartOffset(0);
    input_packet->SetValidLengthBytes(kInputBufferSize);
    input_packet->SetBuffer(input_buffer_.get());
    encoder_->CoreCodecQueueInputPacket(input_packet.get());
    input_packets.push_back(std::move(input_packet));
  }
  ConfigureOutputBuffers(kOutputPacketCount, kOutputPacketSize);

  events_.SetBufferInitializationCompleted();
  events_.WaitForInputPacketsDone();
  events_.WaitForOutputPacketCount(kExpectedOutputPackets);
  events_.ReturnLastOutputPacket();

  CodecStreamStop();

  // One packet was returned, so it was already removed from the list.
  EXPECT_EQ(kExpectedOutputPackets - 1u, events_.output_packet_count());

  EXPECT_EQ(0u, events_.fail_codec_count());
  EXPECT_EQ(0u, events_.fail_stream_count());
}

// Test that we can connect using the CodecFactory.
TEST(H264Encoder, Init) {
  EXPECT_TRUE(VADisplayWrapper::InitializeSingletonForTesting());
  fidl::InterfaceRequest<fuchsia::io::Directory> directory_request;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto codec_services = sys::ServiceDirectory::CreateWithRequest(&directory_request);

  std::thread codec_thread([directory_request = std::move(directory_request)]() mutable {
    CodecRunnerApp<NoAdapter, CodecAdapterVaApiEncoder> runner_app;
    runner_app.Init();
    fidl::InterfaceHandle<fuchsia::io::Directory> outgoing_directory;
    EXPECT_EQ(ZX_OK,
              runner_app.component_context()->outgoing()->Serve(outgoing_directory.NewRequest()));
    EXPECT_EQ(ZX_OK, fdio_open3_at(outgoing_directory.channel().get(), "svc",
                                   uint64_t{fuchsia::io::PERM_READABLE},
                                   directory_request.TakeChannel().release()));
    runner_app.Run();
  });

  fuchsia::mediacodec::CodecFactorySyncPtr codec_factory;
  codec_services->Connect(codec_factory.NewRequest());
  fuchsia::media::StreamProcessorPtr stream_processor;
  fuchsia::mediacodec::CreateEncoder_Params params;
  fuchsia::media::FormatDetails input_details;
  input_details.set_mime_type("video/h264");
  input_details.set_format_details_version_ordinal(1);

  fuchsia::media::DomainFormat domain_format;
  domain_format.video().uncompressed().image_format.display_width = 10;
  domain_format.video().uncompressed().image_format.display_height = 10;
  input_details.set_domain(std::move(domain_format));
  params.set_input_details(std::move(input_details));
  params.set_require_hw(true);
  EXPECT_EQ(ZX_OK, codec_factory->CreateEncoder(std::move(params), stream_processor.NewRequest()));

  stream_processor.set_error_handler([&](zx_status_t status) {
    loop.Quit();
    EXPECT_TRUE(false);
  });

  stream_processor.events().OnInputConstraints =
      [&](fuchsia::media::StreamBufferConstraints constraints) {
        loop.Quit();
        stream_processor.Unbind();
      };

  loop.Run();
  codec_factory.Unbind();

  codec_thread.join();
}

TEST(H264Encoder, BitstreamBufferSizeNoOverflow) {
  constexpr size_t kExpected8MB = 8 * 1024 * 1024;
  constexpr size_t kExpected4MB = 4 * 1024 * 1024;
  constexpr size_t kExpected2MB = 2 * 1024 * 1024;

  // 1. 65536 * 65536 overflows a 32-bit signed int to 0. Verify that buffer
  // sizing uses 64-bit area and selects the 8 MB max buffer size (> 1440p)
  // rather than falling back to 2 MB (or matching the 320x180 table entry).
  const gfx::Size large_size(65536, 65536);
  EXPECT_EQ(kExpected8MB, media::GetEncodeBitstreamBufferSize(large_size));
  EXPECT_EQ(kExpected8MB, media::GetEncodeBitstreamBufferSize(large_size, 20000000u, 30u));

  // 2. 46341 * 46341 overflows a 32-bit signed int to a negative value
  // (-2,147,479,015). Verify that 64-bit area prevents matching the first
  // table entry (<= 320 * 180) and selects the 8 MB max buffer size.
  const gfx::Size negative_overflow_size(46341, 46341);
  EXPECT_EQ(kExpected8MB, media::GetEncodeBitstreamBufferSize(negative_overflow_size));
  EXPECT_EQ(kExpected8MB,
            media::GetEncodeBitstreamBufferSize(negative_overflow_size, 100000u, 30u));

  // 3. Exercise the base::saturated_cast<size_t>(data.buffer_size_in_bytes * ratio)
  // clamping path inside the table loop using a table-matching size (1080p) and
  // an extreme bitrate/framerate ratio.
  const gfx::Size size_1080p(1920, 1080);
  EXPECT_EQ(kExpected2MB, media::GetEncodeBitstreamBufferSize(
                              size_1080p, std::numeric_limits<uint32_t>::max(), 1u));

  // 4. Verify exact resolution tier transitions in GetMaxEncodeBitstreamBufferSize():
  // <= 1080p -> 2 MB, (1080p, 1440p] -> 4 MB, > 1440p -> 8 MB.
  EXPECT_EQ(kExpected2MB, media::GetEncodeBitstreamBufferSize(gfx::Size(1920, 1080)));
  EXPECT_EQ(kExpected4MB, media::GetEncodeBitstreamBufferSize(gfx::Size(1920, 1081)));
  EXPECT_EQ(kExpected4MB, media::GetEncodeBitstreamBufferSize(gfx::Size(2560, 1440)));
  EXPECT_EQ(kExpected8MB, media::GetEncodeBitstreamBufferSize(gfx::Size(2560, 1441)));

  // 5. Verify that negative constructor arguments and setter inputs are clamped
  // to 0, preserving the non-negative invariant required by Area64() (which
  // casts directly to uint64_t without sign checks).
  gfx::Size clamped_size(-100, -200);
  EXPECT_EQ(0, clamped_size.width());
  EXPECT_EQ(0, clamped_size.height());
  EXPECT_EQ(0ULL, clamped_size.Area64());
  EXPECT_EQ(0, clamped_size.GetCheckedArea().ValueOrDie());

  clamped_size.set_width(-50);
  clamped_size.set_height(-75);
  EXPECT_EQ(0, clamped_size.width());
  EXPECT_EQ(0, clamped_size.height());
  EXPECT_EQ(0ULL, clamped_size.Area64());
}

TEST(H264Encoder, GeometrySafeMathAndToString) {
  const gfx::Size overflowing_size(65536, 65536);
  EXPECT_FALSE(overflowing_size.GetCheckedArea().IsValid());
  EXPECT_EQ(4294967296ULL, overflowing_size.Area64());
  EXPECT_EQ("65536x65536", overflowing_size.ToString());

  const gfx::Size valid_size(1920, 1080);
  EXPECT_TRUE(valid_size.GetCheckedArea().IsValid());
  EXPECT_EQ(1920 * 1080, valid_size.GetCheckedArea().ValueOrDie());

  const gfx::Point pt(10, 20);
  EXPECT_EQ("10,20", pt.ToString());

  // Verify Rect preserves x, y, width, height and returns CheckedNumeric from
  // right() and bottom() that detects signed integer overflow via IsValid().
  const gfx::Rect overflowing_rect(std::numeric_limits<int>::max() - 10,
                                   std::numeric_limits<int>::max() - 20, 100, 200);
  EXPECT_EQ(100, overflowing_rect.width());
  EXPECT_EQ(200, overflowing_rect.height());
  EXPECT_FALSE(overflowing_rect.right().IsValid());
  EXPECT_FALSE(overflowing_rect.bottom().IsValid());
  EXPECT_FALSE(overflowing_rect.IsValid());

  const gfx::Rect valid_rect(10, 20, 100, 200);
  EXPECT_TRUE(valid_rect.right().IsValid());
  EXPECT_TRUE(valid_rect.bottom().IsValid());
  EXPECT_TRUE(valid_rect.IsValid());
  EXPECT_EQ(110, valid_rect.right().ValueOrDie());
  EXPECT_EQ(220, valid_rect.bottom().ValueOrDie());
  EXPECT_TRUE(valid_rect.Contains(50, 50));
  EXPECT_FALSE(valid_rect.Contains(200, 200));
  EXPECT_TRUE(valid_rect.Contains(gfx::Rect(20, 30, 50, 50)));
  EXPECT_FALSE(valid_rect.Contains(gfx::Rect(20, 30, 100, 200)));
}

TEST(H264Encoder, H264SPSGetVisibleRectRejectsOverflowingRect) {
  media::H264SPS sps;
  sps.frame_mbs_only_flag = true;
  sps.chroma_format_idc = 1;  // 4:2:0 -> crop_unit_x = 2, crop_unit_y = 2.
  sps.chroma_array_type = 1;

  // 1. Valid 1080p SPS with bottom cropping (1920x1088 cropped by 8 rows to 1920x1080).
  sps.pic_width_in_mbs_minus1 = 119;        // 120 * 16 = 1920
  sps.pic_height_in_map_units_minus1 = 67;  // 68 * 16 = 1088
  sps.frame_cropping_flag = true;
  sps.frame_crop_left_offset = 0;
  sps.frame_crop_right_offset = 0;
  sps.frame_crop_top_offset = 0;
  sps.frame_crop_bottom_offset = 4;  // crop_bottom = 8
  auto valid_visible_rect = sps.GetVisibleRect();
  ASSERT_TRUE(valid_visible_rect.has_value());
  EXPECT_EQ(gfx::Rect(0, 0, 1920, 1080), valid_visible_rect.value());

  // 2. Cropping offsets that cause visible_rect.right() (x + width) to overflow
  // signed 32-bit int without triggering UB during width computation:
  // coded_width = 16 * (134217726 + 1) = 2147483632 (INT_MAX - 15).
  // crop_left = 100, crop_right = -100 -> visible_width = 2147483632 (fits in int).
  // However, visible_rect.right() = 100 + 2147483632 = 2147483732 > INT_MAX,
  // which is detected by visible_rect.IsValid() and rejected cleanly.
  sps.pic_width_in_mbs_minus1 = std::numeric_limits<int>::max() / 16 - 1;
  sps.frame_crop_left_offset = 50;    // crop_left = 100
  sps.frame_crop_right_offset = -50;  // crop_right = -100
  EXPECT_FALSE(sps.GetVisibleRect().has_value());
}

}  // namespace
