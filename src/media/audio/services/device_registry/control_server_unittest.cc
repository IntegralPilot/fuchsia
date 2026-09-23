// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/control_server.h"

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/format_utils.h"
#include "src/media/audio/services/device_registry/packet_stream_server.h"
#include "src/media/audio/services/device_registry/ring_buffer_server.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_codec.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

class ControlServerTest : public AudioDeviceRegistryServerTestBase {
 protected:
  // Obtain a control via ControlCreator/Create (not the synthetic CreateTestControlServer method).
  fidl::Client<fad::Control> ConnectToControl(
      fidl::Client<fad::ControlCreator>& control_creator_client, TokenId token_id) {
    auto [control_client_end, control_server_end] = CreateNaturalAsyncClientOrDie<fad::Control>();
    auto control_client = fidl::Client<fad::Control>(std::move(control_client_end), dispatcher(),
                                                     control_fidl_handler().get());
    bool received_callback = false;

    control_creator_client
        ->Create({{
            .token_id = token_id,
            .control_server = std::move(control_server_end),
        }})
        .Then([&received_callback](fidl::Result<fad::ControlCreator::Create>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          received_callback = true;
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    EXPECT_TRUE(control_client.is_valid());
    return control_client;
  }

  static ElementId ring_buffer_id() { return 0; }
  static ElementId dai_id() { return fad::kDefaultDaiInterconnectElementId; }
};

class ControlServerCodecTest : public ControlServerTest {
 protected:
  static inline const std::string kClassName = "ControlServerCodecTest";
  std::shared_ptr<FakeCodec> CreateAndEnableDriverWithDefaults() {
    auto fake_driver = CreateFakeCodecOutput();

    adr_service()->AddDevice(
        Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                       fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));
    RunLoopUntilIdle();
    return fake_driver;
  }
};

class ControlServerCompositeTest : public ControlServerTest {
 protected:
  static inline const std::string kClassName = "ControlServerCompositeTest";
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
// When client drops their fad::Control, the server should cleanly unwind without hang or WARNING.
TEST_F(ControlServerCodecTest, CleanClientDrop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(ControlServer::count(), 1u);

  (void)control->client().UnbindMaybeGetEndpoint();

  // If Control client doesn't drop cleanly, ControlServer will emit a WARNING, causing a failure.
}

// When server closes a client connection, the shutdown should be orderly without hang or WARNING.
TEST_F(ControlServerCodecTest, CleanServerShutdown) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(ControlServer::count(), 1u);

  control->server().Shutdown(ZX_ERR_PEER_CLOSED);

  // If ControlServer doesn't shutdown cleanly, it emits a WARNING, which will cause a failure.
}

// When client drops their fad::Control, the server should cleanly unwind without hang or WARNING.
//
// (Same as "CleanClientDrop" test case, but the Control is created "properly" through a
// ControlCreator rather than directly via AudioDeviceRegistry::CreateControlServer.)
TEST_F(ControlServerCodecTest, BasicClose) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  auto control_creator = CreateTestControlCreatorServer();
  auto control_client = ConnectToControl(control_creator->client(), *added_id);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlCreatorServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  RunLoopUntilIdle();

  (void)control_client.UnbindMaybeGetEndpoint();
}

// A ControlCreator can be closed without affecting the Controls that it created.
TEST_F(ControlServerCodecTest, ControlCreatorServerShutdownDoesNotAffectControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  auto control_creator = CreateTestControlCreatorServer();
  auto control_client = ConnectToControl(control_creator->client(), *added_id);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlCreatorServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  control_creator->server().Shutdown(ZX_ERR_PEER_CLOSED);

  RunLoopUntilIdle();
  EXPECT_TRUE(control_creator->server().WaitForShutdown(zx::sec(1)));
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  ASSERT_TRUE(control_creator_fidl_error_status().has_value());
  EXPECT_EQ(*control_creator_fidl_error_status(), ZX_ERR_PEER_CLOSED);
}

// Verify that the ControlServer shuts down cleanly if the driver drops its Codec.
TEST_F(ControlServerCodecTest, CodecDropCausesCleanControlServerShutdown) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Drop the driver Codec connection.
  fake_driver->DropCodec();

  RunLoopUntilIdle();
  EXPECT_TRUE(control->server().WaitForShutdown(zx::sec(5)));
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  ASSERT_TRUE(control_fidl_error_status().has_value());
  EXPECT_EQ(*control_fidl_error_status(), ZX_ERR_PEER_CLOSED);
}

