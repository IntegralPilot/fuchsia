// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <zircon/errors.h>

#include <memory>
#include <optional>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/observer_server.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_codec.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class ObserverServerWarningTest : public AudioDeviceRegistryServerTestBase,
                                  public fidl::AsyncEventHandler<fad::Observer> {
 public:
  void handle_unknown_event(fidl::UnknownEventMetadata<fad::Observer> metadata) override;
};

void ObserverServerWarningTest::handle_unknown_event(
    fidl::UnknownEventMetadata<fad::Observer> metadata) {
  FAIL() << "ObserverServerWarningTest: unknown event (Observer) ordinal "
         << metadata.event_ordinal;
}

class ObserverServerCodecWarningTest : public ObserverServerWarningTest {
 protected:
  static inline const std::string kClassName = "ObserverServerCodecWarningTest";
  std::shared_ptr<FakeCodec> CreateAndEnableDriverWithDefaults() {
    auto fake_driver = CreateFakeCodecNoDirection();

    adr_service()->AddDevice(
        Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                       fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));
    RunLoopUntilIdle();
    return fake_driver;
  }
};

class ObserverServerCompositeWarningTest : public ObserverServerWarningTest {
 protected:
  static inline const std::string kClassName = "ObserverServerCompositeWarningTest";
  std::shared_ptr<FakeComposite> CreateAndEnableDriverWithDefaults() {
    auto fake_driver = CreateFakeComposite();

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
// While `WatchPlugState` is pending, calling this again is an error (but non-fatal).
TEST_F(ObserverServerCodecWarningTest, WatchPlugStateWhilePending) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  // We'll always receive an immediate response from the first `WatchPlugState` call.
  auto observer = CreateTestObserverServer(added_device);
  bool received_initial_callback = false;
  observer->client()->WatchPlugState().Then(
      [&received_initial_callback](fidl::Result<fad::Observer::WatchPlugState>& result) mutable {
        received_initial_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_initial_callback);
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
  bool received_second_callback = false;
  // The second `WatchPlugState` call should pend indefinitely (even after the third one fails).
  observer->client()->WatchPlugState().Then(
      [&received_second_callback](fidl::Result<fad::Observer::WatchPlugState>& result) mutable {
        received_second_callback = true;
        FAIL() << "Unexpected completion for pending WatchPlugState call";
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_second_callback);
  bool received_third_callback = false;
  // This third `WatchPlugState` call should fail immediately (domain error ALREADY_PENDING)
  // since the second call has not yet completed.
  observer->client()->WatchPlugState().Then(
      [&received_third_callback](fidl::Result<fad::Observer::WatchPlugState>& result) mutable {
        received_third_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::ObserverWatchPlugStateError::kAlreadyPending)
            << result.error_value();
      });

  RunLoopUntilIdle();
  // After this, the second `WatchPlugState` should still pend and the Observer should still be OK.
  EXPECT_FALSE(received_second_callback);
  EXPECT_TRUE(received_third_callback);
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// Codec: GetReferenceClock is unsupported
TEST_F(ObserverServerCodecWarningTest, GetReferenceClockWrongDeviceType) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(added_device);
  ASSERT_EQ(ObserverServer::count(), 1u);
  bool received_callback = false;

  observer->client()->GetReferenceClock().Then(
      [&received_callback](fidl::Result<fad::Observer::GetReferenceClock>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error())
            << result.error_value().framework_error();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::ObserverGetReferenceClockError::kWrongDeviceType)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// TODO(https://fxbug.dev/323270827): implement signalprocessing for Codec (topology, gain),
// including in the FakeCodec test fixture. Then add negative test cases for
// GetTopologies/GetElements/WatchTopology/WatchElementState, as are in Composite.

// Verify WatchTopology if the driver does not support signalprocessing.
TEST_F(ObserverServerCodecWarningTest, WatchTopologyUnsupported) {
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

  observer->client()->WatchTopology().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value().status(), ZX_ERR_NOT_SUPPORTED);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  // After this failing call, the binding should not be usable.
  observer->client()->WatchPlugState().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchPlugState>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error());
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_NOT_SUPPORTED);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(observer->client().is_valid());
}

// Verify WatchElementState if the driver does not support signalprocessing.
TEST_F(ObserverServerCodecWarningTest, WatchElementStateUnsupported) {
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

  observer->client()
      ->WatchElementState(fad::kDefaultDaiInterconnectElementId)
      .Then([&received_callback](fidl::Result<fad::Observer::WatchElementState>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value().status(), ZX_ERR_NOT_SUPPORTED);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  // After this failing call, the binding should not be usable.
  observer->client()->WatchPlugState().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchPlugState>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error());
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_NOT_SUPPORTED);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(observer->client().is_valid());
}

