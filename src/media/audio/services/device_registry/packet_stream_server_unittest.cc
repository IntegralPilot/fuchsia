// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/packet_stream_server.h"

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_packet_stream.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

class PacketStreamServerTest : public AudioDeviceRegistryServerTestBase,
                               public fidl::AsyncEventHandler<fad::PacketStream> {
 public:
  void handle_unknown_event(fidl::UnknownEventMetadata<fad::PacketStream> metadata) override;

 protected:
  std::pair<fidl::Client<fad::PacketStream>, fidl::ServerEnd<fad::PacketStream>>
  CreatePacketStreamClient();

  std::shared_ptr<FakeComposite> CreateAndEnableDriverWithDefaults(
      std::optional<TopologyId> topology_id = FakeComposite::kSourceDualSupportPsOutputTopologyId) {
    auto fake_driver = CreateFakeComposite();
    if (topology_id) {
      fake_driver->InjectTopologyChange(*topology_id);
    }
    adr_service()->AddDevice(Device::Create(
        adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
        fad::DriverClient::WithComposite(fake_driver->Enable()), "PacketStreamServerTest"));
    RunLoopUntilIdle();
    return fake_driver;
  }

  std::pair<std::unique_ptr<TestServerAndNaturalAsyncClient<ControlServer>>,
            fidl::Client<fad::PacketStream>>
  SetupForCleanShutdownTesting(ElementId element_id);
};

std::pair<fidl::Client<fad::PacketStream>, fidl::ServerEnd<fad::PacketStream>>
PacketStreamServerTest::CreatePacketStreamClient() {
  auto [packet_stream_client_end, packet_stream_server_end] =
      CreateNaturalAsyncClientOrDie<fad::PacketStream>();
  auto packet_stream_client = fidl::Client<fad::PacketStream>(
      std::move(packet_stream_client_end), dispatcher(), packet_stream_fidl_handler().get());
  return std::make_pair(std::move(packet_stream_client), std::move(packet_stream_server_end));
}

std::pair<std::unique_ptr<TestServerAndNaturalAsyncClient<ControlServer>>,
          fidl::Client<fad::PacketStream>>
