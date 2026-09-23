// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/provider_server.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

// These tests rely upon a single, already-created Provider.
class ProviderServerCodecWarningTest : public AudioDeviceRegistryServerTestBase {};
class ProviderServerCompositeWarningTest : public AudioDeviceRegistryServerTestBase {};
class ProviderServerDaiWarningTest : public AudioDeviceRegistryServerTestBase {};

/////////////////////
// Codec tests
//
TEST_F(ProviderServerCodecWarningTest, MissingDeviceName) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeCodecOutput();
  auto received_callback = false;

  fad::ProviderAddDeviceRequest request;
  // missing .device_name
  request.device_type(fad::DeviceType::kCodec);
  request.driver_client(fad::DriverClient::WithCodec(fake_driver->Enable()));

  provider->client()
      ->AddDevice(std::move(request))
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::ProviderAddDeviceError::kInvalidName)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCodecWarningTest, EmptyDeviceName) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeCodecInput();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "",  // empty .device_name
          .device_type = fad::DeviceType::kCodec,
          .driver_client = fad::DriverClient::WithCodec(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::ProviderAddDeviceError::kInvalidName)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCodecWarningTest, MissingDeviceType) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeCodecNoDirection();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          // missing .device_type
          .driver_client = fad::DriverClient::WithCodec(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::ProviderAddDeviceError::kInvalidType)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCodecWarningTest, MissingDriverClient) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          .device_type = fad::DeviceType::kCodec,
          // missing .driver_client
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::ProviderAddDeviceError::kInvalidDriverClient)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCodecWarningTest, InvalidDriverClient) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          .device_type = fad::DeviceType::kCodec,
          .driver_client = fad::DriverClient::WithCodec(fidl::ClientEnd<fha::Codec>()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error()) << result.error_value();
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_INVALID_ARGS)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  ASSERT_TRUE(provider_fidl_error_status().has_value());
  EXPECT_EQ(*provider_fidl_error_status(), ZX_ERR_INVALID_ARGS);
}

TEST_F(ProviderServerCodecWarningTest, WrongDriverClient) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          .device_type = fad::DeviceType::kCodec,
          // driver_client doesn't match kCodec.
          .driver_client = fad::DriverClient::WithComposite(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::ProviderAddDeviceError::kWrongClientType)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

/////////////////////
// Composite tests
//
// Remove this test, once fad::Provider/AddDevice supports the Composite driver type.
TEST_F(ProviderServerCompositeWarningTest, MissingDeviceName) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  auto received_callback = false;

  fad::ProviderAddDeviceRequest request;
  // missing .device_name
  request.device_type(fad::DeviceType::kComposite);
  request.driver_client(fad::DriverClient::WithComposite(fake_driver->Enable()));

  provider->client()
      ->AddDevice(std::move(request))
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::ProviderAddDeviceError::kInvalidName)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCompositeWarningTest, EmptyDeviceName) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "",  // empty .device_name
          .device_type = fad::DeviceType::kComposite,
          .driver_client = fad::DriverClient::WithComposite(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::ProviderAddDeviceError::kInvalidName)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCompositeWarningTest, MissingDeviceType) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          // missing .device_type
          .driver_client = fad::DriverClient::WithComposite(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::ProviderAddDeviceError::kInvalidType)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCompositeWarningTest, MissingDriverClient) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          .device_type = fad::DeviceType::kComposite,
          // missing .driver_client
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::ProviderAddDeviceError::kInvalidDriverClient)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

TEST_F(ProviderServerCompositeWarningTest, InvalidDriverClient) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          .device_type = fad::DeviceType::kComposite,
          .driver_client = fad::DriverClient::WithComposite(fidl::ClientEnd<fha::Composite>()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error()) << result.error_value();
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_INVALID_ARGS)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  ASSERT_TRUE(provider_fidl_error_status().has_value());
  EXPECT_EQ(*provider_fidl_error_status(), ZX_ERR_INVALID_ARGS);
}

TEST_F(ProviderServerCompositeWarningTest, WrongDriverClient) {
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);
  auto fake_driver = CreateFakeCodecOutput();
  auto received_callback = false;

  provider->client()
      ->AddDevice({{
          .device_name = "Test device name",
          .device_type = fad::DeviceType::kComposite,
          // driver_client doesn't match kComposite.
          .driver_client = fad::DriverClient::WithCodec(fake_driver->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::ProviderAddDeviceError::kWrongClientType)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  EXPECT_FALSE(provider_fidl_error_status().has_value()) << *provider_fidl_error_status();
}

}  // namespace
}  // namespace media_audio
