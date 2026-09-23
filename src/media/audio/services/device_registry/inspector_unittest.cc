// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/inspector_unittest.h"

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <lib/inspect/cpp/health.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <algorithm>
#include <set>
#include <string>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"

using ::inspect::BoolPropertyValue;
using ::inspect::IntPropertyValue;
using ::inspect::StringPropertyValue;
using ::inspect::UintPropertyValue;

namespace fad = fuchsia_audio_device;
namespace fhasp = fuchsia_hardware_audio_signalprocessing;

namespace media_audio {

namespace {

const inspect::Hierarchy* GetChild(const inspect::Hierarchy* parent, std::string_view name) {
  if (!parent) {
    return nullptr;
  }
  auto it = std::find_if(parent->children().begin(), parent->children().end(),
                         [name](const inspect::Hierarchy& h) { return h.name() == name; });
  return it == parent->children().end() ? nullptr : &*it;
}

TEST_F(InspectorTest, DefaultValues) {
  auto hierarchy = GetHierarchy();

  auto after_get_hierarchy = zx::clock::get_monotonic();
  ASSERT_EQ(hierarchy.name(), "root");
  EXPECT_EQ(hierarchy.node().properties().size(), 3u);
  // Expect metrics with default values in the root node.
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

  // Expect empty child nodes for Devices and FIDL_servers (with children).
  // Expect fuchsia.inspect.Health to already be in the "starting up" state - this occurs at static
  // initialization time: when audio_device_registry's (or this unittest bin's) main() starts up.
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kDevices; });
  ASSERT_NE(devices_node, hierarchy.children().end());
  ASSERT_TRUE(devices_node->node().properties().empty());
  ASSERT_TRUE(devices_node->children().empty());

  auto fidl_servers_node = GetChild(&hierarchy, kFidlServers);
  ASSERT_NE(fidl_servers_node, nullptr);
  ASSERT_TRUE(fidl_servers_node->node().properties().empty());
  ASSERT_EQ(fidl_servers_node->children().size(), 7u);

  auto registry_servers_node = GetChild(fidl_servers_node, kRegistryServerInstances);
  ASSERT_NE(registry_servers_node, nullptr);
  EXPECT_TRUE(registry_servers_node->node().properties().empty());
  EXPECT_TRUE(registry_servers_node->children().empty());

  auto observer_servers_node = GetChild(fidl_servers_node, kObserverServerInstances);
  ASSERT_NE(observer_servers_node, nullptr);
  EXPECT_TRUE(observer_servers_node->node().properties().empty());
  EXPECT_TRUE(observer_servers_node->children().empty());

  auto control_creator_servers_node = GetChild(fidl_servers_node, kControlCreatorServerInstances);
  ASSERT_NE(control_creator_servers_node, nullptr);
  EXPECT_TRUE(control_creator_servers_node->node().properties().empty());
  EXPECT_TRUE(control_creator_servers_node->children().empty());

  auto control_servers_node = GetChild(fidl_servers_node, kControlServerInstances);
  ASSERT_NE(control_servers_node, nullptr);
  EXPECT_TRUE(control_servers_node->node().properties().empty());
  EXPECT_TRUE(control_servers_node->children().empty());

  auto ring_buffer_servers_node = GetChild(fidl_servers_node, kRingBufferServerInstances);
  ASSERT_NE(ring_buffer_servers_node, nullptr);
  EXPECT_TRUE(ring_buffer_servers_node->node().properties().empty());
  EXPECT_TRUE(ring_buffer_servers_node->children().empty());

  auto packet_stream_servers_node = GetChild(fidl_servers_node, kPacketStreamServerInstances);
  ASSERT_NE(packet_stream_servers_node, nullptr);
  EXPECT_TRUE(packet_stream_servers_node->node().properties().empty());
  EXPECT_TRUE(packet_stream_servers_node->children().empty());

  auto provider_servers_node = GetChild(fidl_servers_node, kProviderServerInstances);
  ASSERT_NE(provider_servers_node, nullptr);
  EXPECT_TRUE(provider_servers_node->node().properties().empty());
  EXPECT_TRUE(provider_servers_node->children().empty());

  auto health_node = GetChild(&hierarchy, inspect::kHealthNodeName);
  ASSERT_NE(health_node, nullptr);
  EXPECT_EQ(health_node->node().properties().size(), 2u);
  EXPECT_EQ(health_node->node().get_property<StringPropertyValue>("status")->value(),
            inspect::kHealthStartingUp);
  EXPECT_LT(health_node->node().get_property<IntPropertyValue>(inspect::kStartTimestamp)->value(),
            after_get_hierarchy.get());
  EXPECT_TRUE(health_node->children().empty());
}

// Relevant fields: `start_timestamp_nanos`, `status` -- found at root/fuchsia.inspect.Health/
TEST_F(InspectorTest, ComponentHealthy) {
  Inspector::Singleton()->RecordHealthOk();
  auto after_health_ok = zx::clock::get_monotonic();

  auto hierarchy = GetHierarchy();
  auto health_node = std::find_if(
      hierarchy.children().begin(), hierarchy.children().end(),
      [](const inspect::Hierarchy& h) { return h.name() == inspect::kHealthNodeName; });
  ASSERT_NE(health_node, hierarchy.children().end());
  EXPECT_EQ(health_node->node().properties().size(), 2u);
  EXPECT_EQ(health_node->node().get_property<StringPropertyValue>("status")->value(),
            inspect::kHealthOk);
  EXPECT_LT(health_node->node().get_property<IntPropertyValue>(inspect::kStartTimestamp)->value(),
            after_health_ok.get());
  EXPECT_TRUE(health_node->children().empty());
}

// Relevant fields: `start_timestamp_nanos`, `status`, `message` -- at root/fuchsia.inspect.Health/
TEST_F(InspectorTest, ComponentUnhealthy) {
  constexpr std::string kUnhealthyMessasge{"Unhealthy message"};

  Inspector::Singleton()->RecordUnhealthy(kUnhealthyMessasge);
  auto after_unhealthy = zx::clock::get_monotonic();

  auto hierarchy = GetHierarchy();
  auto health_node = std::find_if(
      hierarchy.children().begin(), hierarchy.children().end(),
      [](const inspect::Hierarchy& h) { return h.name() == inspect::kHealthNodeName; });
  ASSERT_NE(health_node, hierarchy.children().end());
  EXPECT_EQ(health_node->node().properties().size(), 3u);
  EXPECT_EQ(health_node->node().get_property<StringPropertyValue>("status")->value(),
            inspect::kHealthUnhealthy);
  EXPECT_LT(health_node->node().get_property<IntPropertyValue>(inspect::kStartTimestamp)->value(),
            after_unhealthy.get());
  EXPECT_EQ(health_node->node().get_property<StringPropertyValue>("message")->value(),
            kUnhealthyMessasge);
  EXPECT_TRUE(health_node->children().empty());
}

// Relevant fields: `added_at`, `token_id` and many others -- found at root/Devices/[device name]/
TEST_F(InspectorTest, DetectedDevice) {
  auto before_add = zx::clock::get_monotonic();
  set_fake_driver(CreateAndAddFakeComposite());

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

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());
  ASSERT_TRUE(devices_node->node().properties().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  EXPECT_GE(device_node->node().properties().size(), 8u);
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  ASSERT_TRUE(device_node->node().get_property<IntPropertyValue>(std::string(kAddedAt)));
  EXPECT_GT(device_node->node().get_property<IntPropertyValue>(std::string(kAddedAt))->value(),
            before_add.get());
  ASSERT_TRUE(device_node->node().get_property<UintPropertyValue>(std::string(kTokenId)));
  EXPECT_EQ(device_node->node().get_property<UintPropertyValue>(std::string(kTokenId))->value(),
            0u);
  ASSERT_TRUE(device_node->node().get_property<StringPropertyValue>(std::string(kClockDomain)));
  EXPECT_EQ(
      device_node->node().get_property<StringPropertyValue>(std::string(kClockDomain))->value(),
      FakeComposite::kDefaultClockDomainStr);

  ASSERT_TRUE(device_node->node().get_property<StringPropertyValue>(std::string(kManufacturer)));
  EXPECT_EQ(
      device_node->node().get_property<StringPropertyValue>(std::string(kManufacturer))->value(),
      FakeComposite::kDefaultManufacturer);
  ASSERT_TRUE(device_node->node().get_property<StringPropertyValue>(std::string(kProduct)));
  EXPECT_EQ(device_node->node().get_property<StringPropertyValue>(std::string(kProduct))->value(),
            FakeComposite::kDefaultProduct);
  ASSERT_TRUE(device_node->node().get_property<StringPropertyValue>(std::string(kDeviceType)));
  EXPECT_EQ(
      device_node->node().get_property<StringPropertyValue>(std::string(kDeviceType))->value(),
      "COMPOSITE");
  ASSERT_TRUE(device_node->node().get_property<StringPropertyValue>(std::string(kUniqueId)));
  EXPECT_EQ(device_node->node().get_property<StringPropertyValue>(std::string(kUniqueId))->value(),
            UidToString(FakeComposite::kDefaultUniqueInstanceId));
  ASSERT_EQ(device_node->children().size(), 5u);

  auto ring_buffers_node = GetChild(device_node, kRingBuffers);
  ASSERT_NE(ring_buffers_node, nullptr);
  EXPECT_TRUE(ring_buffers_node->node().properties().empty());
  ASSERT_EQ(ring_buffers_node->children().size(), 2u);

  auto first_rb_element_id = ring_buffers_node->children()
                                 .cbegin()
                                 ->node()
                                 .get_property<UintPropertyValue>(std::string(kElementId))
                                 ->value();
  auto last_rb_element_id = ring_buffers_node->children()
                                .crbegin()
                                ->node()
                                .get_property<UintPropertyValue>(std::string(kElementId))
                                ->value();
  EXPECT_TRUE((first_rb_element_id == FakeComposite::kSourceRbElementId &&
               last_rb_element_id == FakeComposite::kDestRbElementId) ||
              (first_rb_element_id == FakeComposite::kDestRbElementId &&
               last_rb_element_id == FakeComposite::kSourceRbElementId));

  auto ps_elements_node = GetChild(device_node, kPacketStreams);
  ASSERT_NE(ps_elements_node, nullptr);
  EXPECT_TRUE(ps_elements_node->node().properties().empty());
  ASSERT_EQ(ps_elements_node->children().size(), 3u);

  std::set<uint64_t> ps_ids;
  for (const auto& child : ps_elements_node->children()) {
    auto id = child.node().get_property<UintPropertyValue>(std::string(kElementId))->value();
    auto description =
        child.node().get_property<StringPropertyValue>(std::string(kDescription))->value();
    if (id == FakeComposite::kSourcePsElementId) {
      EXPECT_EQ(description, FakeComposite::kSourcePsElementDescription);
    } else if (id == FakeComposite::kDestPsElementId) {
      EXPECT_EQ(description, FakeComposite::kDestPsElementDescription);
    } else if (id == FakeComposite::kSourceDualSupportPsElementId) {
      EXPECT_EQ(description, FakeComposite::kSourceDualSupportPsElementDescription);
    } else {
      ADD_FAILURE() << "Unexpected ps element_id " << id;
    }
    ps_ids.insert(id);
  }
  EXPECT_EQ(ps_ids.size(), 3u);
  EXPECT_EQ(ps_ids.count(FakeComposite::kSourcePsElementId), 1u);
  EXPECT_EQ(ps_ids.count(FakeComposite::kDestPsElementId), 1u);
  EXPECT_EQ(ps_ids.count(FakeComposite::kSourceDualSupportPsElementId), 1u);
}

// Relevant field: `removed_at` -- found at // root/Devices/[device name]/
TEST_F(InspectorTest, RemovedDevice) {
  auto before_add = zx::clock::get_monotonic();
  set_fake_driver(CreateAndAddFakeComposite());

  auto before_drop = zx::clock::get_monotonic();
  fake_driver()->DropComposite();
  RunLoopUntilIdle();

  auto hierarchy = GetHierarchy();
  ASSERT_EQ(hierarchy.name(), "root");
  EXPECT_EQ(hierarchy.node().properties().size(), 3u);
  ASSERT_TRUE(
      hierarchy.node().get_property<UintPropertyValue>(std::string(kDetectionConnectionErrors)));
  EXPECT_EQ(hierarchy.node()
                .get_property<UintPropertyValue>(std::string(kDetectionConnectionErrors))
                ->value(),
            0u);
  ASSERT_TRUE(hierarchy.node().get_property<UintPropertyValue>(std::string(kDetectionOtherErrors)));
  EXPECT_EQ(
      hierarchy.node().get_property<UintPropertyValue>(std::string(kDetectionOtherErrors))->value(),
      0u);
  ASSERT_TRUE(
      hierarchy.node().get_property<UintPropertyValue>(std::string(kDetectionUnsupportedDevices)));
  EXPECT_EQ(hierarchy.node()
                .get_property<UintPropertyValue>(std::string(kDetectionUnsupportedDevices))
                ->value(),
            0u);

  auto devices_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kDevices; });
  ASSERT_NE(devices_node, hierarchy.children().end());
  ASSERT_TRUE(devices_node->node().properties().empty());
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  EXPECT_GT(device_node->node().properties().size(), 3u);
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  ASSERT_TRUE(device_node->node().get_property<IntPropertyValue>(std::string(kAddedAt)));
  EXPECT_GT(device_node->node().get_property<IntPropertyValue>(std::string(kAddedAt))->value(),
            before_add.get());
  ASSERT_TRUE(device_node->node().get_property<IntPropertyValue>(std::string(kRemovedAt)));
  EXPECT_GT(device_node->node().get_property<IntPropertyValue>(std::string(kRemovedAt))->value(),
            before_drop.get());
  // This was removed after a normal add, so there should be a complete set of child nodes
  EXPECT_FALSE(device_node->children().empty());
}

// Relevant fields: `created_at`, `destroyed_at` -- at root/FIDL_servers/RegistryServer_instances/0/
// We don't test kDestroyedAt because of unpredictable cleanup timing.
TEST_F(InspectorServerTest, CreateRegistryServer) {
  auto before_create = zx::clock::get_monotonic();
  auto registry = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node = GetChild(&hierarchy, kFidlServers);
  ASSERT_NE(fidl_servers_node, nullptr);
  auto registry_servers_node = GetChild(fidl_servers_node, kRegistryServerInstances);
  ASSERT_NE(registry_servers_node, nullptr);
  ASSERT_TRUE(registry_servers_node->node().properties().empty());
  ASSERT_FALSE(registry_servers_node->children().empty());

  auto registry_server_node = &registry_servers_node->children().front();
  EXPECT_EQ(registry_server_node->name(), "0");
  ASSERT_FALSE(registry_server_node->node().properties().empty());
  EXPECT_EQ(registry_server_node->node().properties().size(), 1u);
  ASSERT_TRUE(registry_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt)));
  EXPECT_GT(
      registry_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt))->value(),
      before_create.get());
  EXPECT_TRUE(registry_server_node->children().empty());
}

// Relevant fields: `created_at`, `destroyed_at` -- at root/FIDL_servers/ObserverServer_instances/0/
// We don't test kDestroyedAt because of unpredictable cleanup timing.
TEST_F(InspectorServerTest, CreateObserverServer) {
  set_fake_driver(CreateAndAddFakeComposite());
  auto registry = CreateTestRegistryServer();
  std::optional<TokenId> added_device_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_EQ(RegistryServer::count(), 1u);
  ASSERT_TRUE(added_device_id.has_value());

  auto before_create = zx::clock::get_monotonic();
  auto observer = CreateTestObserverServer(*adr_service()->devices().begin());
  ASSERT_EQ(ObserverServer::count(), 1u);

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node = GetChild(&hierarchy, kFidlServers);
  ASSERT_NE(fidl_servers_node, nullptr);
  auto observer_servers_node = GetChild(fidl_servers_node, kObserverServerInstances);
  ASSERT_NE(observer_servers_node, nullptr);
  ASSERT_TRUE(observer_servers_node->node().properties().empty());
  ASSERT_FALSE(observer_servers_node->children().empty());

  auto observer_server_node = &observer_servers_node->children().front();
  EXPECT_EQ(observer_server_node->name(), "0");
  ASSERT_FALSE(observer_server_node->node().properties().empty());
  EXPECT_EQ(observer_server_node->node().properties().size(), 1u);
  ASSERT_TRUE(observer_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt)));
  EXPECT_GT(
      observer_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt))->value(),
      before_create.get());
  EXPECT_TRUE(observer_server_node->children().empty());
}