PacketStreamServerTest::SetupForCleanShutdownTesting(ElementId element_id) {
  auto registry = CreateTestRegistryServer();
  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  FX_CHECK(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  EXPECT_TRUE(presence == AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(device_to_control);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto format =
      SafePacketStreamFormats(element_id, device_to_control->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{.format = format}},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  return std::make_pair(std::move(control), std::move(packet_stream_client));
}

void PacketStreamServerTest::handle_unknown_event(
    fidl::UnknownEventMetadata<fad::PacketStream> metadata) {
  FX_LOGS(WARNING) << "PacketStreamServerTest: unknown event (PacketStream) ordinal "
                   << metadata.event_ordinal;
}

TEST_F(PacketStreamServerTest, ClientDropsPacketStreamControl) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_id);

  auto [presence, device] = adr_service()->FindDeviceByTokenId(*added_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto element_id = *device->packet_stream_ids().begin();
  auto format = SafePacketStreamFormats(element_id, device->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{.format = format}},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_EQ(PacketStreamServer::count(), 1u);

  // Drop the PacketStream client.
  packet_stream_client = {};
  RunLoopUntilIdle();
  EXPECT_EQ(PacketStreamServer::count(), 0u);

  EXPECT_FALSE(packet_stream_fidl_error_status().has_value());
}

TEST_F(PacketStreamServerTest, DriverDropsPacketStreamControl) {
  auto fake_composite = CreateAndEnableDriverWithDefaults();
  auto element_id = FakeComposite::kSourceDualSupportPsElementId;

  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_id);
  auto [presence, device] = adr_service()->FindDeviceByTokenId(*added_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto format = SafePacketStreamFormats(element_id, device->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{
              .format = format,
          }},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_EQ(PacketStreamServer::count(), 1u);

  // Simulate the driver dropping the packet stream.
  fake_composite->DropPacketStream(element_id);
  RunLoopUntilIdle();

  EXPECT_TRUE(packet_stream_fidl_error_status().has_value());
  EXPECT_EQ(*packet_stream_fidl_error_status(), ZX_ERR_PEER_CLOSED);
  EXPECT_EQ(PacketStreamServer::count(), 0u);
}

// TODO(puneetha): Add test cases for PacketStreamSink (data plane) lifetime isolation.
//
// 1. ClientDropsPacketStreamSink
//    - Action: Client (test fixture) drops the PacketStreamSink client-end received in
//    CreatePacketStream response.
//    - Verification: Verify that the PacketStream control channel and ControlServer remain valid
//    (Lifetime isolation).
//
// 2. DriverDropsPacketStreamSink
//    - Action: Simulate driver dropping the PacketStreamSink server-end.
//    - Verification: Client (test fixture) observes peer-closed on the data channel, but the
//    PacketStream control channel held by ADR remains alive.

TEST_F(PacketStreamServerTest, CreatePacketStreamReturnParameters) {
  auto fake_composite = CreateAndEnableDriverWithDefaults();
  auto element_id = FakeComposite::kSourceDualSupportPsElementId;

  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_id);
  auto [presence, device] = adr_service()->FindDeviceByTokenId(*added_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto format = SafePacketStreamFormats(element_id, device->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{
              .format = format,
          }},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback, format](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        ASSERT_TRUE(result->properties());
        ASSERT_TRUE(result->properties()->data_sink());
        ASSERT_TRUE(result->properties()->format());
        EXPECT_EQ(*result->properties()->format(), format);
        ASSERT_TRUE(result->properties()->supported_buffer_types());
        EXPECT_EQ(*result->properties()->supported_buffer_types(),
                  fha::BufferType::kClientOwned | fha::BufferType::kDriverOwned);
        if (format.pcm_format()) {
          ASSERT_TRUE(result->properties()->valid_bits_per_sample());
        }
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
}

TEST_F(PacketStreamServerTest, DriverDropsComposite) {
  auto fake_composite = CreateAndEnableDriverWithDefaults();
  auto element_id = FakeComposite::kSourceDualSupportPsElementId;

  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_id);
  auto [presence, device] = adr_service()->FindDeviceByTokenId(*added_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto format = SafePacketStreamFormats(element_id, device->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{
              .format = format,
          }},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_EQ(PacketStreamServer::count(), 1u);

  // Simulate the driver dropping the composite. The packet stream should cleanly shutdown.
  fake_composite->DropComposite();
  RunLoopUntilIdle();

  EXPECT_EQ(PacketStreamServer::count(), 0u);
}

TEST_F(PacketStreamServerTest, SetBuffers) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_id);
  auto [presence, device] = adr_service()->FindDeviceByTokenId(*added_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto element_id = *device->packet_stream_ids().begin();
  auto format = SafePacketStreamFormats(element_id, device->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{
              .format = format,
          }},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;

  fad::PacketStreamSetBuffersRequest request;
  fha::AllocateVmosConfig alloc_config;
  alloc_config.min_vmo_size(8192);
  alloc_config.vmo_count(1);
  request.vmo_info(fad::PacketStreamSetupVmoInfo::WithAllocateInfo(std::move(alloc_config)));

  packet_stream_client->SetBuffers(std::move(request))
      .Then([&received_callback](fidl::Result<fad::PacketStream::SetBuffers>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
        ASSERT_TRUE(result->packet_stream().has_value());
        ASSERT_TRUE(result->packet_stream()->vmo_infos().has_value());
        EXPECT_FALSE(result->packet_stream()->vmo_infos()->empty());
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
}

TEST_F(PacketStreamServerTest, SetBuffersStripsResizeAndSetPropertyRights) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_id);
  auto [presence, device] = adr_service()->FindDeviceByTokenId(*added_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto element_id = *device->packet_stream_ids().begin();
  auto format = SafePacketStreamFormats(element_id, device->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{
              .format = format,
          }},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;

  zx::vmo vmo, vmo_for_request;
  ASSERT_EQ(ZX_OK, zx::vmo::create(8192, 0, &vmo));
  ASSERT_EQ(ZX_OK, vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_for_request));

  zx_info_handle_basic_t initial_info;
  ASSERT_EQ(ZX_OK, vmo.get_info(ZX_INFO_HANDLE_BASIC, &initial_info, sizeof(initial_info), nullptr,
                                nullptr));
  EXPECT_TRUE(initial_info.rights & ZX_RIGHT_SET_PROPERTY);

  fad::PacketStreamSetBuffersRequest request;
  fha::RegisterVmosConfig register_vmos_config;
  fha::VmoInfo vmo_info;
  vmo_info.id(0);
  vmo_info.vmo(std::move(vmo_for_request));
  std::vector<fha::VmoInfo> vmo_infos;
  vmo_infos.push_back(std::move(vmo_info));
  register_vmos_config.vmo_infos(std::move(vmo_infos));
  request.vmo_info(
      fad::PacketStreamSetupVmoInfo::WithRegisterInfo(std::move(register_vmos_config)));

  packet_stream_client->SetBuffers(std::move(request))
      .Then([&received_callback](fidl::Result<fad::PacketStream::SetBuffers>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);

  auto rights = fake_driver->PacketStreamVmoRights(element_id, 0);
  ASSERT_TRUE(rights.has_value());
  EXPECT_FALSE(*rights & ZX_RIGHT_SET_PROPERTY);
  EXPECT_FALSE(*rights & ZX_RIGHT_RESIZE);
}

TEST_F(PacketStreamServerTest, StartAndStop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto registry = CreateTestRegistryServer();
  auto added_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(added_id);

  auto [presence, device] = adr_service()->FindDeviceByTokenId(*added_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  auto control = CreateTestControlServer(device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  auto element_id = *device->packet_stream_ids().begin();
  auto format = SafePacketStreamFormats(element_id, device->packet_stream_format_sets()).front();

  control->client()
      ->CreatePacketStream({{
          .element_id = element_id,
          .options = fad::PacketStreamOptions{{
              .format = format,
          }},
          .packet_stream_server = std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;

  fad::PacketStreamSetBuffersRequest buffer_request;
  fha::AllocateVmosConfig alloc_config;
  alloc_config.min_vmo_size(8192);
  alloc_config.vmo_count(1);
  buffer_request.vmo_info(fad::PacketStreamSetupVmoInfo::WithAllocateInfo(std::move(alloc_config)));

  packet_stream_client->SetBuffers(std::move(buffer_request))
      .Then([&received_callback](fidl::Result<fad::PacketStream::SetBuffers>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;

  packet_stream_client->Start({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Start>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  received_callback = false;

  packet_stream_client->Stop({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Stop>& result) {
        ASSERT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
}

TEST_F(PacketStreamServerTest, ControlClientDropCausesPacketStreamDrop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto element_id = FakeComposite::kSourceDualSupportPsElementId;
  auto [control, packet_stream_client] = SetupForCleanShutdownTesting(element_id);

  EXPECT_EQ(PacketStreamServer::count(), 1u);

  (void)control->client().UnbindMaybeGetEndpoint();

  RunLoopUntilIdle();
  EXPECT_TRUE(control->server().WaitForShutdown());
  EXPECT_EQ(PacketStreamServer::count(), 0u);
}

TEST_F(PacketStreamServerTest, ControlServerShutdownCausesPacketStreamDrop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto element_id = FakeComposite::kSourceDualSupportPsElementId;
  auto [control, packet_stream_client] = SetupForCleanShutdownTesting(element_id);

  EXPECT_EQ(ControlServer::count(), 1u);
  EXPECT_EQ(PacketStreamServer::count(), 1u);

  control->server().Shutdown(ZX_ERR_PEER_CLOSED);

  RunLoopUntilIdle();
  EXPECT_TRUE(control->server().WaitForShutdown());
  EXPECT_EQ(PacketStreamServer::count(), 0u);
}

TEST_F(PacketStreamServerTest, SecondPacketStreamAfterDrop) {
  auto fake_driver = CreateAndEnableDriverWithDefaults();
  auto element_id = FakeComposite::kSourceDualSupportPsElementId;
  auto registry = CreateTestRegistryServer();
  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id.has_value());
  auto [presence, device_to_control] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(presence, AudioDeviceRegistry::DevicePresence::Active);

  {
    auto control = CreateTestControlServer(device_to_control);
    auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
    bool received_callback = false;

    auto format =
        SafePacketStreamFormats(element_id, device_to_control->packet_stream_format_sets()).front();

    control->client()
        ->CreatePacketStream({{
            .element_id = element_id,
            .options = fad::PacketStreamOptions{{.format = format}},
            .packet_stream_server = std::move(packet_stream_server_end),
        }})
        .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          received_callback = true;
        });

    RunLoopUntilIdle();
    ASSERT_TRUE(received_callback);
    EXPECT_EQ(PacketStreamServer::count(), 1u);

    // Drop connections.
    packet_stream_client = {};
    (void)control->client().UnbindMaybeGetEndpoint();

    RunLoopUntilIdle();
    EXPECT_EQ(PacketStreamServer::count(), 0u);
  }

  {
    auto control = CreateTestControlServer(device_to_control);
    auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
    bool received_callback = false;

    auto format =
        SafePacketStreamFormats(element_id, device_to_control->packet_stream_format_sets()).front();

    control->client()
        ->CreatePacketStream({{
            .element_id = element_id,
            .options = fad::PacketStreamOptions{{.format = format}},
            .packet_stream_server = std::move(packet_stream_server_end),
        }})
        .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          received_callback = true;
        });

    RunLoopUntilIdle();
    ASSERT_TRUE(received_callback);
    EXPECT_EQ(PacketStreamServer::count(), 1u);
  }
}

}  // namespace
}  // namespace media_audio