// Validate basic SetDaiFormat functionality, including valid CodecFormatInfo returned.
TEST_F(ControlServerCodecTest, SetDaiFormat) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);
  // Determine a safe DaiFormat to set
  auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id(), device->dai_format_sets());
  auto received_callback = false;

  control->client()
      ->SetDaiFormat({{
          .dai_format = dai_format,
      }})
      .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->state());
        EXPECT_TRUE(ValidateCodecFormatInfo(*result->state()));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Validate basic SetDaiFormat functionality, including valid CodecFormatInfo returned.
TEST_F(ControlServerCodecTest, DaiFormatAlreadySet) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  {
    auto device = *adr_service()->devices().begin();
    auto control = CreateTestControlServer(device);

    RunLoopUntilIdle();
    ASSERT_EQ(RegistryServer::count(), 1u);
    ASSERT_EQ(ControlServer::count(), 1u);
    // Determine a safe DaiFormat to set
    auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id(), device->dai_format_sets());
    auto received_callback = false;
    control->client()
        ->SetDaiFormat({{
            .dai_format = dai_format,
        }})
        .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          ASSERT_TRUE(result->state());
          EXPECT_TRUE(ValidateCodecFormatInfo(*result->state()));
        });

    RunLoopUntilIdle();
    ASSERT_TRUE(received_callback);
    received_callback = false;
    control->client()->CodecStart().Then(
        [&received_callback](fidl::Result<fad::Control::CodecStart>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          EXPECT_TRUE(result->start_time().has_value());
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    EXPECT_EQ(ControlServer::count(), 1u);
    EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
    EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();

    // Now relinquish this control, and obtain a new one.
  }

  {
    auto device = *adr_service()->devices().begin();
    auto control = CreateTestControlServer(device);

    // Wait for an unexpected WARNING: DaiFormatIsNotChanged, CodecIsNotStarted, CodecIsNotStopped.
    RunLoopUntilIdle();
    ASSERT_EQ(RegistryServer::count(), 1u);
    ASSERT_EQ(ControlServer::count(), 1u);
    EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
    EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  }
}

