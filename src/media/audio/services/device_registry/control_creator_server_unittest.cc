// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/control_creator_server.h"

#include <fidl/fuchsia.audio.device/cpp/common_types.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class ControlCreatorServerTest : public AudioDeviceRegistryServerTestBase {};
class ControlCreatorServerCodecTest : public ControlCreatorServerTest {
 protected:
  static inline const std::string kClassName = "ControlCreatorServerCodecTest";
};
class ControlCreatorServerCompositeTest : public ControlCreatorServerTest {
 protected:
  static inline const std::string kClassName = "ControlCreatorServerCompositeTest";
};

/////////////////////
// Device-less tests
//
// Verify that the ControlCreator client can be dropped cleanly without generating a WARNING.
TEST_F(ControlCreatorServerTest, CleanClientDrop) {
  auto control_creator = CreateTestControlCreatorServer();
  ASSERT_EQ(ControlCreatorServer::count(), 1u);

  (void)control_creator->client().UnbindMaybeGetEndpoint();
}

// Verify that the ControlCreator server can shutdown cleanly without generating a WARNING.
TEST_F(ControlCreatorServerTest, CleanServerShutdown) {
  auto control_creator = CreateTestControlCreatorServer();
  ASSERT_EQ(ControlCreatorServer::count(), 1u);

  control_creator->server().Shutdown(ZX_ERR_PEER_CLOSED);
}

/////////////////////
// Codec tests
//
// Validate the ControlCreator/CreateControl method for Codec devices.
TEST_F(ControlCreatorServerCodecTest, CreateControl) {
  auto control_creator = CreateTestControlCreatorServer();
  ASSERT_EQ(ControlCreatorServer::count(), 1u);
  auto fake_driver = CreateFakeCodecOutput();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto control_endpoints = fidl::Endpoints<fad::Control>::Create();
  auto control_client =
      fidl::Client<fad::Control>(std::move(control_endpoints.client), dispatcher());
  auto received_callback = false;

  control_creator->client()
      ->Create({{
          .token_id = (*adr_service()->devices().begin())->token_id(),
          .control_server = std::move(control_endpoints.server),
      }})
      .Then([&received_callback](fidl::Result<fad::ControlCreator::Create>& result) mutable {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(control_creator_fidl_error_status().has_value());
}

/////////////////////
// Composite tests
//
// Validate the ControlCreator/CreateControl method for Composite devices.
TEST_F(ControlCreatorServerCompositeTest, CreateControl) {
  auto control_creator = CreateTestControlCreatorServer();
  ASSERT_EQ(ControlCreatorServer::count(), 1u);
  auto fake_driver = CreateFakeComposite();
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
      fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 0u);
  auto control_endpoints = fidl::Endpoints<fad::Control>::Create();
  auto control_client =
      fidl::Client<fad::Control>(std::move(control_endpoints.client), dispatcher());
  auto received_callback = false;

  control_creator->client()
      ->Create({{
          .token_id = (*adr_service()->devices().begin())->token_id(),
          .control_server = std::move(control_endpoints.server),
      }})
      .Then([&received_callback](fidl::Result<fad::ControlCreator::Create>& result) mutable {
        received_callback = true;
        EXPECT_TRUE(result.is_ok()) << result.error_value();
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_FALSE(control_creator_fidl_error_status().has_value());
}

}  // namespace
}  // namespace media_audio
