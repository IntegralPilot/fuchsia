// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/ring_buffer_server.h"

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_ring_buffer.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

class RingBufferServerTest : public AudioDeviceRegistryServerTestBase,
                             public fidl::AsyncEventHandler<fad::RingBuffer> {
 public:
  void handle_unknown_event(fidl::UnknownEventMetadata<fad::RingBuffer> metadata) override;

 protected:
  static inline const fad::RingBufferOptions kDefaultRingBufferOptions{{
      .format = fuchsia_audio::Format{{
          .sample_type = fuchsia_audio::SampleType::kInt16,
          .channel_count = 2,
          .frames_per_second = 48000,
      }},
      .ring_buffer_min_bytes = 2000,
  }};

  std::pair<fidl::Client<fad::RingBuffer>, fidl::ServerEnd<fad::RingBuffer>>
  CreateRingBufferClient();

  std::pair<std::unique_ptr<TestServerAndNaturalAsyncClient<ControlServer>>,
            fidl::Client<fad::RingBuffer>>
  SetupForCleanShutdownTesting(ElementId element_id,
                               const fad::RingBufferOptions& options = kDefaultRingBufferOptions);
};

void RingBufferServerTest::handle_unknown_event(
    fidl::UnknownEventMetadata<fad::RingBuffer> metadata) {
  FAIL() << "RingBufferServerWarningTest: unknown event (RingBuffer) ordinal "
         << metadata.event_ordinal;
}

std::pair<fidl::Client<fad::RingBuffer>, fidl::ServerEnd<fad::RingBuffer>>
RingBufferServerTest::CreateRingBufferClient() {
  auto [ring_buffer_client_end, ring_buffer_server_end] =
      CreateNaturalAsyncClientOrDie<fad::RingBuffer>();
  auto ring_buffer_client =
      fidl::Client<fad::RingBuffer>(std::move(ring_buffer_client_end), dispatcher(), this);
  return std::make_pair(std::move(ring_buffer_client), std::move(ring_buffer_server_end));
}

std::pair<std::unique_ptr<TestServerAndNaturalAsyncClient<ControlServer>>,
          fidl::Client<fad::RingBuffer>>
RingBufferServerTest::SetupForCleanShutdownTesting(ElementId element_id,
                                                   const fad::RingBufferOptions& options) {
  auto registry = CreateTestRegistryServer();
  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  FX_CHECK(token_id.has_value());
  auto control_creator = CreateTestControlCreatorServer();
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_TRUE(presence == AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = options,
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_creator_fidl_error_status().has_value());
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  return std::make_pair(std::move(control), std::move(ring_buffer_client));
}

class RingBufferServerCompositeTest : public RingBufferServerTest {
 protected:
  static inline const std::string kClassName = "RingBufferServerCompositeTest";
  std::shared_ptr<Device> EnableDriverAndAddDevice(
      const std::shared_ptr<FakeComposite>& fake_driver) {
    auto device = Device::Create(
        adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
        fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName);
    adr_service()->AddDevice(device);

    RunLoopUntilIdle();
    return device;
  }
};

TEST_F(RingBufferServerCompositeTest, CleanClientDrop) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());

  auto [control, ring_buffer_client] =
      SetupForCleanShutdownTesting(element_id, fad::RingBufferOptions{{
                                                   .format = format,
                                                   .ring_buffer_min_bytes = 2000,
                                               }});

  (void)ring_buffer_client.UnbindMaybeGetEndpoint();

  RunLoopUntilIdle();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  // If RingBuffer client doesn't drop cleanly, RingBufferServer emits a WARNING, which will fail.
}

TEST_F(RingBufferServerCompositeTest, DriverRingBufferDropCausesCleanRingBufferServerShutdown) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());

  auto [control, ring_buffer_client] =
      SetupForCleanShutdownTesting(element_id, fad::RingBufferOptions{{
                                                   .format = format,
                                                   .ring_buffer_min_bytes = 2000,
                                               }});

  fake_driver->DropRingBuffer(element_id);

  RunLoopUntilIdle();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  // If RingBufferServer doesn't shutdown cleanly, it emits a WARNING, which will cause a failure.
}

TEST_F(RingBufferServerCompositeTest, DriverCompositeDropCausesCleanRingBufferServerShutdown) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());

  auto [control, ring_buffer_client] =
      SetupForCleanShutdownTesting(element_id, fad::RingBufferOptions{{
                                                   .format = format,
                                                   .ring_buffer_min_bytes = 2000,
                                               }});

  fake_driver->DropComposite();

  RunLoopUntilIdle();
  ASSERT_TRUE(control_fidl_error_status().has_value());
  EXPECT_EQ(control_fidl_error_status(), ZX_ERR_PEER_CLOSED);
  // If RingBufferServer doesn't shutdown cleanly, it emits a WARNING, which will cause a failure.
}