// Validate basic CodecStart functionality including a current start_time.
TEST_F(ControlServerCodecTest, CodecStart) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);
  auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id(), device->dai_format_sets());
  auto received_callback = false;

  control->client()
      ->SetDaiFormat({{
          .dai_format = dai_format,
      }})
      .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  auto start_time = zx::time::infinite_past();
  auto time_before_start = zx::clock::get_monotonic();
  received_callback = false;

  control->client()->CodecStart().Then(
      [&received_callback, &start_time](fidl::Result<fad::Control::CodecStart>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->start_time().has_value());
        start_time = zx::time(*result->start_time());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_GT(start_time.get(), time_before_start.get());
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Validate basic CodecStop functionality including a current stop_time.
TEST_F(ControlServerCodecTest, CodecStop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);
  auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id(), device->dai_format_sets());
  auto received_callback = false;

  control->client()
      ->SetDaiFormat({{
          .dai_format = dai_format,
      }})
      .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;

  control->client()->CodecStart().Then(
      [&received_callback](fidl::Result<fad::Control::CodecStart>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  auto stop_time = zx::time::infinite_past();
  auto time_before_stop = zx::clock::get_monotonic();
  received_callback = false;

  control->client()->CodecStop().Then(
      [&received_callback, &stop_time](fidl::Result<fad::Control::CodecStop>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->stop_time().has_value());
        stop_time = zx::time(*result->stop_time());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_GT(stop_time.get(), time_before_stop.get());
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Reset - validate that the DaiFormat and the Start state are reset.
TEST_F(ControlServerCodecTest, Reset) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);
  auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id(), device->dai_format_sets());
  auto received_callback = false;

  control->client()
      ->SetDaiFormat({{
          .dai_format = dai_format,
      }})
      .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;
  zx::time first_start_time;

  control->client()->CodecStart().Then(
      [&received_callback, &first_start_time](fidl::Result<fad::Control::CodecStart>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->start_time());
        first_start_time = zx::time(*result->start_time());
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;

  control->client()->Reset().Then([&received_callback](fidl::Result<fad::Control::Reset>& result) {
    received_callback = true;
    EXPECT_TRUE(result.is_ok()) << result.error_value();
  });

  // Only way to verify that DaiFormat is reset: set the same format again.
  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  control->client()
      ->SetDaiFormat({{
          .dai_format = dai_format,
      }})
      .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  // Only way to verify that Start state is reset: call CodecStart again.
  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;
  zx::time second_start_time;

  control->client()->CodecStart().Then(
      [&received_callback, &second_start_time](fidl::Result<fad::Control::CodecStart>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->start_time());
        second_start_time = zx::time(*result->start_time());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_GT(second_start_time.get(), first_start_time.get());
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

//
// TODO(https://fxbug.dev/323270827): implement signalprocessing for Codec (topology, gain),
// including in the FakeCodec test fixture. Then add positive test cases for
// GetTopologies/GetElements and WatchTopology/WatchElementState, as are in Composite as well as
// for SetTopology/SetElementState (once implemented).

// Verify GetTopologies if the driver does not support signalprocessing.
TEST_F(ControlServerCodecTest, GetTopologiesUnsupported) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  ASSERT_FALSE(device->info()->signal_processing_topologies().has_value());
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);
  auto received_callback = false;

  control->client()->GetTopologies().Then([&received_callback](
                                              fidl::Result<fad::Control::GetTopologies>& result) {
    received_callback = true;
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value().framework_error();
    EXPECT_EQ(result.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  // After this failing call, the binding should still be usable.
  auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id(), device->dai_format_sets());
  received_callback = false;
  control->client()
      ->SetDaiFormat({{
          .dai_format = dai_format,
      }})
      .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->state());
        EXPECT_TRUE(ValidateCodecFormatInfo(*result->state()));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ControlServer::count(), 1u);

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify GetElements if the driver does not support signalprocessing.
TEST_F(ControlServerCodecTest, GetElementsUnsupported) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  ASSERT_FALSE(device->info()->signal_processing_topologies().has_value());
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);
  auto received_callback = false;

  control->client()->GetElements().Then([&received_callback](
                                            fidl::Result<fad::Control::GetElements>& result) {
    received_callback = true;
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value().framework_error();
    EXPECT_EQ(result.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  // After this failing call, the binding should still be usable.
  auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id(), device->dai_format_sets());
  received_callback = false;
  control->client()
      ->SetDaiFormat({{
          .dai_format = dai_format,
      }})
      .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->state());
        EXPECT_TRUE(ValidateCodecFormatInfo(*result->state()));
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ControlServer::count(), 1u);

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

/////////////////////
// Composite tests
//
// When client drops their fad::Control, the server should cleanly unwind without hang or WARNING.
TEST_F(ControlServerCompositeTest, CleanClientDrop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(ControlServer::count(), 1u);

  (void)control->client().UnbindMaybeGetEndpoint();

  // If Control client doesn't drop cleanly, ControlServer will emit a WARNING, causing a failure.
}

// When server closes a client connection, the shutdown should be orderly without hang or WARNING.
TEST_F(ControlServerCompositeTest, CleanServerShutdown) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(ControlServer::count(), 1u);

  control->server().Shutdown(ZX_ERR_PEER_CLOSED);

  // If ControlServer doesn't shutdown cleanly, it emits a WARNING, which will cause a failure.
}

// When client drops their fad::Control, the server should cleanly unwind without hang or WARNING.
//
// (Same as "CleanClientDrop" test case, but the Control is created "properly" through a
// ControlCreator rather than directly via AudioDeviceRegistry::CreateControlServer.)
TEST_F(ControlServerCompositeTest, BasicClose) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  auto control_creator = CreateTestControlCreatorServer();
  auto control_client = ConnectToControl(control_creator->client(), *added_id);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlCreatorServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  RunLoopUntilIdle();

  (void)control_client.UnbindMaybeGetEndpoint();
}

// A ControlCreator can be closed without affecting the Controls that it created.
TEST_F(ControlServerCompositeTest, ControlCreatorServerShutdownDoesNotAffectControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  auto control_creator = CreateTestControlCreatorServer();
  auto control_client = ConnectToControl(control_creator->client(), *added_id);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlCreatorServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  control_creator->server().Shutdown(ZX_ERR_PEER_CLOSED);

  RunLoopUntilIdle();
  EXPECT_TRUE(control_creator->server().WaitForShutdown(zx::sec(1)));
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
  ASSERT_TRUE(control_creator_fidl_error_status().has_value());
  EXPECT_EQ(*control_creator_fidl_error_status(), ZX_ERR_PEER_CLOSED);
}

// Verify that the ControlServer shuts down cleanly if the driver drops its Composite.
TEST_F(ControlServerCompositeTest, CompositeDropCausesCleanControlServerShutdown) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  (void)WaitForAddedDeviceTokenId(registry->client());
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Drop the driver Composite connection.
  fake_driver->DropComposite();

  RunLoopUntilIdle();
  EXPECT_TRUE(control->server().WaitForShutdown(zx::sec(5)));
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  ASSERT_TRUE(control_fidl_error_status().has_value());
  EXPECT_EQ(*control_fidl_error_status(), ZX_ERR_PEER_CLOSED);
}

