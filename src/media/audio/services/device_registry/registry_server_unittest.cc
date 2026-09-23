// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/registry_server.h"

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <lib/fidl/cpp/client.h>

#include <optional>

#include <gtest/gtest.h>

#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class RegistryServerTest : public AudioDeviceRegistryServerTestBase {};
class RegistryServerCodecTest : public RegistryServerTest {
 protected:
  static inline const std::string kClassName = "RegistryServerCodecTest";
};
class RegistryServerCompositeTest : public RegistryServerTest {
 protected:
  static inline const std::string kClassName = "RegistryServerCompositeTest";
};

/////////////////////
// An important aspect validated by the RegistryServer tests is the state machine that determines
// when clients are notified of device arrivals. Here are event sequences we test:
//    R W.C|                WatchAddedPendsUntilDiscovery
//    R C W|                WatchWithNoDevicesCompletes
//    D R W.C|W.D           WatchBeforeDiscoveryComplete
//    D R C W|D W           DiscoveryCompleteBeforeWatch
//    R D W.D.C|            WatchBeforeDiscoveryWithDevicesDuring
//    R D C D W|            DiscoveryBeforeWatchWithDevicesDuring
// Where D is device arrival, R is Registry creation,       C is device discovery complete,
// . is expected NO callback, W is first WatchDevicesAdded, | is expected first callback.

/////////////////////
// Device-less tests
//
// A client can drop their Registry connection without hang, and without WARNING being logged.
TEST_F(RegistryServerTest, CleanClientDrop) {
  auto registry = CreateTestRegistryServer();
  EXPECT_EQ(RegistryServer::count(), 1u);

  (void)registry->client().UnbindMaybeGetEndpoint();
}

// Server can cleanly shutdown without hang, and without WARNING being logged.
TEST_F(RegistryServerTest, CleanServerShutdown) {
  auto registry = CreateTestRegistryServer();
  EXPECT_EQ(RegistryServer::count(), 1u);

  registry->server().Shutdown(ZX_ERR_PEER_CLOSED);
}

// On a system without devices, the initial call to WatchDevicesAdded should pend until the server
// is notified that initial device discovery is complete.  R W.C|
TEST_F(RegistryServerTest, WatchAddedPendsUntilDiscovery) {
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);
  bool received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices().has_value());
        EXPECT_TRUE(result->devices()->empty());
        received_callback = true;
      });

  // Validate that the request has not yet completed.
  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  // Inform registry server of initial device discovery complete.
  registry->server().InitialDeviceDiscoveryIsComplete();

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(RegistryServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// On a system without devices, the initial call to WatchDevicesAdded should complete.  R C W|
TEST_F(RegistryServerTest, WatchWithNoDevicesCompletes) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  bool received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices().has_value());
        EXPECT_TRUE(result->devices()->empty());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(RegistryServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

/////////////////////
// Codec tests
//
// Device already exists before the Registry connection is created.
// Client calls WatchDevicesAdded and is notified.  D R W.C|W.D
TEST_F(RegistryServerCodecTest, WatchBeforeDiscoveryComplete) {
  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kCodec);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  registry->server().InitialDeviceDiscoveryIsComplete();

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kCodec);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  auto fake_driver2 = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name 2", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Client calls WatchDevicesAdded, then add device and client is notified.  D R C W|D W
TEST_F(RegistryServerCodecTest, DiscoveryCompleteBeforeWatch) {
  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);
  registry->server().InitialDeviceDiscoveryIsComplete();

  RunLoopUntilIdle();
  auto received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kCodec);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  auto fake_driver2 = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name 2", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kCodec);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Add device, client calls WatchDevicesAdded, is notified upon discovery complete.  R D W.D.C|
