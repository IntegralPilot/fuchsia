// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.audio/cpp/wire.h>
#include <lib/inspect/testing/cpp/zxtest/inspect.h>
#include <lib/simple-audio-stream/simple-audio-stream.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <set>

#include <audio-proto-utils/format-utils.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace audio {

namespace audio_fidl = fuchsia_hardware_audio;

namespace {

audio_fidl::wire::PcmFormat GetDefaultPcmFormat() {
  audio_fidl::wire::PcmFormat format;
  format.number_of_channels = 2;
  format.sample_format = audio_fidl::wire::SampleFormat::kPcmSigned;
  format.frame_rate = 48000;
  format.bytes_per_sample = 2;
  format.valid_bits_per_sample = 16;
  return format;
}

fidl::WireSyncClient<audio_fidl::StreamConfig> GetStreamClient(
    fidl::WireSyncClient<audio_fidl::StreamConfigConnector> client) {
  auto [stream_channel_local, stream_channel_remote] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result = client->Connect(std::move(stream_channel_remote));
  if (!result.ok()) {
    return {};
  }
  return fidl::WireSyncClient<audio_fidl::StreamConfig>(std::move(stream_channel_local));
}

class MockSimpleAudio : public SimpleAudioStream {
 public:
  static constexpr uint32_t kTestFrameRate = 48000;
  static constexpr uint8_t kTestNumberOfChannels = 2;
  static constexpr uint32_t kTestDriverTransferBytes = 16;
  static constexpr int64_t kTestExternalDelay = 123456789;
  static constexpr uint32_t kTestClockDomain = audio_fidl::wire::kClockDomainExternal;
  static constexpr uint32_t kTestPositionNotify = 4;
  static constexpr float kTestGain = 1.2345f;

  explicit MockSimpleAudio(zx_device_t* parent) : SimpleAudioStream(parent, false /* is input */) {}

  void PostSetPlugState(bool plugged, zx::duration delay) {
    async::PostDelayedTask(
        dispatcher(),
        [this, plugged]() {
          ScopedToken t(domain_token());
          SetPlugState(plugged);
        },
        delay);
  }
  inspect::Inspector& inspect() { return SimpleAudioStream::inspect(); }

 protected:
  zx_status_t Init() __TA_REQUIRES(domain_token()) override {
    fbl::AllocChecker ac;
    supported_formats_.reserve(1, &ac);
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
    SimpleAudioStream::SupportedFormat format = {};
    format.range.min_channels = kTestNumberOfChannels;
    format.range.max_channels = kTestNumberOfChannels;
    format.range.sample_formats = AUDIO_SAMPLE_FORMAT_16BIT;
    format.range.min_frames_per_second = kTestFrameRate;
    format.range.max_frames_per_second = kTestFrameRate;
    format.range.flags = ASF_RANGE_FLAG_FPS_48000_FAMILY;

    format.frequency_ranges.push_back({
        .min_frequency = 40,
        .max_frequency = 3'000,
    });
    format.frequency_ranges.push_back({
        .min_frequency = 3'000,
        .max_frequency = 25'000,
    });

    supported_formats_.push_back(std::move(format));

    external_delay_nsec_ = kTestExternalDelay;
    driver_transfer_bytes_ = kTestDriverTransferBytes;
    clock_domain_ = static_cast<int32_t>(kTestClockDomain);

    // Set our gain capabilities.
    cur_gain_state_.cur_gain = 0;
    cur_gain_state_.cur_mute = false;
    cur_gain_state_.cur_agc = false;
    cur_gain_state_.min_gain = 0;
    cur_gain_state_.max_gain = 100;
    cur_gain_state_.gain_step = 0;
    cur_gain_state_.can_mute = true;
    cur_gain_state_.can_agc = true;

    SetInitialPlugState(AUDIO_PDNF_CAN_NOTIFY);

    snprintf(device_name_, sizeof(device_name_), "test-audio-in");
    snprintf(mfr_name_, sizeof(mfr_name_), "Bike Sheds, Inc.");
    snprintf(prod_name_, sizeof(prod_name_), "testy_mctestface");

    unique_id_ = AUDIO_STREAM_UNIQUE_ID_BUILTIN_MICROPHONE;

    return ZX_OK;
  }

  zx_status_t SetGain(const audio_proto::SetGainReq& req) __TA_REQUIRES(domain_token()) override {
    if (req.flags & AUDIO_SGF_GAIN_VALID) {
      cur_gain_state_.cur_gain = req.gain;
    }
    if (req.flags & AUDIO_SGF_AGC_VALID) {
      cur_gain_state_.cur_agc = req.flags & AUDIO_SGF_AGC;
    }
    if (req.flags & AUDIO_SGF_MUTE_VALID) {
      cur_gain_state_.cur_mute = req.flags & AUDIO_SGF_MUTE;
    }
    return ZX_OK;
  }

  zx_status_t ChangeFormat(const audio_proto::StreamSetFmtReq& req)
      __TA_REQUIRES(domain_token()) override {
    return ZX_OK;
  }

  zx_status_t GetBuffer(const audio_proto::RingBufGetBufferReq& req, uint32_t* out_num_rb_frames,
                        zx::vmo* out_buffer) __TA_REQUIRES(domain_token()) override {
    if (req.min_ring_buffer_frames > MockSimpleAudio::kTestFrameRate * 2) {
      return ZX_ERR_NO_MEMORY;
    }
    zx::vmo rb;
    *out_num_rb_frames = req.min_ring_buffer_frames;
    zx::vmo::create(static_cast<uint64_t>(*out_num_rb_frames) * 2 * 2, 0, &rb);
    us_per_notification_ = (req.notifications_per_ring
                                ? (1'000'000 * req.min_ring_buffer_frames) /
                                      (MockSimpleAudio::kTestFrameRate * req.notifications_per_ring)
                                : 0);
    constexpr uint32_t rights = ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_TRANSFER;
    return rb.duplicate(rights, out_buffer);
  }

  zx_status_t Start(uint64_t* out_start_time) __TA_REQUIRES(domain_token()) override {
    *out_start_time = zx::clock::get_monotonic().get();
    if (us_per_notification_) {
      notify_timer_.PostDelayed(dispatcher(), zx::usec(static_cast<int64_t>(us_per_notification_)));
    }
    return ZX_OK;
  }

