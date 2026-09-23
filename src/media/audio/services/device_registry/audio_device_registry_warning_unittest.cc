// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/common_types.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/audio_device_registry.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class AudioDeviceRegistryServerWarningTest : public AudioDeviceRegistryServerTestBase {};

// Device-less tests
TEST_F(AudioDeviceRegistryServerWarningTest, FindDeviceByTokenIdUnknown) {
  EXPECT_EQ(adr_service()->FindDeviceByTokenId(-1).first,
            AudioDeviceRegistry::DevicePresence::Unknown);
}

// Tests that use multiple device types
TEST_F(AudioDeviceRegistryServerWarningTest, UnhealthyDevices) {
  auto fake_codec = CreateFakeCodecOutput();
  auto fake_composite = CreateFakeComposite();

  fake_codec->set_health_state(false);
  fake_composite->set_health_state(false);

  auto codec_client = fake_codec->Enable();
  auto composite_client = fake_composite->Enable();

  AddDeviceForDetection("test codec", fad::DeviceType::kCodec,
                        fad::DriverClient::WithCodec(std::move(codec_client)));
  AddDeviceForDetection("test composite", fad::DeviceType::kComposite,
                        fad::DriverClient::WithComposite(std::move(composite_client)));

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 2u);
}

/////////////////////
// Codec cases
TEST_F(AudioDeviceRegistryServerWarningTest, FindCodecByTokenIdError) {
  auto fake_driver = CreateFakeCodecInput();
  fake_driver->set_health_state(false);
  auto client = fake_driver->Enable();
  AddDeviceForDetection("test codec", fad::DeviceType::kCodec,
                        fad::DriverClient::WithCodec(std::move(client)));

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 1u);
  auto token_id = adr_service()->unhealthy_devices().begin()->get()->token_id();

  EXPECT_EQ(adr_service()->FindDeviceByTokenId(token_id).first,
            AudioDeviceRegistry::DevicePresence::Error);
}

TEST_F(AudioDeviceRegistryServerWarningTest, FindCodecByTokenIdRemoved) {
  auto fake_driver = CreateFakeCodecNoDirection();
  auto client = fake_driver->Enable();
  AddDeviceForDetection("test codec", fad::DeviceType::kCodec,
                        fad::DriverClient::WithCodec(std::move(client)));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto token_id = adr_service()->devices().begin()->get()->token_id();

  fake_driver->DropCodec();
  RunLoopUntilIdle();

  EXPECT_EQ(adr_service()->FindDeviceByTokenId(token_id).first,
            AudioDeviceRegistry::DevicePresence::Unknown);
}

/////////////////////
// Composite cases
TEST_F(AudioDeviceRegistryServerWarningTest, FindCompositeByTokenIdError) {
  auto fake_driver = CreateFakeComposite();
  fake_driver->set_health_state(false);
  auto client = fake_driver->Enable();
  AddDeviceForDetection("test composite", fad::DeviceType::kComposite,
                        fad::DriverClient::WithComposite(std::move(client)));

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 0u);
  ASSERT_EQ(adr_service()->unhealthy_devices().size(), 1u);
  auto token_id = adr_service()->unhealthy_devices().begin()->get()->token_id();

  EXPECT_EQ(adr_service()->FindDeviceByTokenId(token_id).first,
            AudioDeviceRegistry::DevicePresence::Error);
}

TEST_F(AudioDeviceRegistryServerWarningTest, FindCompositeByTokenIdRemoved) {
  auto fake_driver = CreateFakeComposite();
  auto client = fake_driver->Enable();
  AddDeviceForDetection("test composite", fad::DeviceType::kComposite,
                        fad::DriverClient::WithComposite(std::move(client)));

  RunLoopUntilIdle();
  ASSERT_EQ(adr_service()->devices().size(), 1u);
  auto token_id = adr_service()->devices().begin()->get()->token_id();

  fake_driver->DropComposite();
  RunLoopUntilIdle();

  EXPECT_EQ(adr_service()->FindDeviceByTokenId(token_id).first,
            AudioDeviceRegistry::DevicePresence::Unknown);
}

}  // namespace
}  // namespace media_audio
