// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/markers.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class ProviderServerTest : public AudioDeviceRegistryServerTestBase {};
class ProviderServerCodecTest : public ProviderServerTest {};
class ProviderServerCompositeTest : public ProviderServerTest {};

/////////////////////
// Device-less tests
//
// A client can drop a Producer connection without hangs or WARNING logging.
TEST_F(ProviderServerTest, CleanClientDrop) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);

  (void)provider->client().UnbindMaybeGetEndpoint();

  // No WARNING logging should occur.
}

// A server can shutdown a Producer connection without hangs or WARNING logging.
TEST_F(ProviderServerTest, CleanServerShutdown) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);

  provider->server().Shutdown(ZX_ERR_PEER_CLOSED);

  // No WARNING logging should occur.
}

/////////////////////
// Codec tests
//
// An added Codec lives even after the used Provider connection is dropped.
TEST_F(ProviderServerCodecTest, AddedDeviceThatOutlivesProvider) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeCodecOutput();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test codec",
          .device_type = fad::DeviceType::kCodec,
          .driver_client = fad::DriverClient::WithCodec(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  (void)provider->client().UnbindMaybeGetEndpoint();

  RunLoopUntilIdle();
  EXPECT_TRUE(provider->server().WaitForShutdown(zx::sec(1)));
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
}

// An added Codec can be dropped without affecting the used Provider.
TEST_F(ProviderServerCodecTest, ProviderCanOutliveAddedDevice) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeCodecInput();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test codec",
          .device_type = fad::DeviceType::kCodec,
          .driver_client = fad::DriverClient::WithCodec(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  fake_driver->DropCodec();

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(ProviderServer::count(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// For Codecs added by Provider, ensure that Add-then-Watch works as expected.
TEST_F(ProviderServerCodecTest, AddThenWatch) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto registry_wrapper = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto fake_driver = CreateFakeCodecNoDirection();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test codec",
          .device_type = fad::DeviceType::kCodec,
          .driver_client = fad::DriverClient::WithCodec(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  received_callback = false;

  registry_wrapper->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// For Codecs added by Provider, ensure that Watch-then-Add works as expected.
TEST_F(ProviderServerCodecTest, WatchThenAdd) {
  auto registry_wrapper = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto received_callback_0 = false, received_callback_1 = false, received_callback_2 = false;
  registry_wrapper->client()->WatchDevicesAdded().Then(
      [&received_callback_0](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        EXPECT_TRUE(result->devices()->empty());
        received_callback_0 = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback_0);

  registry_wrapper->client()->WatchDevicesAdded().Then(
      [&received_callback_1](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback_1 = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
      });

  RunLoopUntilIdle();
  ASSERT_FALSE(received_callback_1);
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeCodecOutput();

  provider->client()
      ->AddDevice({{
          .device_name = "Test codec",
          .device_type = fad::DeviceType::kCodec,
          .driver_client = fad::DriverClient::WithCodec(fake_driver->Enable()),
      }})
      .Then([&received_callback_2](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback_2 = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback_1);
  EXPECT_TRUE(received_callback_2);
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

/////////////////////
// Composite tests
//
// An added Composite lives even after the used Provider connection is dropped.
TEST_F(ProviderServerCompositeTest, AddedDeviceThatOutlivesProvider) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test composite",
          .device_type = fad::DeviceType::kComposite,
          .driver_client = fad::DriverClient::WithComposite(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  provider->client() = fidl::Client<fad::Provider>();

  RunLoopUntilIdle();
  EXPECT_TRUE(provider->server().WaitForShutdown(zx::sec(1)));
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
}

// An added Composite can be dropped without affecting the used Provider.
TEST_F(ProviderServerCompositeTest, ProviderCanOutliveAddedDevice) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test composite",
          .device_type = fad::DeviceType::kComposite,
          .driver_client = fad::DriverClient::WithComposite(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  fake_driver->DropComposite();

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(ProviderServer::count(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

// For Composites added by Provider, ensure that Add-then-Watch works as expected.
TEST_F(ProviderServerCompositeTest, AddThenWatch) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto registry_wrapper = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test composite",
          .device_type = fad::DeviceType::kComposite,
          .driver_client = fad::DriverClient::WithComposite(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  received_callback = false;

  registry_wrapper->client()->WatchDevicesAdded().Then(
      [&received_callback](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// For Composites added by Provider, ensure that Watch-then-Add works as expected.
TEST_F(ProviderServerCompositeTest, WatchThenAdd) {
  auto registry_wrapper = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto received_callback_0 = false, received_callback_1 = false, received_callback_2 = false;
  registry_wrapper->client()->WatchDevicesAdded().Then(
      [&received_callback_0](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        EXPECT_TRUE(result->devices()->empty());
        received_callback_0 = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback_0);

  registry_wrapper->client()->WatchDevicesAdded().Then(
      [&received_callback_1](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback_1 = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices());
        ASSERT_EQ(result->devices()->size(), 1u);
      });

  RunLoopUntilIdle();
  ASSERT_FALSE(received_callback_1);
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();

  provider->client()
      ->AddDevice({{
          .device_name = "Test composite",
          .device_type = fad::DeviceType::kComposite,
          .driver_client = fad::DriverClient::WithComposite(fake_driver->Enable()),
      }})
      .Then([&received_callback_2](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback_2 = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback_1);
  EXPECT_TRUE(received_callback_2);
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

}  // namespace
}  // namespace media_audio