  zx_status_t Stop() __TA_REQUIRES(domain_token()) override {
    notify_timer_.Cancel();
    return ZX_OK;
  }
  zx_status_t ChangeActiveChannels(uint64_t mask, zx_time_t* set_time_out)
      __TA_REQUIRES(domain_token()) override {
    return ZX_ERR_NOT_SUPPORTED;
  }

  void ProcessRingNotification() {
    ScopedToken t(domain_token());
    audio_proto::RingBufPositionNotify resp = {};
    resp.hdr.cmd = AUDIO_RB_POSITION_NOTIFY;
    resp.monotonic_time = zx::clock::get_monotonic().get();
    resp.ring_buffer_pos = kTestPositionNotify;
    NotifyPosition(resp);
    if (us_per_notification_) {
      notify_timer_.PostDelayed(dispatcher(), zx::usec(static_cast<int64_t>(us_per_notification_)));
    }
  }

  void ShutdownHook() __TA_REQUIRES(domain_token()) override { Stop(); }

 private:
  async::TaskClosureMethod<MockSimpleAudio, &MockSimpleAudio::ProcessRingNotification> notify_timer_
      TA_GUARDED(domain_token()){this};

  uint64_t us_per_notification_ = 0;  // if 0, do not emit position notifications
};

class MockSimpleAudioWithExtension : public MockSimpleAudio {
 public:
  explicit MockSimpleAudioWithExtension(zx_device_t* parent) : MockSimpleAudio(parent) {}

 protected:
  zx_status_t ChangeActiveChannels(uint64_t mask, zx_time_t* set_time_out) override
      __TA_REQUIRES(domain_token()) {
    return (mask > (1u << kTestNumberOfChannels) - 1u) ? ZX_ERR_INVALID_ARGS : ZX_OK;
  }
};

class SimpleAudioTest : public inspect::InspectTestHelper, public zxtest::Test {
 public:
  void SetUp() override {}

  void TearDown() override {}

  fidl::WireSyncClient<audio_fidl::StreamConfigConnector> GetClient(MockSimpleAudio* server) {
    loop_.StartThread();
    auto endpoints = fidl::CreateEndpoints<audio_fidl::StreamConfigConnector>();
    fidl::BindServer(loop_.dispatcher(), std::move(endpoints->server), server);
    return fidl::WireSyncClient<audio_fidl::StreamConfigConnector>(std::move(endpoints->client));
  }

  template <typename T>
  static void CheckPropertyNotEqual(const inspect::NodeValue& node, const std::string& property,
                                    T not_expected_value) {
    const T* actual_value = node.get_property<T>(property);
    EXPECT_TRUE(actual_value);
    if (!actual_value) {
      return;
    }
    EXPECT_NE(not_expected_value.value(), actual_value->value());
  }