// Relevant: `created_at`, `destroyed_at` at root/FIDL_servers/ControlCreatorServer_instances/0/
// We don't test kDestroyedAt because of unpredictable cleanup timing.
TEST_F(InspectorServerTest, CreateControlCreatorServer) {
  auto before_create = zx::clock::get_monotonic();
  auto control_creator = CreateTestControlCreatorServer();
  ASSERT_EQ(ControlCreatorServer::count(), 1u);

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kFidlServers; });
  ASSERT_NE(fidl_servers_node, hierarchy.children().end());
  ASSERT_TRUE(fidl_servers_node->node().properties().empty());
  ASSERT_FALSE(fidl_servers_node->children().empty());

  auto control_creator_servers_node = std::find_if(
      fidl_servers_node->children().begin(), fidl_servers_node->children().end(),
      [](const inspect::Hierarchy& h) { return h.name() == kControlCreatorServerInstances; });
  ASSERT_NE(control_creator_servers_node, fidl_servers_node->children().end());
  ASSERT_TRUE(control_creator_servers_node->node().properties().empty());
  ASSERT_FALSE(control_creator_servers_node->children().empty());

  auto control_creator_server_node = control_creator_servers_node->children().cbegin();
  EXPECT_EQ(control_creator_server_node->name(), "0");
  ASSERT_FALSE(control_creator_server_node->node().properties().empty());
  EXPECT_EQ(control_creator_server_node->node().properties().size(), 1u);
  ASSERT_TRUE(
      control_creator_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt)));
  EXPECT_GT(control_creator_server_node->node()
                .get_property<IntPropertyValue>(std::string(kCreatedAt))
                ->value(),
            before_create.get());
  EXPECT_TRUE(control_creator_server_node->children().empty());
}

// Relevant fields: `created_at`, `destroyed_at` -- at root/FIDL_servers/ControlServer_instances/0/
// We don't test kDestroyedAt because of unpredictable cleanup timing.
TEST_F(InspectorServerTest, CreateControlServer) {
  set_fake_driver(CreateFakeComposite());
  auto before_create = zx::clock::get_monotonic();
  CreateControlledDevice();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kFidlServers; });
  ASSERT_NE(fidl_servers_node, hierarchy.children().end());
  ASSERT_TRUE(fidl_servers_node->node().properties().empty());
  ASSERT_FALSE(fidl_servers_node->children().empty());

  auto control_servers_node =
      std::find_if(fidl_servers_node->children().begin(), fidl_servers_node->children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kControlServerInstances; });
  ASSERT_NE(control_servers_node, fidl_servers_node->children().end());
  ASSERT_TRUE(control_servers_node->node().properties().empty());
  ASSERT_FALSE(control_servers_node->children().empty());

  auto control_server_node = control_servers_node->children().cbegin();
  EXPECT_EQ(control_server_node->name(), "0");
  ASSERT_FALSE(control_server_node->node().properties().empty());
  EXPECT_EQ(control_server_node->node().properties().size(), 1u);
  ASSERT_TRUE(control_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt)));
  EXPECT_GT(
      control_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt))->value(),
      before_create.get());
  EXPECT_TRUE(control_server_node->children().empty());
}

// Relevant fields: `created_at`, `destroyed_at` at root/FIDL_servers/RingBufferServer_instances/0/
// We don't test kDestroyedAt because of unpredictable cleanup timing.
TEST_F(InspectorServerTest, CreateRingBufferServer) {
  set_fake_driver(CreateFakeComposite());
  auto element_id = FakeComposite::kMaxRingBufferElementId;
  fake_driver()->ReserveRingBufferSize(element_id, 8192);

  CreateControlledDevice();

  auto [ring_buffer_client_end, ring_buffer_server_end] =
      CreateNaturalAsyncClientOrDie<fad::RingBuffer>();
  auto ring_buffer_client = fidl::Client<fad::RingBuffer>(
      std::move(ring_buffer_client_end), dispatcher(), ring_buffer_fidl_handler().get());
  auto before_create = zx::clock::get_monotonic();
  auto ring_buffer = adr_service()->CreateRingBufferServer(std::move(ring_buffer_server_end),
                                                           control()->server_ptr(),
                                                           device(),  // device_to_control,
                                                           0);
  RunLoopUntilIdle();
  EXPECT_TRUE(ring_buffer_client.is_valid());

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kFidlServers; });
  ASSERT_NE(fidl_servers_node, hierarchy.children().end());
  ASSERT_TRUE(fidl_servers_node->node().properties().empty());
  ASSERT_FALSE(fidl_servers_node->children().empty());

  auto ring_buffer_servers_node = std::find_if(
      fidl_servers_node->children().begin(), fidl_servers_node->children().end(),
      [](const inspect::Hierarchy& h) { return h.name() == kRingBufferServerInstances; });
  ASSERT_NE(ring_buffer_servers_node, fidl_servers_node->children().end());
  ASSERT_TRUE(ring_buffer_servers_node->node().properties().empty());
  ASSERT_FALSE(ring_buffer_servers_node->children().empty());

  auto ring_buffer_server_node = ring_buffer_servers_node->children().cbegin();
  EXPECT_EQ(ring_buffer_server_node->name(), "0");
  ASSERT_FALSE(ring_buffer_server_node->node().properties().empty());
  EXPECT_EQ(ring_buffer_server_node->node().properties().size(), 1u);
  ASSERT_TRUE(
      ring_buffer_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt)));
  EXPECT_GT(ring_buffer_server_node->node()
                .get_property<IntPropertyValue>(std::string(kCreatedAt))
                ->value(),
            before_create.get());
  EXPECT_TRUE(ring_buffer_server_node->children().empty());
}

// Test `created_at` at root/FIDL_servers/PacketStreamServer_instances/0/
// We don't test kDestroyedAt because of unpredictable cleanup timing.
TEST_F(InspectorServerTest, CreatePacketStreamServer) {
  set_fake_driver(CreateFakeComposite());
  auto element_id = FakeComposite::kMaxPacketStreamElementId;

  CreateControlledDevice();

  auto [packet_stream_client_end, packet_stream_server_end] =
      CreateNaturalAsyncClientOrDie<fuchsia_audio_device::PacketStream>();
  auto packet_stream_client = fidl::Client<fuchsia_audio_device::PacketStream>(
      std::move(packet_stream_client_end), dispatcher(), packet_stream_fidl_handler().get());
  auto before_create = zx::clock::get_monotonic();

  auto packet_stream = adr_service()->CreatePacketStreamServer(std::move(packet_stream_server_end),
                                                               control()->server_ptr(),
                                                               device(),  // device_to_control,
                                                               element_id);
  RunLoopUntilIdle();
  EXPECT_TRUE(packet_stream_client.is_valid());

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node =
      std::find_if(hierarchy.children().begin(), hierarchy.children().end(),
                   [](const inspect::Hierarchy& h) { return h.name() == kFidlServers; });
  ASSERT_NE(fidl_servers_node, hierarchy.children().end());

  auto packet_stream_servers_node = std::find_if(
      fidl_servers_node->children().begin(), fidl_servers_node->children().end(),
      [](const inspect::Hierarchy& h) { return h.name() == kPacketStreamServerInstances; });
  ASSERT_NE(packet_stream_servers_node, fidl_servers_node->children().end());
  ASSERT_TRUE(packet_stream_servers_node->node().properties().empty());
  ASSERT_FALSE(packet_stream_servers_node->children().empty());

  auto packet_stream_server_node = packet_stream_servers_node->children().cbegin();
  EXPECT_EQ(packet_stream_server_node->name(), "0");
  ASSERT_FALSE(packet_stream_server_node->node().properties().empty());
  EXPECT_EQ(packet_stream_server_node->node().properties().size(), 1u);
  ASSERT_TRUE(
      packet_stream_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt)));
  EXPECT_GT(packet_stream_server_node->node()
                .get_property<IntPropertyValue>(std::string(kCreatedAt))
                ->value(),
            before_create.get());
  EXPECT_TRUE(packet_stream_server_node->children().empty());
}

// Relevant fields: `created_at`, `destroyed_at` -- at root/FIDL_servers/ProviderServer_instances/0/
// We don't test kDestroyedAt because of unpredictable cleanup timing.
TEST_F(InspectorServerTest, CreateProviderServer) {
  auto before_create = zx::clock::get_monotonic();
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node = GetChild(&hierarchy, kFidlServers);
  ASSERT_NE(fidl_servers_node, nullptr);
  auto provider_servers_node = GetChild(fidl_servers_node, kProviderServerInstances);
  ASSERT_NE(provider_servers_node, nullptr);
  ASSERT_TRUE(provider_servers_node->node().properties().empty());
  ASSERT_FALSE(provider_servers_node->children().empty());

  auto provider_node = &provider_servers_node->children().front();
  ASSERT_EQ(provider_node->name(), "0");
  ASSERT_FALSE(provider_node->node().properties().empty());
  ASSERT_EQ(provider_node->node().properties().size(), 1u);
  ASSERT_TRUE(provider_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt)));
  ASSERT_GT(provider_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt))->value(),
            before_create.get());
  ASSERT_EQ(provider_node->children().size(), 1u);

  auto added_devices_node = &provider_node->children().front();
  EXPECT_EQ(added_devices_node->name(), std::string(kAddedDevices));
  EXPECT_TRUE(added_devices_node->node().properties().empty());
  EXPECT_TRUE(added_devices_node->children().empty());
}

// Verify that multiple instances are tracked separately: check instance name and `created_at`.
TEST_F(InspectorServerTest, CreateMultipleServerInstances) {
  auto registry0 = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 1u);
  auto after_create0 = zx::clock::get_monotonic();

  auto registry1 = CreateTestRegistryServer();
  ASSERT_EQ(RegistryServer::count(), 2u);

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node = GetChild(&hierarchy, kFidlServers);
  ASSERT_NE(fidl_servers_node, nullptr);
  auto registry_servers_node = GetChild(fidl_servers_node, kRegistryServerInstances);
  ASSERT_NE(registry_servers_node, nullptr);
  ASSERT_TRUE(registry_servers_node->node().properties().empty());
  ASSERT_FALSE(registry_servers_node->children().empty());

  auto registry_server0_node = &registry_servers_node->children().front();
  ASSERT_EQ(registry_server0_node->name(), "0");
  ASSERT_EQ(registry_server0_node->node().properties().size(), 1u);
  ASSERT_LT(registry_server0_node->node()
                .get_property<IntPropertyValue>(std::string(kCreatedAt))
                ->value(),
            after_create0.get());
  ASSERT_TRUE(registry_server0_node->children().empty());

  auto registry_server1_node = &registry_servers_node->children().back();
  ASSERT_EQ(registry_server1_node->name(), "1");
  ASSERT_EQ(registry_server1_node->node().properties().size(), 1u);
  ASSERT_GT(registry_server1_node->node()
                .get_property<IntPropertyValue>(std::string(kCreatedAt))
                ->value(),
            after_create0.get());
  ASSERT_TRUE(registry_server1_node->children().empty());
}