// Validate (in depth) the ring_buffer and properties returned from CreateRingBuffer -
// and do this for all RingBuffer elements in the topology.
TEST_F(ControlServerCompositeTest, CreateRingBuffer) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto control = CreateTestControlServer(*adr_service()->devices().begin());
  auto device = *adr_service()->devices().begin();

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Validate every RingBuffer on this device.
  for (auto ring_buffer_id : device->ring_buffer_ids()) {
    fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
    auto [ring_buffer_client_end, ring_buffer_server_end] =
        CreateNaturalAsyncClientOrDie<fad::RingBuffer>();
    bool received_callback = false;

    auto ring_buffer_client = fidl::Client<fad::RingBuffer>(
        std::move(ring_buffer_client_end), dispatcher(), ring_buffer_fidl_handler().get());
    auto requested_format = SafeRingBufferFormatFromElementRingBufferFormatSets(
        ring_buffer_id, device->ring_buffer_format_sets());
    uint32_t requested_ring_buffer_bytes = 2000;

    control->client()
        ->CreateRingBuffer({{
            .element_id = ring_buffer_id,
            .options = fad::RingBufferOptions{{
                .format = requested_format,
                .ring_buffer_min_bytes = requested_ring_buffer_bytes,
            }},
            .ring_buffer_server = std::move(ring_buffer_server_end),
        }})
        .Then([&received_callback, requested_format,
               requested_ring_buffer_bytes](fidl::Result<fad::Control::CreateRingBuffer>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();

          // validate ring_buffer
          ASSERT_TRUE(result->ring_buffer()->buffer().has_value());
          EXPECT_GT(result->ring_buffer()->buffer()->size(), requested_ring_buffer_bytes);
          EXPECT_TRUE(result->ring_buffer()->buffer()->vmo().is_valid());

          ASSERT_TRUE(result->ring_buffer()->consumer_bytes().has_value());
          EXPECT_GT(*result->ring_buffer()->consumer_bytes(), 0u);
          ASSERT_TRUE(result->ring_buffer()->producer_bytes().has_value());
          EXPECT_GT(*result->ring_buffer()->producer_bytes(), 0u);

          EXPECT_TRUE(*result->ring_buffer()->consumer_bytes() >= requested_ring_buffer_bytes ||
                      *result->ring_buffer()->producer_bytes() >= requested_ring_buffer_bytes);

          ASSERT_TRUE(result->ring_buffer()->format()->channel_count().has_value());
          EXPECT_EQ(*result->ring_buffer()->format()->channel_count(),
                    requested_format.channel_count());

          ASSERT_TRUE(result->ring_buffer()->format()->sample_type().has_value());

          ASSERT_TRUE(result->ring_buffer()->format()->frames_per_second().has_value());
          EXPECT_EQ(*result->ring_buffer()->format()->frames_per_second(),
                    requested_format.frames_per_second());

          ASSERT_TRUE(result->ring_buffer()->reference_clock().has_value());
          EXPECT_TRUE(result->ring_buffer()->reference_clock()->is_valid());

          ASSERT_TRUE(result->ring_buffer()->reference_clock_domain().has_value());
          EXPECT_EQ(*result->ring_buffer()->reference_clock_domain(),
                    fuchsia_hardware_audio::kClockDomainMonotonic);

          // validate properties - turn_on_delay
          ASSERT_TRUE(result->properties()->turn_on_delay().has_value());
          EXPECT_GE(*result->properties()->turn_on_delay(), 0);

          // validate properties - valid_bits_per_sample
          const auto sample_type = *requested_format.sample_type();
          ASSERT_TRUE(result->properties()->valid_bits_per_sample().has_value());
          auto driver_pcm = MapSampleTypeToDriverPcm(sample_type);
          ASSERT_TRUE(driver_pcm.has_value());
          EXPECT_LE(*result->properties()->valid_bits_per_sample(), driver_pcm->max_valid_bits);
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }

  // Allow the ControlServer to destruct, if it (erroneously) wants to.
  RunLoopUntilIdle();
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that the Control lives, even if the client drops its child RingBuffer.
TEST_F(ControlServerCompositeTest, CreatePacketStream) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto control = CreateTestControlServer(*adr_service()->devices().begin());
  auto device = *adr_service()->devices().begin();

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // First set the topology to one that includes the output PacketStream.
  control->client()
      ->SetTopology(FakeComposite::kPacketStreamOutputTopologyId)
      .Then([](auto& result) { ASSERT_TRUE(result.is_ok()); });
  std::optional<TopologyId> current_topology;
  while (current_topology != FakeComposite::kPacketStreamOutputTopologyId) {
    bool received = false;
    control->client()->WatchTopology().Then([&](auto& result) {
      ASSERT_TRUE(result.is_ok());
      current_topology = result->topology_id();
      received = true;
    });
    RunLoopUntilIdle();
    ASSERT_TRUE(received);
  }

  // Validate every PacketStream on this device, across all supported formats.
  for (auto packet_stream_id : device->packet_stream_ids()) {
    for (auto requested_format :
         SafePacketStreamFormats(packet_stream_id, device->packet_stream_format_sets())) {
      auto [packet_stream_client_end, packet_stream_server_end] =
          CreateNaturalAsyncClientOrDie<fad::PacketStream>();
      bool received_callback = false;

      auto packet_stream_client = fidl::Client<fad::PacketStream>(
          std::move(packet_stream_client_end), dispatcher(), packet_stream_fidl_handler().get());

      fad::ControlCreatePacketStreamRequest request;
      request.element_id(packet_stream_id);
      request.options(fad::PacketStreamOptions{{.format = requested_format}});
      request.packet_stream_server(std::move(packet_stream_server_end));

      control->client()
          ->CreatePacketStream(std::move(request))
          .Then([&received_callback, requested_format,
                 packet_stream_id](fidl::Result<fad::Control::CreatePacketStream>& result) {
            received_callback = true;
            ASSERT_TRUE(result.is_ok()) << result.error_value();

            // validate properties
            ASSERT_TRUE(result->properties());
            if (packet_stream_id == FakeComposite::kSourcePsElementId) {
              ASSERT_TRUE(result->properties()->data_sink());
              EXPECT_TRUE(result->properties()->data_sink()->is_valid());
            }

            ASSERT_TRUE(result->properties()->format());
            EXPECT_EQ(*result->properties()->format(), requested_format);

            // validate properties - valid_bits_per_sample
            if (requested_format.pcm_format().has_value()) {
              const auto sample_type = *requested_format.pcm_format()->sample_type();
              ASSERT_TRUE(result->properties()->valid_bits_per_sample().has_value());
              auto driver_pcm = MapSampleTypeToDriverPcm(sample_type);
              ASSERT_TRUE(driver_pcm.has_value());
              EXPECT_LE(*result->properties()->valid_bits_per_sample(), driver_pcm->max_valid_bits);
            }
          });

      RunLoopUntilIdle();
      EXPECT_TRUE(received_callback);
    }
  }

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that the Control lives, even if the client drops its child PacketStream.
TEST_F(ControlServerCompositeTest, ClientPacketStreamDropDoesNotAffectControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto control = CreateTestControlServer(*adr_service()->devices().begin());
  auto device = *adr_service()->devices().begin();

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Validate every PacketStream on this device.
  for (auto packet_stream_id : device->packet_stream_ids()) {
    for (auto requested_format :
         SafePacketStreamFormats(packet_stream_id, device->packet_stream_format_sets())) {
      auto [packet_stream_client_end, packet_stream_server_end] =
          CreateNaturalAsyncClientOrDie<fad::PacketStream>();
      bool received_callback = false;

      auto packet_stream_client = fidl::Client<fad::PacketStream>(
          std::move(packet_stream_client_end), dispatcher(), packet_stream_fidl_handler().get());

      fad::ControlCreatePacketStreamRequest request;
      request.element_id(packet_stream_id);
      request.options(fad::PacketStreamOptions{{.format = requested_format}});
      request.packet_stream_server(std::move(packet_stream_server_end));

      control->client()
          ->CreatePacketStream(std::move(request))
          .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
            received_callback = true;
            ASSERT_TRUE(result.is_ok()) << result.error_value();
          });

      RunLoopUntilIdle();
      EXPECT_TRUE(received_callback);
      // Let our PacketStream client connection drop.
      (void)packet_stream_client.UnbindMaybeGetEndpoint();

      // Wait for the PacketStreamServer to destruct.
      while (PacketStreamServer::count() > 0u) {
        RunLoopUntilIdle();
      }
    }
  }

  // Allow the ControlServer to destruct, if it (erroneously) wants to.
  RunLoopUntilIdle();
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that the Control lives, even if the driver drops its PacketStream connection.
TEST_F(ControlServerCompositeTest, DriverPacketStreamDropDoesNotAffectControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Validate every PacketStream on this device.
  for (auto packet_stream_id : device->packet_stream_ids()) {
    for (auto requested_format :
         SafePacketStreamFormats(packet_stream_id, device->packet_stream_format_sets())) {
      auto [packet_stream_client_end, packet_stream_server_end] =
          CreateNaturalAsyncClientOrDie<fad::PacketStream>();
      bool received_callback = false;

      fad::ControlCreatePacketStreamRequest request;
      request.element_id(packet_stream_id);
      request.options(fad::PacketStreamOptions{{.format = requested_format}});
      request.packet_stream_server(std::move(packet_stream_server_end));

      control->client()
          ->CreatePacketStream(std::move(request))
          .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
            ASSERT_TRUE(result.is_ok()) << result.error_value();
            received_callback = true;
          });

      RunLoopUntilIdle();
      EXPECT_EQ(PacketStreamServer::count(), 1u);
      EXPECT_TRUE(received_callback);

      // Drop the driver PacketStream connection.
      fake_driver->DropPacketStream(packet_stream_id);

      // Wait for the PacketStreamServer to destruct.
      while (PacketStreamServer::count() > 0u) {
        RunLoopUntilIdle();
      }
    }
  }

  // Allow the ControlServer to destruct, if it (erroneously) wants to.
  RunLoopUntilIdle();
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that the Control lives, even if the client drops its child RingBuffer.
TEST_F(ControlServerCompositeTest, ClientRingBufferDropDoesNotAffectControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto control = CreateTestControlServer(*adr_service()->devices().begin());
  auto device = *adr_service()->devices().begin();

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Validate every RingBuffer on this device.
  for (auto ring_buffer_id : device->ring_buffer_ids()) {
    fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
    auto [ring_buffer_client_end, ring_buffer_server_end] =
        CreateNaturalAsyncClientOrDie<fad::RingBuffer>();
    bool received_callback = false;

    auto ring_buffer_client = fidl::Client<fad::RingBuffer>(
        std::move(ring_buffer_client_end), dispatcher(), ring_buffer_fidl_handler().get());

    control->client()
        ->CreateRingBuffer({{
            .element_id = ring_buffer_id,
            .options = fad::RingBufferOptions{{
                .format = SafeRingBufferFormatFromElementRingBufferFormatSets(
                    ring_buffer_id, device->ring_buffer_format_sets()),
                .ring_buffer_min_bytes = 2000,
            }},
            .ring_buffer_server = std::move(ring_buffer_server_end),
        }})
        .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    // Let our RingBuffer client connection drop.
    (void)ring_buffer_client.UnbindMaybeGetEndpoint();

    // Wait for the RingBufferServer to destruct.
    while (RingBufferServer::count() > 0u) {
      RunLoopUntilIdle();
    }
  }

  // Allow the ControlServer to destruct, if it (erroneously) wants to.
  RunLoopUntilIdle();
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that the Control lives, even if the driver drops its RingBuffer connection.
TEST_F(ControlServerCompositeTest, DriverRingBufferDropDoesNotAffectControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(*adr_service()->devices().begin());

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Validate every RingBuffer on this device.
  for (auto ring_buffer_id : device->ring_buffer_ids()) {
    fake_driver->ReserveRingBufferSize(ring_buffer_id, 8192);
    auto [ring_buffer_client, ring_buffer_server_end] =
        CreateNaturalAsyncClientOrDie<fad::RingBuffer>();
    bool received_callback = false;

    control->client()
        ->CreateRingBuffer({{
            .element_id = ring_buffer_id,
            .options = fad::RingBufferOptions{{
                .format = SafeRingBufferFormatFromElementRingBufferFormatSets(
                    ring_buffer_id, device->ring_buffer_format_sets()),
                .ring_buffer_min_bytes = 2000,
            }},
            .ring_buffer_server = std::move(ring_buffer_server_end),
        }})
        .Then([&received_callback](fidl::Result<fad::Control::CreateRingBuffer>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          received_callback = true;
        });

    RunLoopUntilIdle();
    EXPECT_EQ(RingBufferServer::count(), 1u);
    EXPECT_TRUE(received_callback);

    // Drop the driver RingBuffer connection.
    fake_driver->DropRingBuffer(ring_buffer_id);

    // Wait for the RingBufferServer to destruct.
    while (RingBufferServer::count() > 0u) {
      RunLoopUntilIdle();
    }
  }

  // Allow the ControlServer to destruct, if it (erroneously) wants to.
  RunLoopUntilIdle();
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Validate basic SetDaiFormat functionality.
TEST_F(ControlServerCompositeTest, SetDaiFormat) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Validate every DaiInterconnect on this device.
  for (ElementId dai_id : device->dai_ids()) {
    auto received_callback = false;

    // Determine a safe DaiFormat to set
    auto dai_format = SecondDaiFormatFromElementDaiFormatSets(dai_id, device->dai_format_sets());
    control->client()
        ->SetDaiFormat({{
            .element_id = dai_id,
            .dai_format = dai_format,
        }})
        .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          EXPECT_FALSE(result->state());
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    EXPECT_EQ(ControlServer::count(), 1u);
  }
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that calling SetDaiFormat twice with identical format on a Composite device returns
// nullopt state without dereferencing nullopt.
TEST_F(ControlServerCompositeTest, SetDaiFormatTwiceSameFormat) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  for (ElementId dai_id : device->dai_ids()) {
    auto dai_format = SecondDaiFormatFromElementDaiFormatSets(dai_id, device->dai_format_sets());

    auto received_callback = false;
    control->client()
        ->SetDaiFormat({{
            .element_id = dai_id,
            .dai_format = dai_format,
        }})
        .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          EXPECT_FALSE(result->state());
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);

    // Set the exact same DAI format a second time.
    received_callback = false;
    control->client()
        ->SetDaiFormat({{
            .element_id = dai_id,
            .dai_format = dai_format,
        }})
        .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
          received_callback = true;
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          EXPECT_FALSE(result->state());
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
  }
}