/////////////////////
// Composite tests
//
// Verify that the Observer cannot handle a WatchPlugState request from this type of device.
TEST_F(ObserverServerCompositeWarningTest, WatchPlugStateWrongDeviceType) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(added_device);
  ASSERT_EQ(ObserverServer::count(), 1u);
  bool received_callback = false;

  observer->client()->WatchPlugState().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchPlugState>& result) mutable {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error())
            << result.error_value().framework_error();
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::ObserverWatchPlugStateError::kWrongDeviceType)
            << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ObserverServer::count(), 1u);
  EXPECT_FALSE(observer_fidl_error_status().has_value()) << *observer_fidl_error_status();
}

// WatchTopology cases (without using SetTopology): Watch-while-pending
TEST_F(ObserverServerCompositeWarningTest, WatchTopologyWhilePending) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ObserverServer::count(), 1u);

  //
  // Receive the initial Topology for this device.
  auto received_callback = false;
  observer->client()->WatchTopology().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  //
  // Now call WatchTopology again, which should pend. Then call WatchTopology AGAIN. This should
  // cause BOTH watches to fail and render the Observer unusable.
  received_callback = false;
  observer->client()->WatchTopology().Then(
      [&received_callback](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value().status(), ZX_ERR_BAD_STATE);
      });

  RunLoopUntilIdle();
  // This should pend, since we have not changed the device's topology.
  EXPECT_FALSE(received_callback);

  auto received_callback2 = false;
  observer->client()->WatchTopology().Then(
      [&received_callback2](fidl::Result<fad::Observer::WatchTopology>& result) {
        received_callback2 = true;
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value().status(), ZX_ERR_BAD_STATE);
      });

  RunLoopUntilIdle();
  // This should complete with error ZX_ERR_BAD_STATE.
  EXPECT_TRUE(received_callback2);
  // After the above failure, the PREVIOUS WatchTopology should also complete with failure.
  EXPECT_TRUE(received_callback);
}

// WatchElementState cases (without SetElementState): unknown ElementId, Watch-while-pending
TEST_F(ObserverServerCompositeWarningTest, WatchElementStateUnknownElementId) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ObserverServer::count(), 1u);
  auto& elements_from_device = element_map(device);
  ElementId unknown_element_id = 0;
  while (true) {
    if (!elements_from_device.contains(unknown_element_id)) {
      break;
    }
    ++unknown_element_id;
  }
  auto received_callback = false;

  observer->client()
      ->WatchElementState(unknown_element_id)
      .Then([&received_callback](fidl::Result<fad::Observer::WatchElementState>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value().status(), ZX_ERR_INVALID_ARGS);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  // After a failing WatchElementState call, the binding should not be usable.
  observer->client()->GetReferenceClock().Then(
      [&received_callback](fidl::Result<fad::Observer::GetReferenceClock>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error());
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_INVALID_ARGS);
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(observer->client().is_valid());
}

TEST_F(ObserverServerCompositeWarningTest, WatchElementStateWhilePending) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto observer = CreateTestObserverServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ObserverServer::count(), 1u);
  auto& elements_from_device = element_map(device);
  auto element_id = elements_from_device.begin()->first;

  //
  // Receive the initial ElementState for this element_id.
  auto received_callback = false;
  observer->client()
      ->WatchElementState(element_id)
      .Then([&received_callback](fidl::Result<fad::Observer::WatchElementState>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  //
  // Now call WatchElementState again, which should pend. Then call WatchElementState AGAIN for the
  // same element_id. This should cause BOTH watches to fail and render the Observer unusable.
  received_callback = false;
  observer->client()
      ->WatchElementState(element_id)
      .Then([&received_callback](fidl::Result<fad::Observer::WatchElementState>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value().status(), ZX_ERR_BAD_STATE) << result.error_value();
      });

  RunLoopUntilIdle();
  // This should pend, since we have not changed this element's state
  EXPECT_FALSE(received_callback);

  auto received_callback2 = false;
  observer->client()
      ->WatchElementState(element_id)
      .Then([&received_callback2](fidl::Result<fad::Observer::WatchElementState>& result) {
        received_callback2 = true;
        ASSERT_TRUE(result.is_error());
        EXPECT_EQ(result.error_value().status(), ZX_ERR_BAD_STATE) << result.error_value();
      });

  RunLoopUntilIdle();
  // This should complete with error ZX_ERR_BAD_STATE.
  EXPECT_TRUE(received_callback2);
  // After the above failure, the PREVIOUS WatchElementState should also complete with failure.
  EXPECT_TRUE(received_callback);

  //
  // The observer binding should not be usable now. Try a non-signalprocessing method to confirm.
  received_callback = false;
  observer->client()->GetReferenceClock().Then(
      [&received_callback](fidl::Result<fad::Observer::GetReferenceClock>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_framework_error()) << result.error_value();
        EXPECT_EQ(result.error_value().framework_error().status(), ZX_ERR_BAD_STATE)
            << result.error_value().framework_error();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
}

}  // namespace
}  // namespace media_audio
