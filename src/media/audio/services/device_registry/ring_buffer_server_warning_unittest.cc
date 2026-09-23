// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/zx/clock.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/ring_buffer_server.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_ring_buffer.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class RingBufferServerWarningTest : public AudioDeviceRegistryServerTestBase,
                                    public fidl::AsyncEventHandler<fad::RingBuffer> {
 public:
  void handle_unknown_event(fidl::UnknownEventMetadata<fad::RingBuffer> metadata) override;

 protected:
  static fad::RingBufferOptions DefaultRingBufferOptions() {
    return {{
        .format = fuchsia_audio::Format{{
            .sample_type = fuchsia_audio::SampleType::kInt16,
            .channel_count = 2,
            .frames_per_second = 48000,
        }},
        .ring_buffer_min_bytes = 2000,
    }};
  }

  std::pair<fidl::Client<fad::RingBuffer>, fidl::ServerEnd<fad::RingBuffer>>
  CreateRingBufferClient() {
    auto [ring_buffer_client_end, ring_buffer_server_end] =
        CreateNaturalAsyncClientOrDie<fad::RingBuffer>();
    auto ring_buffer_client =
        fidl::Client<fad::RingBuffer>(std::move(ring_buffer_client_end), dispatcher(), this);
    return std::make_pair(std::move(ring_buffer_client), std::move(ring_buffer_server_end));
  }
};

void RingBufferServerWarningTest::handle_unknown_event(
    fidl::UnknownEventMetadata<fad::RingBuffer> metadata) {
  FAIL() << "RingBufferServerWarningTest: unknown event (RingBuffer) ordinal "
         << metadata.event_ordinal;
}

class RingBufferServerCompositeWarningTest : public RingBufferServerWarningTest {
 protected:
  static inline const std::string kClassName = "RingBufferServerCompositeWarningTest";
  std::shared_ptr<Device> EnableDriverAndAddDevice(
      const std::shared_ptr<FakeComposite>& fake_driver) {
    auto device = Device::Create(
        adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
        fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName);
    adr_service()->AddDevice(device);

    RunLoopUntilIdle();
    return device;
  }

  fidl::Client<fad::RingBuffer>& ring_buffer_client() { return ring_buffer_client_; }

 private:
  std::unique_ptr<TestServerAndAsyncClient<media_audio::ControlServer, fidl::Client>> control_;
  std::shared_ptr<Device> device_;
  fidl::Client<fad::RingBuffer> ring_buffer_client_;
};

TEST_F(RingBufferServerCompositeWarningTest, SetActiveChannelsMissingChannelBitmask) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;

  ring_buffer_client
      ->SetActiveChannels({
          // No `channel_bitmask` value is included in this call.
      })
      .Then([&received_callback](fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RingBufferSetActiveChannelsError::kInvalidChannelBitmask)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  // This should be entirely unchanged.
  EXPECT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
}

TEST_F(RingBufferServerCompositeWarningTest, SetActiveChannelsBadChannelBitmask) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;

  ring_buffer_client
      ->SetActiveChannels({{
          0xFFFF,  // This channel bitmask includes values outside the total number of channels.
      }})
      .Then([&received_callback](fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RingBufferSetActiveChannelsError::kChannelOutOfRange)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
}

// Test calling SetActiveChannels, before the previous SetActiveChannels has completed.
TEST_F(RingBufferServerCompositeWarningTest, SetActiveChannelsWhilePending) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  bool received_callback_1 = false, received_callback_2 = false;

  ring_buffer_client
      ->SetActiveChannels({{
          1,
      }})
      .Then([&received_callback_1](fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback_1 = true;
      });
  ring_buffer_client
      ->SetActiveChannels({{
          0,
      }})
      .Then([&received_callback_2](fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        ASSERT_TRUE(result.is_error()) << result.error_value();
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RingBufferSetActiveChannelsError::kAlreadyPending)
            << result.error_value();
        received_callback_2 = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback_1 && received_callback_2);
  EXPECT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id), 0x1u);
  EXPECT_EQ(RingBufferServer::count(), 1u);
}

// Test Start-Start, when the second Start is called before the first Start completes.
TEST_F(RingBufferServerCompositeWarningTest, StartWhilePending) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  bool received_callback_1 = false, received_callback_2 = false;

  ring_buffer_client->Start({}).Then([&received_callback_1, &fake_driver,
                                      element_id](fidl::Result<fad::RingBuffer::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_TRUE(fake_driver->RingBufferStarted(element_id));
    received_callback_1 = true;
  });
  ring_buffer_client->Start({}).Then([&received_callback_2, &fake_driver,
                                      element_id](fidl::Result<fad::RingBuffer::Start>& result) {
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
    EXPECT_EQ(result.error_value().domain_error(), fad::RingBufferStartError::kAlreadyPending)
        << result.error_value();
    EXPECT_TRUE(fake_driver->RingBufferStarted(element_id));
    received_callback_2 = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback_1 && received_callback_2);
  EXPECT_TRUE(fake_driver->RingBufferStarted(element_id));
  EXPECT_EQ(RingBufferServer::count(), 1u);
}

