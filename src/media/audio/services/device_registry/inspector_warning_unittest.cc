// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector_unittest.h"

using ::inspect::BoolPropertyValue;
using ::inspect::IntPropertyValue;
using ::inspect::StringPropertyValue;
using ::inspect::UintPropertyValue;

namespace fad = fuchsia_audio_device;

namespace media_audio {
namespace {
class InspectorWarningTest : public InspectorTest {
 protected:
  static constexpr std::string kClassName = "InspectorWarningTest";
};

// The relevant fields are `added at` and `failed at` -- located at // root/Devices/[device name]/
TEST_F(InspectorWarningTest, FailedDevice) {
  auto fake_driver = CreateFakeCodecOutput();
  fake_driver->set_health_state(false);
  auto before_added = zx::clock::get_monotonic();
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));
  RunLoopUntilIdle();

  auto hierarchy = GetHierarchy();
  ASSERT_EQ(hierarchy.name(), "root");
  EXPECT_EQ(hierarchy.node().properties().size(), 3u);
  // Eventually we should test these Device detection failure modes as well.
  EXPECT_EQ(hierarchy.node()
                .get_property<UintPropertyValue>(std::string(kDetectionConnectionErrors))
                ->value(),
            0u);
  EXPECT_EQ(
      hierarchy.node().get_property<UintPropertyValue>(std::string(kDetectionOtherErrors))->value(),
      0u);
  EXPECT_EQ(hierarchy.node()
                .get_property<UintPropertyValue>(std::string(kDetectionUnsupportedDevices))
                ->value(),
            0u);

  auto devices_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kDevices; });
  ASSERT_NE(devices_node, hierarchy.children().end());
  ASSERT_TRUE(devices_node->node().properties().empty());
  ASSERT_EQ(devices_node->children().size(), 1u);

  auto device_node = devices_node->children().cbegin();
  EXPECT_GT(device_node->node().properties().size(), 4u);
  EXPECT_GT(device_node->node().get_property<IntPropertyValue>(std::string(kAddedAt))->value(),
            before_added.get());
  EXPECT_GT(device_node->node().get_property<IntPropertyValue>(std::string(kFailedAt))->value(),
            before_added.get());
  EXPECT_FALSE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());
  EXPECT_EQ(
      device_node->node().get_property<StringPropertyValue>(std::string(kDeviceType))->value(),
      "CODEC");
  // Inspect still indicates the DAIs and/or RingBuffers of a failed device.
  EXPECT_FALSE(device_node->children().empty());
}

// The relevant fields are `failed at` and `removed at` -- located at // root/Devices/[device name]/
TEST_F(InspectorWarningTest, FailedThenRemovedDevice) {
  auto fake_driver = CreateFakeCodecOutput();
  fake_driver->set_health_state(false);
  adr_service()->AddDevice(
      Device::Create(adr_service(), dispatcher(), "Test codec name", fad::DeviceType::kCodec,
                     fad::DriverClient::WithCodec(fake_driver->Enable()), kClassName));
  RunLoopUntilIdle();

  // We should see that it failed before it was removed.
  auto before_removed = zx::clock::get_monotonic();
  fake_driver->DropCodec();
  RunLoopUntilIdle();

  auto hierarchy = GetHierarchy();
  ASSERT_EQ(hierarchy.name(), "root");
  EXPECT_EQ(hierarchy.node().properties().size(), 3u);
  EXPECT_EQ(hierarchy.node()
                .get_property<UintPropertyValue>(std::string(kDetectionConnectionErrors))
                ->value(),
            0u);
  EXPECT_EQ(
      hierarchy.node().get_property<UintPropertyValue>(std::string(kDetectionOtherErrors))->value(),
      0u);
  EXPECT_EQ(hierarchy.node()
                .get_property<UintPropertyValue>(std::string(kDetectionUnsupportedDevices))
                ->value(),
            0u);

  auto devices_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kDevices; });
  ASSERT_NE(devices_node, hierarchy.children().end());
  ASSERT_TRUE(devices_node->node().properties().empty());
  ASSERT_EQ(devices_node->children().size(), 1u);

  auto device_node = devices_node->children().cbegin();
  EXPECT_GT(device_node->node().properties().size(), 4u);
  EXPECT_LT(device_node->node().get_property<IntPropertyValue>(std::string(kFailedAt))->value(),
            before_removed.get());
  EXPECT_GT(device_node->node().get_property<IntPropertyValue>(std::string(kRemovedAt))->value(),
            before_removed.get());
  EXPECT_FALSE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());
  // Inspect still indicates the DAIs and/or RingBuffers for a failed+removed device.
  EXPECT_FALSE(device_node->children().empty());
}

}  // namespace
}  // namespace media_audio