// Relevant fields: `added_at` and `type` (as well as [device name]) -- found at
// root/FIDL_servers/ProviderServer_instances/0/Added_devices/[device name]/
// We add two devices, to validate that these can be tracked separately.
TEST_F(InspectorServerTest, ProviderAddedDevice) {
  auto before_create = zx::clock::get_monotonic();
  auto provider = CreateTestProviderServer();
  ASSERT_EQ(ProviderServer::count(), 1u);

  auto fake_codec = CreateFakeCodecInput();
  auto received_callback = false;
  auto before_add_codec = zx::clock::get_monotonic();
  provider->client()
      ->AddDevice({{
          .device_name = "Test codec",
          .device_type = fad::DeviceType::kCodec,
          .driver_client = fad::DriverClient::WithCodec(fake_codec->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);

  auto fake_composite = CreateFakeComposite();
  received_callback = false;
  auto before_add_composite = zx::clock::get_monotonic();
  provider->client()
      ->AddDevice({{
          .device_name = "Test composite",
          .device_type = fad::DeviceType::kComposite,
          .driver_client = fad::DriverClient::WithComposite(fake_composite->Enable()),
      }})
      .Then([&received_callback](fidl::Result<fad::Provider::AddDevice>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto fidl_servers_node = GetChild(&hierarchy, kFidlServers);
  ASSERT_NE(fidl_servers_node, nullptr);
  auto provider_servers_node = GetChild(fidl_servers_node, kProviderServerInstances);
  ASSERT_NE(provider_servers_node, nullptr);
  ASSERT_TRUE(provider_servers_node->node().properties().empty());
  ASSERT_FALSE(provider_servers_node->children().empty());

  auto provider_server_node = &provider_servers_node->children().front();
  ASSERT_EQ(provider_server_node->name(), "0");
  ASSERT_EQ(provider_server_node->node().properties().size(), 1u);
  ASSERT_GT(
      provider_server_node->node().get_property<IntPropertyValue>(std::string(kCreatedAt))->value(),
      before_create.get());
  ASSERT_FALSE(provider_server_node->children().empty());

  auto added_devices_node = GetChild(provider_server_node, kAddedDevices);
  ASSERT_NE(added_devices_node, nullptr);
  ASSERT_TRUE(added_devices_node->node().properties().empty());
  ASSERT_FALSE(added_devices_node->children().empty());

  auto first_device = &added_devices_node->children().front();
  EXPECT_EQ(first_device->name(), "Test codec");
  EXPECT_EQ(first_device->node().properties().size(), 5u);
  EXPECT_GT(first_device->node().get_property<IntPropertyValue>(std::string(kAddedAt))->value(),
            before_add_codec.get());
  EXPECT_EQ(
      first_device->node().get_property<StringPropertyValue>(std::string(kDeviceType))->value(),
      "CODEC");
  EXPECT_TRUE(first_device->children().empty());

  auto last_device = &added_devices_node->children().back();
  EXPECT_EQ(last_device->name(), "Test composite");
  EXPECT_EQ(last_device->node().properties().size(), 5u);
  EXPECT_EQ(
      last_device->node().get_property<StringPropertyValue>(std::string(kDeviceType))->value(),
      "COMPOSITE");
  EXPECT_GT(last_device->node().get_property<IntPropertyValue>(std::string(kAddedAt))->value(),
            before_add_composite.get());
  EXPECT_TRUE(last_device->children().empty());
}

// Validate the overall SupportedFormatSets for each DAI element. Schema is as follows:
// Devices
//   12345678
//     DAIs
//       0:
//         description = 'bluetooth_dai'
//         element_id = 2
//         supported_format_sets:
//           dai_format_set_0:
//             bits_per_frame = [16, 32]
//             bits_per_sample = [16, 20]
//             channel_count = [1, 2]
//             frames_per_second = [44100, 48000]
//             frame_format = ["FrameFormatStandard::I2S", "FrameFormatStandard::NONE"]
//             sample_format = ["PCM SIGNED", "PCM FLOAT"]
TEST_F(InspectorDaiTest, SupportedDaiFormats) {
  // Boot up the device and check each DAI element's SupportedDaiFormats
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto dais_node = GetChild(device_node, kDAIs);
  ASSERT_NE(dais_node, nullptr);
  ASSERT_FALSE(dais_node->children().empty());
  ASSERT_LE(dais_node->children().size(), fuchsia_audio_device::kMaxProcessingElementCount);

  auto dai_node = &dais_node->children().front();
  ASSERT_EQ(dai_node->name(), "0");
  ASSERT_FALSE(dai_node->children().empty());

  auto dai_format_sets_node = GetChild(dai_node, kSupportedFormats);
  ASSERT_NE(dai_format_sets_node, nullptr);
  ASSERT_EQ(dai_format_sets_node->name(), kSupportedFormats);
  ASSERT_FALSE(dai_format_sets_node->children().empty());
  EXPECT_LE(dai_format_sets_node->children().size(), fuchsia_audio_device::kMaxDaiFormatCount);

  auto dai_format_set_node = &dai_format_sets_node->children().front();
  ASSERT_EQ(dai_format_set_node->name(), "dai_format_set_0");
  EXPECT_TRUE(dai_format_set_node->children().empty());
  EXPECT_EQ(dai_format_set_node->node().properties().size(), 6u);

  const auto& bits_per_slot = dai_format_set_node->node()
                                  .get_property<inspect::UintArrayValue>(std::string(kBitsPerFrame))
                                  ->value();
  EXPECT_FALSE(bits_per_slot.empty());
  EXPECT_LE(bits_per_slot.size(), fuchsia_hardware_audio::kMaxCountDaiSupportedBitsPerSlot);
  for (auto& bits : bits_per_slot) {
    EXPECT_GT(bits, 0u);
    EXPECT_LE(bits, 255u);
  }

  const auto& bits_per_sample =
      dai_format_set_node->node()
          .get_property<inspect::UintArrayValue>(std::string(kBitsPerSample))
          ->value();
  EXPECT_FALSE(bits_per_sample.empty());
  EXPECT_LE(bits_per_sample.size(), fuchsia_hardware_audio::kMaxCountDaiSupportedBitsPerSample);
  for (auto& bits : bits_per_sample) {
    EXPECT_GT(bits, 0u);
    EXPECT_LE(bits, 255u);
  }

  const auto& channel_counts =
      dai_format_set_node->node()
          .get_property<inspect::UintArrayValue>(std::string(kChannelCount))
          ->value();
  EXPECT_FALSE(channel_counts.empty());
  EXPECT_LE(channel_counts.size(), fuchsia_hardware_audio::kMaxCountDaiSupportedNumberOfChannels);
  for (auto& channel_count : channel_counts) {
    EXPECT_GT(channel_count, 0u);
  }

  const auto& rates = dai_format_set_node->node()
                          .get_property<inspect::UintArrayValue>(std::string(kFramesPerSecond))
                          ->value();
  EXPECT_FALSE(rates.empty());
  EXPECT_LE(rates.size(), fuchsia_hardware_audio::kMaxCountDaiSupportedRates);
  for (auto& rate : rates) {
    EXPECT_GT(rate, 0u);
  }

  const auto& frame_formats =
      dai_format_set_node->node()
          .get_property<inspect::StringArrayValue>(std::string(kFrameFormat))
          ->value();
  EXPECT_FALSE(frame_formats.empty());
  EXPECT_LE(frame_formats.size(), fuchsia_hardware_audio::kMaxCountDaiSupportedFrameFormats);
  for (auto& format : frame_formats) {
    EXPECT_FALSE(format.empty());
  }

  const auto& sample_formats =
      dai_format_set_node->node()
          .get_property<inspect::StringArrayValue>(std::string(kSampleFormat))
          ->value();
  EXPECT_FALSE(sample_formats.empty());
  EXPECT_LE(sample_formats.size(), fuchsia_hardware_audio::kMaxCountDaiSupportedSampleFormats);
  for (auto& format : sample_formats) {
    EXPECT_FALSE(format.empty());
  }
}

// Relevant fields: `channel_count`, `channels_to_use_bitmask`, `sample_format`, `frame_format`,
// `bits_per_frame`, `bits_per_sample` -- found at root/Devices/[device name]/DAIs/0.
TEST_F(InspectorDaiTest, SetDaiFormat) {
  set_fake_driver(CreateFakeComposite());
  auto element_id = FakeComposite::kSourceDaiElementId;

  CreateControlledDevice();

  bool received_callback = false;
  control()
      ->client()
      ->SetDaiFormat({{
          .element_id = element_id,
          .dai_format = FakeComposite::kDefaultDaiFormat,
      }})
      .Then(
          [&received_callback](fidl::Result<fuchsia_audio_device::Control::SetDaiFormat>& result) {
            received_callback = true;
            ASSERT_TRUE(result.is_ok()) << result.error_value();
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(control()->client().is_valid());

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto dais_node = GetChild(device_node, kDAIs);
  ASSERT_NE(dais_node, nullptr) << "No DAI elements node found";
  ASSERT_FALSE(dais_node->children().empty()) << "No DAI element children";

  auto dai_node = &dais_node->children().front();
  ASSERT_EQ(dai_node->name(), "0");
  ASSERT_EQ(dai_node->node().properties().size(), 2u);
  ASSERT_EQ(dai_node->node().get_property<UintPropertyValue>(std::string(kElementId))->value(),
            element_id);
  ASSERT_FALSE(dai_node->children().empty());

  auto format_node = GetChild(dai_node, kFormatProps);
  ASSERT_NE(format_node, nullptr);

  EXPECT_EQ(format_node->node().name(), kFormatProps);
  EXPECT_TRUE(format_node->children().empty());

  ASSERT_FALSE(format_node->node().properties().empty());
  EXPECT_EQ(format_node->node().properties().size(), 7u);

  ASSERT_TRUE(format_node->node().get_property<UintPropertyValue>(std::string(kBitsPerFrame)))
      << "'" << kBitsPerFrame << "' property not found";
  EXPECT_EQ(
      format_node->node().get_property<UintPropertyValue>(std::string(kBitsPerFrame))->value(),
      FakeComposite::kDefaultDaiFormat.bits_per_slot());

  ASSERT_TRUE(format_node->node().get_property<UintPropertyValue>(std::string(kBitsPerSample)))
      << "'" << kBitsPerSample << "' property not found";
  EXPECT_EQ(
      format_node->node().get_property<UintPropertyValue>(std::string(kBitsPerSample))->value(),
      FakeComposite::kDefaultDaiFormat.bits_per_sample());

  ASSERT_TRUE(format_node->node().get_property<UintPropertyValue>(std::string(kChannelCount)))
      << "'" << kChannelCount << "' property not found";
  EXPECT_EQ(
      format_node->node().get_property<UintPropertyValue>(std::string(kChannelCount))->value(),
      FakeComposite::kDefaultDaiFormat.number_of_channels());

  ASSERT_TRUE(format_node->node().get_property<UintPropertyValue>(std::string(kChannelBitmask)))
      << "'" << kChannelBitmask << "' property not found";
  EXPECT_EQ(
      format_node->node().get_property<UintPropertyValue>(std::string(kChannelBitmask))->value(),
      FakeComposite::kDefaultDaiFormat.channels_to_use_bitmask());

  ASSERT_TRUE(format_node->node().get_property<UintPropertyValue>(std::string(kFramesPerSecond)))
      << "'" << kFramesPerSecond << "' property not found";
  EXPECT_EQ(
      format_node->node().get_property<UintPropertyValue>(std::string(kFramesPerSecond))->value(),
      FakeComposite::kDefaultDaiFormat.frame_rate());

  ASSERT_TRUE(format_node->node().get_property<StringPropertyValue>(std::string(kFrameFormat)))
      << "'" << kFrameFormat << "' property not found";
  std::ostringstream frame_fmt_stream;
  frame_fmt_stream << FakeComposite::kDefaultDaiFrameFormat;
  EXPECT_EQ(
      format_node->node().get_property<StringPropertyValue>(std::string(kFrameFormat))->value(),
      frame_fmt_stream.str());

  ASSERT_TRUE(format_node->node().get_property<StringPropertyValue>(std::string(kSampleFormat)))
      << "'" << kSampleFormat << "' property not found";
  std::ostringstream sample_fmt_stream;
  sample_fmt_stream << FakeComposite::kDefaultDaiSampleFormat;
  EXPECT_EQ(
      format_node->node().get_property<StringPropertyValue>(std::string(kSampleFormat))->value(),
      sample_fmt_stream.str());
}

// Validate the overall Topologies list. Schema is as follows:
//  Devices:
//    12345678:
//      Topologies:
//        0:
//          topology_id = 1
//          edge_pairs:
//            0:
//              from_element_id = 1
//              to_element_id = 2
//            1:
//              from_element_id = 2
//              to_element_id = 3
//        1:
//          topology_id = 0
//          edge_pairs:
//            0:
//              from_element_id = 2
//              to_element_id = 1
TEST_F(InspectorTest, Topologies) {
  // Boot up the device and check the topologies
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto topologies_node = GetChild(device_node, kTopologies);
  ASSERT_NE(topologies_node, nullptr);
  ASSERT_FALSE(topologies_node->children().empty());
  ASSERT_LE(topologies_node->children().size(), fhasp::kMaxCountTopologies);

  for (size_t i = 0; i < topologies_node->children().size(); i++) {
    auto topology_node = &topologies_node->children().at(i);
    EXPECT_EQ(topology_node->name(), std::to_string(i));
    EXPECT_EQ(topology_node->node().properties().size(), 1u);
    ASSERT_TRUE(
        topology_node->node().get_property<inspect::UintPropertyValue>(std::string(kTopologyId)));
    EXPECT_GE(topology_node->node()
                  .get_property<inspect::UintPropertyValue>(std::string(kTopologyId))
                  ->value(),
              media_audio::FakeComposite::kStartTopologyId);
    EXPECT_LT(topology_node->node()
                  .get_property<inspect::UintPropertyValue>(std::string(kTopologyId))
                  ->value(),
              media_audio::FakeComposite::kEndTopologyId);

    ASSERT_FALSE(topology_node->children().empty());
    EXPECT_EQ(topology_node->children().size(), 1u);
    auto edge_pair_node = &topology_node->children().front();
    EXPECT_EQ(edge_pair_node->name(), kEdgePairs);
    for (auto j = 0u; j < edge_pair_node->children().size(); ++j) {
      auto& edge_pair_child_node = edge_pair_node->children().at(j);
      EXPECT_EQ(edge_pair_child_node.name(), std::to_string(j));
      ASSERT_FALSE(edge_pair_child_node.node().properties().empty());
      EXPECT_EQ(edge_pair_child_node.node().properties().size(), 2u);
      // At least check that the fields exist (eventually check whether ElementIds are known).
      EXPECT_TRUE(edge_pair_child_node.node().get_property<inspect::UintPropertyValue>(
          std::string(kEdgeFromElementId)));
      EXPECT_TRUE(edge_pair_child_node.node().get_property<inspect::UintPropertyValue>(
          std::string(kEdgeToElementId)));
    }
  }
}

// Validate the overall Element list. Schema is as follows:
//  Devices
//    12345678
//      Elements
//        0:
//          element_id = 8
//          properties:
//            can_bypass = <none> (cannot be bypassed)
//            can_stop = true
//            description = speaker_dai
//            type = DAI_INTERCONNECT
//            type_specific = ...
//        1:
//          ...
TEST_F(InspectorTest, Elements) {
  // Boot up the device and check the elements
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);
  ASSERT_FALSE(elements_node->children().empty());
  EXPECT_LE(elements_node->children().size(), fhasp::kMaxCountProcessingElements);

  for (auto i = 0u; i < elements_node->children().size(); i++) {
    auto& element_node = elements_node->children().at(i);
    EXPECT_EQ(element_node.name(), std::to_string(i));
    EXPECT_EQ(element_node.node().properties().size(), 1u);
    EXPECT_TRUE(
        element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId)));

    auto properties_node = GetChild(&element_node, kProperties);
    EXPECT_EQ(properties_node->node().properties().size(), 4u);
    EXPECT_TRUE(
        properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kType)));
    EXPECT_TRUE(properties_node->node().get_property<inspect::StringPropertyValue>(
        std::string(kDescription)));
    EXPECT_TRUE(
        properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kCanStop)));
    EXPECT_TRUE(properties_node->node().get_property<inspect::StringPropertyValue>(
        std::string(kCanBypass)));

    // Don't check type_specific in this test case.
  }
}

// On device-add, there should be a DYNAMICS element with specific properties. Here is the schema:
//  Devices:
//    1903763576:
//      Elements:
//        0:
//          element_id = 321
//          properties:
//            can_bypass = true
//            can_stop = <none> (cannot be stopped)
//            description = Dynamics node description
//            type = DYNAMICS
//            type_specific:
//              bands = [3, 0]
//              supported_controls = KNEE_WIDTH | ATTACK | RELEASE | OUTPUT_GAIN | INPUT_GAIN |
//                                   LOOKAHEAD | LEVEL_TYPE | LINKED_CHANNELS | THRESHOLD_TYPE
TEST_F(InspectorTest, TypeSpecificElementDynamics) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);
  ASSERT_FALSE(elements_node->children().empty());

  std::ostringstream type;
  type << fhasp::ElementType::kDynamics;
  const std::string& type_str = type.str();
  for (const auto& element_node : elements_node->children()) {
    auto properties_node = GetChild(&element_node, kProperties);
    ASSERT_NE(properties_node, nullptr);
    ASSERT_TRUE(
        properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kType)));
    // Check all the DYNAMICS elements (not just the first).
    if (properties_node->node()
            .get_property<inspect::StringPropertyValue>(std::string(kType))
            ->value() == type_str) {
      auto type_specific_node = GetChild(properties_node, kTypeSpecific);
      ASSERT_NE(type_specific_node, nullptr);

      const auto& bands = type_specific_node->node()
                              .get_property<inspect::UintArrayValue>(std::string(kBands))
                              ->value();
      EXPECT_EQ(bands.size(), 2u);
      EXPECT_EQ(bands.at(0), FakeComposite::kDynamicsBandId1);
      EXPECT_EQ(bands.at(1), FakeComposite::kDynamicsBandId2);

      auto supported_controls_prop =
          type_specific_node->node().get_property<inspect::StringPropertyValue>(
              std::string(kSupportedControls));
      ASSERT_TRUE(supported_controls_prop);
      std::ostringstream stream;
      stream << FakeComposite::kDynamicsSupportedControls;
      EXPECT_EQ(supported_controls_prop->value(), stream.str());
    }
  }
}

// On device-add, there should be a EQUALIZER element with specific properties. Here is the schema:
//  Devices:
//    1903763576:
//      Elements:
//        0:
//          element_id = 321
//            can_bypass = <none> (cannot be bypassed)
//            can_stop = <none> (cannot be stopped)
//            description = Equalizer node description
//            type = EQUALIZER
//            type_specific:
//              bands = [3, 0]
//              can_disable_bands = true | <none>
//              max_frequency = 20000
//              max_gain_db = 20.0 | <none> (depends on supported_controls)
//              max_q = 1.0 | <none>
//              min_frequency = 20
//              min_gain_db = -20.0 | <none> (depends on supported_controls)
//              supported_controls = KNEE_WIDTH | ATTACK | RELEASE | OUTPUT_GAIN | INPUT_GAIN |
//                                   LOOKAHEAD | LEVEL_TYPE | LINKED_CHANNELS | THRESHOLD_TYPE |
//                                   <none>
TEST_F(InspectorTest, TypeSpecificElementEqualizer) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);
  ASSERT_FALSE(elements_node->children().empty());

  std::ostringstream type;
  type << fhasp::ElementType::kEqualizer;
  const std::string& type_str = type.str();
  for (const auto& element_node : elements_node->children()) {
    auto properties_node = GetChild(&element_node, kProperties);
    ASSERT_NE(properties_node, nullptr);
    ASSERT_TRUE(
        properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kType)));
    // Check all the EQUALIZER elements (not just the first)
    if (properties_node->node()
            .get_property<inspect::StringPropertyValue>(std::string(kType))
            ->value() == type_str) {
      auto type_specific_node = GetChild(properties_node, kTypeSpecific);
      ASSERT_NE(type_specific_node, nullptr);

      const auto& bands = type_specific_node->node()
                              .get_property<inspect::UintArrayValue>(std::string(kBands))
                              ->value();
      EXPECT_EQ(bands.size(), 2u);
      EXPECT_EQ(bands.at(0), FakeComposite::kEqualizerBandId1);
      EXPECT_EQ(bands.at(1), FakeComposite::kEqualizerBandId2);

      auto supported_controls_prop =
          type_specific_node->node().get_property<inspect::StringPropertyValue>(
              std::string(kSupportedControls));
      ASSERT_TRUE(supported_controls_prop);
      std::ostringstream stream;
      stream << FakeComposite::kEqualizerSupportedControls;
      EXPECT_EQ(supported_controls_prop->value(), stream.str());

      ASSERT_TRUE(type_specific_node->node().get_property<inspect::BoolPropertyValue>(
          std::string(kCanDisableBands)));
      EXPECT_TRUE(type_specific_node->node()
                      .get_property<inspect::BoolPropertyValue>(std::string(kCanDisableBands))
                      ->value());

      ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kMinFrequency)));
      EXPECT_EQ(type_specific_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kMinFrequency))
                    ->value(),
                std::to_string(FakeComposite::kEqualizerMinFrequency));

      ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kMaxFrequency)));
      EXPECT_EQ(type_specific_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kMaxFrequency))
                    ->value(),
                std::to_string(FakeComposite::kEqualizerMaxFrequency));

      ASSERT_TRUE(type_specific_node->node().get_property<inspect::DoublePropertyValue>(
          std::string(kMaxQ)));
      EXPECT_EQ(type_specific_node->node()
                    .get_property<inspect::DoublePropertyValue>(std::string(kMaxQ))
                    ->value(),
                FakeComposite::kEqualizerMaxQ);

      ASSERT_TRUE(type_specific_node->node().get_property<inspect::DoublePropertyValue>(
          std::string(kMinGainDb)));
      EXPECT_EQ(type_specific_node->node()
                    .get_property<inspect::DoublePropertyValue>(std::string(kMinGainDb))
                    ->value(),
                FakeComposite::kEqualizerMinGainDb);

      ASSERT_TRUE(type_specific_node->node().get_property<inspect::DoublePropertyValue>(
          std::string(kMaxGainDb)));
      EXPECT_EQ(type_specific_node->node()
                    .get_property<inspect::DoublePropertyValue>(std::string(kMaxGainDb))
                    ->value(),
                FakeComposite::kEqualizerMaxGainDb);
    }
  }
}

