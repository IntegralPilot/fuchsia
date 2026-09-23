// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/registry_server.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class RegistryServerWarningTest : public AudioDeviceRegistryServerTestBase {};
class RegistryServerCodecWarningTest : public RegistryServerWarningTest {
 protected:
  static inline const std::string kClassName = "RegistryServerCodecWarningTest";
};
class RegistryServerCompositeWarningTest : public RegistryServerWarningTest {
 protected:
  static inline const std::string kClassName = "RegistryServerCompositeWarningTest";
};

/////////////////////
// Device-less tests
//
// A subsequent call to WatchDevicesAdded before the previous one completes should fail.
TEST_F(RegistryServerWarningTest, WatchDevicesTwiceBeforeDeviceDiscoveryCompletes) {
  auto registry = CreateTestRegistryServerNoDeviceDiscovery();
  ASSERT_EQ(RegistryServer::count(), 1u);
  bool received_callback_0 = false, received_callback_1 = false;

  // Before initial device discovery, first `WatchDevicesAdded` call should pend indefinitely
  // (and should continue to, even after a subsequent `WatchDevicesAdded` call fails).
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback_0](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback_0 = true;
        ADD_FAILURE() << "Unexpected completion for WatchDevicesAdded; should pend indefinitely";
      });

  RunLoopUntilIdle();
  ASSERT_FALSE(received_callback_0);

  // The next `WatchDevicesAdded` call should fail immediately with domain error
  // ALREADY_PENDING, since the previous call has not yet completed.
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback_1](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback_1 = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RegistryWatchDevicesAddedError::kAlreadyPending)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback_0);
  EXPECT_TRUE(received_callback_1);
  EXPECT_EQ(RegistryServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// A subsequent call to WatchDevicesAdded before the previous one completes should fail.
TEST_F(RegistryServerWarningTest, WatchDevicesAddedWhilePending) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  bool received_callback_0 = false, received_callback_1 = false, received_callback_2 = false;

  // The first `WatchDevicesAdded` call should complete immediately, even without devices.
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback_0](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->devices().has_value());
        EXPECT_TRUE(result->devices()->empty())
            << "Unexpectedly returned " << result->devices()->size() << " devices";
        received_callback_0 = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback_0);

  // Following the initial completion, the next `WatchDevicesAdded` should pend indefinitely
  // (and should continue to, even after a subsequent `WatchDevicesAdded` call fails).
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback_1](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback_1 = true;
        ADD_FAILURE() << "Unexpected completion for WatchDevicesAdded; should pend indefinitely";
      });

  RunLoopUntilIdle();
  ASSERT_FALSE(received_callback_1);

  // The next `WatchDevicesAdded` call should fail immediately with domain error
  // ALREADY_PENDING, since the previous call has not yet completed.
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback_2](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback_2 = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RegistryWatchDevicesAddedError::kAlreadyPending)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback_1);
  EXPECT_TRUE(received_callback_2);
  EXPECT_EQ(RegistryServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// A subsequent call to WatchDeviceRemoved before the previous one completes should fail.
TEST_F(RegistryServerWarningTest, WatchDeviceRemovedWhilePending) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  bool received_callback_1 = false, received_callback_2 = false;

  // The first `WatchDeviceRemoved` call should pend indefinitely (even after the second one fails).
  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback_1](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback_1 = true;
        ADD_FAILURE() << "Unexpected completion for initial WatchDeviceRemoved call";
      });

  RunLoopUntilIdle();
  ASSERT_FALSE(received_callback_1);

  // The second `WatchDeviceRemoved` call should fail immediately with domain error
  // ALREADY_PENDING, since the first call has not yet completed.
  registry->client()->WatchDeviceRemoved().Then(
      [&received_callback_2](fidl::Result<fad::Registry::WatchDeviceRemoved>& result) mutable {
        received_callback_2 = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RegistryWatchDeviceRemovedError::kAlreadyPending)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback_1);
  EXPECT_TRUE(received_callback_2);
  EXPECT_EQ(RegistryServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// If the required 'id' field is not set, CreateObserver should fail with kInvalidTokenId.
TEST_F(RegistryServerWarningTest, CreateObserverMissingToken) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto [observer_client_end, observer_server_end] = CreateNaturalAsyncClientOrDie<fad::Observer>();
  auto observer_client = fidl::Client<fad::Observer>(std::move(observer_client_end), dispatcher(),
                                                     observer_fidl_handler().get());
  bool received_callback = false;

  registry->client()
      ->CreateObserver({{
          .observer_server = std::move(observer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RegistryCreateObserverError::kInvalidTokenId)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// If 'id' doesn't identify an initialized device, CreateObserver should fail with kDeviceNotFound.
TEST_F(RegistryServerWarningTest, CreateObserverBadToken) {
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto [observer_client_end, observer_server_end] = CreateNaturalAsyncClientOrDie<fad::Observer>();
  auto observer_client = fidl::Client<fad::Observer>(std::move(observer_client_end), dispatcher(),
                                                     observer_fidl_handler().get());
  bool received_callback = false;

  registry->client()
      ->CreateObserver({{
          .token_id = 0,  // no device is present yet.
          .observer_server = std::move(observer_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RegistryCreateObserverError::kDeviceNotFound)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

/////////////////////
// Codec tests
//
// If required 'observer_server' is not set, CreateObserver should fail with kInvalidObserver.
TEST_F(RegistryServerCodecWarningTest, CreateObserverMissingObserver) {
  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_id;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1u);
        ASSERT_TRUE(result->devices()->at(0).token_id());
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
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RegistryCreateObserverError::kInvalidObserver)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// If 'observer_server' is not set to a valid handle, we should fail with ZX_ERR_INVALID_ARGS.
TEST_F(RegistryServerCodecWarningTest, CreateObserverBadObserver) {
  auto fake_driver = CreateFakeCodecInput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_id;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1u);
        ASSERT_TRUE(result->devices()->at(0).token_id());
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
          .observer_server = fidl::ServerEnd<fad::Observer>(),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error()) << result.error_value();
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_INVALID_ARGS)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(registry_fidl_error_status().has_value());
  EXPECT_EQ(*registry_fidl_error_status(), ZX_ERR_INVALID_ARGS);
}

// TODO(https://fxbug.dev/42068381): If Health can change post-initialization, add a test case for:
//   CreateObserver with token of Codec that was ready & Added, but then became unhealthy.

/////////////////////
// Composite tests
//
// If required 'observer_server' is not set, CreateObserver should fail with kInvalidObserver.
TEST_F(RegistryServerCompositeWarningTest, CreateObserverMissingObserver) {
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_id;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1u);
        ASSERT_TRUE(result->devices()->at(0).token_id());
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
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::RegistryCreateObserverError::kInvalidObserver)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
}

// If 'observer_server' is not set to a valid handle, we should fail with ZX_ERR_INVALID_ARGS.
TEST_F(RegistryServerCompositeWarningTest, CreateObserverBadObserver) {
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto received_callback = false;
  std::optional<TokenId> added_id;
  registry->client()->WatchDevicesAdded().Then(
      [&received_callback,
       &added_id](fidl::Result<fad::Registry::WatchDevicesAdded>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_ok() && result->devices() && result->devices()->size() == 1u);
        ASSERT_TRUE(result->devices()->at(0).token_id());
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
          .observer_server = fidl::ServerEnd<fad::Observer>(),
      }})
      .Then([&received_callback](fidl::Result<fad::Registry::CreateObserver>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error()) << result.error_value();
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_INVALID_ARGS)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  ASSERT_TRUE(registry_fidl_error_status().has_value());
  EXPECT_EQ(*registry_fidl_error_status(), ZX_ERR_INVALID_ARGS);
}

// TODO(https://fxbug.dev/42068381): If Health can change post-initialization, add a test case for:
//   CreateObserver with token of Composite that was ready & Added, but then became unhealthy.

}  // namespace
}  // namespace media_audio