// Reset - validate that the DaiFormat and the Start state are reset.
TEST_F(ControlServerCompositeTest, Reset) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  (void)WaitForAddedDeviceTokenId(registry->client());
  auto device = *adr_service()->devices().begin();
  auto control = CreateTestControlServer(device);

  RunLoopUntilIdle();
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_EQ(ControlServer::count(), 1u);

  // Validate every DaiInterconnect on this device.
  for (ElementId dai_id : device->dai_ids()) {
    auto dai_format = SafeDaiFormatFromElementDaiFormatSets(dai_id, device->dai_format_sets());
    auto received_callback = false;
    control->client()
        ->SetDaiFormat({{
            .element_id = dai_id,
            .dai_format = dai_format,
        }})
        .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
          received_callback = true;
          EXPECT_TRUE(result.is_ok()) << result.error_value();
        });

    RunLoopUntilIdle();
    ASSERT_TRUE(received_callback);

    // Verify that the ControlNotify received the DaiFormat notification.

    received_callback = false;

    control->client()->Reset().Then(
        [&received_callback](fidl::Result<fad::Control::Reset>& result) {
          received_callback = true;
          EXPECT_TRUE(result.is_ok()) << result.error_value();
        });

    // Only way to verify that DaiFormat is reset: set the same format again.
    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    received_callback = false;
    control->client()
        ->SetDaiFormat({{
            .element_id = dai_id,
            .dai_format = dai_format,
        }})
        .Then([&received_callback](fidl::Result<fad::Control::SetDaiFormat>& result) {
          received_callback = true;
          EXPECT_TRUE(result.is_ok()) << result.error_value();
        });

    RunLoopUntilIdle();
    EXPECT_TRUE(received_callback);
    EXPECT_EQ(ControlServer::count(), 1u);

    // Verify that the ControlNotify received the DaiFormat notification.
  }
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Retrieves the static list of Topologies and their properties.
// Compare results from fad::Control/GetTopologies to the topologies returned in the Device info.
TEST_F(ControlServerCompositeTest, GetTopologies) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto initial_topologies = device->info()->signal_processing_topologies();
  ASSERT_TRUE(initial_topologies.has_value() && !initial_topologies->empty());

  auto control = CreateTestControlServer(device);
  auto received_callback = false;
  std::vector<fhasp::Topology> received_topologies;

  control->client()->GetTopologies().Then([&received_callback, &received_topologies](
                                              fidl::Result<fad::Control::GetTopologies>& result) {
    received_callback = true;
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    received_topologies = result->topologies();
    EXPECT_FALSE(received_topologies.empty());
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(initial_topologies->size(), received_topologies.size());
  EXPECT_THAT(received_topologies, testing::ElementsAreArray(*initial_topologies));
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Retrieves the static list of Elements and their properties.
// Compare results from fad::Control/GetElements to the elements returned in the Device info.
TEST_F(ControlServerCompositeTest, GetElements) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto initial_elements = device->info()->signal_processing_elements();
  ASSERT_TRUE(initial_elements.has_value() && !initial_elements->empty());

  auto control = CreateTestControlServer(device);
  auto received_callback = false;
  std::vector<fhasp::Element> received_elements;

  control->client()->GetElements().Then(
      [&received_callback, &received_elements](fidl::Result<fad::Control::GetElements>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_elements = result->processing_elements();
        EXPECT_FALSE(received_elements.empty());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(initial_elements->size(), received_elements.size());
  EXPECT_THAT(received_elements, testing::ElementsAreArray(*initial_elements));

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that WatchTopology correctly returns the initial topology state.
TEST_F(ControlServerCompositeTest, WatchTopologyInitial) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto received_callback = false;
  std::optional<TopologyId> topology_id;

  control->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Control::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(topology_id.has_value());
  EXPECT_TRUE(topology_map(device).contains(*topology_id));

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that WatchTopology pends when called a second time (if no change).
TEST_F(ControlServerCompositeTest, WatchTopologyNoChange) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto received_callback = false;
  std::optional<TopologyId> topology_id;

  control->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Control::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(topology_id.has_value());
  received_callback = false;

  control->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Control::WatchTopology>& result) {
        received_callback = true;
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        topology_id = result->topology_id();
      });

  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that WatchTopology works with dynamic changes, after initial query.
TEST_F(ControlServerCompositeTest, WatchTopologyUpdate) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto received_callback = false;
  std::optional<TopologyId> topology_id;

  control->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Control::WatchTopology>& result) {
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

  control->client()->WatchTopology().Then(
      [&received_callback, &topology_id](fidl::Result<fad::Control::WatchTopology>& result) {
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

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that WatchElementState correctly returns the initial states of all elements.
TEST_F(ControlServerCompositeTest, WatchElementStateInitial) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto& elements_from_device = element_map(device);
  auto received_callback = false;
  std::unordered_map<ElementId, fhasp::ElementState> element_states;

  // Gather the complete set of initial element states.
  for (auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    control->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states](fidl::Result<fad::Control::WatchElementState>& result) {
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
    EXPECT_EQ(element_states.at(element_id), state_from_device);
  }

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that WatchElementState pends indefinitely, if there has been no change.
TEST_F(ControlServerCompositeTest, WatchElementStateNoChange) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto& elements_from_device = element_map(device);
  auto received_callback = false;
  std::unordered_map<ElementId, fhasp::ElementState> element_states;

  // Gather the complete set of initial element states.
  for (auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    control->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states](fidl::Result<fad::Control::WatchElementState>& result) {
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
    control->client()
        ->WatchElementState(element_id)
        .Then([&received_callback,
               element_id](fidl::Result<fad::Control::WatchElementState>& result) {
          received_callback = true;
          FAIL() << "Unexpected WatchElementState completion for element_id " << element_id;
        });
  }

  // We request all the states from the Elements again, then wait once.
  RunLoopUntilIdle();
  EXPECT_FALSE(received_callback);
  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify that WatchElementState works with dynamic changes, after initial query.
TEST_F(ControlServerCompositeTest, WatchElementStateUpdate) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto& elements_from_device = element_map(device);
  auto received_callback = false;
  std::unordered_map<ElementId, fhasp::ElementState> element_states;

  // Gather the complete set of initial element states.
  for (auto& element_map_entry : elements_from_device) {
    auto element_id = element_map_entry.first;
    control->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states](fidl::Result<fad::Control::WatchElementState>& result) {
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

    control->client()
        ->WatchElementState(element_id)
        .Then([&received_callback, element_id,
               &element_states_received](fidl::Result<fad::Control::WatchElementState>& result) {
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
    const auto& state_injected = element_states_to_inject.at(element_id);
    EXPECT_EQ(state_received, state_injected);

    // Compare the updates received by the client to the collection held by the Device object.
    ASSERT_TRUE(elements_from_device.contains(element_id));
    const auto& state_from_device = elements_from_device.at(element_id).state;
    EXPECT_EQ(state_received, state_from_device);
  }

  EXPECT_FALSE(registry_fidl_error_status().has_value()) << *registry_fidl_error_status();
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify SetTopology (OK to use WatchTopology in doing this)
TEST_F(ControlServerCompositeTest, SetTopology) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();

  auto added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_device_id.has_value());
  auto [status, device] = adr_service()->FindDeviceByTokenId(*added_device_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  if (device->topology_ids().size() == 1) {
    GTEST_SKIP() << "Fake driver does not expose multiple topologies";
  }

  auto control = CreateTestControlServer(device);
  auto received_callback = false;
  std::optional<TopologyId> current_topology_id;

  control->client()->WatchTopology().Then([&received_callback, &current_topology_id](
                                              fidl::Result<fad::Control::WatchTopology>& result) {
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

  control->client()->WatchTopology().Then(
      [&received_callback, &new_topology_id](fidl::Result<fad::Control::WatchTopology>& result) {
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
  EXPECT_FALSE(control_fidl_error_status().has_value()) << *control_fidl_error_status();
}

// Verify SetElementState (OK to use WatchElementState in doing this)

}  // namespace
}  // namespace media_audio