// On device-add, there should be a GAIN element with specific properties. Here is the schema:
//  Devices:
//    1903763576:
//      Elements:
//        0:
//          element_id = 321
//          properties:
//            can_bypass = true
//            can_stop = <none> (cannot be stopped)
//            description = Gain node description
//            type = GAIN
//            type_specific:
//              gain_type = PERCENT | DECIBELS | <none> (non-compliant)
//              gain_domain = DIGITAL | ANALOG | MIXED | <none>
//              min_gain = -40.0 | <none> (non-compliant)
//              max_gain = 10.0 | <none> (non-compliant)
//              min_gain_step = 0.1 | <none> (non-compliant)
TEST_F(InspectorTest, TypeSpecificElementGain) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);
  ASSERT_FALSE(elements_node->children().empty());
  ASSERT_LE(elements_node->children().size(), fhasp::kMaxCountProcessingElements);

  std::ostringstream type;
  type << fhasp::ElementType::kGain;
  const std::string& type_str = type.str();
  for (const auto& element_node : elements_node->children()) {
    auto properties_node = GetChild(&element_node, kProperties);
    ASSERT_NE(properties_node, nullptr);
    ASSERT_TRUE(
        properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kType)));
    // Check all the GAIN elements (not just the first)
    if (properties_node->node()
            .get_property<inspect::StringPropertyValue>(std::string(kType))
            ->value() == type_str) {
      auto type_specific_node = GetChild(properties_node, kTypeSpecific);
      ASSERT_NE(type_specific_node, nullptr);

      EXPECT_EQ(type_specific_node->node().properties().size(), 5u);
      {
        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kGainType)));
        std::ostringstream stream;
        stream << FakeComposite::kGainType;
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kGainType))
                      ->value(),
                  stream.str());
      }
      {
        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kGainDomain)));
        std::ostringstream stream;
        stream << FakeComposite::kGainDomain;
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kGainDomain))
                      ->value(),
                  stream.str());
      }
      {
        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kMinGain)));
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMinGain))
                      ->value(),
                  std::to_string(FakeComposite::kGainMin));
      }
      {
        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kMaxGain)));
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMaxGain))
                      ->value(),
                  std::to_string(FakeComposite::kGainMax));
      }
      {
        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kMinGainStep)));
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMinGainStep))
                      ->value(),
                  std::to_string(FakeComposite::kGainStep));
      }
    }
  }
}

// On device-add, there should be a VENDOR_SPECIFIC element with a specific schema:
//  Devices:
//    1903763576:
//      Elements:
//        0:
//          element_id = 321
//          properties:
//            can_bypass = true
//            can_stop = <none> (cannot be stopped)
//            description = Vendor-specific node description
//            type = VENDOR_SPECIFIC
//            type_specific:  (VendorSpecific)
TEST_F(InspectorTest, DISABLED_TypeSpecificElementVendorSpecific) {
  // There don't seem to be additional things that we can validate. ?
}

// On device-add, initial_topology_id and current_topology_id should be populated (and equal).
// These are properties directly below the device instance.
//  Devices:
//    12345678:
//      initial_topology_id = 0
//      current_topology_id = 0
TEST_F(InspectorTest, InitialTopology) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  ASSERT_TRUE(device_node->node().get_property<inspect::UintPropertyValue>(
      std::string(kInitialTopologyId)));
  EXPECT_EQ(device_node->node()
                .get_property<inspect::UintPropertyValue>(std::string(kInitialTopologyId))
                ->value(),
            media_audio::FakeComposite::kDefaultTopologyId);

  ASSERT_TRUE(device_node->node().get_property<inspect::UintPropertyValue>(
      std::string(kCurrentTopologyId)));
  EXPECT_EQ(device_node->node()
                .get_property<inspect::UintPropertyValue>(std::string(kCurrentTopologyId))
                ->value(),
            media_audio::FakeComposite::kDefaultTopologyId);
}

// Validate that current_topology_id changes, and initial_topology_id does not.
//  Devices:
//    12345678:
//      initial_topology_id = 42
//      current_topology_id = 68
TEST_F(InspectorTest, ChangedTopology) {
  // Boot up the device and check the initial/current topology IDs.
  auto fake_driver = CreateAndAddFakeComposite();
  RunLoopUntilIdle();

  // Change the topology. 'current_topology_id' should change; 'initial_topology_id' should not.
  // (kSubsequentTopologyId is guaranteed to differ from kDefaultTopologyId)
  fake_driver->InjectTopologyChange(media_audio::FakeComposite::kSubsequentTopologyId);
  RunLoopUntilIdle();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  ASSERT_TRUE(device_node->node().get_property<inspect::UintPropertyValue>(
      std::string(kInitialTopologyId)));
  EXPECT_EQ(device_node->node()
                .get_property<inspect::UintPropertyValue>(std::string(kInitialTopologyId))
                ->value(),
            media_audio::FakeComposite::kDefaultTopologyId);

  ASSERT_TRUE(device_node->node().get_property<inspect::UintPropertyValue>(
      std::string(kCurrentTopologyId)));
  EXPECT_EQ(device_node->node()
                .get_property<inspect::UintPropertyValue>(std::string(kCurrentTopologyId))
                ->value(),
            media_audio::FakeComposite::kSubsequentTopologyId);
}

// Drivers are not required to respond IMMEDIATELY to WatchTopology with their power-up topology.
// In that case, Inspect should not be populated. Once a topology is set, they should be populated.
TEST_F(InspectorTest, AbsentTopology) {
  // Boot the device in a mode where it does not complete the initial WatchTopology() ... yet.
  auto fake_driver = CreateFakeComposite();
  fake_driver->InjectTopologyChange(std::nullopt);
  adr_service()->AddDevice(Device::Create(
      adr_service(), dispatcher(), "Test composite name",
      fuchsia_audio_device::DeviceType::kComposite,
      fuchsia_audio_device::DriverClient::WithComposite(fake_driver->Enable()), kClassName));
  RunLoopUntilIdle();

  {
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    ASSERT_FALSE(device_node->node().properties().empty());
    ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
    ASSERT_TRUE(
        device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

    // These properties should be absent.
    EXPECT_FALSE(device_node->node().get_property<inspect::UintPropertyValue>(
        std::string(kInitialTopologyId)));
    EXPECT_FALSE(device_node->node().get_property<inspect::UintPropertyValue>(
        std::string(kCurrentTopologyId)));
  }

  // Now set the topology. Pended WatchTopology should complete, and Inspect should be populated.
  fake_driver->InjectTopologyChange(media_audio::FakeComposite::kDefaultTopologyId);
  RunLoopUntilIdle();

  {
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    ASSERT_FALSE(device_node->node().properties().empty());
    ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
    ASSERT_TRUE(
        device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

    ASSERT_TRUE(device_node->node().get_property<inspect::UintPropertyValue>(
        std::string(kInitialTopologyId)));
    EXPECT_EQ(device_node->node()
                  .get_property<inspect::UintPropertyValue>(std::string(kInitialTopologyId))
                  ->value(),
              media_audio::FakeComposite::kDefaultTopologyId);

    ASSERT_TRUE(device_node->node().get_property<inspect::UintPropertyValue>(
        std::string(kCurrentTopologyId)));
    EXPECT_EQ(device_node->node()
                  .get_property<inspect::UintPropertyValue>(std::string(kCurrentTopologyId))
                  ->value(),
              media_audio::FakeComposite::kDefaultTopologyId);
  }
}

// Validate the initial non-TypeSpecific ElementState
//
// Elements:
//   1:
//     element_id = 5432
//     properties:
//       ...
//     state:
//       bypassed = false
//       processing_delay = <none>
//       started = true
//       turn_off_delay = <none>
//       turn_on_delay = 0
TEST_F(InspectorTest, InitialElementStateGeneral) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);
  ASSERT_FALSE(elements_node->children().empty());

  // Let's find the Source PacketStream element.
  for (const auto& element_node : elements_node->children()) {
    auto properties_node = GetChild(&element_node, kProperties);
    ASSERT_NE(properties_node, nullptr);
    ASSERT_TRUE(
        properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kType)));

    auto element_id_prop =
        element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
    ASSERT_TRUE(element_id_prop);

    if (element_id_prop->value() == FakeComposite::kSourcePsElementId) {
      auto state_node = GetChild(&element_node, kState);
      ASSERT_NE(state_node, nullptr);

      // Verify initial started state against the source of truth in FakeComposite.
      ASSERT_TRUE(
          state_node->node().get_property<inspect::StringPropertyValue>(std::string(kStarted)));
      EXPECT_EQ(state_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kStarted))
                    ->value(),
                *FakeComposite::kSourcePsElementInitState.started() ? "true" : "false");

      // Verify initial bypassed state against the source of truth in FakeComposite.
      ASSERT_TRUE(
          state_node->node().get_property<inspect::StringPropertyValue>(std::string(kBypassed)));
      EXPECT_EQ(state_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kBypassed))
                    ->value(),
                *FakeComposite::kSourcePsElementInitState.bypassed() ? "true" : "false");

      ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kProcessingDelay)));
      EXPECT_EQ(state_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kProcessingDelay))
                    ->value(),
                std::to_string(FakeComposite::kSourcePsElementProcessingDelay.get()));
      break;
    }
  }
}

// Validate the initial DaiInterconnect-specific ElementState
//
// 4:
//   element_id = 555
//   properties:
//     type = DAI_INTERCONNECT
//     can_bypass = false
//     ...
//     type_specific:
//       plug_detect_capabilities = HARDWIRED
//   state:
//     bypassed = false
//     ...
//     type_specific:
//       external_delay = 654321
//       plug_state:
//         plugged = plugged-in
//         plug_state_time = 12345678
TEST_F(InspectorTest, InitialElementStateDaiInterconnect) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);
  ASSERT_FALSE(elements_node->children().empty());

  // Let's find the Dest DaiInterconnect element.
  for (const auto& element_node : elements_node->children()) {
    auto properties_node = GetChild(&element_node, kProperties);
    ASSERT_NE(properties_node, nullptr);

    auto element_id_prop =
        element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
    ASSERT_TRUE(element_id_prop);

    if (element_id_prop->value() == FakeComposite::kDestDaiElementId) {
      auto state_node = GetChild(&element_node, kState);
      ASSERT_NE(state_node, nullptr);

      auto type_specific_state_node = GetChild(state_node, kTypeSpecific);
      ASSERT_NE(type_specific_state_node, nullptr);

      ASSERT_TRUE(type_specific_state_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kExternalDelay)));
      EXPECT_EQ(type_specific_state_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kExternalDelay))
                    ->value(),
                std::to_string(FakeComposite::kDestDaiElementExternalDelay.get()));

      auto plug_state_node = GetChild(type_specific_state_node, kPlugState);
      ASSERT_NE(plug_state_node, nullptr);

      ASSERT_TRUE(plug_state_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kPlugged)));
      bool expected_plugged = *FakeComposite::kDestDaiElementInitState.type_specific()
                                   ->dai_interconnect()
                                   ->plug_state()
                                   ->plugged();
      EXPECT_EQ(plug_state_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kPlugged))
                    ->value(),
                expected_plugged ? kPluggedInStr : kUnpluggedStr);

      ASSERT_TRUE(plug_state_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kPlugStateTime)));
      std::string expected_time_str =
          std::to_string(*FakeComposite::kDestDaiElementInitState.type_specific()
                              ->dai_interconnect()
                              ->plug_state()
                              ->plug_state_time());
      EXPECT_EQ(plug_state_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kPlugStateTime))
                    ->value(),
                expected_time_str);
      break;
    }
  }
}

// Validate the initial Dynamics-specific ElementState
//
// Elements:
//   3:
//     element_id = 4321
//     properties:
//       type = DYNAMICS
//       ...
//       type_specific:
//         bands = [ ... ]
//         supported_controls = { ... }
//     state:
//       ...
//       type_specific:
//         band_states:
//           0:
//             attack_ns = 40'000'000
//             band_id = 42
//             input_gain_db = 0.000000
//             knee_width_db = 4.000000
//             level_type = PEAK
//             linked_channels = true
//             lookahead_ns = 200'000'000
//             max_frequency = 12000
//             min_frequency = 12
//             output_gain_db = 0.000000
//             ratio = 0.333000
//             release_ns = 160'000'000
//             threshold_db = -4.000000
//             threshold_type = BELOW
//           1:
//             ...
TEST_F(InspectorTest, InitialElementStateDynamics) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);

  for (const auto& element_node : elements_node->children()) {
    auto element_id_prop =
        element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
    ASSERT_TRUE(element_id_prop);

    if (element_id_prop->value() == FakeComposite::kDynamicsElementId) {
      auto state_node = GetChild(&element_node, kState);
      ASSERT_NE(state_node, nullptr);

      auto type_specific_node = GetChild(state_node, kTypeSpecific);
      ASSERT_NE(type_specific_node, nullptr);

      auto band_states_node = GetChild(type_specific_node, kBandStates);
      ASSERT_NE(band_states_node, nullptr);

      const auto& expected_bands =
          *FakeComposite::kDynamicsElement.type_specific()->dynamics()->bands();
      EXPECT_EQ(band_states_node->children().size(), expected_bands.size());

      {
        // Verify Band 0
        auto band_0_node = GetChild(band_states_node, "0");
        ASSERT_NE(band_0_node, nullptr);

        ASSERT_TRUE(
            band_0_node->node().get_property<inspect::StringPropertyValue>(std::string(kBandId)));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kBandId))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsBandId1));

        ASSERT_TRUE(band_0_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kMinFrequency)));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMinFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMinFrequency1));

        ASSERT_TRUE(band_0_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kMaxFrequency)));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMaxFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMaxFrequency1));

        ASSERT_TRUE(band_0_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kThresholdDb)));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kThresholdDb))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsThresholdDb1));

        ASSERT_TRUE(band_0_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kThresholdType)));
        {
          std::ostringstream stream;
          stream << FakeComposite::kDynamicsThresholdType1;
          EXPECT_EQ(band_0_node->node()
                        .get_property<inspect::StringPropertyValue>(std::string(kThresholdType))
                        ->value(),
                    stream.str());
        }

        ASSERT_TRUE(
            band_0_node->node().get_property<inspect::StringPropertyValue>(std::string(kRatio)));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kRatio))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsRatio1));
      }
      {
        // Verify Band 1
        auto band_1_node = GetChild(band_states_node, "1");
        ASSERT_NE(band_1_node, nullptr);

        ASSERT_TRUE(
            band_1_node->node().get_property<inspect::StringPropertyValue>(std::string(kBandId)));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kBandId))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsBandId2));

        ASSERT_TRUE(band_1_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kMinFrequency)));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMinFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMinFrequency2));

        ASSERT_TRUE(band_1_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kMaxFrequency)));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMaxFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMaxFrequency2));

        ASSERT_TRUE(band_1_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kThresholdDb)));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kThresholdDb))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsThresholdDb2));

        ASSERT_TRUE(band_1_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kThresholdType)));
        {
          std::ostringstream stream;
          stream << FakeComposite::kDynamicsThresholdType2;
          EXPECT_EQ(band_1_node->node()
                        .get_property<inspect::StringPropertyValue>(std::string(kThresholdType))
                        ->value(),
                    stream.str());
        }

        ASSERT_TRUE(
            band_1_node->node().get_property<inspect::StringPropertyValue>(std::string(kRatio)));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kRatio))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsRatio2));
      }

      for (const char* band_idx : {"0", "1"}) {
        auto band_node = GetChild(band_states_node, band_idx);
        ASSERT_NE(band_node, nullptr);
        EXPECT_EQ(
            band_node->node().get_property<inspect::DoublePropertyValue>(std::string(kKneeWidthDb)),
            nullptr);
        EXPECT_EQ(band_node->node().get_property<inspect::IntPropertyValue>(std::string(kAttackNs)),
                  nullptr);
        EXPECT_EQ(
            band_node->node().get_property<inspect::IntPropertyValue>(std::string(kReleaseNs)),
            nullptr);
        EXPECT_EQ(band_node->node().get_property<inspect::DoublePropertyValue>(
                      std::string(kOutputGainDb)),
                  nullptr);
        EXPECT_EQ(
            band_node->node().get_property<inspect::DoublePropertyValue>(std::string(kInputGainDb)),
            nullptr);
        EXPECT_EQ(
            band_node->node().get_property<inspect::StringPropertyValue>(std::string(kLevelType)),
            nullptr);
        EXPECT_EQ(
            band_node->node().get_property<inspect::IntPropertyValue>(std::string(kLookaheadNs)),
            nullptr);
        EXPECT_EQ(band_node->node().get_property<inspect::BoolPropertyValue>(
                      std::string(kLinkedChannels)),
                  nullptr);
      }
    }
  }
}