// Verify that Control/CreateRingBuffer succeeds and returns the expected parameters.
TEST_F(RingBufferServerCompositeTest, CreateRingBufferReturnParameters) {
  auto fake_driver = CreateFakeComposite();
  auto element_id =
      FakeComposite::kMaxRingBufferElementId;  // Element is an Outgoing fad::RingBuffer.
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto requested_ring_buffer_bytes = 2000u;
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = requested_ring_buffer_bytes,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback, format,
             requested_ring_buffer_bytes](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();

        ASSERT_TRUE(result->properties());
        ASSERT_TRUE(result->properties()->valid_bits_per_sample());
        EXPECT_EQ(*result->properties()->valid_bits_per_sample(),
                  FakeComposite::kDefaultRbValidBitsPerSample2);
        ASSERT_TRUE(result->properties()->turn_on_delay());
        EXPECT_EQ(*result->properties()->turn_on_delay(), 0);

        ASSERT_TRUE(result->ring_buffer());
        ASSERT_TRUE(result->ring_buffer()->buffer());
        ASSERT_TRUE(result->ring_buffer()->producer_bytes());
        ASSERT_TRUE(result->ring_buffer()->consumer_bytes());
        ASSERT_TRUE(result->ring_buffer()->reference_clock());
        EXPECT_TRUE(result->ring_buffer()->buffer()->vmo().is_valid());
        EXPECT_GE(
            result->ring_buffer()->buffer()->size(),
            requested_ring_buffer_bytes + FakeCompositeRingBuffer::kDefaultDriverTransferBytes);
        EXPECT_EQ(*result->ring_buffer()->format(), format);
        EXPECT_GE(result->ring_buffer()->producer_bytes(), requested_ring_buffer_bytes);
        EXPECT_EQ(result->ring_buffer()->consumer_bytes(),
                  FakeCompositeRingBuffer::kDefaultDriverTransferBytes);
        EXPECT_TRUE(result->ring_buffer()->reference_clock()->is_valid());
        EXPECT_EQ(
            result->ring_buffer()->reference_clock_domain().value_or(fha::kClockDomainMonotonic),
            fha::kClockDomainMonotonic);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that RingBuffer/SetActiveChannels succeeds and returns an expected set_time.
TEST_F(RingBufferServerCompositeTest, DriverSupportsSetActiveChannels) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  fake_driver->EnableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  received_callback = false;
  auto before_set_active_channels = zx::clock::get_monotonic();

  ring_buffer_client
      ->SetActiveChannels({{
          .channel_bitmask = 0x0,
      }})
      .Then([&received_callback,
             before_set_active_channels](fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->set_time());
        EXPECT_GT(*result->set_time(), before_set_active_channels.get());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id), 0x0u);
  EXPECT_GT(fake_driver->RingBufferSetActiveChannelsCompletedAt(element_id),
            before_set_active_channels);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

