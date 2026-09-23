// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/observer_server.h"

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <memory>
#include <optional>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/basic_types.h"
#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_codec.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

class ObserverServerTest : public AudioDeviceRegistryServerTestBase,
                           public fidl::AsyncEventHandler<fad::Observer> {
 public:
  void handle_unknown_event(fidl::UnknownEventMetadata<fad::Observer> metadata) override;

 protected:
  std::optional<TokenId> WaitForRemovedDeviceTokenId(fidl::Client<fad::Registry>& registry_client) {
    std::optional<TokenId> removed_device_id;
    registry_client->WatchDeviceRemoved().Then(
        [&removed_device_id](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          ASSERT_TRUE(result->token_id());
          removed_device_id = result->token_id();
        });

    RunLoopUntilIdle();
    return removed_device_id;
  }

  fidl::Client<fad::Observer> ConnectToObserver(fidl::Client<fad::Registry>& registry_client,
                                                TokenId token_id) {
    auto [observer_client_end, observer_server_end] =
        CreateNaturalAsyncClientOrDie<fad::Observer>();
    auto observer_client =
        fidl::Client<fad::Observer>(std::move(observer_client_end), dispatcher(), this);
    bool received_callback = false;
    registry_client
        ->CreateObserver({{
            .token_id = token_id,
            .observer_server = std::move(observer_server_end),
        }})
        .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
          received_callback = true;
          EXPECT_TRUE(result.is_ok()) << result.error_value();
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    EXPECT_TRUE(observer_client.is_valid());
    return observer_client;
  }

  std::pair<fidl::Client<fad::RingBuffer>, fidl::ServerEnd<fad::RingBuffer>>
  CreateRingBufferClient() {
    auto [ring_buffer_client_end, ring_buffer_server_end] =
        CreateNaturalAsyncClientOrDie<fad::RingBuffer>();
    auto ring_buffer_client =
        fidl::Client<fad::RingBuffer>(std::move(ring_buffer_client_end), dispatcher());
    return std::make_pair(std::move(ring_buffer_client), std::move(ring_buffer_server_end));
  }

  std::pair<fidl::Client<fad::PacketStream>, fidl::ServerEnd<fad::PacketStream>>
  CreatePacketStreamClient() {
    auto [packet_stream_client_end, packet_stream_server_end] =
        CreateNaturalAsyncClientOrDie<fad::PacketStream>();
    auto packet_stream_client =
        fidl::Client<fad::PacketStream>(std::move(packet_stream_client_end), dispatcher());
    return std::make_pair(std::move(packet_stream_client), std::move(packet_stream_server_end));
  }
};

void ObserverServerTest::handle_unknown_event(fidl::UnknownEventMetadata<fad::Observer> metadata) {
  FAIL() << "ObserverServerTest: unknown event (Observer) ordinal " << metadata.event_ordinal;
}

class ObserverServerCodecTest : public ObserverServerTest {
 protected:
  static inline const std::string kClassName = "ObserverServerCodecTest";
  std::shared_ptr<FakeCodec> CreateAndEnableDriverWithDefaults() {
    EXPECT_EQ(dispatcher(), test_loop().dispatcher());
    auto codec_endpoints = fidl::Endpoints<fuchsia_hardware_audio::Codec>::Create();
    auto fake_driver = std::make_shared<FakeCodec>(
        codec_endpoints.server.TakeChannel(), codec_endpoints.client.TakeChannel(), dispatcher());

    adr_service()->AddDevice(
        Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                       fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

    RunLoopUntilIdle();
    return fake_driver;
  }
};

class ObserverServerCompositeTest : public ObserverServerTest {
 protected:
  static inline const std::string kClassName = "ObserverServerCompositeTest";
  std::shared_ptr<FakeComposite> CreateAndEnableDriverWithDefaults(
      std::optional<TopologyId> topology_id = FakeComposite::kDefaultTopologyId) {
    auto fake_driver = CreateFakeComposite();
    if (topology_id) {
      fake_driver->InjectTopologyChange(*topology_id);
    }

    adr_service()->AddDevice(Device::Create(
        adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
        fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));
    RunLoopUntilIdle();
    return fake_driver;
  }
};

/////////////////////
// Codec tests
//
// Verify that an Observer client can drop cleanly (without generating a WARNING or ERROR).
TEST_F(ObserverServerCodecTest, CleanClientDrop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto observer = CreateTestObserverServer(*adr_service()->devices().begin());
  ASSERT_EQ(ObserverServer::count(), 1u);

  (void)observer->client().UnbindMaybeGetEndpoint();

  RunLoopUntilIdle();
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();

  // No WARNING logging should occur during test case shutdown.
}

// Verify that an Observer server can shutdown cleanly (without generating a WARNING or ERROR).
TEST_F(ObserverServerCodecTest, CleanServerShutdown) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto observer = CreateTestObserverServer(*adr_service()->devices().begin());
  ASSERT_EQ(ObserverServer::count(), 1u);