TEST_F(RegistryServerCodecTest, WatchBeforeDiscoveryWithDevicesDuring) {
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  auto received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 2u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kCodec);
        EXPECT_FALSE(*result->devices()->at(0).is_input());
        EXPECT_EQ(result->devices()->at(1).device_type(), fad::DeviceType::kCodec);
        EXPECT_TRUE(*result->devices()->at(1).is_input());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  auto fake_driver2 = CreateFakeCodecInput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name 2", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(received_callback);

  registry->server().InitialDeviceDiscoveryIsComplete();

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Add device, discovery completes, client is notified upon WatchDevicesAdded.  R D C D W|
TEST_F(RegistryServerCodecTest, DiscoveryBeforeWatchWithDevicesDuring) {
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  registry->server().InitialDeviceDiscoveryIsComplete();

  auto fake_driver2 = CreateFakeCodecInput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name 2", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  auto received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 2u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kCodec);
        EXPECT_FALSE(*result->devices()->at(0).is_input());
        EXPECT_EQ(result->devices()->at(1).device_type(), fad::DeviceType::kCodec);
        EXPECT_TRUE(*result->devices()->at(1).is_input());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Client calls WatchDeviceRemoved, then remove device, then client is notified.
TEST_F(RegistryServerCodecTest, WatchRemovesThenDeviceRemove) {
  auto fake_driver = CreateFakeCodecInput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_device_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_device_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kCodec &&
                    result->devices()->at(0).token_id().has_value());
        added_device_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(added_device_id.has_value());
  received_callback = false;
  std::optional<TokenId> removed_device_id;

  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback,
       &removed_device_id](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->token_id());
        removed_device_id = result->token_id();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
  EXPECT_FALSE(removed_device_id.has_value());
  fake_driver->DropCodec();

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*added_device_id, *removed_device_id);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Remove device, see ADR remove it, then client calls WatchDeviceRemoved and is notified.
TEST_F(RegistryServerCodecTest, DeviceRemoveThenWatchRemoves) {
  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_device_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_device_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kCodec &&
                    result->devices()->at(0).token_id().has_value());
        added_device_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(added_device_id.has_value());
  fake_driver->DropCodec();

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 0u);
  received_callback = false;
  std::optional<TokenId> removed_device_id;

  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback,
       &removed_device_id](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->token_id());
        removed_device_id = result->token_id();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*added_device_id, *removed_device_id);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Create a Registry connection then add and remove device (see ADR count go up and down).
// Then when client calls WatchDevicesAdded and WatchDeviceRemoved, no notifications should occur.
TEST_F(RegistryServerCodecTest, DeviceAddRemoveThenWatches) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  bool received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        EXPECT_TRUE(result->devices()->empty());
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);

  auto fake_driver = CreateFakeCodecInput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  fake_driver->DropCodec();

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 0u);
  bool received_add_response = false, received_remove_response = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_add_response](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_add_response = true;
        FAIL() << "Unexpected WatchDevicesAdded response";
      });
  registry->client()->WatchDeviceRemoved().Then(
      [&received_remove_response](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_remove_response = true;
        FAIL() << "Unexpected WatchDeviceRemoved response";
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_add_response);
  EXPECT_FALSE(received_remove_response);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Remove device, add device, WatchDevicesAdded/WatchDeviceRemoved (id's differ: should notify).
TEST_F(RegistryServerCodecTest, DeviceRemoveAddThenWatches) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto fake_driver = CreateFakeCodecNoDirection();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  bool received_callback = false;
  std::optional<TokenId> first_added_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &first_added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kCodec &&
                    result->devices()->at(0).token_id().has_value());
        first_added_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(first_added_id.has_value());
  fake_driver->DropCodec();

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 0u);
  fake_driver = CreateFakeCodecNoDirection();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  received_callback = false;
  std::optional<TokenId> removed_device_id;

  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback,
       &removed_device_id](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->token_id());
        removed_device_id = result->token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*first_added_id, *removed_device_id);
  received_callback = false;
  std::optional<TokenId> second_added_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &second_added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kCodec &&
                    result->devices()->at(0).token_id().has_value());
        second_added_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(second_added_id.has_value());
  EXPECT_NE(*first_added_id, *second_added_id);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Client can open an Observer connection on an added Codec device.