// Validate the initial Equalizer-specific ElementState
//
// Elements:
//   1:
//     element_id = 5432
//     properties:
//       type = EQUALIZER
//       ...
//       type_specific:
//         bands = [ ... ]
//         supported_controls = { ... }
//         ...
//     state:
//       ...
//       type_specific:
//         band_states:
//           0:
//             id = 7
//             enabled = true
//             frequency = 200
//             gain_db = 3.000000
//             q_factor = 2.000000
//             type = NOTCH
//           1:
//             ...
TEST_F(InspectorTest, InitialElementStateEqualizer) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);

  for (const auto& element_node : elements_node->children()) {
    auto element_id_prop =
        element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
    ASSERT_TRUE(element_id_prop);

    if (element_id_prop->value() == FakeComposite::kEqualizerElementId) {
      auto state_node = GetChild(&element_node, kState);
      ASSERT_NE(state_node, nullptr);

      auto type_specific_node = GetChild(state_node, kTypeSpecific);
      ASSERT_NE(type_specific_node, nullptr);

      auto band_states_node = GetChild(type_specific_node, kBandStates);
      ASSERT_NE(band_states_node, nullptr);

      const auto& expected_bands =
          *FakeComposite::kEqualizerElement.type_specific()->equalizer()->bands();
      EXPECT_EQ(band_states_node->children().size(), expected_bands.size());

      // Verify Band 0
      auto band_0_node = GetChild(band_states_node, "0");
      ASSERT_NE(band_0_node, nullptr);
      EXPECT_EQ(band_0_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kBandId))
                    ->value(),
                std::to_string(FakeComposite::kEqualizerBandId1));
      EXPECT_EQ(band_0_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kType))
                    ->value(),
                "LOW_SHELF");
      EXPECT_EQ(band_0_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kFrequency))
                    ->value(),
                std::to_string(FakeComposite::kEqualizerFrequency1));

      // Verify Band 1
      auto band_1_node = GetChild(band_states_node, "1");
      ASSERT_NE(band_1_node, nullptr);
      EXPECT_EQ(band_1_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kBandId))
                    ->value(),
                std::to_string(FakeComposite::kEqualizerBandId2));
      EXPECT_EQ(band_1_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kType))
                    ->value(),
                "NOTCH");
      EXPECT_EQ(band_1_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kFrequency))
                    ->value(),
                std::to_string(FakeComposite::kEqualizerFrequency2));
    }
  }
}

// Validate the initial Gain-specific ElementState
//
// Elements:
//   1:
//     element_id = 321
//     properties:
//       type = GAIN
//       ...
//       type_specific:
//         gain_type = DECIBELS
//         ...
//     state:
//       ...
//       type_specific:
//         gain_db = -6.000000
TEST_F(InspectorTest, InitialElementStateGain) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);

  for (const auto& element_node : elements_node->children()) {
    auto element_id_prop =
        element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
    ASSERT_TRUE(element_id_prop);

    if (element_id_prop->value() == FakeComposite::kGainElementId) {
      auto state_node = GetChild(&element_node, kState);
      ASSERT_NE(state_node, nullptr);

      auto type_specific_node = GetChild(state_node, kTypeSpecific);
      ASSERT_NE(type_specific_node, nullptr);

      ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kGainDb)));
      EXPECT_EQ(type_specific_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kGainDb))
                    ->value(),
                std::to_string(FakeComposite::kGainInitValue));
      break;
    }
  }
}

// Validate the initial VendorSpecific ElementState
//
// Elements:
//   2:
//     element_id = 3210
//     properties:
//       type = VENDOR_SPECIFIC
//       ...
//       type_specific:
//     state:
//       ...
//       vendor_specific_data = uint8[256]
//       type_specific:
TEST_F(InspectorTest, InitialElementStateVendorSpecific) {
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  auto elements_node = GetChild(device_node, kElements);
  ASSERT_NE(elements_node, nullptr);

  for (const auto& element_node : elements_node->children()) {
    auto element_id_prop =
        element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
    ASSERT_TRUE(element_id_prop);

    if (element_id_prop->value() == FakeComposite::kVendorSpecificElementId) {
      auto state_node = GetChild(&element_node, kState);
      ASSERT_NE(state_node, nullptr);

      ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
          std::string(kVendorSpecificData)));
      std::string expected_val =
          "uint8[" + std::to_string(FakeComposite::kVendorSpecificDataLength) + "]";
      EXPECT_EQ(state_node->node()
                    .get_property<inspect::StringPropertyValue>(std::string(kVendorSpecificData))
                    ->value(),
                expected_val);
    }
  }
}

// Validate that the non-TypeSpecific ElementState can be changed after it is initially populated.
//
// Elements:
//   1:
//     element_id = 5432
//     properties:
//       ...
//     state:
//       bypassed = true        -> false
//       processing_delay = 456 -> 0
//       started = false        -> true
//       turn_off_delay = 345   -> 0
//       turn_on_delay = 234    -> 0
TEST_F(InspectorTest, ChangedElementState) {
  // Boot up the device with the normal initial state.
  auto fake_driver = CreateAndAddFakeComposite();

  bool new_started = true, new_bypassed = false;
  {
    // Check that the Equalizer element is by default bypassed and stopped.
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);
    ASSERT_NE(elements_node, nullptr);
    ASSERT_FALSE(elements_node->children().empty());

    // Let's find the Equalizer element and make sure it is bypassed and stopped.
    for (const auto& element_node : elements_node->children()) {
      auto properties_node = GetChild(&element_node, kProperties);
      ASSERT_NE(properties_node, nullptr);
      ASSERT_TRUE(
          properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kType)));
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);
      if (element_id_prop->value() == FakeComposite::kEqualizerElementId) {
        auto state_node = GetChild(&element_node, kState);
        ASSERT_NE(state_node, nullptr);
        ASSERT_TRUE(
            state_node->node().get_property<inspect::StringPropertyValue>(std::string(kStarted)));
        ASSERT_TRUE(
            state_node->node().get_property<inspect::StringPropertyValue>(std::string(kBypassed)));
        ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kTurnOnDelay)));
        ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kTurnOffDelay)));
        ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kProcessingDelay)));

        EXPECT_EQ(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kStarted))
                      ->value(),
                  *FakeComposite::kEqualizerElementInitState.started() ? "true" : "false");
        EXPECT_EQ(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kBypassed))
                      ->value(),
                  *FakeComposite::kEqualizerElementInitState.bypassed() ? "true" : "false");
        EXPECT_NE(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kTurnOnDelay))
                      ->value(),
                  "0");
        EXPECT_NE(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kTurnOffDelay))
                      ->value(),
                  "0");
        EXPECT_NE(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kProcessingDelay))
                      ->value(),
                  "0");

        new_started = state_node->node()
                          .get_property<inspect::StringPropertyValue>(std::string(kStarted))
                          ->value() == "false";
        new_bypassed = state_node->node()
                           .get_property<inspect::StringPropertyValue>(std::string(kBypassed))
                           ->value() != "true";
        break;
      }
    }
  }

  // Change this element's state. Both started and bypassed can be toggled for this Element.
  fhasp::ElementState new_state = {{
      .type_specific = FakeComposite::kEqualizerElementInitState.type_specific(),
      .started = new_started,
      .bypassed = new_bypassed,
      .turn_on_delay = 0,
      .turn_off_delay = 0,
      .processing_delay = 0,
  }};
  fake_driver->InjectElementStateChange(FakeComposite::kEqualizerElementId, new_state);
  RunLoopUntilIdle();

  {
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);
    ASSERT_NE(elements_node, nullptr);
    ASSERT_FALSE(elements_node->children().empty());
    for (const auto& element_node : elements_node->children()) {
      auto properties_node = GetChild(&element_node, kProperties);
      ASSERT_NE(properties_node, nullptr);
      ASSERT_TRUE(
          properties_node->node().get_property<inspect::StringPropertyValue>(std::string(kType)));
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);
      if (element_id_prop->value() == FakeComposite::kEqualizerElementId) {
        auto state_node = GetChild(&element_node, kState);
        ASSERT_NE(state_node, nullptr);
        ASSERT_TRUE(
            state_node->node().get_property<inspect::StringPropertyValue>(std::string(kStarted)));
        ASSERT_TRUE(
            state_node->node().get_property<inspect::StringPropertyValue>(std::string(kBypassed)));
        ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kTurnOnDelay)));
        ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kTurnOffDelay)));
        ASSERT_TRUE(state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kProcessingDelay)));

        EXPECT_EQ(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kStarted))
                      ->value(),
                  new_started ? "true" : "false");
        EXPECT_EQ(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kBypassed))
                      ->value(),
                  new_bypassed ? "true" : "false");
        EXPECT_EQ(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kTurnOnDelay))
                      ->value(),
                  "0");
        EXPECT_EQ(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kTurnOffDelay))
                      ->value(),
                  "0");
        EXPECT_EQ(state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kProcessingDelay))
                      ->value(),
                  "0");
        break;
      }
    }
  }
}

TEST_F(InspectorTest, ChangedElementStateDaiInterconnect) {
  // Boot up the device with normal initial state.
  auto fake_driver = CreateAndAddFakeComposite();
  RunLoopUntilIdle();

  {
    // Verify initial state for Dest DaiInterconnect element.
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);
    ASSERT_NE(elements_node, nullptr);
    ASSERT_FALSE(elements_node->children().empty());

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);

      if (element_id_prop->value() == FakeComposite::kDestDaiElementId) {
        auto state_node = GetChild(&element_node, kState);
        ASSERT_NE(state_node, nullptr);

        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        ASSERT_NE(type_specific_node, nullptr);

        auto plug_state_node = GetChild(type_specific_node, kPlugState);
        ASSERT_NE(plug_state_node, nullptr);

        // Verify initial plugged state.
        ASSERT_TRUE(plug_state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kPlugged)));
        EXPECT_EQ(plug_state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kPlugged))
                      ->value(),
                  kPluggedInStr);

        // Verify initial plug state time.
        ASSERT_TRUE(plug_state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kPlugStateTime)));
        EXPECT_EQ(plug_state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kPlugStateTime))
                      ->value(),
                  std::to_string(0u));

        // Verify initial external delay.
        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kExternalDelay)));
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kExternalDelay))
                      ->value(),
                  std::to_string(FakeComposite::kDestDaiElementExternalDelay.get()));
        break;
      }
    }
  }

  // Compute new values.
  bool new_plugged = false;
  uint64_t new_plug_state_time = 999;
  zx::duration new_external_delay = zx::nsec(456);

  // Inject state change.
  fhasp::ElementState new_state = {{
      .type_specific = fhasp::TypeSpecificElementState::WithDaiInterconnect({{
          .plug_state = fhasp::PlugState{{
              .plugged = new_plugged,
              .plug_state_time = new_plug_state_time,
          }},
          .external_delay = new_external_delay.get(),
      }}),
      .started = true,
      .bypassed = false,
  }};
  fake_driver->InjectElementStateChange(FakeComposite::kDestDaiElementId, new_state);
  RunLoopUntilIdle();

  {
    // Verify updated state.
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());
    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);
    ASSERT_NE(elements_node, nullptr);
    ASSERT_FALSE(elements_node->children().empty());

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);
      if (element_id_prop->value() == FakeComposite::kDestDaiElementId) {
        auto state_node = GetChild(&element_node, kState);
        ASSERT_NE(state_node, nullptr);
        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        ASSERT_NE(type_specific_node, nullptr);
        auto plug_state_node = GetChild(type_specific_node, kPlugState);
        ASSERT_NE(plug_state_node, nullptr);

        // Verify updated plugged state.
        ASSERT_TRUE(plug_state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kPlugged)));
        EXPECT_EQ(plug_state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kPlugged))
                      ->value(),
                  kUnpluggedStr);

        // Verify updated plug state time.
        ASSERT_TRUE(plug_state_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kPlugStateTime)));
        EXPECT_EQ(plug_state_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kPlugStateTime))
                      ->value(),
                  std::to_string(new_plug_state_time));

        // Verify updated external delay.
        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kExternalDelay)));
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kExternalDelay))
                      ->value(),
                  std::to_string(new_external_delay.get()));
        break;
      }
    }
  }
}