  observer->server().Shutdown(ZX_ERR_PEER_CLOSED);

  RunLoopUntilIdle();
  ASSERT_TRUE(observer_fidl_error_status().has_value());
  EXPECT_EQ(*observer_fidl_error_status(), ZX_ERR_PEER_CLOSED);

  // No WARNING logging should occur during test case shutdown.
}

// Validate creation of an Observer via the Registry/CreateObserver method. Most other test cases
// directly create an Observer server and client synthetically via CreateTestObserverServer.
TEST_F(ObserverServerCodecTest, Creation) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [observer_client_end, observer_server_end] = CreateNaturalAsyncClientOrDie<fad::Observer>();
  auto observer_client = fidl::Client<fad::Observer>(std::move(observer_client_end), dispatcher(),
                                                     observer_fidl_handler().get());
  bool received_callback = false;

  registry->client()
      ->CreateObserver({{
          .token_id = added_device_id,
          .observer_server = std::move(observer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(observer_client.is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that when an observed device is removed, the Observer is dropped.
TEST_F(ObserverServerCodecTest, ObservedDeviceRemoved) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);

  fake_driver->DropCodec();

  // RunLoopUntilIdle();
  auto removed_device_id = WaitForRemovedDeviceTokenId(registry->client());
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*added_device_id, *removed_device_id);

  RunLoopUntilIdle();
  ASSERT_TRUE(observer_fidl_error_status().has_value());
  EXPECT_EQ(*observer_fidl_error_status(), ZX_ERR_PEER_CLOSED);
}

// Verify that the Observer receives the initial plug state of the observed device.
// To ensure we correctly receive this, change the default state we we are initially kUnplugged.
TEST_F(ObserverServerCodecTest, InitialPlugState) {
  auto fake_driver = CreateFakeCodecOutput();
  auto initial_plug_time = zx::clock::get_monotonic();
  fake_driver->InjectUnpluggedAt(initial_plug_time);

  RunLoopUntilIdle();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);
  bool received_callback = false;
  zx::time reported_plug_time = zx::time::infinite_past();

  observer->client()->WatchPlugState().Then(
      [&received_callback,
       &reported_plug_time](fidl::Result<fad::Observer::WatchPlugState>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->state());
        EXPECT_EQ(*result->state(), fad::PlugState::kUnplugged);
        ASSERT_TRUE(result->plug_time());
        reported_plug_time = zx::time(*result->plug_time());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(initial_plug_time.get(), reported_plug_time.get());
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that the Observer receives changes in the plug state of the observed device.
TEST_F(ObserverServerCodecTest, PlugChange) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);
  auto time_after_device_added = zx::clock::get_monotonic();
  zx::time received_plug_time;
  bool received_callback = false;

  observer->client()->WatchPlugState().Then(
      [&received_callback,
       &received_plug_time](fidl::Result<fad::Observer::WatchPlugState>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->state());
        EXPECT_EQ(*result->state(), fad::PlugState::kPlugged);  // default state
        ASSERT_TRUE(result->plug_time());
        received_plug_time = zx::time(*result->plug_time());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_LT(received_plug_time.get(), time_after_device_added.get());
  auto time_of_plug_change = zx::clock::get_monotonic();
  received_callback = false;

  observer->client()->WatchPlugState().Then(
      [&received_callback,
       &received_plug_time](fidl::Result<fad::Observer::WatchPlugState>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->state());
        EXPECT_EQ(*result->state(), fad::PlugState::kUnplugged);  // new state
        ASSERT_TRUE(result->plug_time());
        received_plug_time = zx::time(*result->plug_time());
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
  fake_driver->InjectUnpluggedAt(time_of_plug_change);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(received_plug_time.get(), time_of_plug_change.get());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that an Observer does not drop, if the observed device's Control client is dropped.
TEST_F(ObserverServerCodecTest, ObserverDoesNotDropIfClientControlDrops) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);

  {
    auto received_callback = false;
    auto control = CreateTestControlServer(device);
    control->client()->Reset().Then(
        [&received_callback](fidl::Result<fad::Control::Reset>& result) {
          received_callback = true;
          EXPECT_TRUE(result.is_ok()) << result.error_value();
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }

  RunLoopUntilIdle();
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_TRUE(observer->client().is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// TODO(https://fxbug.dev/323270827): implement signalprocessing for Codec (topology, gain),
// including in the FakeCodec test fixture. Then add positive test cases for
// GetTopologies/GetElements/WatchTopology/WatchElementState, as are in Composite.

// Verify GetTopologies if the driver does not support signalprocessing.
TEST_F(ObserverServerCodecTest, GetTopologiesUnsupported) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  ASSERT_FALSE(device->info()->signal_processing_topologies().has_value());
  auto observer = CreateTestObserverServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ObserverServer::count(), 1u);
  auto received_callback = false;

  observer->client()->GetTopologies().Then([&received_callback](
                                               fidl::Result<fad::Observer::GetTopologies>& result) {
    received_callback = true;
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value().framework_error();
    EXPECT_EQ(result.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  // After this failing call, the binding should still be usable.
  received_callback = false;
  observer->client()->WatchPlugState().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchPlugState>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(observer->client().is_valid());
}

// Verify GetElements if the driver does not support signalprocessing.
TEST_F(ObserverServerCodecTest, GetElementsUnsupported) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  ASSERT_FALSE(device->info()->signal_processing_topologies().has_value());
  auto observer = CreateTestObserverServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ObserverServer::count(), 1u);
  auto received_callback = false;

  observer->client()->GetElements().Then([&received_callback](
                                             fidl::Result<fad::Observer::GetElements>& result) {
    received_callback = true;
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value().framework_error();
    EXPECT_EQ(result.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  // After this failing call, the binding should still be usable.
  received_callback = false;
  observer->client()->WatchPlugState().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchPlugState>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(observer->client().is_valid());
}

/////////////////////
// Composite tests
//
// Verify that an Observer client can drop cleanly (without generating a WARNING or ERROR).
TEST_F(ObserverServerCompositeTest, CleanClientDrop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto observer = CreateTestObserverServer(*adr_service()->devices().begin());
  ASSERT_EQ(ObserverServer::count(), 1u);

  (void)observer->client().UnbindMaybeGetEndpoint();

  RunLoopUntilIdle();
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();

  // No WARNING logging should occur during test case shutdown.
}

// Verify that an Observer server can shutdown cleanly (without generating a WARNING or ERROR).
TEST_F(ObserverServerCompositeTest, CleanServerShutdown) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto observer = CreateTestObserverServer(*adr_service()->devices().begin());
  ASSERT_EQ(ObserverServer::count(), 1u);

  observer->server().Shutdown(ZX_ERR_PEER_CLOSED);

  RunLoopUntilIdle();
  ASSERT_TRUE(observer_fidl_error_status().has_value());
  EXPECT_EQ(*observer_fidl_error_status(), ZX_ERR_PEER_CLOSED);

  // No WARNING logging should occur during test case shutdown.
}

// Validate creation of an Observer via the Registry/CreateObserver method. Most other test cases
// directly create an Observer server and client synthetically via CreateTestObserverServer.
TEST_F(ObserverServerCompositeTest, Creation) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [observer_client_end, observer_server_end] = CreateNaturalAsyncClientOrDie<fad::Observer>();
  auto observer_client = fidl::Client<fad::Observer>(std::move(observer_client_end), dispatcher(),
                                                     observer_fidl_handler().get());
  bool received_callback = false;

  registry->client()
      ->CreateObserver({{
          .token_id = added_device_id,
          .observer_server = std::move(observer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(observer_client.is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that when an observed device is removed, the Observer is dropped.
TEST_F(ObserverServerCompositeTest, ObservedDeviceRemoved) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);

  fake_driver->DropComposite();

  auto removed_device_id = WaitForRemovedDeviceTokenId(registry->client());
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*added_device_id, *removed_device_id);

  RunLoopUntilIdle();
  ASSERT_TRUE(observer_fidl_error_status().has_value());
  EXPECT_EQ(*observer_fidl_error_status(), ZX_ERR_PEER_CLOSED);
}

// Verify that the Observer receives the observed device's reference clock, and that it is valid.
TEST_F(ObserverServerCompositeTest, GetReferenceClock) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);
  bool received_callback = false;

  observer->client()->GetReferenceClock().Then(
      [&received_callback](fidl::Result<fad::Observer::GetReferenceClock>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->reference_clock());
        zx::clock clock = std::move(*result->reference_clock());
        EXPECT_TRUE(clock.is_valid());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that an Observer does not drop, if an observed device's driver RingBuffer is dropped.
TEST_F(ObserverServerCompositeTest, ObserverDoesNotDropIfDriverRingBufferDrops) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device);
  auto observer = CreateTestObserverServer(device);

  auto ring_buffer_id = *device->ring_buffer_ids().begin();
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      ring_buffer_id, device->ring_buffer_format_sets());
  fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
  auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
  bool received_callback = false;

  control->client()
      ->CreateRingBuffer({{
          .element_id = ring_buffer_id,
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

  fake_driver->DropRingBuffer(ring_buffer_id);

  RunLoopUntilIdle();
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_TRUE(observer->client().is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that an Observer does not drop, if an observed device's RingBuffer client is dropped.
TEST_F(ObserverServerCompositeTest, ObserverDoesNotDropIfClientRingBufferDrops) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device);
  auto observer = CreateTestObserverServer(device);

  auto ring_buffer_id = *device->ring_buffer_ids().begin();
  auto format = SafeRingBufferFormatFromElementRingBufferFormatSets(
      ring_buffer_id, device->ring_buffer_format_sets());
  fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
  {
    auto [ring_buffer_client, ring_buffer_server_end] = CreateRingBufferClient();
    bool received_callback = false;

    control->client()
        ->CreateRingBuffer({{
            .element_id = ring_buffer_id,
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
  }

  RunLoopUntilIdle();
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_TRUE(observer->client().is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that an Observer does not drop, if an observed device's driver PacketStream is dropped.
TEST_F(ObserverServerCompositeTest, ObserverDoesNotDropIfDriverPacketStreamDrops) {
  auto fake_driver =
      CreateAndEnableDriverWithDefaults(FakeComposite::kPacketStreamOutputTopologyId);
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device);
  auto observer = CreateTestObserverServer(device);

  auto packet_stream_id = FakeComposite::kSourcePsElementId;
  auto format =
      SafePacketStreamFormats(packet_stream_id, device->packet_stream_format_sets()).front();
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  fad::ControlCreatePacketStreamRequest request;
  request.element_id(packet_stream_id);
  request.options(fad::PacketStreamOptions{{.format = format}});
  request.packet_stream_server(std::move(packet_stream_server_end));

  control->client()
      ->CreatePacketStream(std::move(request))
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  fake_driver->DropPacketStream(packet_stream_id);

  RunLoopUntilIdle();
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_TRUE(observer->client().is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that an Observer does not drop, if an observed device's PacketStream client is dropped.
TEST_F(ObserverServerCompositeTest, ObserverDoesNotDropIfClientPacketStreamDrops) {
  auto fake_driver =
      CreateAndEnableDriverWithDefaults(FakeComposite::kPacketStreamOutputTopologyId);
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device);
  auto observer = CreateTestObserverServer(device);

  auto packet_stream_id = FakeComposite::kSourcePsElementId;
  auto format =
      SafePacketStreamFormats(packet_stream_id, device->packet_stream_format_sets()).front();
  {
    auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
    bool received_callback = false;

    fad::ControlCreatePacketStreamRequest request;
    request.element_id(packet_stream_id);
    request.options(fad::PacketStreamOptions{{.format = format}});
    request.packet_stream_server(std::move(packet_stream_server_end));

    control->client()
        ->CreatePacketStream(std::move(request))
        .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
          received_callback = true;
          EXPECT_TRUE(result.is_ok()) << result.error_value();
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }

  RunLoopUntilIdle();
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_TRUE(observer->client().is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that an Observer does not drop, if the observed device's Control client is dropped.
TEST_F(ObserverServerCompositeTest, ObserverDoesNotDropIfClientControlDrops) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);

  {
    auto control = CreateTestControlServer(device);
    bool received_callback = false;

    control->client()->Reset().Then(
        [&received_callback](fidl::Result<fad::Control::Reset>& result) {
          received_callback = true;
          EXPECT_TRUE(result.is_ok()) << result.error_value();
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }

  RunLoopUntilIdle();
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_TRUE(observer->client().is_valid());
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Retrieves the static list of Topologies and their properties.
// Compare results from fad::Observer/GetTopologies to the topologies returned in the Device info.
TEST_F(ObserverServerCompositeTest, GetTopologies) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto initial_topologies = device->info()->signal_processing_topologies();
  ASSERT_TRUE(initial_topologies.has_value() && !initial_topologies->empty());

  auto observer = CreateTestObserverServer(device);
  auto received_callback = false;
  std::vector<fhasp::Topology> received_topologies;

  observer->client()->GetTopologies().Then([&received_callback, &received_topologies](
                                               fidl::Result<fad::Observer::GetTopologies>& result) {
    received_callback = true;
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    received_topologies = result->topologies();
    EXPECT_FALSE(received_topologies.empty());
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(initial_topologies->size(), received_topologies.size());
  EXPECT_THAT(received_topologies, testing::ElementsAreArray(*initial_topologies));
}

// Retrieves the static list of Elements and their properties.
// Compare results from fad::Observer/GetElements to the elements returned in the Device info.
TEST_F(ObserverServerCompositeTest, GetElements) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto initial_elements = device->info()->signal_processing_elements();
  ASSERT_TRUE(initial_elements.has_value() && !initial_elements->empty());

  auto observer = CreateTestObserverServer(device);
  auto received_callback = false;
  std::vector<fhasp::Element> received_elements;

  observer->client()->GetElements().Then(
      [&received_callback, &received_elements](fidl::Result<fad::Observer::GetElements>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_elements = result->processing_elements();
        EXPECT_FALSE(received_elements.empty());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(initial_elements->size(), received_elements.size());
  EXPECT_THAT(received_elements, testing::ElementsAreArray(*initial_elements));
}

// Verify that an Observer receives topology changes when a Control changes the topology.
TEST_F(ObserverServerCompositeTest, TopologyChangeViaControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  if (device->topology_ids().size() <= 1) {
    GTEST_SKIP() << "Fake driver does not expose multiple topologies";
  }

  auto observer = CreateTestObserverServer(device);
  auto control = CreateTestControlServer(device);

  auto received_callback = false;
  std::optional<TopologyId> current_topology_id;

  observer->client()->WatchTopology().Then([&received_callback, &current_topology_id](
                                               fidl::Result<fad::Observer::WatchTopology>& result) {
    received_callback = true;
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    current_topology_id = result->topology_id();
  });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(current_topology_id.has_value());
  ASSERT_TRUE(device->topology_ids().contains(*current_topology_id));

  TopologyId topology_id_to_set = 0;
  for (auto id : device->topology_ids()) {
    if (id != *current_topology_id) {
      topology_id_to_set = id;
      break;
    }
  }

  received_callback = false;
  std::optional<TopologyId> new_topology_id;

  observer->client()->WatchTopology().Then(
      [&received_callback, &new_topology_id](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        new_topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  auto received_callback2 = false;

  control->client()
      ->SetTopology(topology_id_to_set)
      .Then([&received_callback2](fidl::Result<fad::Control::SetTopology>& result) {
        received_callback2 = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback2);
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(new_topology_id.has_value());
  EXPECT_EQ(*new_topology_id, topology_id_to_set);

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Verify that WatchTopology correctly returns the initial topology state.
TEST_F(ObserverServerCompositeTest, WatchTopologyInitial) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto observer = CreateTestObserverServer(device);
  auto received_callback = false;
  std::optional<TopologyId> topology_id;

  observer->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(topology_id.has_value());
  EXPECT_TRUE(topology_map(device).contains(*topology_id));
}

// Verify that WatchTopology pends when called a second time (if no change).
TEST_F(ObserverServerCompositeTest, WatchTopologyNoChange) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto observer = CreateTestObserverServer(device);
  auto received_callback = false;
  std::optional<TopologyId> topology_id;

  observer->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(topology_id.has_value());
  received_callback = false;

  observer->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
}

// Verify that WatchTopology works with dynamic changes, after initial query.
TEST_F(ObserverServerCompositeTest, WatchTopologyUpdate) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto observer = CreateTestObserverServer(device);
  auto received_callback = false;
  std::optional<TopologyId> topology_id;

  observer->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(topology_id.has_value());
  ASSERT_TRUE(topology_map(device).contains(*topology_id));
  std::optional<TopologyId> topology_id_to_inject;
  for (const auto& [id, _] : topology_map(device)) {
    if (id != *topology_id) {
      topology_id_to_inject = id;
      break;
    }
  }
  if (!topology_id_to_inject.has_value()) {
    GTEST_SKIP() << "Fake driver does not expose multiple topologies";
  }
  received_callback = false;
  topology_id.reset();

  observer->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  fake_driver->InjectTopologyChange(topology_id_to_inject);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(topology_id.has_value());
  EXPECT_TRUE(topology_map(device).contains(*topology_id));
  EXPECT_EQ(*topology_id, *topology_id_to_inject);
}

// Verify that WatchElementState correctly returns the initial states of all elements.
TEST_F(ObserverServerCompositeTest, WatchElementStateInitial) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto observer = CreateTestObserverServer(device);
  auto& elements_from_device = element_map(device);
  auto received_callback = false;
  std::unordered_map<ElementId, fhasp::ElementState> element_states;

  // Gather the complete set of initial element states.
  for (auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    observer->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states](fidl::Result<fad::Observer::WatchElementState>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          element_states.insert_or_assign(element_id, result->state());
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }

  // Compare them to the collection held by the Device object.
  EXPECT_EQ(element_states.size(), elements_from_device.size());
  for (const auto& [element_id, element_record] : elements_from_device) {
    ASSERT_TRUE(element_states.contains(element_id))
        << "WatchElementState response not received for element_id " << element_id;
    const auto& state_from_device = element_record.state;
    ASSERT_TRUE(state_from_device.has_value())
        << "Device element_map did not contain ElementState for element_id ";
    EXPECT_EQ(element_states.find(element_id)->second, state_from_device);
  }
}

// Verify that WatchElementState pends indefinitely, if there has been no change.
TEST_F(ObserverServerCompositeTest, WatchElementStateNoChange) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto observer = CreateTestObserverServer(device);
  auto& elements_from_device = element_map(device);
  auto received_callback = false;
  std::unordered_map<ElementId, fhasp::ElementState> element_states;

  // Gather the complete set of initial element states.
  for (auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    observer->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states](fidl::Result<fad::Observer::WatchElementState>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          element_states.insert_or_assign(element_id, result->state());
        });

    // We wait for each WatchElementState in turn.
    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    received_callback = false;
  }

  for (auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    observer->client()
        ->WatchElementState(element_id)
        .Then([&received_callback,
               element_id](fidl::Result<fad::Observer::WatchElementState>& result) {
          received_callback = true;
          FAIL() << "Unexpected WatchElementState completion for element_id " << element_id;
        });
  }

  // We request all the states from the Elements again, then wait once.
  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
}

// Verify that WatchElementState works with dynamic changes, after initial query.
TEST_F(ObserverServerCompositeTest, WatchElementStateUpdate) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto observer = CreateTestObserverServer(device);
  auto& elements_from_device = element_map(device);
  auto received_callback = false;
  std::unordered_map<ElementId, fhasp::ElementState> element_states;

  // Gather the complete set of initial element states.
  for (auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    observer->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states](fidl::Result<fad::Observer::WatchElementState>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          element_states.insert_or_assign(element_id, result->state());
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }

  // Determine which states we can change.
  std::unordered_map<ElementId, fhasp::ElementState> element_states_to_inject;
  auto plug_change_time_to_inject = zx::clock::get_monotonic();
  for (const auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    const auto& element = element_map_entry.second.element;
    const auto& state = element_map_entry.second.state;
    if (element.type() != fhasp::ElementType::kDaiInterconnect ||
        !element.type_specific().has_value() ||
        !element.type_specific()->dai_interconnect().has_value() ||
        !element.type_specific()->dai_interconnect()->plug_detect_capabilities().has_value() ||
        element.type_specific()->dai_interconnect()->plug_detect_capabilities() !=
            fhasp::PlugDetectCapabilities::kCanAsyncNotify) {
      continue;
    }
    if (!state.has_value() || !state->type_specific().has_value() ||
        !state->type_specific()->dai_interconnect().has_value() ||
        !state->type_specific()->dai_interconnect()->plug_state().has_value() ||
        !state->type_specific()->dai_interconnect()->plug_state()->plugged().has_value() ||
        !state->type_specific()->dai_interconnect()->plug_state()->plug_state_time().has_value()) {
      continue;
    }
    auto was_plugged = state->type_specific()->dai_interconnect()->plug_state()->plugged();
    auto new_state = fhasp::ElementState{{
        .type_specific = fhasp::TypeSpecificElementState::WithDaiInterconnect(
            fhasp::DaiInterconnectElementState{{
                .plug_state = fhasp::PlugState{{
                    .plugged = !was_plugged,
                    .plug_state_time = plug_change_time_to_inject.get(),
                }},
                .external_delay = ZX_MSEC(element_id),
            }}),
        .vendor_specific_data = {{
            '0',
            '1',
            '2',
            '3',
            '4',
            '5',
            '6',
            '7',
            '8',
            '9',
            'A',
            'B',
            'C',
            'D',
            'E',
            'F',
            'Z',
        }},  // 'Z' is located at byte [16].
        .started = false,
        .bypassed = false,
        .processing_delay = ZX_USEC(element_id),
    }};
    ASSERT_EQ(new_state.vendor_specific_data()->size(), 17u) << "Test configuration error";
    element_states_to_inject.insert_or_assign(element_id, new_state);
  }

  if (element_states_to_inject.empty()) {
    GTEST_SKIP()
        << "No element states can be changed, so dynamic element_state change cannot be tested";
  }

  std::unordered_map<ElementId, fhasp::ElementState> element_states_received;

  // Inject the changes.
  for (const auto& element_state_entry : element_states_to_inject) {
    auto& element_id = element_state_entry.first;
    auto& element_state = element_state_entry.second;
    fake_driver->InjectElementStateChange(element_id, element_state);
    received_callback = false;

    observer->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states_received](fidl::Result<fad::Observer::WatchElementState>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          element_states_received.insert_or_assign(element_id, result->state());
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }

  EXPECT_EQ(element_states_to_inject.size(), element_states_received.size());
  for (const auto& [element_id, state_received] : element_states_received) {
    // Compare to actual static values we know.
    ASSERT_TRUE(state_received.type_specific().has_value());
    ASSERT_TRUE(state_received.type_specific()->dai_interconnect().has_value());
    ASSERT_TRUE(state_received.type_specific()->dai_interconnect()->plug_state().has_value());
    ASSERT_TRUE(
        state_received.type_specific()->dai_interconnect()->plug_state()->plugged().has_value());
    ASSERT_TRUE(state_received.type_specific()
                    ->dai_interconnect()
                    ->plug_state()
                    ->plug_state_time()
                    .has_value());
    EXPECT_EQ(*state_received.type_specific()->dai_interconnect()->plug_state()->plug_state_time(),
              plug_change_time_to_inject.get());
    ASSERT_TRUE(state_received.type_specific()->dai_interconnect()->external_delay().has_value());
    EXPECT_EQ(*state_received.type_specific()->dai_interconnect()->external_delay(),
              ZX_MSEC(element_id));

    ASSERT_TRUE(state_received.vendor_specific_data().has_value());
    ASSERT_EQ(state_received.vendor_specific_data()->size(), 17u);
    EXPECT_EQ(state_received.vendor_specific_data()->at(16), 'Z');

    ASSERT_TRUE(state_received.started().has_value());
    EXPECT_FALSE(*state_received.started());

    ASSERT_TRUE(state_received.bypassed().has_value());
    EXPECT_FALSE(*state_received.bypassed());

    ASSERT_TRUE(state_received.processing_delay().has_value());
    EXPECT_EQ(*state_received.processing_delay(), ZX_USEC(element_id));

    // Compare to what we injected.
    ASSERT_TRUE(element_states_to_inject.contains(element_id))
        << "Unexpected WatchElementState response received for element_id " << element_id;
    const auto& state_injected = element_states_to_inject.find(element_id)->second;
    EXPECT_EQ(state_received, state_injected);

    // Compare the updates received by the client to the collection held by the Device object.
    ASSERT_TRUE(elements_from_device.contains(element_id));
    const auto& state_from_device = elements_from_device.find(element_id)->second.state;
    EXPECT_EQ(state_received, state_from_device);
  }
}

}  // namespace
}  // namespace media_audio