// Test Start-Start, when the second Start occurs after the first has successfully completed.
TEST_F(RingBufferServerCompositeWarningTest, StartWhileStarted) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;
  auto before_start = zx::clock::get_monotonic();

  ring_buffer_client->Start({}).Then([&received_callback, before_start, &fake_driver,
                                      element_id](fidl::Result<fad::RingBuffer::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result->start_time());
    EXPECT_EQ(*result->start_time(), fake_driver->RingBufferMonoStartTime(element_id).get());
    EXPECT_GT(*result->start_time(), before_start.get());
    EXPECT_TRUE(fake_driver->RingBufferStarted(element_id));
    received_callback = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;
  ring_buffer_client->Start({}).Then(
      [&received_callback](fidl::Result<fad::RingBuffer::Start>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::RingBufferStartError::kAlreadyStarted)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(RingBufferServer::count(), 1u);
}

// Test Stop when not yet Started.
TEST_F(RingBufferServerCompositeWarningTest, StopBeforeStarted) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;

  ring_buffer_client->Stop({}).Then(
      [&received_callback](fidl::Result<fad::RingBuffer::Stop>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::RingBufferStopError::kAlreadyStopped)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
}

// Test Start-Stop-Stop, when the second Stop is called before the first one completes.
TEST_F(RingBufferServerCompositeWarningTest, StopWhilePending) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;
  auto before_start = zx::clock::get_monotonic();

  ring_buffer_client->Start({}).Then([&received_callback, before_start, &fake_driver,
                                      element_id](fidl::Result<fad::RingBuffer::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result->start_time());
    EXPECT_EQ(*result->start_time(), fake_driver->RingBufferMonoStartTime(element_id).get());
    EXPECT_GT(*result->start_time(), before_start.get());
    EXPECT_TRUE(fake_driver->RingBufferStarted(element_id));
    received_callback = true;
  });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  bool received_callback_1 = false, received_callback_2 = false;

  ring_buffer_client->Stop({}).Then([&received_callback_1, &fake_driver,
                                     element_id](fidl::Result<fad::RingBuffer::Stop>& result) {
    EXPECT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_FALSE(fake_driver->RingBufferStarted(element_id));
    received_callback_1 = true;
  });
  ring_buffer_client->Stop({}).Then(
      [&received_callback_2](fidl::Result<fad::RingBuffer::Stop>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::RingBufferStopError::kAlreadyPending)
            << result.error_value();
        received_callback_2 = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback_1 && received_callback_2);
  EXPECT_EQ(RingBufferServer::count(), 1u);
}

// Test Start-Stop-Stop, when the first Stop successfully completed before the second is called.
TEST_F(RingBufferServerCompositeWarningTest, StopAfterStopped) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;
  auto before_start = zx::clock::get_monotonic();

  ring_buffer_client->Start({}).Then([&received_callback, before_start, &fake_driver,
                                      element_id](fidl::Result<fad::RingBuffer::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result->start_time());
    EXPECT_EQ(*result->start_time(), fake_driver->RingBufferMonoStartTime(element_id).get());
    EXPECT_GT(*result->start_time(), before_start.get());
    EXPECT_TRUE(fake_driver->RingBufferStarted(element_id));
    received_callback = true;
  });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;

  ring_buffer_client->Stop({}).Then(
      [&received_callback, &fake_driver, element_id](fidl::Result<fad::RingBuffer::Stop>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        EXPECT_FALSE(fake_driver->RingBufferStarted(element_id));
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  ring_buffer_client->Stop({}).Then(
      [&received_callback](fidl::Result<fad::RingBuffer::Stop>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::RingBufferStopError::kAlreadyStopped)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
}

// Test WatchDelayInfo when already watching - should fail with kAlreadyPending.
TEST_F(RingBufferServerCompositeWarningTest, WatchDelayInfoWhilePending) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control_ = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control_->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  ASSERT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id),
            (1u << *format.channel_count()) - 1u);
  received_callback = false;

  ring_buffer_client->WatchDelayInfo().Then(
      [&received_callback](fidl::Result<fad::RingBuffer::WatchDelayInfo>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->delay_info());
        ASSERT_TRUE(result->delay_info()->internal_delay());
        EXPECT_FALSE(result->delay_info()->external_delay());
        EXPECT_EQ(*result->delay_info()->internal_delay(),
                  FakeCompositeRingBuffer::kDefaultInternalDelay->get());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  ring_buffer_client->WatchDelayInfo().Then(
      [&received_callback](fidl::Result<fad::RingBuffer::WatchDelayInfo>& result) {
        ADD_FAILURE() << "Unexpected WatchDelayInfo response received: "
                      << (result.is_ok() ? "OK" : result.error_value().FormatDescription());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
  received_callback = false;

  ring_buffer_client->WatchDelayInfo().Then(
      [&received_callback](fidl::Result<fad::RingBuffer::WatchDelayInfo>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RingBufferWatchDelayInfoError::kAlreadyPending)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
}

}  // namespace
}  // namespace media_audio