TEST_F(InspectorTest, ChangedElementStateDynamics) {
  auto fake_driver = CreateAndAddFakeComposite();
  RunLoopUntilIdle();

  {
    // Verify initial state.
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);
    ASSERT_NE(elements_node, nullptr);

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);

      if (element_id_prop->value() == FakeComposite::kDynamicsElementId) {
        auto state_node = GetChild(&element_node, kState);
        ASSERT_NE(state_node, nullptr);

        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        ASSERT_NE(type_specific_node, nullptr);

        auto band_states_node = GetChild(type_specific_node, kBandStates);
        ASSERT_NE(band_states_node, nullptr);

        const auto& expected_bands =
            *FakeComposite::kDynamicsElement.type_specific()->dynamics()->bands();
        EXPECT_EQ(band_states_node->children().size(), expected_bands.size());

        // Verify Band 0 initial values.
        auto band_0_node = GetChild(band_states_node, "0");
        ASSERT_NE(band_0_node, nullptr);
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMinFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMinFrequency1));
        break;
      }
    }
  }

  // Inject state change with only required fields (all optional fields absent).
  uint32_t new_min_frequency = 100;
  fhasp::DynamicsBandState bs1;
  bs1.id(FakeComposite::kDynamicsBandId1);
  bs1.min_frequency(new_min_frequency);
  bs1.max_frequency(FakeComposite::kDynamicsMaxFrequency1);
  bs1.threshold_db(-12.0);
  bs1.threshold_type(fhasp::ThresholdType::kBelow);
  bs1.ratio(0.333);

  fhasp::DynamicsBandState bs2;
  bs2.id(FakeComposite::kDynamicsBandId2);
  bs2.min_frequency(FakeComposite::kDynamicsMinFrequency2);
  bs2.max_frequency(FakeComposite::kDynamicsMaxFrequency2);
  bs2.threshold_db(-6.0);
  bs2.threshold_type(fhasp::ThresholdType::kAbove);
  bs2.ratio(0.5);

  std::vector<fhasp::DynamicsBandState> band_states;
  band_states.push_back(std::move(bs1));
  band_states.push_back(std::move(bs2));

  fhasp::DynamicsElementState des;
  des.band_states(std::move(band_states));

  fhasp::ElementState new_state = {{
      .type_specific = fhasp::TypeSpecificElementState::WithDynamics(std::move(des)),
      .started = true,
      .bypassed = false,
  }};
  fake_driver->InjectElementStateChange(FakeComposite::kDynamicsElementId, new_state);
  RunLoopUntilIdle();

  {
    // Verify updated state and verify that absent optional fields do not dereference
    // nulled-out optionals or publish uninitialized stack values to Inspect.
    auto hierarchy = GetHierarchy();
    auto devices_node = GetChild(&hierarchy, kDevices);
    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);

      if (element_id_prop->value() == FakeComposite::kDynamicsElementId) {
        auto state_node = GetChild(&element_node, kState);
        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        auto band_states_node = GetChild(type_specific_node, kBandStates);

        auto band_0_node = GetChild(band_states_node, "0");
        ASSERT_NE(band_0_node, nullptr);
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kBandId))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsBandId1));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMinFrequency))
                      ->value(),
                  std::to_string(new_min_frequency));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMaxFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMaxFrequency1));
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kThresholdDb))
                      ->value(),
                  std::to_string(-12.0f));
        {
          std::ostringstream stream;
          stream << fhasp::ThresholdType::kBelow;
          EXPECT_EQ(band_0_node->node()
                        .get_property<inspect::StringPropertyValue>(std::string(kThresholdType))
                        ->value(),
                    stream.str());
        }
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kRatio))
                      ->value(),
                  std::to_string(0.333f));

        auto band_1_node = GetChild(band_states_node, "1");
        ASSERT_NE(band_1_node, nullptr);
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kBandId))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsBandId2));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMinFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMinFrequency2));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kMaxFrequency))
                      ->value(),
                  std::to_string(FakeComposite::kDynamicsMaxFrequency2));
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kThresholdDb))
                      ->value(),
                  std::to_string(-6.0f));
        {
          std::ostringstream stream;
          stream << fhasp::ThresholdType::kAbove;
          EXPECT_EQ(band_1_node->node()
                        .get_property<inspect::StringPropertyValue>(std::string(kThresholdType))
                        ->value(),
                    stream.str());
        }
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kRatio))
                      ->value(),
                  std::to_string(0.5f));

        for (const char* band_idx : {"0", "1"}) {
          auto band_node = GetChild(band_states_node, band_idx);
          ASSERT_NE(band_node, nullptr);

          // Optional fields must be absent (nullptr in Inspect).
          EXPECT_EQ(band_node->node().get_property<inspect::DoublePropertyValue>(
                        std::string(kKneeWidthDb)),
                    nullptr);
          EXPECT_EQ(
              band_node->node().get_property<inspect::IntPropertyValue>(std::string(kAttackNs)),
              nullptr);
          EXPECT_EQ(
              band_node->node().get_property<inspect::IntPropertyValue>(std::string(kReleaseNs)),
              nullptr);
          EXPECT_EQ(band_node->node().get_property<inspect::DoublePropertyValue>(
                        std::string(kOutputGainDb)),
                    nullptr);
          EXPECT_EQ(band_node->node().get_property<inspect::DoublePropertyValue>(
                        std::string(kInputGainDb)),
                    nullptr);
          EXPECT_EQ(
              band_node->node().get_property<inspect::StringPropertyValue>(std::string(kLevelType)),
              nullptr);
          EXPECT_EQ(
              band_node->node().get_property<inspect::IntPropertyValue>(std::string(kLookaheadNs)),
              nullptr);
          EXPECT_EQ(band_node->node().get_property<inspect::BoolPropertyValue>(
                        std::string(kLinkedChannels)),
                    nullptr);
        }
        break;
      }
    }
  }

  // Inject a second state change to populate optional fields in supported_controls (attack,
  // release, output_gain_db) and verify they transition from nullptr to expected values.
  bs1.attack(10'000'000);   // 10 ms
  bs1.release(50'000'000);  // 50 ms
  bs1.output_gain_db(3.5f);

  bs2.attack(20'000'000);    // 20 ms
  bs2.release(100'000'000);  // 100 ms
  bs2.output_gain_db(-1.5f);

  std::vector<fhasp::DynamicsBandState> band_states2;
  band_states2.push_back(std::move(bs1));
  band_states2.push_back(std::move(bs2));

  fhasp::DynamicsElementState des2;
  des2.band_states(std::move(band_states2));

  fhasp::ElementState new_state2 = {{
      .type_specific = fhasp::TypeSpecificElementState::WithDynamics(std::move(des2)),
      .started = true,
      .bypassed = false,
  }};
  fake_driver->InjectElementStateChange(FakeComposite::kDynamicsElementId, new_state2);
  RunLoopUntilIdle();

  {
    // Verify optional fields transitioned from nullptr to expected values.
    auto hierarchy = GetHierarchy();
    auto devices_node = GetChild(&hierarchy, kDevices);
    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));

      if (element_id_prop->value() == FakeComposite::kDynamicsElementId) {
        auto state_node = GetChild(&element_node, kState);
        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        auto band_states_node = GetChild(type_specific_node, kBandStates);

        auto band_0_node = GetChild(band_states_node, "0");
        ASSERT_NE(band_0_node, nullptr);
        ASSERT_NE(
            band_0_node->node().get_property<inspect::IntPropertyValue>(std::string(kAttackNs)),
            nullptr);
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::IntPropertyValue>(std::string(kAttackNs))
                      ->value(),
                  10'000'000);
        ASSERT_NE(
            band_0_node->node().get_property<inspect::IntPropertyValue>(std::string(kReleaseNs)),
            nullptr);
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::IntPropertyValue>(std::string(kReleaseNs))
                      ->value(),
                  50'000'000);
        ASSERT_NE(band_0_node->node().get_property<inspect::DoublePropertyValue>(
                      std::string(kOutputGainDb)),
                  nullptr);
        EXPECT_DOUBLE_EQ(band_0_node->node()
                             .get_property<inspect::DoublePropertyValue>(std::string(kOutputGainDb))
                             ->value(),
                         3.5);

        auto band_1_node = GetChild(band_states_node, "1");
        ASSERT_NE(band_1_node, nullptr);
        ASSERT_NE(
            band_1_node->node().get_property<inspect::IntPropertyValue>(std::string(kAttackNs)),
            nullptr);
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::IntPropertyValue>(std::string(kAttackNs))
                      ->value(),
                  20'000'000);
        ASSERT_NE(
            band_1_node->node().get_property<inspect::IntPropertyValue>(std::string(kReleaseNs)),
            nullptr);
        EXPECT_EQ(band_1_node->node()
                      .get_property<inspect::IntPropertyValue>(std::string(kReleaseNs))
                      ->value(),
                  100'000'000);
        ASSERT_NE(band_1_node->node().get_property<inspect::DoublePropertyValue>(
                      std::string(kOutputGainDb)),
                  nullptr);
        EXPECT_DOUBLE_EQ(band_1_node->node()
                             .get_property<inspect::DoublePropertyValue>(std::string(kOutputGainDb))
                             ->value(),
                         -1.5);

        // Verify unpopulated optionals remain absent.
        for (const char* band_idx : {"0", "1"}) {
          auto band_node = GetChild(band_states_node, band_idx);
          EXPECT_EQ(band_node->node().get_property<inspect::DoublePropertyValue>(
                        std::string(kKneeWidthDb)),
                    nullptr);
          EXPECT_EQ(band_node->node().get_property<inspect::DoublePropertyValue>(
                        std::string(kInputGainDb)),
                    nullptr);
          EXPECT_EQ(
              band_node->node().get_property<inspect::StringPropertyValue>(std::string(kLevelType)),
              nullptr);
          EXPECT_EQ(
              band_node->node().get_property<inspect::IntPropertyValue>(std::string(kLookaheadNs)),
              nullptr);
          EXPECT_EQ(band_node->node().get_property<inspect::BoolPropertyValue>(
                        std::string(kLinkedChannels)),
                    nullptr);
        }
        break;
      }
    }
  }
}

TEST_F(InspectorTest, ChangedElementStateEqualizer) {
  auto fake_driver = CreateAndAddFakeComposite();
  RunLoopUntilIdle();

  {
    // Verify initial state.
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);
    ASSERT_NE(elements_node, nullptr);

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);

      if (element_id_prop->value() == FakeComposite::kEqualizerElementId) {
        auto state_node = GetChild(&element_node, kState);
        ASSERT_NE(state_node, nullptr);

        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        ASSERT_NE(type_specific_node, nullptr);

        auto band_states_node = GetChild(type_specific_node, kBandStates);
        ASSERT_NE(band_states_node, nullptr);

        // Verify Band 0 initial values.
        auto band_0_node = GetChild(band_states_node, "0");
        ASSERT_NE(band_0_node, nullptr);
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kFrequency))
                      ->value(),
                  std::to_string(500));
        break;
      }
    }
  }

  // Inject state change.
  uint32_t new_frequency = 200;
  fhasp::EqualizerBandState bs1;
  bs1.id(FakeComposite::kEqualizerBandId1);
  bs1.type(fhasp::EqualizerBandType::kPeak);
  bs1.frequency(new_frequency);
  bs1.q(1.0);
  bs1.gain_db(0.0);
  bs1.enabled(true);

  // We must provide state for ALL bands!
  fhasp::EqualizerBandState bs2;
  bs2.id(FakeComposite::kEqualizerBandId2);
  bs2.type(fhasp::EqualizerBandType::kPeak);
  bs2.frequency(1000);
  bs2.q(1.0);
  bs2.gain_db(0.0);
  bs2.enabled(true);

  std::vector<fhasp::EqualizerBandState> band_states;
  band_states.push_back(std::move(bs1));
  band_states.push_back(std::move(bs2));

  fhasp::EqualizerElementState ees;
  ees.band_states(std::move(band_states));

  fhasp::ElementState new_state = {{
      .type_specific = fhasp::TypeSpecificElementState::WithEqualizer(std::move(ees)),
      .started = true,
      .bypassed = false,
  }};
  fake_driver->InjectElementStateChange(FakeComposite::kEqualizerElementId, new_state);
  RunLoopUntilIdle();

  {
    // Verify updated state.
    auto hierarchy = GetHierarchy();
    auto devices_node = GetChild(&hierarchy, kDevices);
    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      if (element_id_prop->value() == FakeComposite::kEqualizerElementId) {
        auto state_node = GetChild(&element_node, kState);
        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        auto band_states_node = GetChild(type_specific_node, kBandStates);

        auto band_0_node = GetChild(band_states_node, "0");
        ASSERT_NE(band_0_node, nullptr);
        EXPECT_EQ(band_0_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kFrequency))
                      ->value(),
                  std::to_string(new_frequency));
        break;
      }
    }
  }
}

TEST_F(InspectorTest, ChangedElementStateGain) {
  auto fake_driver = CreateAndAddFakeComposite();
  RunLoopUntilIdle();

  {
    auto hierarchy = GetHierarchy();
    ASSERT_FALSE(hierarchy.children().empty());
    auto devices_node = GetChild(&hierarchy, kDevices);
    ASSERT_NE(devices_node, nullptr);
    ASSERT_FALSE(devices_node->children().empty());

    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);
    ASSERT_NE(elements_node, nullptr);

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      ASSERT_TRUE(element_id_prop);

      if (element_id_prop->value() == FakeComposite::kGainElementId) {
        auto state_node = GetChild(&element_node, kState);
        ASSERT_NE(state_node, nullptr);

        auto type_specific_node = GetChild(state_node, kTypeSpecific);
        ASSERT_NE(type_specific_node, nullptr);

        ASSERT_TRUE(type_specific_node->node().get_property<inspect::StringPropertyValue>(
            std::string(kGainDb)));
        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kGainDb))
                      ->value(),
                  std::to_string(FakeComposite::kGainInitValue));
        break;
      }
    }
  }

  // Inject state change.
  double new_gain_db = -10.0;
  fhasp::ElementState new_state = {{
      .type_specific = fhasp::TypeSpecificElementState::WithGain({{
          .gain = new_gain_db,
      }}),
      .started = true,
      .bypassed = false,
  }};
  fake_driver->InjectElementStateChange(FakeComposite::kGainElementId, new_state);
  RunLoopUntilIdle();

  {
    auto hierarchy = GetHierarchy();
    auto devices_node = GetChild(&hierarchy, kDevices);
    auto device_node = &devices_node->children().front();
    auto elements_node = GetChild(device_node, kElements);

    for (const auto& element_node : elements_node->children()) {
      auto element_id_prop =
          element_node.node().get_property<inspect::UintPropertyValue>(std::string(kElementId));
      if (element_id_prop->value() == FakeComposite::kGainElementId) {
        auto state_node = GetChild(&element_node, kState);
        auto type_specific_node = GetChild(state_node, kTypeSpecific);

        EXPECT_EQ(type_specific_node->node()
                      .get_property<inspect::StringPropertyValue>(std::string(kGainDb))
                      ->value(),
                  std::to_string(new_gain_db));
        break;
      }
    }
  }
}

// Validate the overall SupportedFormatSets for each RingBuffer element. Schema is as follows:
// Devices
//   12345678
//     RingBuffers
//       0:
//         description = 'bluetooth_hfp_outgoing_ring_buffer'
//         element_id = 1
//         supported_format_sets:
//           rb_format_set_0:
//             frames_per_second = [44100, 48000]
//             sample_format = ["INT_16", "FLOAT_32"]
//             channel_count:
//               channel_set_0:
//                 channel_0:
//                   min_frequency = 50
//                   max_frequency = 22000
//               channel_set_1:
//                 channel_0:
//                   (absent means unlimited: to the edge of the theoretical range)
//                 channel_1:
//                   min_frequency = 2000
//                   max_frequency = 22050
//
TEST_F(InspectorRingBufferTest, SupportedRingBufferFormats) {
  // Boot up the device and validate that each RingBuffer element has SupportedFormatSets.
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ring_buffers_node = GetChild(device_node, kRingBuffers);
  ASSERT_NE(ring_buffers_node, nullptr);
  ASSERT_FALSE(ring_buffers_node->children().empty());

  // Check each different RingBuffer.
  for (auto idx = 0u; idx < ring_buffers_node->children().size(); ++idx) {
    const auto& ring_buffer_node = ring_buffers_node->children()[idx];

    ASSERT_EQ(ring_buffer_node.name(), std::to_string(idx));
    // We depend on this element_id, when checking the specific values below.
    ASSERT_TRUE(ring_buffer_node.node().get_property<UintPropertyValue>(std::string(kElementId)));
    ElementId rb_element_id =
        ring_buffer_node.node().get_property<UintPropertyValue>(std::string(kElementId))->value();
    ASSERT_TRUE(rb_element_id == FakeComposite::kMinRingBufferElementId ||
                rb_element_id == FakeComposite::kMaxRingBufferElementId);
    const bool kFirstRbElement = (rb_element_id == FakeComposite::kMinRingBufferElementId);
    ASSERT_FALSE(ring_buffer_node.children().empty());

    auto ring_buffer_format_sets_node = GetChild(&ring_buffer_node, kSupportedFormats);
    ASSERT_NE(ring_buffer_format_sets_node, nullptr);
    ASSERT_EQ(ring_buffer_format_sets_node->name(), kSupportedFormats);
    ASSERT_FALSE(ring_buffer_format_sets_node->children().empty());
    ASSERT_LE(ring_buffer_format_sets_node->children().size(),
              fuchsia_audio_device::kMaxFormatCount);
    EXPECT_TRUE(ring_buffer_format_sets_node->node().properties().empty());

    auto ring_buffer_format_set_node = &ring_buffer_format_sets_node->children().front();
    ASSERT_EQ(ring_buffer_format_set_node->name(), "rb_format_set_0");
    EXPECT_EQ(ring_buffer_format_set_node->node().properties().size(), 2u);
    EXPECT_EQ(ring_buffer_format_set_node->children().size(), 1u);

    const auto& rates = ring_buffer_format_set_node->node()
                            .get_property<inspect::UintArrayValue>(std::string(kFramesPerSecond))
                            ->value();
    EXPECT_EQ(rates.size(), 1u);
    for (auto& rate : rates) {
      EXPECT_EQ(rate, kFirstRbElement ? FakeComposite::kDefaultRbFrameRate1
                                      : FakeComposite::kDefaultRbFrameRate2);
    }

    const auto& sample_formats =
        ring_buffer_format_set_node->node()
            .get_property<inspect::StringArrayValue>(std::string(kSampleFormat))
            ->value();
    EXPECT_EQ(sample_formats.size(), 1u);

    std::ostringstream stream16;
    stream16 << fuchsia_audio::SampleType::kInt16;
    std::ostringstream stream32;
    stream32 << fuchsia_audio::SampleType::kInt32;
    for (auto& sample_format : sample_formats) {
      EXPECT_EQ(sample_format, kFirstRbElement ? stream16.str() : stream32.str());
    }

    auto channel_counts_node = GetChild(ring_buffer_format_set_node, kChannelCount);
    ASSERT_NE(channel_counts_node, nullptr);
    ASSERT_EQ(channel_counts_node->name(), kChannelCount);
    EXPECT_TRUE(channel_counts_node->node().properties().empty());
    ASSERT_FALSE(channel_counts_node->children().empty());
    EXPECT_LE(channel_counts_node->children().size(), fuchsia_audio_device::kMaxChannelSetCount);

    auto channel_count_node = &channel_counts_node->children().front();
    EXPECT_EQ(channel_count_node->name(), "channel_set_0");
    EXPECT_TRUE(channel_count_node->node().properties().empty());
    EXPECT_EQ(channel_count_node->children().size(), FakeComposite::kDefaultNumberOfChannels2);

    auto channel_node = &channel_count_node->children().front();
    EXPECT_EQ(channel_node->name(), "channel_0");
    EXPECT_TRUE(channel_node->children().empty());
    ASSERT_FALSE(channel_node->node().properties().empty());

    if (kFirstRbElement) {
      EXPECT_EQ(channel_node->node().properties().size(), 2u);
      ASSERT_TRUE(channel_node->node().get_property<UintPropertyValue>(std::string(kMinFrequency)));
      EXPECT_EQ(
          channel_node->node().get_property<UintPropertyValue>(std::string(kMinFrequency))->value(),
          FakeComposite::kDefaultChannelAttributes1MinFrequency);
      ASSERT_TRUE(channel_node->node().get_property<UintPropertyValue>(std::string(kMaxFrequency)));
      EXPECT_EQ(
          channel_node->node().get_property<UintPropertyValue>(std::string(kMaxFrequency))->value(),
          FakeComposite::kDefaultChannelAttributes1MaxFrequency);
    } else {
      EXPECT_EQ(channel_node->node().properties().size(), 1u);
      ASSERT_TRUE(channel_node->node().get_property<UintPropertyValue>(std::string(kMinFrequency)));
      EXPECT_EQ(
          channel_node->node().get_property<UintPropertyValue>(std::string(kMinFrequency))->value(),
          FakeComposite::kDefaultChannelAttributes2MinFrequency);
      // FakeComposite::kDefaultChannelAttributes2MaxFrequency is not defined.
      EXPECT_FALSE(
          channel_node->node().get_property<UintPropertyValue>(std::string(kMaxFrequency)));
    }
  }
}