 protected:
  std::shared_ptr<MockDevice> root_ = MockDevice::FakeRootParent();
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

TEST_F(SimpleAudioTest, DdkLifeCycleTest) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);
  ddk::SuspendTxn txn(server->zxdev(), 0, false, DEVICE_SUSPEND_REASON_SELECTIVE_SUSPEND);
  server->DdkSuspend(std::move(txn));

  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, UnbindAndAlsoShutdown) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);
  server->DdkAsyncRemove();
  server->Shutdown();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, SetAndGetGain) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  {
    fidl::Arena allocator;
    auto gain_state =
        audio_fidl::wire::GainState::Builder(allocator).gain_db(MockSimpleAudio::kTestGain).Build();
    auto status = stream_client->SetGain(gain_state);
    ASSERT_OK(status.status());
  }

  auto gain_state = stream_client->WatchGainState();
  ASSERT_OK(gain_state.status());
  ASSERT_EQ(MockSimpleAudio::kTestGain, gain_state.value().gain_state.gain_db());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, WatchGainAndCloseStreamBeforeReply) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  {
    fidl::Arena allocator;
    auto gain_state =
        audio_fidl::wire::GainState::Builder(allocator).gain_db(MockSimpleAudio::kTestGain).Build();
    auto status = stream_client->SetGain(gain_state);
    ASSERT_OK(status.status());
  }

  // One watch for initial reply.
  auto gain_state = stream_client->WatchGainState();
  ASSERT_OK(gain_state.status());
  ASSERT_EQ(MockSimpleAudio::kTestGain, gain_state.value().gain_state.gain_db());

  // A second watch with no reply since there is no change of gain.
  // We use an asynchronous WireClient on a local client_loop to send this hanging get without
  // blocking the main test thread.
  async::Loop client_loop(&kAsyncLoopConfigAttachToCurrentThread);
  std::optional<fidl::WireClient<audio_fidl::StreamConfig>> client;
  client.emplace(stream_client.TakeClientEnd(), client_loop.dispatcher());

  // Send secondary WatchGainState request on the channel. Because the gain state has not changed
  // since the initial reply above, the server will not reply immediately; this becomes a pending
  // hanging get request on the server side.
  (*client)->WatchGainState().Then([](auto& result) {});

  bool watch_ready = false;
  (*client)->GetProperties().Then([&](auto& result) { watch_ready = true; });

  // Run the loop, blocking until events are processed, rather than busy-waiting.
  // Passing once=true causes Run() to return after processing a single event.
  while (!watch_ready) {
    client_loop.Run(zx::time::infinite(), /* once= */ true);
  }

  // Now close the channel while the secondary WatchGainState hanging get is still pending.
  // This verifies that SimpleAudioStream cleanly handles peer closure during pending hanging gets.
  client.reset();
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, SetAndGetAgc) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  {
    fidl::Arena allocator;
    auto gain_state = audio_fidl::wire::GainState::Builder(allocator).agc_enabled(true).Build();
    auto status = stream_client->SetGain(gain_state);
    ASSERT_OK(status.status());
  }

  auto gain_state1 = stream_client->WatchGainState();
  ASSERT_OK(gain_state1.status());
  ASSERT_TRUE(gain_state1.value().gain_state.agc_enabled());

  {
    fidl::Arena allocator;
    auto gain_state = audio_fidl::wire::GainState::Builder(allocator).agc_enabled(false).Build();
    auto status = stream_client->SetGain(gain_state);
    ASSERT_OK(status.status());
  }

  auto gain_state2 = stream_client->WatchGainState();
  ASSERT_OK(gain_state2.status());
  ASSERT_FALSE(gain_state2.value().gain_state.agc_enabled());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, SetAndGetMute) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  {
    fidl::Arena allocator;
    auto gain_state = audio_fidl::wire::GainState::Builder(allocator).muted(true).Build();
    auto status = stream_client->SetGain(gain_state);
    ASSERT_OK(status.status());
  }

  auto gain_state1 = stream_client->WatchGainState();
  ASSERT_OK(gain_state1.status());
  ASSERT_TRUE(gain_state1.value().gain_state.muted());

  {
    fidl::Arena allocator;
    auto gain_state = audio_fidl::wire::GainState::Builder(allocator).muted(false).Build();
    auto status = stream_client->SetGain(gain_state);
    ASSERT_OK(status.status());
  }

  auto gain_state2 = stream_client->WatchGainState();
  ASSERT_OK(gain_state2.status());
  ASSERT_FALSE(gain_state2.value().gain_state.muted());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, SetMuteWhenDisabled) {
  struct MockSimpleAudioLocal : public MockSimpleAudio {
    explicit MockSimpleAudioLocal(zx_device_t* parent) : MockSimpleAudio(parent) {}

   protected:
    zx_status_t Init() __TA_REQUIRES(domain_token()) override {
      auto status = MockSimpleAudio::Init();
      cur_gain_state_.can_mute = false;
      return status;
    }
  };
  auto server = SimpleAudioStream::Create<MockSimpleAudioLocal>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  {
    fidl::Arena allocator;
    auto gain_state = audio_fidl::wire::GainState::Builder(allocator).muted(true).Build();
    auto status = stream_client->SetGain(gain_state);
    ASSERT_OK(status.status());
  }

  auto gain_state1 = stream_client->WatchGainState();
  ASSERT_OK(gain_state1.status());
  ASSERT_FALSE(gain_state1.value().gain_state.has_muted());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, Enumerate1) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto ret = stream_client->GetSupportedFormats();
  auto& supported_formats = ret.value().supported_formats;
  auto& formats = supported_formats[0].pcm_supported_formats();
  ASSERT_EQ(1, formats.channel_sets().size());
  ASSERT_EQ(2, formats.channel_sets()[0].attributes().size());
  ASSERT_EQ(1, formats.sample_formats().size());
  ASSERT_EQ(audio_fidl::wire::SampleFormat::kPcmSigned, formats.sample_formats()[0]);
  ASSERT_EQ(1, formats.frame_rates().size());
  ASSERT_EQ(48000, formats.frame_rates()[0]);
  ASSERT_EQ(1, formats.bytes_per_sample().size());
  ASSERT_EQ(2, formats.bytes_per_sample()[0]);
  ASSERT_EQ(1, formats.valid_bits_per_sample().size());
  ASSERT_EQ(16, formats.valid_bits_per_sample()[0]);

  auto& channels_attributes = formats.channel_sets()[0].attributes();
  ASSERT_EQ(2, channels_attributes.size());
  ASSERT_EQ(40, channels_attributes[0].min_frequency());
  ASSERT_EQ(3'000, channels_attributes[0].max_frequency());
  ASSERT_EQ(3'000, channels_attributes[1].min_frequency());
  ASSERT_EQ(25'000, channels_attributes[1].max_frequency());

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, Enumerate2) {
  struct MockSimpleAudioLocal : public MockSimpleAudio {
    explicit MockSimpleAudioLocal(zx_device_t* parent) : MockSimpleAudio(parent) {}

   protected:
    zx_status_t Init() __TA_REQUIRES(domain_token()) override {
      auto status = MockSimpleAudio::Init();

      SimpleAudioStream::SupportedFormat format1 = {};
      SimpleAudioStream::SupportedFormat format2 = {};

      format1.range.min_channels = 2;
      format1.range.max_channels = 4;
      format1.range.sample_formats = AUDIO_SAMPLE_FORMAT_24BIT_IN32;
      format1.range.min_frames_per_second = 48'000;
      format1.range.max_frames_per_second = 768'000;
      format1.range.flags = ASF_RANGE_FLAG_FPS_48000_FAMILY;

      format2.range.min_channels = 1;
      format2.range.max_channels = 1;
      format2.range.sample_formats = AUDIO_SAMPLE_FORMAT_32BIT_FLOAT;
      format2.range.min_frames_per_second = 88'200;
      format2.range.max_frames_per_second = 88'200;
      format2.range.flags =
          ASF_RANGE_FLAG_FPS_CONTINUOUS;  // Ok only because min and max fps are equal.

      supported_formats_ = fbl::Vector<SupportedFormat>{format1, format2};
      return status;
    }
  };

  auto server = SimpleAudioStream::Create<MockSimpleAudioLocal>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto ret = stream_client->GetSupportedFormats();
  auto& supported_formats = ret.value().supported_formats;
  ASSERT_EQ(2, supported_formats.size());

  auto& formats1 = supported_formats[0].pcm_supported_formats();
  ASSERT_EQ(3, formats1.channel_sets().size());
  ASSERT_EQ(2, formats1.channel_sets()[0].attributes().size());
  ASSERT_EQ(3, formats1.channel_sets()[1].attributes().size());
  ASSERT_EQ(4, formats1.channel_sets()[2].attributes().size());
  ASSERT_EQ(1, formats1.sample_formats().size());
  ASSERT_EQ(audio_fidl::wire::SampleFormat::kPcmSigned, formats1.sample_formats()[0]);
  ASSERT_EQ(5, formats1.frame_rates().size());
  std::set<uint32_t> rates1;
  for (auto& i : formats1.frame_rates()) {
    rates1.insert(i);
  }
  ASSERT_EQ(rates1, std::set<uint32_t>({48'000, 96'000, 192'000, 384'000, 768'000}));
  ASSERT_EQ(1, formats1.bytes_per_sample().size());
  ASSERT_EQ(4, formats1.bytes_per_sample()[0]);
  ASSERT_EQ(1, formats1.valid_bits_per_sample().size());
  ASSERT_EQ(24, formats1.valid_bits_per_sample()[0]);

  auto& formats2 = supported_formats[1].pcm_supported_formats();
  ASSERT_EQ(1, formats2.channel_sets().size());
  ASSERT_EQ(1, formats2.channel_sets()[0].attributes().size());
  ASSERT_EQ(1, formats2.sample_formats().size());
  ASSERT_EQ(audio_fidl::wire::SampleFormat::kPcmFloat, formats2.sample_formats()[0]);
  ASSERT_EQ(1, formats2.frame_rates().size());
  std::set<uint32_t> rates2;
  for (auto& i : formats2.frame_rates()) {
    rates2.insert(i);
  }
  ASSERT_EQ(rates2, std::set<uint32_t>({88'200}));
  ASSERT_EQ(1, formats2.bytes_per_sample().size());
  ASSERT_EQ(4, formats2.bytes_per_sample()[0]);
  ASSERT_EQ(1, formats2.valid_bits_per_sample().size());
  ASSERT_EQ(32, formats2.valid_bits_per_sample()[0]);
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, EnumerateMultipleRateFamilies) {
  struct MockSimpleAudioLocal : public MockSimpleAudio {
    explicit MockSimpleAudioLocal(zx_device_t* parent) : MockSimpleAudio(parent) {}

   protected:
    zx_status_t Init() __TA_REQUIRES(domain_token()) override {
      auto status = MockSimpleAudio::Init();

      SimpleAudioStream::SupportedFormat format = {};

      format.range.min_channels = 2;
      format.range.max_channels = 2;
      format.range.sample_formats = AUDIO_SAMPLE_FORMAT_24BIT_IN32;
      format.range.min_frames_per_second = 44'100;
      format.range.max_frames_per_second = 96'000;
      format.range.flags = ASF_RANGE_FLAG_FPS_48000_FAMILY | ASF_RANGE_FLAG_FPS_44100_FAMILY;

      supported_formats_ = fbl::Vector<SupportedFormat>{format};
      return status;
    }
  };

  auto server = SimpleAudioStream::Create<MockSimpleAudioLocal>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto ret = stream_client->GetSupportedFormats();
  auto& supported_formats = ret.value().supported_formats;
  ASSERT_EQ(1, supported_formats.size());

  auto& formats = supported_formats[0].pcm_supported_formats();
  ASSERT_EQ(1, formats.channel_sets().size());
  ASSERT_EQ(2, formats.channel_sets()[0].attributes().size());
  ASSERT_EQ(1, formats.sample_formats().size());
  ASSERT_EQ(audio_fidl::wire::SampleFormat::kPcmSigned, formats.sample_formats()[0]);
  ASSERT_EQ(4, formats.frame_rates().size());
  // Must enumerate them in ascending order.
  ASSERT_EQ(formats.frame_rates()[0], 44'100);
  ASSERT_EQ(formats.frame_rates()[1], 48'000);
  ASSERT_EQ(formats.frame_rates()[2], 88'200);
  ASSERT_EQ(formats.frame_rates()[3], 96'000);

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, CreateRingBuffer1) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();
  auto result0 = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(result0.status());

  auto result1 = fidl::WireCall(local)->GetProperties();
  ASSERT_OK(result1.status());
  ASSERT_EQ(result1.value().properties.driver_transfer_bytes(),
            MockSimpleAudio::kTestDriverTransferBytes);
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, CreateRingBuffer2) {
  struct MockSimpleAudioLocal : public MockSimpleAudio {
    explicit MockSimpleAudioLocal(zx_device_t* parent) : MockSimpleAudio(parent) {}

   protected:
    zx_status_t Init() __TA_REQUIRES(domain_token()) override {
      SimpleAudioStream::SupportedFormat format = {};
      format.range.min_channels = 1;
      format.range.max_channels = 4;
      format.range.sample_formats =
          AUDIO_SAMPLE_FORMAT_24BIT_IN32 | AUDIO_SAMPLE_FORMAT_FLAG_UNSIGNED;
      format.range.min_frames_per_second = 22050;
      format.range.max_frames_per_second = 88200;
      format.range.flags = ASF_RANGE_FLAG_FPS_44100_FAMILY;
      supported_formats_.push_back(std::move(format));
      return MockSimpleAudio::Init();
    }
  };
  auto server = SimpleAudioStream::Create<MockSimpleAudioLocal>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  audio_fidl::wire::PcmFormat pcm_format;
  pcm_format.number_of_channels = 4;
  pcm_format.sample_format = audio_fidl::wire::SampleFormat::kPcmUnsigned;
  pcm_format.frame_rate = 44100;
  pcm_format.bytes_per_sample = 4;
  pcm_format.valid_bits_per_sample = 24;

  fidl::Arena allocator;
  auto format = audio_fidl::wire::Format::Builder(allocator).pcm_format(pcm_format).Build();
  auto result0 = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(result0.status());

  auto result1 = fidl::WireCall(local)->GetProperties();
  ASSERT_OK(result1.status());
  ASSERT_EQ(result1.value().properties.driver_transfer_bytes(),
            MockSimpleAudio::kTestDriverTransferBytes);
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, SetBadFormat1) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  // Define a pretty bad format.
  audio_fidl::wire::PcmFormat pcm_format;
  pcm_format.sample_format = audio_fidl::wire::SampleFormat::kPcmSigned;

  fidl::Arena allocator;
  auto format = audio_fidl::wire::Format::Builder(allocator).pcm_format(pcm_format).Build();

  auto result0 = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_EQ(ZX_OK, result0.status());  // CreateRingBuffer is sent successfully.

  auto result1 = stream_client->GetSupportedFormats();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, result1.status());  // With a bad format we get a channel close.

  auto result2 = fidl::WireCall(local)->GetProperties();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, result2.status());  // With a bad format we get a channel close.
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, SetBadFormat2) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  // Define an almost good format.
  audio_fidl::wire::PcmFormat pcm_format = GetDefaultPcmFormat();
  pcm_format.frame_rate = 48001;  // Bad rate.

  fidl::Arena allocator;
  auto format = audio_fidl::wire::Format::Builder(allocator).pcm_format(pcm_format).Build();

  auto result0 = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_EQ(ZX_OK, result0.status());  // CreateRingBuffer is sent successfully.

  auto result1 = stream_client->GetSupportedFormats();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, result1.status());  // With a bad format we get a channel close.

  auto result2 = fidl::WireCall(local)->GetProperties();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, result2.status());  // With a bad format we get a channel close.
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, GetIds) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto result = stream_client->GetProperties();
  ASSERT_OK(result.status());

  audio_stream_unique_id_t mic = AUDIO_STREAM_UNIQUE_ID_BUILTIN_MICROPHONE;
  ASSERT_BYTES_EQ(result.value().properties.unique_id().data(), mic.data,
                  strlen(reinterpret_cast<char*>(mic.data)));
  ASSERT_BYTES_EQ(result.value().properties.manufacturer().data(), "Bike Sheds, Inc.",
                  strlen("Bike Sheds, Inc."));
  ASSERT_EQ(result.value().properties.clock_domain(), MockSimpleAudio::kTestClockDomain);
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, MultipleChannelsPlugDetectState) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  // We get 2 clients from the one FIDL channel acquired via FidlClient().
  fidl::WireSyncClient client_wrap{GetClient(server.get())};
  ASSERT_TRUE(client_wrap.is_valid());
  auto [stream_channel_local1, stream_channel_remote1] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result0 = client_wrap->Connect(std::move(stream_channel_remote1));
  ASSERT_OK(result0.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client1(std::move(stream_channel_local1));
  auto [stream_channel_local2, stream_channel_remote2] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result1 = client_wrap->Connect(std::move(stream_channel_remote2));
  ASSERT_OK(result1.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client2(std::move(stream_channel_local2));

  auto prop1 = stream_client1->GetProperties();
  auto prop2 = stream_client2->GetProperties();
  ASSERT_OK(prop1.status());
  ASSERT_OK(prop2.status());

  ASSERT_EQ(prop1.value().properties.plug_detect_capabilities(),
            audio_fidl::wire::PlugDetectCapabilities::kCanAsyncNotify);
  ASSERT_EQ(prop2.value().properties.plug_detect_capabilities(),
            audio_fidl::wire::PlugDetectCapabilities::kCanAsyncNotify);

  auto state1 = stream_client1->WatchPlugState();
  auto state2 = stream_client2->WatchPlugState();
  ASSERT_OK(state1.status());
  ASSERT_OK(state2.status());
  ASSERT_FALSE(state1.value().plug_state.plugged());
  ASSERT_FALSE(state2.value().plug_state.plugged());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, WatchPlugDetectAndCloseStreamBeforeReply) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  // We get 2 clients from the one FIDL channel acquired via FidlClient().
  fidl::WireSyncClient client_wrap{GetClient(server.get())};
  ASSERT_TRUE(client_wrap.is_valid());
  auto [stream_channel_local1, stream_channel_remote1] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result0 = client_wrap->Connect(std::move(stream_channel_remote1));
  ASSERT_OK(result0.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client1(std::move(stream_channel_local1));
  auto [stream_channel_local2, stream_channel_remote2] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result1 = client_wrap->Connect(std::move(stream_channel_remote2));
  ASSERT_OK(result1.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client2(std::move(stream_channel_local2));

  auto prop1 = stream_client1->GetProperties();
  auto prop2 = stream_client2->GetProperties();
  ASSERT_OK(prop1.status());
  ASSERT_OK(prop2.status());

  ASSERT_EQ(prop1.value().properties.plug_detect_capabilities(),
            audio_fidl::wire::PlugDetectCapabilities::kCanAsyncNotify);
  ASSERT_EQ(prop2.value().properties.plug_detect_capabilities(),
            audio_fidl::wire::PlugDetectCapabilities::kCanAsyncNotify);

  // Watch each channel for initial reply. The first call to WatchPlugState returns immediately
  // with the current plug detect state.
  auto state1 = stream_client1->WatchPlugState();
  auto state2 = stream_client2->WatchPlugState();
  ASSERT_OK(state1.status());
  ASSERT_OK(state2.status());
  ASSERT_FALSE(state1.value().plug_state.plugged());
  ASSERT_FALSE(state2.value().plug_state.plugged());

  // Secondary watches with no reply since there is no change of plug detect state.
  // We use asynchronous WireClients on a local client_loop to send these hanging gets without
  // blocking the main test thread.
  async::Loop client_loop(&kAsyncLoopConfigAttachToCurrentThread);
  std::optional<fidl::WireClient<audio_fidl::StreamConfig>> client1;
  client1.emplace(stream_client1.TakeClientEnd(), client_loop.dispatcher());
  std::optional<fidl::WireClient<audio_fidl::StreamConfig>> client2;
  client2.emplace(stream_client2.TakeClientEnd(), client_loop.dispatcher());

  // Send secondary WatchPlugState requests on each channel. Because the plug state has not changed
  // since the initial replies above, the server will not reply immediately; these become pending
  // hanging get requests on the server side.
  (*client1)->WatchPlugState().Then([](auto& result) {});
  (*client2)->WatchPlugState().Then([](auto& result) {});

  bool watch1_ready = false, watch2_ready = false;
  (*client1)->GetProperties().Then([&](auto& result) { watch1_ready = true; });
  (*client2)->GetProperties().Then([&](auto& result) { watch2_ready = true; });

  // Run the loop, blocking until events are processed, rather than busy-waiting.
  // Passing once=true causes Run() to return after processing a single event.
  while (!watch1_ready || !watch2_ready) {
    client_loop.Run(zx::time::infinite(), /* once= */ true);
  }

  // Now close the channels while the secondary WatchPlugState hanging gets are still pending.
  // This verifies that SimpleAudioStream cleanly handles peer closure during pending hanging gets.
  client1.reset();
  client2.reset();
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, MultipleChannelsPlugDetectNotify) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  fidl::WireSyncClient client_wrap{GetClient(server.get())};
  ASSERT_TRUE(client_wrap.is_valid());
  auto [stream_channel_local1, stream_channel_remote1] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result0 = client_wrap->Connect(std::move(stream_channel_remote1));
  ASSERT_OK(result0.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client1(std::move(stream_channel_local1));
  auto [stream_channel_local2, stream_channel_remote2] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result1 = client_wrap->Connect(std::move(stream_channel_remote2));
  ASSERT_OK(result1.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client2(std::move(stream_channel_local2));
  auto [stream_channel_local3, stream_channel_remote3] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result2 = client_wrap->Connect(std::move(stream_channel_remote3));
  ASSERT_OK(result2.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client3(std::move(stream_channel_local3));

  auto state1a = stream_client1->WatchPlugState();
  auto state2a = stream_client2->WatchPlugState();
  auto state3a = stream_client3->WatchPlugState();
  ASSERT_OK(state1a.status());
  ASSERT_OK(state2a.status());
  ASSERT_OK(state3a.status());
  ASSERT_FALSE(state1a.value().plug_state.plugged());
  ASSERT_FALSE(state2a.value().plug_state.plugged());
  ASSERT_FALSE(state3a.value().plug_state.plugged());

  server->PostSetPlugState(true, zx::duration(zx::msec(100)));

  auto state1b = stream_client1->WatchPlugState();
  auto state2b = stream_client2->WatchPlugState();
  auto state3b = stream_client3->WatchPlugState();
  ASSERT_OK(state1b.status());
  ASSERT_OK(state2b.status());
  ASSERT_OK(state3b.status());
  ASSERT_TRUE(state1b.value().plug_state.plugged());
  ASSERT_TRUE(state2b.value().plug_state.plugged());
  ASSERT_TRUE(state3b.value().plug_state.plugged());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, MultipleChannelsGainState) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  fidl::WireSyncClient client_wrap{GetClient(server.get())};
  ASSERT_TRUE(client_wrap.is_valid());
  auto [stream_channel_local1, stream_channel_remote1] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result0 = client_wrap->Connect(std::move(stream_channel_remote1));
  ASSERT_OK(result0.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client1(std::move(stream_channel_local1));
  auto [stream_channel_local2, stream_channel_remote2] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result1 = client_wrap->Connect(std::move(stream_channel_remote2));
  ASSERT_OK(result1.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client2(std::move(stream_channel_local2));

  auto state1 = stream_client1->WatchGainState();
  auto state2 = stream_client2->WatchGainState();
  ASSERT_OK(state1.status());
  ASSERT_OK(state2.status());
  ASSERT_EQ(0.0f, state1.value().gain_state.gain_db());
  ASSERT_EQ(0.0f, state2.value().gain_state.gain_db());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, MultipleChannelsGainStateNotify) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  fidl::WireSyncClient client_wrap{GetClient(server.get())};
  ASSERT_TRUE(client_wrap.is_valid());
  auto [stream_channel_local1, stream_channel_remote1] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result0 = client_wrap->Connect(std::move(stream_channel_remote1));
  ASSERT_OK(result0.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client1(std::move(stream_channel_local1));
  auto [stream_channel_local2, stream_channel_remote2] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result1 = client_wrap->Connect(std::move(stream_channel_remote2));
  ASSERT_OK(result1.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client2(std::move(stream_channel_local2));
  auto [stream_channel_local3, stream_channel_remote3] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result2 = client_wrap->Connect(std::move(stream_channel_remote3));
  ASSERT_OK(result2.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client3(std::move(stream_channel_local3));

  auto state1a = stream_client1->WatchGainState();
  auto state2a = stream_client2->WatchGainState();
  auto state3a = stream_client3->WatchGainState();
  ASSERT_OK(state1a.status());
  ASSERT_OK(state2a.status());
  ASSERT_OK(state3a.status());
  ASSERT_EQ(0.0f, state1a.value().gain_state.gain_db());
  ASSERT_EQ(0.0f, state2a.value().gain_state.gain_db());
  ASSERT_EQ(0.0f, state3a.value().gain_state.gain_db());

  // Use asynchronous WireClients on a local client_loop to send secondary WatchGainState requests
  // without blocking the main test thread.
  async::Loop client_loop(&kAsyncLoopConfigAttachToCurrentThread);
  std::optional<fidl::WireClient<audio_fidl::StreamConfig>> client1;
  client1.emplace(stream_client1.TakeClientEnd(), client_loop.dispatcher());
  std::optional<fidl::WireClient<audio_fidl::StreamConfig>> client2;
  client2.emplace(stream_client2.TakeClientEnd(), client_loop.dispatcher());
  std::optional<fidl::WireClient<audio_fidl::StreamConfig>> client3;
  client3.emplace(stream_client3.TakeClientEnd(), client_loop.dispatcher());

  bool watch1_done = false, watch2_done = false, watch3_done = false;
  (*client1)->WatchGainState().Then([&](auto& result) {
    ASSERT_OK(result.status());
    ASSERT_EQ(MockSimpleAudio::kTestGain, result.value().gain_state.gain_db());
    watch1_done = true;
  });
  (*client2)->WatchGainState().Then([&](auto& result) {
    ASSERT_OK(result.status());
    ASSERT_EQ(MockSimpleAudio::kTestGain, result.value().gain_state.gain_db());
    watch2_done = true;
  });
  (*client3)->WatchGainState().Then([&](auto& result) {
    ASSERT_OK(result.status());
    ASSERT_EQ(MockSimpleAudio::kTestGain, result.value().gain_state.gain_db());
    watch3_done = true;
  });

  // Use GetProperties as a FIFO barrier on each channel to ensure the server has dequeued and
  // registered all three hanging get requests before we change the gain.
  bool ready1 = false, ready2 = false, ready3 = false;
  (*client1)->GetProperties().Then([&](auto& result) { ready1 = true; });
  (*client2)->GetProperties().Then([&](auto& result) { ready2 = true; });
  (*client3)->GetProperties().Then([&](auto& result) { ready3 = true; });

  while (!ready1 || !ready2 || !ready3) {
    client_loop.Run(zx::time::infinite(), /* once= */ true);
  }

  // Now change the gain. This should trigger completion of the pending WatchGainState requests
  // on all three channels.
  fidl::Arena allocator;
  auto gain_state = audio_fidl::wire::GainState::Builder(allocator)
                        .muted(false)
                        .agc_enabled(false)
                        .gain_db(MockSimpleAudio::kTestGain)
                        .Build();
  auto status = (*client1)->SetGain(gain_state);
  ASSERT_OK(status.status());

  while (!watch1_done || !watch2_done || !watch3_done) {
    client_loop.Run(zx::time::infinite(), /* once= */ true);
  }
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, RingBufferTests) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto default_format = GetDefaultPcmFormat();
  auto format = audio_fidl::wire::Format::Builder(allocator).pcm_format(default_format).Build();
  uint32_t frame_size = default_format.number_of_channels * default_format.bytes_per_sample;

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  constexpr uint32_t kNumberOfPositionNotifications = 5;
  // Buffer is set to hold at least 1 second, with kNumberOfPositionNotifications notifications
  // per ring buffer (i.e. per second) we set the time waiting for the watch below to 200ms+.
  constexpr uint32_t kMinFrames = MockSimpleAudio::kTestFrameRate;
  auto vmo = fidl::WireCall(local)->GetVmo(kMinFrames, kNumberOfPositionNotifications);
  ASSERT_OK(vmo.status());
  uint32_t frames_expected =
      kMinFrames + ((MockSimpleAudio::kTestDriverTransferBytes + frame_size - 1) / frame_size);
  ASSERT_EQ(vmo->value()->num_frames, frames_expected);

  constexpr uint64_t kSomeActiveChannelsMask = 0xc3;
  auto active_channels = fidl::WireCall(local)->SetActiveChannels(kSomeActiveChannelsMask);
  ASSERT_TRUE(active_channels->is_error());
  ASSERT_EQ(active_channels->error_value(), ZX_ERR_NOT_SUPPORTED);

  // Check inspect state.
  {
    ASSERT_NO_FATAL_FAILURE(ReadInspect(server->inspect().DuplicateVmo()));
    auto* simple_audio = hierarchy().GetByPath({"simple_audio_stream"});
    ASSERT_TRUE(simple_audio);
    ASSERT_NO_FATAL_FAILURE(
        CheckProperty(simple_audio->node(), "state", inspect::StringPropertyValue("created")));
    ASSERT_NO_FATAL_FAILURE(
        CheckProperty(simple_audio->node(), "start_time", inspect::IntPropertyValue(0)));
    ASSERT_NO_FATAL_FAILURE(
        CheckProperty(simple_audio->node(), "frames_requested",
                      inspect::UintPropertyValue(MockSimpleAudio::kTestFrameRate)));
  }

  auto start = fidl::WireCall(local)->Start();
  ASSERT_OK(start.status());

  // Check updated inspect state.
  {
    ASSERT_NO_FATAL_FAILURE(ReadInspect(server->inspect().DuplicateVmo()));
    auto* simple_audio = hierarchy().GetByPath({"simple_audio_stream"});
    ASSERT_TRUE(simple_audio);
    ASSERT_NO_FATAL_FAILURE(
        CheckProperty(simple_audio->node(), "state", inspect::StringPropertyValue("started")));
    ASSERT_NO_FATAL_FAILURE(
        CheckPropertyNotEqual(simple_audio->node(), "start_time", inspect::IntPropertyValue(0)));
  }

  auto position = fidl::WireCall(local)->WatchClockRecoveryPositionInfo();
  ASSERT_EQ(MockSimpleAudio::kTestPositionNotify, position.value().position_info.position);

  auto stop = fidl::WireCall(local)->Stop();
  ASSERT_OK(stop.status());
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

// Validate that the library can succeed and fail for SetActiveChannels calls.
TEST_F(SimpleAudioTest, SetActiveChannels) {
  auto server = SimpleAudioStream::Create<MockSimpleAudioWithExtension>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto vmo = fidl::WireCall(local)->GetVmo(MockSimpleAudio::kTestFrameRate, 0);
  ASSERT_OK(vmo.status());

  auto active_channels1 = fidl::WireCall(local)->SetActiveChannels(0x01);
  ASSERT_TRUE(active_channels1->is_ok());

  auto active_channels2 = fidl::WireCall(local)->SetActiveChannels(0x0fff);
  ASSERT_TRUE(active_channels2->is_error());
  ASSERT_EQ(active_channels2->error_value(), ZX_ERR_INVALID_ARGS);

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, RingBufferStartBeforeGetVmo) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  // Start() before GetVmo() must result in channel closure
  auto start = fidl::WireCall(local)->Start();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, start.status());  // We get a channel close.

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, RingBufferStartAfterFailedGetVmo) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  // First GetVmo succeeds.
  auto vmo1 = fidl::WireCall(local)->GetVmo(MockSimpleAudio::kTestFrameRate, 0);
  ASSERT_OK(vmo1.status());

  // Second GetVmo requests a large frame count exceeding driver limits, causing GetBuffer to fail.
  auto vmo2 = fidl::WireCall(local)->GetVmo(MockSimpleAudio::kTestFrameRate * 10, 0);
  ASSERT_TRUE(vmo2.ok());
  ASSERT_TRUE(vmo2.value().is_error());

  // Calling Start() after a failed GetVmo() must not start the driver and must close the channel.
  auto start = fidl::WireCall(local)->Start();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, start.status());

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, RingBufferGetVmoExceedingSizeCap) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  // Requesting frames exceeding 4 MiB cap returns kInvalidArgs.
  auto vmo = fidl::WireCall(local)->GetVmo(2'000'000, 0);
  ASSERT_TRUE(vmo.ok());
  ASSERT_TRUE(vmo.value().is_error());
  ASSERT_EQ(audio_fidl::wire::GetVmoError::kInvalidArgs, vmo.value().error_value());

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, RingBufferStartWhileStarted) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto vmo = fidl::WireCall(local)->GetVmo(MockSimpleAudio::kTestFrameRate, 0);
  ASSERT_OK(vmo.status());

  auto start = fidl::WireCall(local)->Start();
  ASSERT_OK(start.status());

  // Start() while already started must result in channel closure
  auto restart = fidl::WireCall(local)->Start();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, restart.status());  // We get a channel close.

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, RingBufferStopBeforeGetVmo) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  // Stop() before GetVmo() must result in channel closure
  auto stop = fidl::WireCall(local)->Stop();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, stop.status());  // We get a channel close.

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, RingBufferStopWhileStopped) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  auto vmo = fidl::WireCall(local)->GetVmo(MockSimpleAudio::kTestFrameRate, 0);
  ASSERT_OK(vmo.status());

  // We are already stopped, but this should be harmless
  auto stop = fidl::WireCall(local)->Stop();
  ASSERT_OK(stop.status());
  // Another stop immediately afterward should also be harmless
  auto restop = fidl::WireCall(local)->Stop();
  ASSERT_OK(restop.status());

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, WatchPositionAndCloseRingBufferBeforeReply) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());

  constexpr uint32_t kNumberOfPositionNotifications = 5;
  // Buffer is set to hold at least 1 second, with kNumberOfPositionNotifications notifications
  // per ring buffer (i.e. per second) the time waiting before getting a position reply is 200ms+.

  auto vmo = fidl::WireCall(local)->GetVmo(MockSimpleAudio::kTestFrameRate,
                                           kNumberOfPositionNotifications);
  ASSERT_OK(vmo.status());

  auto start = fidl::WireCall(local)->Start();
  ASSERT_OK(start.status());

  // Watch position notifications using an asynchronous WireClient on a local client_loop to send
  // this hanging get without blocking the main test thread.
  async::Loop client_loop(&kAsyncLoopConfigAttachToCurrentThread);
  std::optional<fidl::WireClient<audio_fidl::RingBuffer>> client;
  client.emplace(std::move(local), client_loop.dispatcher());

  (*client)->WatchClockRecoveryPositionInfo().Then([](auto& result) {});

  // Use GetProperties as a FIFO barrier to ensure the server has dequeued and registered the
  // WatchClockRecoveryPositionInfo hanging get before we close the channel.
  bool watch_ready = false;
  (*client)->GetProperties().Then([&](auto& result) { watch_ready = true; });

  while (!watch_ready) {
    client_loop.Run(zx::time::infinite(), /* once= */ true);
  }

  // Now close the channel while the WatchClockRecoveryPositionInfo hanging get is still pending.
  // This verifies that SimpleAudioStream cleanly handles peer closure during pending hanging gets.
  client.reset();
  stream_client.TakeClientEnd().TakeChannel().reset();
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, GetDriverTransferBytes) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();
  auto result0 = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_EQ(ZX_OK, result0.status());  // CreateRingBuffer is sent successfully.

  auto result1 = fidl::WireCall(local)->GetProperties();
  ASSERT_OK(result1.status());
  ASSERT_EQ(result1.value().properties.driver_transfer_bytes(),
            MockSimpleAudio::kTestDriverTransferBytes);

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, WatchDelays) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());
  auto [local, remote] = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto rb = stream_client->CreateRingBuffer(format, std::move(remote));
  ASSERT_OK(rb.status());
  auto delay_info = fidl::WireCall(local)->WatchDelayInfo();
  ASSERT_OK(delay_info.status());
  ASSERT_EQ(delay_info->delay_info.internal_delay(), 0);
  ASSERT_EQ(delay_info->delay_info.external_delay(), MockSimpleAudio::kTestExternalDelay);

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, ClientCloseStreamConfigProtocol) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  // To make sure the 1-way Connect call is completed in the StreamConfigConnector server,
  // make a 2-way call. Since StreamConfigConnector does not have a 2-way call, we use
  // StreamConfig synchronously.
  auto result = stream_client->GetProperties();
  ASSERT_OK(result.status());

  stream_client.TakeClientEnd().TakeChannel().reset();
  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, ClientCloseRingBufferProtocol) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto endpoints = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto ret = stream_client->CreateRingBuffer(format, std::move(endpoints.server));
  ASSERT_OK(ret.status());

  // To make sure the 1-way Connect call is completed in the StreamConfigConnector server,
  // make a 2-way call. Since StreamConfigConnector does not have a 2-way call, we use
  // StreamConfig synchronously.
  auto result = stream_client->GetProperties();
  ASSERT_OK(result.status());

  endpoints.client.reset();

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, ClientCloseStreamConfigProtocolWithARingBufferProtocol) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  auto stream_client = GetStreamClient(GetClient(server.get()));
  ASSERT_TRUE(stream_client.is_valid());

  auto endpoints = fidl::CreateEndpoints<audio_fidl::RingBuffer>();
  ASSERT_OK(endpoints.status_value());

  fidl::Arena allocator;
  auto format =
      audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

  auto ret = stream_client->CreateRingBuffer(format, std::move(endpoints->server));
  ASSERT_OK(ret.status());

  // To make sure the 1-way Connect call is completed in the StreamConfigConnector server,
  // make a 2-way call. Since StreamConfigConnector does not have a 2-way call, we use
  // StreamConfig synchronously.
  auto result = stream_client->GetProperties();
  ASSERT_OK(result.status());

  stream_client = {};

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

