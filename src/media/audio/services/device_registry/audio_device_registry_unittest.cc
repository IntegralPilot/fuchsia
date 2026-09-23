// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/audio_device_registry.h"

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;

class AudioDeviceRegistryServerTest : public AudioDeviceRegistryServerTestBase {};

TEST_F(AudioDeviceRegistryServerTest, DeviceInitialization) {
  auto fake_codec = CreateFakeCodecOutput();
  auto fake_composite = CreateFakeComposite();

  auto codec_client = fake_codec->Enable();
  auto composite_client = fake_composite->Enable();

  AddDeviceForDetection("test codec", fad::DeviceType::kCodec,
                        fad::DriverClient::WithCodec(std::move(codec_client)));
  AddDeviceForDetection("test composite", fad::DeviceType::kComposite,
                        fad::DriverClient::WithComposite(std::move(composite_client)));

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 2u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
}

TEST_F(AudioDeviceRegistryServerTest, DeviceRemoval) {
  auto fake_codec = CreateFakeCodecInput();
  auto fake_composite = CreateFakeComposite();

  auto codec_client = fake_codec->Enable();
  auto composite_client = fake_composite->Enable();

  AddDeviceForDetection("test codec", fad::DeviceType::kCodec,
                        fad::DriverClient::WithCodec(std::move(codec_client)));
  AddDeviceForDetection("test composite", fad::DeviceType::kComposite,
                        fad::DriverClient::WithComposite(std::move(composite_client)));

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 2u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);

  fake_codec->DropCodec();
  fake_composite->DropComposite();
  RunLoopUntilIdle();

  EXPECT_EQ(adr_service()->devices().size(), 0u);
  EXPECT_EQ(adr_service()->unhealthy_devices().size(), 0u);
}

/////////////////////
// Codec cases
TEST_F(AudioDeviceRegistryServerTest, FindCodecByTokenId) {
  auto fake_driver = CreateFakeCodecNoDirection();

  auto client = fake_driver->Enable();
  AddDeviceForDetection("test codec", fad::DeviceType::kCodec,
                        fad::DriverClient::WithCodec(std::move(client)));

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  auto token_id = adr_service()->devices().begin()->get()->token_id();

  EXPECT_EQ(adr_service()->FindDeviceByTokenId(token_id).first,
            AudioDeviceRegistry::DevicePresence::Active);
}

/////////////////////
// Composite cases
TEST_F(AudioDeviceRegistryServerTest, FindCompositeByTokenId) {
  auto fake_driver = CreateFakeComposite();
  auto client = fidl::ClientEnd<fuchsia_hardware_audio::Composite>(fake_driver->Enable());
  AddDeviceForDetection("test composite", fad::DeviceType::kComposite,
                        fad::DriverClient::WithComposite(std::move(client)));

  RunLoopUntilIdle();
  EXPECT_EQ(adr_service()->devices().size(), 1u);
  auto token_id = adr_service()->devices().begin()->get()->token_id();

  EXPECT_EQ(adr_service()->FindDeviceByTokenId(token_id).first,
            AudioDeviceRegistry::DevicePresence::Active);
}

}  // namespace
}  // namespace media_audio