TEST_F(RingBufferServerCompositeTest, DriverDoesNotSupportSetActiveChannels) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  // Set this fake hardware to NOT support powering-down channels.
  fake_driver->DisableActiveChannelsSupport(element_id);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto channel_count = *format.channel_count();
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(added_device);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
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
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(ring_buffer_client.is_valid());
  received_callback = false;

  ring_buffer_client
      ->SetActiveChannels({{
          0,
      }})
      .Then([&received_callback](fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        ASSERT_TRUE(result.is_error()) << result.error_value();
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RingBufferSetActiveChannelsError::kMethodNotSupported)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(fake_driver->RingBufferActiveChannelsBitmask(element_id), (1u << channel_count) - 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that RingBuffer/Start and /Stop function as expected, including start_time.
TEST_F(RingBufferServerCompositeTest, StartAndStop) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->DisableActiveChannelsSupport(element_id);
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
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
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(fake_driver->RingBufferStarted(element_id));
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
  // Now that we are started, we want the RingBuffer server to take definitive action on some OTHER
  // request before we stop. We will use SetActiveChannels, which we expressly set as 'unsupported'
  // above, in the fake driver test fixture.
  received_callback = false;

  ring_buffer_client
      ->SetActiveChannels({{
          .channel_bitmask = 0x0,
      }})
      .Then([&received_callback](fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RingBufferSetActiveChannelsError::kMethodNotSupported)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  ring_buffer_client->Stop({}).Then(
      [&received_callback, &fake_driver, element_id](fidl::Result<fad::RingBuffer::Stop>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        EXPECT_FALSE(fake_driver->RingBufferStarted(element_id));
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that RingBuffer/WatchDelayInfo notifies of the delay received during initialization.
// While we are here, validate a non-default value for turn_on_delay.
TEST_F(RingBufferServerCompositeTest, WatchDelayInfoInitialValues) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->PresetTurnOnDelay(element_id, zx::msec(42));
  fake_driver->PresetInternalExternalDelays(element_id, zx::nsec(987'654'321),
                                            zx::nsec(123'456'789));
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
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
        ASSERT_TRUE(result->properties());
        ASSERT_TRUE(result->properties()->turn_on_delay());
        EXPECT_EQ(*result->properties()->turn_on_delay(), 42'000'000);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(fake_driver->RingBufferStarted(element_id));
  received_callback = false;

  ring_buffer_client->WatchDelayInfo().Then(
      [&received_callback](fidl::Result<fad::RingBuffer::WatchDelayInfo>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->delay_info());
        ASSERT_TRUE(result->delay_info()->internal_delay());
        ASSERT_TRUE(result->delay_info()->external_delay());
        EXPECT_EQ(*result->delay_info()->internal_delay(), 987'654'321u);
        EXPECT_EQ(*result->delay_info()->external_delay(), 123'456'789u);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that RingBuffer/WatchDelayInfo notifies of delay changes after initialization.
TEST_F(RingBufferServerCompositeTest, WatchDelayInfoDynamicUpdates) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
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
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(fake_driver->RingBufferStarted(element_id));
  received_callback = false;

  ring_buffer_client->WatchDelayInfo().Then(
      [&received_callback](fidl::Result<fad::RingBuffer::WatchDelayInfo>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->delay_info());
        ASSERT_TRUE(result->delay_info()->internal_delay());
        EXPECT_FALSE(result->delay_info()->external_delay());
        EXPECT_EQ(result->delay_info()->internal_delay().value(),
                  FakeCompositeRingBuffer::kDefaultInternalDelay->get());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  ring_buffer_client->WatchDelayInfo().Then(
      [&received_callback](fidl::Result<fad::RingBuffer::WatchDelayInfo>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->delay_info());
        ASSERT_TRUE(result->delay_info()->internal_delay());
        ASSERT_TRUE(result->delay_info()->external_delay());
        EXPECT_EQ(*result->delay_info()->internal_delay(), 987'654'321u);
        EXPECT_EQ(*result->delay_info()->external_delay(), 123'456'789u);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
  fake_driver->RingBufferInjectDelayUpdate(element_id, zx::nsec(987'654'321),
                                           zx::nsec(123'456'789));

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that the RingBufferServer is destructed if the client drops the Control.
TEST_F(RingBufferServerCompositeTest, ControlClientDropCausesRingBufferDrop) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();

  (void)control->client().UnbindMaybeGetEndpoint();

  RunLoopUntilIdle();
  EXPECT_TRUE(control->server().WaitForShutdown());
  EXPECT_EQ(RingBufferServer::count(), 0u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Verify that the RingBufferServer is destructed if the ControlServer shuts down.
TEST_F(RingBufferServerCompositeTest, ControlServerShutdownCausesRingBufferDrop) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
      ->CreateRingBuffer({{
          .element_id = element_id,
          .options = fad::RingBufferOptions{{
              .format = format,
              .ring_buffer_min_bytes = 2000,
          }},
          .ring_buffer_server = std::move(ring_buffer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_EQ(RingBufferServer::count(), 1u);

  control->server().Shutdown(ZX_ERR_PEER_CLOSED);

  RunLoopUntilIdle();
  EXPECT_TRUE(control->server().WaitForShutdown());
  EXPECT_EQ(RingBufferServer::count(), 0u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Verify that RingBuffer works as expected, after RingBuffer being created/destroyed/recreated.
TEST_F(RingBufferServerCompositeTest, SecondRingBufferAfterDrop) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver->DisableActiveChannelsSupport(element_id);
  fake_driver->ReserveRingBufferSize(element_id, 8192);
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      element_id, device->ring_buffer_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);
  {
    auto control = CreateTestControlServer(device_to_control);
    auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
    bool received_callback = false;
    ASSERT_TRUE(control->client().is_valid());

    control->client()
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
    ASSERT_FALSE(fake_driver->RingBufferStarted(element_id));
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
    EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();

    // Now that we are started, we want to suddenly drop the RingBuffer connection.
    (void)ring_buffer_client.UnbindMaybeGetEndpoint();
    (void)control->client().UnbindMaybeGetEndpoint();

    RunLoopUntilIdle();
    control->server().WaitForShutdown();
  }

  {
    auto control = CreateTestControlServer(device_to_control);
    auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
    auto received_callback = false;
    ASSERT_TRUE(control->client().is_valid());

    control->client()
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
    EXPECT_TRUE(received_callback);
    EXPECT_FALSE(fake_driver->RingBufferStarted(element_id));
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
    EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  }
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

}  // namespace
}  // namespace media_audio