// Relevant fields: `started at` and `stopped at` -- found at
//  root/Devices/[device name]/RingBuffers/0/instance_0/running_intervals/0/
// We test multiple start/stop calls, to validate running intervals are tracked separately.
TEST_F(InspectorRingBufferTest, RingBufferStartStop) {
  AddDeviceAndCreateRingBuffer();

  zx::time start_time0;
  bool received_callback = false;
  auto before_start0 = zx::clock::get_monotonic();
  ring_buffer_client()->Start({}).Then(
      [&received_callback, &start_time0](fidl::Result<fad::RingBuffer::Start>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->start_time());
        start_time0 = zx::time(*result->start_time());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(fake_driver()->RingBufferStarted(element_id()));
  EXPECT_EQ(start_time0, fake_driver()->RingBufferMonoStartTime(element_id()));
  EXPECT_GT(start_time0.get(), before_start0.get());

  auto before_stop0 = zx::clock::get_monotonic();
  received_callback = false;
  ring_buffer_client()->Stop({}).Then(
      [&received_callback](fidl::Result<fad::RingBuffer::Stop>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(fake_driver()->RingBufferStarted(element_id()));
  auto after_stop0 = zx::clock::get_monotonic();

  // Now we do another start/stop, to validate multiple running intervals.
  zx::time start_time1;
  received_callback = false;
  auto before_start1 = zx::clock::get_monotonic();
  ring_buffer_client()->Start({}).Then(
      [&received_callback, &start_time1](fidl::Result<fad::RingBuffer::Start>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->start_time());
        start_time1 = zx::time(*result->start_time());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(fake_driver()->RingBufferStarted(element_id()));
  EXPECT_EQ(start_time1, fake_driver()->RingBufferMonoStartTime(element_id()));
  EXPECT_GT(start_time1.get(), before_start1.get());

  auto before_stop1 = zx::clock::get_monotonic();
  received_callback = false;
  ring_buffer_client()->Stop({}).Then(
      [&received_callback](fidl::Result<fad::RingBuffer::Stop>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(fake_driver()->RingBufferStarted(element_id()));
  auto after_stop1 = zx::clock::get_monotonic();

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ring_buffers_node = GetChild(device_node, kRingBuffers);
  ASSERT_NE(ring_buffers_node, nullptr);
  ASSERT_FALSE(ring_buffers_node->children().empty());

  auto ring_buffer_node = &ring_buffers_node->children().front();
  ASSERT_EQ(ring_buffer_node->name(), "0");
  ASSERT_EQ(
      ring_buffer_node->node().get_property<UintPropertyValue>(std::string(kElementId))->value(),
      element_id());
  ASSERT_FALSE(ring_buffer_node->children().empty());

  auto ring_buffer_instance_node = &ring_buffer_node->children().front();
  ASSERT_EQ(ring_buffer_instance_node->name(), "instance_0");
  ASSERT_FALSE(ring_buffer_instance_node->children().empty());

  auto running_intervals = GetChild(ring_buffer_instance_node, kRunningIntervals);
  ASSERT_NE(running_intervals, nullptr);
  EXPECT_TRUE(running_intervals->node().properties().empty());
  ASSERT_EQ(running_intervals->children().size(), 2u);

  auto& first_start_stop = running_intervals->children().cbegin()->node();
  auto& last_start_stop = running_intervals->children().crbegin()->node();

  EXPECT_EQ(first_start_stop.name(), "0");
  EXPECT_EQ(first_start_stop.properties().size(), 2u);
  EXPECT_GT(first_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_start0.get());
  EXPECT_LT(first_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_stop0.get());
  EXPECT_GT(first_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            before_stop0.get());
  EXPECT_LT(first_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            after_stop0.get());
  EXPECT_TRUE(running_intervals->children().cbegin()->children().empty());

  EXPECT_EQ(last_start_stop.name(), "1");
  EXPECT_EQ(last_start_stop.properties().size(), 2u);
  EXPECT_GT(last_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_start1.get());
  EXPECT_LT(last_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_stop1.get());
  EXPECT_GT(last_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            before_stop1.get());
  EXPECT_LT(last_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            after_stop1.get());
  EXPECT_TRUE(running_intervals->children().crbegin()->children().empty());
}

// Relevant fields: `called_at`, `completed_at` and `channel_bitmask` -- found at
// root/Devices/[device name]/RingBuffers/0/instance_0/SetActiveChannels_calls/0/
// We test multiple SetActiveChannels calls to ensure these are tracked separately.
TEST_F(InspectorRingBufferTest, SetActiveChannels) {
  AddDeviceAndCreateRingBuffer();

  bool received_callback = false;
  zx::time set_active_channels_completed_at0;
  auto before_set_active_channels0 = zx::clock::get_monotonic();
  ring_buffer_client()
      ->SetActiveChannels({{
          .channel_bitmask = 0x0,
      }})
      .Then([&received_callback, &set_active_channels_completed_at0](
                fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->set_time().has_value());
        set_active_channels_completed_at0 = zx::time(*result->set_time());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_GT(set_active_channels_completed_at0.get(), before_set_active_channels0.get());

  received_callback = false;
  zx::time set_active_channels_completed_at1;
  auto before_set_active_channels1 = zx::clock::get_monotonic();
  ring_buffer_client()
      ->SetActiveChannels({{
          .channel_bitmask = 0x1,
      }})
      .Then([&received_callback, &set_active_channels_completed_at1](
                fidl::Result<fad::RingBuffer::SetActiveChannels>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->set_time().has_value());
        set_active_channels_completed_at1 = zx::time(*result->set_time());
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_GT(set_active_channels_completed_at1.get(), before_set_active_channels1.get());

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ring_buffers_node = GetChild(device_node, kRingBuffers);
  ASSERT_NE(ring_buffers_node, nullptr);
  ASSERT_FALSE(ring_buffers_node->children().empty());

  auto ring_buffer_node = &ring_buffers_node->children().front();
  ASSERT_EQ(ring_buffer_node->name(), "0");
  ASSERT_EQ(ring_buffer_node->node().properties().size(), 2u);
  ASSERT_EQ(
      ring_buffer_node->node().get_property<UintPropertyValue>(std::string(kElementId))->value(),
      element_id());
  ASSERT_FALSE(ring_buffer_node->children().empty());

  auto ring_buffer_instance_node = &ring_buffer_node->children().front();
  ASSERT_EQ(ring_buffer_instance_node->name(), "instance_0");
  ASSERT_FALSE(ring_buffer_instance_node->children().empty());

  auto set_active_channels_calls_node =
      GetChild(ring_buffer_instance_node, kSetActiveChannelsCalls);
  ASSERT_NE(set_active_channels_calls_node, nullptr);
  EXPECT_TRUE(set_active_channels_calls_node->node().properties().empty());
  ASSERT_FALSE(set_active_channels_calls_node->children().empty());
  ASSERT_EQ(set_active_channels_calls_node->children().size(), 2u);

  auto& first_set_call = set_active_channels_calls_node->children().front().node();
  auto& last_set_call = set_active_channels_calls_node->children().back().node();

  EXPECT_EQ(first_set_call.name(), "0");
  EXPECT_EQ(first_set_call.properties().size(), 3u);
  EXPECT_GT(first_set_call.get_property<IntPropertyValue>(std::string(kCalledAt))->value(),
            before_set_active_channels0.get());
  EXPECT_EQ(first_set_call.get_property<IntPropertyValue>(std::string(kCompletedAt))->value(),
            set_active_channels_completed_at0.get());
  EXPECT_EQ(first_set_call.get_property<UintPropertyValue>(std::string(kChannelBitmask))->value(),
            0ull);
  EXPECT_TRUE(set_active_channels_calls_node->children().front().children().empty());

  EXPECT_EQ(last_set_call.name(), "1");
  EXPECT_EQ(last_set_call.properties().size(), 3u);
  EXPECT_GT(last_set_call.get_property<IntPropertyValue>(std::string(kCalledAt))->value(),
            before_set_active_channels1.get());
  EXPECT_EQ(last_set_call.get_property<IntPropertyValue>(std::string(kCompletedAt))->value(),
            set_active_channels_completed_at1.get());
  EXPECT_EQ(last_set_call.get_property<UintPropertyValue>(std::string(kChannelBitmask))->value(),
            1ull);
  EXPECT_TRUE(set_active_channels_calls_node->children().back().children().empty());
}

// Relevant fields: `requested_bytes`, `client_frames`, `driver_frames`, `vmo_bytes` -- found at
// root/Devices/[device name]/RingBuffers/0/instance_0/.
TEST_F(InspectorRingBufferTest, RbBufferProperties) {
  AddDeviceAndCreateRingBuffer();

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ring_buffers_node = GetChild(device_node, kRingBuffers);
  ASSERT_NE(ring_buffers_node, nullptr);
  ASSERT_FALSE(ring_buffers_node->children().empty());

  auto ring_buffer_node = &ring_buffers_node->children().front();
  ASSERT_EQ(ring_buffer_node->name(), "0");
  ASSERT_EQ(ring_buffer_node->node().properties().size(), 2u);
  ASSERT_EQ(
      ring_buffer_node->node().get_property<UintPropertyValue>(std::string(kElementId))->value(),
      element_id());
  ASSERT_FALSE(ring_buffer_node->children().empty());

  auto ring_buffer_instance_node = &ring_buffer_node->children().front();
  ASSERT_EQ(ring_buffer_instance_node->name(), "instance_0");
  ASSERT_FALSE(ring_buffer_instance_node->children().empty());

  auto buffer_node = GetChild(ring_buffer_instance_node, kBufferProps);
  ASSERT_NE(buffer_node, nullptr);
  ASSERT_EQ(buffer_node->node().name(), kBufferProps);
  ASSERT_TRUE(buffer_node->children().empty());

  EXPECT_FALSE(buffer_node->node().properties().empty());
  ASSERT_EQ(buffer_node->node().properties().size(), 4u);
  auto bytes_per_frame = frame_size(rb_format());
  EXPECT_EQ(
      buffer_node->node().get_property<UintPropertyValue>(std::string(kRequestedBytes))->value(),
      requested_ring_buffer_bytes());
  EXPECT_EQ(
      buffer_node->node().get_property<UintPropertyValue>(std::string(kProducerFrames))->value(),
      *ring_buffer()->producer_bytes() / bytes_per_frame);
  EXPECT_EQ(
      buffer_node->node().get_property<UintPropertyValue>(std::string(kConsumerFrames))->value(),
      *ring_buffer()->consumer_bytes() / bytes_per_frame);
  EXPECT_EQ(buffer_node->node().get_property<UintPropertyValue>(std::string(kVmoBytes))->value(),
            ring_buffer()->buffer()->size());
}

// Relevant fields: `frames_per_second`, `channel_count`, `sample_format` -- found at
// root/Devices/[device name]/RingBuffers/0/instance_0/.
TEST_F(InspectorRingBufferTest, RingBufferFormat) {
  AddDeviceAndCreateRingBuffer();

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ring_buffers_node = GetChild(device_node, kRingBuffers);
  ASSERT_NE(ring_buffers_node, nullptr);
  ASSERT_FALSE(ring_buffers_node->children().empty());

  auto ring_buffer_node = &ring_buffers_node->children().front();
  ASSERT_EQ(ring_buffer_node->name(), "0");
  ASSERT_EQ(ring_buffer_node->node().properties().size(), 2u);
  ASSERT_EQ(
      ring_buffer_node->node().get_property<UintPropertyValue>(std::string(kElementId))->value(),
      element_id());
  ASSERT_FALSE(ring_buffer_node->children().empty());

  auto ring_buffer_instance_node = &ring_buffer_node->children().front();
  ASSERT_EQ(ring_buffer_instance_node->name(), "instance_0");
  ASSERT_FALSE(ring_buffer_instance_node->children().empty());

  auto format_node = GetChild(ring_buffer_instance_node, kFormatProps);
  ASSERT_NE(format_node, nullptr);

  EXPECT_EQ(format_node->node().name(), kFormatProps);
  EXPECT_TRUE(format_node->children().empty());

  EXPECT_FALSE(format_node->node().properties().empty());
  EXPECT_EQ(format_node->node().properties().size(), 3u);
  EXPECT_EQ(
      format_node->node().get_property<UintPropertyValue>(std::string(kFramesPerSecond))->value(),
      *rb_format().frames_per_second());
  EXPECT_EQ(
      format_node->node().get_property<UintPropertyValue>(std::string(kChannelCount))->value(),
      *rb_format().channel_count());
  FX_LOGS(INFO) << *rb_format().sample_type();
  EXPECT_EQ(
      format_node->node().get_property<StringPropertyValue>(std::string(kSampleFormat))->value(),
      "INT_32");
}

TEST_F(InspectorPacketStreamTest, PacketStreamInstance) {
  auto before_instance = zx::clock::get_monotonic();
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ps_elements_node = GetChild(device_node, kPacketStreams);
  ASSERT_NE(ps_elements_node, nullptr);
  ASSERT_EQ(ps_elements_node->children().size(), 3u);

  auto* ps_element_node = ps_elements_node->children().data();
  ASSERT_EQ(ps_element_node->children().size(), 1u);
  EXPECT_EQ(ps_element_node->children().at(0).name(), kSupportedFormats);

  // Now record an instance.
  auto presence = adr_service()->FindDeviceByTokenId(0);
  ASSERT_EQ(presence.first, AudioDeviceRegistry::DevicePresence::Active);
  auto device = presence.second;

  auto ps_element_id = FakeComposite::kSourcePsElementId;
  auto ps_instance = device->inspect()->RecordPacketStreamInstance(ps_element_id, before_instance);
  ASSERT_NE(ps_instance, nullptr);

  hierarchy = GetHierarchy();
  devices_node = GetChild(&hierarchy, kDevices);
  device_node = &devices_node->children().front();
  ps_elements_node = GetChild(device_node, kPacketStreams);

  // Find the node with the correct element_id.
  const inspect::Hierarchy* target_ps_element_node = nullptr;
  for (const auto& child : ps_elements_node->children()) {
    if (child.node().get_property<inspect::UintPropertyValue>(std::string(kElementId))->value() ==
        ps_element_id) {
      target_ps_element_node = &child;
      break;
    }
  }
  ASSERT_NE(target_ps_element_node, nullptr);
  ASSERT_EQ(target_ps_element_node->children().size(), 2u);
  auto* supported_formats_node = GetChild(target_ps_element_node, kSupportedFormats);
  ASSERT_NE(supported_formats_node, nullptr);
  EXPECT_EQ(supported_formats_node->name(), kSupportedFormats);
  auto* instance_node = GetChild(target_ps_element_node, "instance_0");
  ASSERT_NE(instance_node, nullptr);
  EXPECT_EQ(instance_node->node()
                .get_property<inspect::IntPropertyValue>(std::string(kCreatedAt))
                ->value(),
            before_instance.get());
}

TEST_F(InspectorPacketStreamTest, SupportedPacketStreamFormats) {
  // Boot up the device and validate that each PacketStream element has SupportedFormatSets.
  auto fake_driver = CreateAndAddFakeComposite();

  auto hierarchy = GetHierarchy();
  ASSERT_FALSE(hierarchy.children().empty());

  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ps_elements_node = GetChild(device_node, kPacketStreams);
  ASSERT_NE(ps_elements_node, nullptr);
  ASSERT_FALSE(ps_elements_node->children().empty());

  // Check each different PacketStream element.
  for (auto idx = 0u; idx < ps_elements_node->children().size(); ++idx) {
    const auto& ps_element_node = ps_elements_node->children()[idx];

    ASSERT_EQ(ps_element_node.name(), std::to_string(idx));
    // We depend on this element_id, when checking the specific values below.
    ASSERT_TRUE(ps_element_node.node().get_property<UintPropertyValue>(std::string(kElementId)));
    ElementId ps_element_id =
        ps_element_node.node().get_property<UintPropertyValue>(std::string(kElementId))->value();
    ASSERT_TRUE(ps_element_id == FakeComposite::kDestPsElementId ||
                ps_element_id == FakeComposite::kSourcePsElementId ||
                ps_element_id == FakeComposite::kSourceDualSupportPsElementId);
    ASSERT_FALSE(ps_element_node.children().empty());

    auto ps_format_sets_node = GetChild(&ps_element_node, kSupportedFormats);
    ASSERT_NE(ps_format_sets_node, nullptr);
    ASSERT_EQ(ps_format_sets_node->name(), kSupportedFormats);
    ASSERT_FALSE(ps_format_sets_node->children().empty());
    EXPECT_TRUE(ps_format_sets_node->node().properties().empty());

    if (ps_element_id == FakeComposite::kDestPsElementId) {
      ASSERT_EQ(ps_format_sets_node->children().size(), 2u);
      auto ps_format_set_node0 = GetChild(ps_format_sets_node, "ps_pcm_format_set_0");
      ASSERT_NE(ps_format_set_node0, nullptr);
      EXPECT_EQ(ps_format_set_node0->node().properties().size(), 2u);

      const auto& rates0 = ps_format_set_node0->node()
                               .get_property<inspect::UintArrayValue>(std::string(kFramesPerSecond))
                               ->value();
      EXPECT_EQ(rates0.at(0), FakeComposite::kDefaultRbFrameRate1);

      const auto& sample_formats0 =
          ps_format_set_node0->node()
              .get_property<inspect::StringArrayValue>(std::string(kSampleFormat))
              ->value();
      std::ostringstream stream;
      stream << fuchsia_audio::SampleType::kInt16;
      EXPECT_EQ(sample_formats0.at(0), stream.str());

      auto ps_format_set_node1 = GetChild(ps_format_sets_node, "ps_pcm_format_set_1");
      ASSERT_NE(ps_format_set_node1, nullptr);
      EXPECT_EQ(ps_format_set_node1->node().properties().size(), 2u);

      const auto& rates1 = ps_format_set_node1->node()
                               .get_property<inspect::UintArrayValue>(std::string(kFramesPerSecond))
                               ->value();
      EXPECT_EQ(rates1.at(0), FakeComposite::kDefaultRbFrameRate2);

      const auto& sample_formats1 =
          ps_format_set_node1->node()
              .get_property<inspect::StringArrayValue>(std::string(kSampleFormat))
              ->value();
      stream.clear();
      stream.str("");
      stream << fuchsia_audio::SampleType::kFloat32;
      EXPECT_EQ(sample_formats1.at(0), stream.str());
    } else if (ps_element_id == FakeComposite::kSourcePsElementId) {
      auto ps_format_set_node = GetChild(ps_format_sets_node, "ps_encoding_set_0");
      ASSERT_NE(ps_format_set_node, nullptr);
      EXPECT_EQ(ps_format_set_node->node().properties().size(), 2u);

      const auto& rates = ps_format_set_node->node()
                              .get_property<inspect::UintArrayValue>(std::string(kFramesPerSecond))
                              ->value();
      EXPECT_EQ(rates.at(0), FakeComposite::kDefaultPsFrameRate2);

      const auto& encoding_types =
          ps_format_set_node->node()
              .get_property<inspect::StringArrayValue>(std::string(kEncodingType))
              ->value();
      std::ostringstream stream;
      stream << fuchsia_hardware_audio::EncodingType::kAac;
      EXPECT_EQ(encoding_types.at(0), stream.str());
    } else if (ps_element_id == FakeComposite::kSourceDualSupportPsElementId) {
      ASSERT_EQ(ps_format_sets_node->children().size(), 2u);
      auto ps_pcm_format_set_node = GetChild(ps_format_sets_node, "ps_pcm_format_set_0");
      ASSERT_NE(ps_pcm_format_set_node, nullptr);
      auto ps_encoding_set_node = GetChild(ps_format_sets_node, "ps_encoding_set_0");
      ASSERT_NE(ps_encoding_set_node, nullptr);

      // Verify PCM part
      const auto& rates = ps_pcm_format_set_node->node()
                              .get_property<inspect::UintArrayValue>(std::string(kFramesPerSecond))
                              ->value();
      EXPECT_EQ(rates.at(0), FakeComposite::kDefaultRbFrameRate1);

      // Verify encoding part
      const auto& encoding_types =
          ps_encoding_set_node->node()
              .get_property<inspect::StringArrayValue>(std::string(kEncodingType))
              ->value();
      std::ostringstream stream;
      stream << fuchsia_hardware_audio::EncodingType::kAac;
      EXPECT_EQ(encoding_types.at(0), stream.str());
    } else {
      ADD_FAILURE() << "Unexpected ps element_id " << ps_element_id;
    }
  }
}

TEST_F(InspectorPacketStreamTest, PacketStreamStartStop) {
  AddDeviceAndCreatePacketStream(FakeComposite::kMaxPacketStreamElementId);

  bool received_callback = false;
  auto before_start0 = zx::clock::get_monotonic();
  packet_stream_client()->Start({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Start>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(fake_driver()->PacketStreamStarted(element_id()));
  auto start_time0 = fake_driver()->PacketStreamMonoStartTime(element_id());
  EXPECT_GT(start_time0.get(), before_start0.get());

  auto before_stop0 = zx::clock::get_monotonic();
  received_callback = false;
  packet_stream_client()->Stop({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Stop>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(fake_driver()->PacketStreamStarted(element_id()));
  auto after_stop0 = zx::clock::get_monotonic();

  // Now we do another start/stop, to validate multiple running intervals.
  received_callback = false;
  auto before_start1 = zx::clock::get_monotonic();
  packet_stream_client()->Start({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Start>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(fake_driver()->PacketStreamStarted(element_id()));
  auto start_time1 = fake_driver()->PacketStreamMonoStartTime(element_id());
  EXPECT_GT(start_time1.get(), before_start1.get());

  auto before_stop1 = zx::clock::get_monotonic();
  received_callback = false;
  packet_stream_client()->Stop({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Stop>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_FALSE(fake_driver()->PacketStreamStarted(element_id()));
  auto after_stop1 = zx::clock::get_monotonic();

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ps_elements_node = GetChild(device_node, kPacketStreams);
  ASSERT_NE(ps_elements_node, nullptr);
  ASSERT_FALSE(ps_elements_node->children().empty());

  const inspect::Hierarchy* target_ps_element_node = nullptr;
  for (const auto& child : ps_elements_node->children()) {
    if (child.node().get_property<UintPropertyValue>(std::string(kElementId))->value() ==
        element_id()) {
      target_ps_element_node = &child;
      break;
    }
  }
  ASSERT_NE(target_ps_element_node, nullptr);
  ASSERT_FALSE(target_ps_element_node->children().empty());

  auto instance_node = GetChild(target_ps_element_node, "instance_0");
  ASSERT_NE(instance_node, nullptr);
  ASSERT_FALSE(instance_node->children().empty());

  auto running_intervals = GetChild(instance_node, kRunningIntervals);
  ASSERT_NE(running_intervals, nullptr);
  EXPECT_TRUE(running_intervals->node().properties().empty());
  ASSERT_EQ(running_intervals->children().size(), 2u);

  auto& first_start_stop = running_intervals->children().cbegin()->node();
  auto& last_start_stop = running_intervals->children().crbegin()->node();

  EXPECT_EQ(first_start_stop.name(), "0");
  EXPECT_EQ(first_start_stop.properties().size(), 2u);
  EXPECT_GT(first_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_start0.get());
  EXPECT_LT(first_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_stop0.get());
  EXPECT_GT(first_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            before_stop0.get());
  EXPECT_LT(first_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            after_stop0.get());
  EXPECT_TRUE(running_intervals->children().cbegin()->children().empty());

  EXPECT_EQ(last_start_stop.name(), "1");
  EXPECT_EQ(last_start_stop.properties().size(), 2u);
  EXPECT_GT(last_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_start1.get());
  EXPECT_LT(last_start_stop.get_property<IntPropertyValue>(std::string(kStartedAt))->value(),
            before_stop1.get());
  EXPECT_GT(last_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            before_stop1.get());
  EXPECT_LT(last_start_stop.get_property<IntPropertyValue>(std::string(kStoppedAt))->value(),
            after_stop1.get());
  EXPECT_TRUE(running_intervals->children().crbegin()->children().empty());
}

TEST_F(InspectorPacketStreamTest, PsBufferProperties) {
  AddDeviceAndCreatePacketStream(FakeComposite::kMaxPacketStreamElementId);

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ps_elements_node = GetChild(device_node, kPacketStreams);
  ASSERT_NE(ps_elements_node, nullptr);
  ASSERT_FALSE(ps_elements_node->children().empty());

  const inspect::Hierarchy* target_ps_element_node = nullptr;
  for (const auto& child : ps_elements_node->children()) {
    if (child.node().get_property<UintPropertyValue>(std::string(kElementId))->value() ==
        element_id()) {
      target_ps_element_node = &child;
      break;
    }
  }
  ASSERT_NE(target_ps_element_node, nullptr);
  ASSERT_FALSE(target_ps_element_node->children().empty());

  auto instance_node = GetChild(target_ps_element_node, "instance_0");
  ASSERT_NE(instance_node, nullptr);
  ASSERT_FALSE(instance_node->children().empty());

  auto buffer_node = GetChild(instance_node, kBufferProps);
  ASSERT_NE(buffer_node, nullptr);
  ASSERT_EQ(buffer_node->node().name(), kBufferProps);

  EXPECT_FALSE(buffer_node->node().properties().empty());
  EXPECT_EQ(buffer_node->node().properties().size(), 2u);
  EXPECT_EQ(
      buffer_node->node().get_property<StringPropertyValue>(std::string(kBufferType))->value(),
      "CLIENT_OWNED");
  EXPECT_EQ(buffer_node->node().get_property<UintPropertyValue>(std::string(kVmoBytes))->value(),
            8192u);

  auto vmo_infos_node = GetChild(buffer_node, kVmoInfos);
  ASSERT_NE(vmo_infos_node, nullptr);
  ASSERT_EQ(vmo_infos_node->children().size(), 1u);

  auto vmo_node = &vmo_infos_node->children().front();
  EXPECT_EQ(vmo_node->node().get_property<UintPropertyValue>(std::string(kVmoId))->value(), 0u);
  EXPECT_EQ(vmo_node->node().get_property<UintPropertyValue>(std::string(kVmoBytes))->value(),
            8192u);
}

struct ElementAndFormat {
  ElementId element_id;
  fuchsia_audio_device::PacketStreamFormat format;

  // Define equality operator for runtime validation
  bool operator==(const ElementAndFormat& other) const = default;
};

class InspectorPacketStreamFormatTest : public InspectorTest,
                                        public ::testing::WithParamInterface<ElementAndFormat> {};

TEST_P(InspectorPacketStreamFormatTest, PacketStreamFormat) {
  const auto& param = GetParam();
  auto element_id = param.element_id;
  auto format = param.format;

  AddDeviceAndCreatePacketStream(element_id, format);

  auto hierarchy = GetHierarchy();
  auto devices_node = GetChild(&hierarchy, kDevices);
  ASSERT_NE(devices_node, nullptr);
  ASSERT_FALSE(devices_node->children().empty());

  auto device_node = &devices_node->children().front();
  ASSERT_FALSE(device_node->node().properties().empty());
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy)));
  ASSERT_TRUE(device_node->node().get_property<BoolPropertyValue>(std::string(kHealthy))->value());

  auto ps_elements_node = GetChild(device_node, kPacketStreams);
  ASSERT_NE(ps_elements_node, nullptr);
  ASSERT_FALSE(ps_elements_node->children().empty());

  const inspect::Hierarchy* target_ps_element_node = nullptr;
  for (const auto& child : ps_elements_node->children()) {
    if (child.node().get_property<UintPropertyValue>(std::string(kElementId))->value() ==
        element_id) {
      target_ps_element_node = &child;
      break;
    }
  }
  ASSERT_NE(target_ps_element_node, nullptr);
  ASSERT_FALSE(target_ps_element_node->children().empty());

  auto instance_node = GetChild(target_ps_element_node, "instance_0");
  ASSERT_NE(instance_node, nullptr);

  auto format_node = GetChild(instance_node, kFormatProps);
  ASSERT_NE(format_node, nullptr);

  if (format.pcm_format().has_value()) {
    EXPECT_EQ(
        format_node->node().get_property<UintPropertyValue>(std::string(kFramesPerSecond))->value(),
        *format.pcm_format()->frames_per_second());
    EXPECT_EQ(
        format_node->node().get_property<UintPropertyValue>(std::string(kChannelCount))->value(),
        *format.pcm_format()->channel_count());
    std::ostringstream stream;
    stream << *format.pcm_format()->sample_type();
    EXPECT_EQ(
        format_node->node().get_property<StringPropertyValue>(std::string(kSampleFormat))->value(),
        stream.str());
  } else if (format.encoding().has_value()) {
    std::ostringstream stream;
    stream << *format.encoding()->encoding_type();
    EXPECT_EQ(
        format_node->node().get_property<StringPropertyValue>(std::string(kEncodingType))->value(),
        stream.str());
    EXPECT_EQ(
        format_node->node().get_property<UintPropertyValue>(std::string(kChannelCount))->value(),
        *format.encoding()->decoded_channel_count());
    EXPECT_EQ(
        format_node->node().get_property<UintPropertyValue>(std::string(kFramesPerSecond))->value(),
        *format.encoding()->decoded_frame_rate());
  } else {
    ADD_FAILURE() << "Unknown PacketStreamFormat variant";
  }
}

INSTANTIATE_TEST_SUITE_P(
    InspectorPacketStreamFormatTest, InspectorPacketStreamFormatTest,
    ::testing::Values(
        ElementAndFormat{FakeComposite::kDestPsElementId,
                         fad::PacketStreamFormat::WithPcmFormat(FakeComposite::kDefaultPsFormat1)},
        ElementAndFormat{FakeComposite::kDestPsElementId,
                         fad::PacketStreamFormat::WithPcmFormat(FakeComposite::kDefaultPsFormat2)},
        ElementAndFormat{FakeComposite::kSourcePsElementId,
                         fad::PacketStreamFormat::WithEncoding(FakeComposite::kDefaultPsFormat3)},
        ElementAndFormat{FakeComposite::kSourceDualSupportPsElementId,
                         fad::PacketStreamFormat::WithPcmFormat(FakeComposite::kDefaultPsFormat1)},
        ElementAndFormat{FakeComposite::kSourceDualSupportPsElementId,
                         fad::PacketStreamFormat::WithEncoding(FakeComposite::kDefaultPsFormat3)}));

}  // namespace
}  // namespace media_audio