TEST_F(RegistryServerCodecTest, CreateObserver) {
  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = true;
  std::optional<TokenId> added_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kCodec &&
                    result->devices()->at(0).token_id().has_value());
        added_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(added_id.has_value());
  auto [observer_client_end, observer_server_end] = CreateNaturalAsyncClientOrDie<fad::Observer>();
  auto observer_client = fidl::Client<fad::Observer>(std::move(observer_client_end), dispatcher(),
                                                     observer_fidl_handler().get());
  received_callback = false;

  registry->client()
      ->CreateObserver({{
          .token_id = added_id,
          .observer_server = std::move(observer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

/////////////////////
// Composite tests
//
// Device already exists before the Registry connection is created.
// Client calls WatchDevicesAdded and is notified.  D R W.C|W.D
TEST_F(RegistryServerCompositeTest, WatchBeforeDiscoveryComplete) {
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kComposite);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  registry->server().InitialDeviceDiscoveryIsComplete();

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kComposite);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  auto fake_driver2 = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name 2", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Client calls WatchDevicesAdded, then add device and client is notified.  D R C W|D W
TEST_F(RegistryServerCompositeTest, DiscoveryCompleteBeforeWatch) {
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);
  registry->server().InitialDeviceDiscoveryIsComplete();

  RunLoopUntilIdle();
  auto received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kComposite);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  auto fake_driver2 = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name 2", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  received_callback = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kComposite);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Add device, client calls WatchDevicesAdded, is notified upon discovery complete.  R D W.D.C|
TEST_F(RegistryServerCompositeTest, WatchBeforeDiscoveryWithDevicesDuring) {
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  auto received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 2u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kComposite);
        EXPECT_EQ(result->devices()->at(1).device_type(), fad::DeviceType::kComposite);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  auto fake_driver2 = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name 2", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(received_callback);

  registry->server().InitialDeviceDiscoveryIsComplete();

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Add device, discovery completes, client is notified upon WatchDevicesAdded.  R D C D W|
TEST_F(RegistryServerCompositeTest, DiscoveryBeforeWatchWithDevicesDuring) {
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  registry->server().InitialDeviceDiscoveryIsComplete();

  auto fake_driver2 = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name 2", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver2->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 2u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  auto received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 2u);
        EXPECT_EQ(result->devices()->at(0).device_type(), fad::DeviceType::kComposite);
        EXPECT_EQ(result->devices()->at(1).device_type(), fad::DeviceType::kComposite);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Client calls WatchDeviceRemoved, then remove device, then client is notified.
TEST_F(RegistryServerCompositeTest, WatchRemovesThenDeviceRemove) {
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_device_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_device_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kComposite &&
                    result->devices()->at(0).token_id().has_value());
        added_device_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(added_device_id.has_value());
  received_callback = false;
  std::optional<TokenId> removed_device_id;

  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback,
       &removed_device_id](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->token_id());
        removed_device_id = result->token_id();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
  EXPECT_FALSE(removed_device_id.has_value());
  fake_driver->DropComposite();

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*added_device_id, *removed_device_id);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Remove device, see ADR remove it, then client calls WatchDeviceRemoved and is notified.
TEST_F(RegistryServerCompositeTest, DeviceRemoveThenWatchRemoves) {
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_device_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_device_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kComposite &&
                    result->devices()->at(0).token_id().has_value());
        added_device_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(added_device_id.has_value());
  fake_driver->DropComposite();

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 0u);
  received_callback = false;
  std::optional<TokenId> removed_device_id;

  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback,
       &removed_device_id](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->token_id());
        removed_device_id = result->token_id();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*added_device_id, *removed_device_id);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Create a Registry connection then add and remove device (see ADR count go up and down).
// Then when client calls WatchDevicesAdded and WatchDeviceRemoved, no notifications should occur.
TEST_F(RegistryServerCompositeTest, DeviceAddRemoveThenWatches) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  bool received_callback = false;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        EXPECT_TRUE(result->devices()->empty());
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);

  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  fake_driver->DropComposite();

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 0u);
  bool received_add_response = false, received_remove_response = false;

  registry->client()->WatchDevicesAdded().Then(
      [&received_add_response](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_add_response = true;
        FAIL() << "Unexpected WatchDevicesAdded response";
      });
  registry->client()->WatchDeviceRemoved().Then(
      [&received_remove_response](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_remove_response = true;
        FAIL() << "Unexpected WatchDeviceRemoved response";
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_add_response);
  EXPECT_FALSE(received_remove_response);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Remove device, add device, WatchDevicesAdded/WatchDeviceRemoved (id's differ: should notify).
TEST_F(RegistryServerCompositeTest, DeviceRemoveAddThenWatches) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  bool received_callback = false;
  std::optional<TokenId> first_added_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &first_added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kComposite &&
                    result->devices()->at(0).token_id().has_value());
        first_added_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(first_added_id.has_value());
  fake_driver->DropComposite();

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 0u);
  fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  received_callback = false;
  std::optional<TokenId> removed_device_id;

  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback,
       &removed_device_id](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->token_id());
        removed_device_id = result->token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(removed_device_id.has_value());
  EXPECT_EQ(*first_added_id, *removed_device_id);
  received_callback = false;
  std::optional<TokenId> second_added_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &second_added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kComposite &&
                    result->devices()->at(0).token_id().has_value());
        second_added_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(second_added_id.has_value());
  EXPECT_NE(*first_added_id, *second_added_id);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// Client can open an Observer connection on an added Composite device.
TEST_F(RegistryServerCompositeTest, CreateObserver) {
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = true;
  std::optional<TokenId> added_id;

  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1);
        ASSERT_TRUE(result->devices()->at(0).device_type() == fad::DeviceType::kComposite &&
                    result->devices()->at(0).token_id().has_value());
        added_id = result->devices()->at(0).token_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(added_id.has_value());
  auto [observer_client_end, observer_server_end] = CreateNaturalAsyncClientOrDie<fad::Observer>();
  auto observer_client = fidl::Client<fad::Observer>(std::move(observer_client_end), dispatcher(),
                                                     observer_fidl_handler().get());
  received_callback = false;

  registry->client()
      ->CreateObserver({{
          .token_id = added_id,
          .observer_server = std::move(observer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

}  // namespace
}  // namespace media_audio