TEST_F(SimpleAudioTest, NonPrivileged) {
  auto server = SimpleAudioStream::Create<MockSimpleAudio>(root_.get());
  ASSERT_NOT_NULL(server);

  fidl::WireSyncClient client_wrap{GetClient(server.get())};
  ASSERT_TRUE(client_wrap.is_valid());
  auto [stream_channel_local1, stream_channel_remote1] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result0 = client_wrap->Connect(std::move(stream_channel_remote1));
  ASSERT_OK(result0.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client1(std::move(stream_channel_local1));
  auto [stream_channel_local2, stream_channel_remote2] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result1 = client_wrap->Connect(std::move(stream_channel_remote2));
  ASSERT_OK(result1.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client2(std::move(stream_channel_local2));
  auto [stream_channel_local3, stream_channel_remote3] =
      fidl::Endpoints<audio_fidl::StreamConfig>::Create();
  auto result2 = client_wrap->Connect(std::move(stream_channel_remote3));
  ASSERT_OK(result2.status());
  fidl::WireSyncClient<audio_fidl::StreamConfig> stream_client3(std::move(stream_channel_local3));

  auto endpoints1 = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  {
    fidl::Arena allocator;
    auto format =
        audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

    auto ret1 = stream_client1->CreateRingBuffer(format, std::move(endpoints1.server));
    ASSERT_OK(ret1.status());
  }
  fidl::WireSyncClient<audio_fidl::RingBuffer> ringbuffer1(std::move(endpoints1.client));
  auto vmo1 = ringbuffer1->GetVmo(MockSimpleAudio::kTestFrameRate,
                                  /* clock_recovery_notifications_per_ring = */ 0);
  ASSERT_OK(vmo1.status());

  auto endpoints2 = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  {
    fidl::Arena allocator;
    auto format =
        audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

    auto ret2 = stream_client2->CreateRingBuffer(format, std::move(endpoints2.server));
    ASSERT_OK(ret2.status());
  }
  fidl::WireSyncClient<audio_fidl::RingBuffer> ringbuffer2(std::move(endpoints2.client));
  auto vmo2 = ringbuffer2->GetVmo(MockSimpleAudio::kTestFrameRate, 0);
  ASSERT_NOT_OK(vmo2.status());  // Non-privileged channel.

  auto endpoints3 = fidl::Endpoints<audio_fidl::RingBuffer>::Create();

  {
    fidl::Arena allocator;
    auto format =
        audio_fidl::wire::Format::Builder(allocator).pcm_format(GetDefaultPcmFormat()).Build();

    auto ret3 = stream_client3->CreateRingBuffer(format, std::move(endpoints3.server));
    ASSERT_OK(ret3.status());
  }
  fidl::WireSyncClient<audio_fidl::RingBuffer> ringbuffer3(std::move(endpoints3.client));
  auto vmo3 = ringbuffer3->GetVmo(MockSimpleAudio::kTestFrameRate, 0);
  ASSERT_NOT_OK(vmo3.status());  // Non-privileged channel.

  loop_.Shutdown();
  server->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
}

}  // namespace

}  // namespace audio
