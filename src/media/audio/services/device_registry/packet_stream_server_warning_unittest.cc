// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/zx/clock.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/adr_server_unittest_base.h"
#include "src/media/audio/services/device_registry/common_unittest.h"
#include "src/media/audio/services/device_registry/packet_stream_server.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_packet_stream.h"
#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {
namespace {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

class PacketStreamServerWarningTest : public AudioDeviceRegistryServerTestBase,
                                      public fidl::AsyncEventHandler<fad::PacketStream> {
 protected:
  std::pair<fidl::Client<fad::PacketStream>, fidl::ServerEnd<fad::PacketStream>>
  CreatePacketStreamClient() {
    auto [packet_stream_client_end, packet_stream_server_end] =
        CreateNaturalAsyncClientOrDie<fad::PacketStream>();
    auto packet_stream_client =
        fidl::Client<fad::PacketStream>(std::move(packet_stream_client_end), dispatcher(), this);
    return std::make_pair(std::move(packet_stream_client), std::move(packet_stream_server_end));
  }

  void SetBuffers(fidl::Client<fad::PacketStream>& packet_stream_client) {
    fad::PacketStreamSetBuffersRequest request;
    fha::AllocateVmosConfig alloc_config;
    alloc_config.min_vmo_size(8192);
    alloc_config.vmo_count(1);
    request.vmo_info(fad::PacketStreamSetupVmoInfo::WithAllocateInfo(std::move(alloc_config)));

    bool set_buffers_done = false;
    packet_stream_client->SetBuffers(std::move(request)).Then([&set_buffers_done](auto& result) {
      ASSERT_TRUE(result.is_ok()) << result.error_value();
      set_buffers_done = true;
    });
    RunLoopUntilIdle();
    ASSERT_TRUE(set_buffers_done);
  }

  void handle_unknown_event(fidl::UnknownEventMetadata<fad::PacketStream> metadata) override {
    FX_LOGS(WARNING) << "PacketStreamServerWarningTest: unknown event (PacketStream) ordinal "
                     << metadata.event_ordinal;
  }
};

class PacketStreamServerCompositeWarningTest : public PacketStreamServerWarningTest {
 protected:
  static inline const std::string kClassName = "PacketStreamServerCompositeWarningTest";
  std::shared_ptr<Device> EnableDriverAndAddDevice(
      const std::shared_ptr<FakeComposite>& fake_driver) {
    auto device = Device::Create(
        adr_service(), dispatcher(), "Test composite name", fad::DeviceType::kComposite,
        fad::DriverClient::WithComposite(fake_driver->Enable()), kClassName);
    adr_service()->AddDevice(device);

    RunLoopUntilIdle();
    return device;
  }
};

// Test SetBuffers-SetBuffers, when the second SetBuffers is called after the first successfully
// completes.
TEST_F(PacketStreamServerCompositeWarningTest, SetBuffersAlreadyConfigured) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());

  SetBuffers(packet_stream_client);

  received_callback = false;

  fad::PacketStreamSetBuffersRequest request;
  fha::AllocateVmosConfig alloc_config;
  alloc_config.min_vmo_size(8192);
  alloc_config.vmo_count(1);
  request.vmo_info(fad::PacketStreamSetupVmoInfo::WithAllocateInfo(std::move(alloc_config)));

  packet_stream_client->SetBuffers(std::move(request))
      .Then([&received_callback](fidl::Result<fad::PacketStream::SetBuffers>& result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_TRUE(result.error_value().is_domain_error());
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::PacketStreamSetBufferError::kAlreadyConfigured);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());
  EXPECT_TRUE(control->client().is_valid());
}

// Test Start-Start, when the second Start is called before the first Start completes.
TEST_F(PacketStreamServerCompositeWarningTest, StartWhilePending) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());

  SetBuffers(packet_stream_client);

  bool received_callback_1 = false, received_callback_2 = false;

  packet_stream_client->Start({}).Then([&received_callback_1, &fake_driver, element_id](
                                           fidl::Result<fad::PacketStream::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_TRUE(fake_driver->PacketStreamStarted(element_id));
    received_callback_1 = true;
  });
  packet_stream_client->Start({}).Then([&received_callback_2, &fake_driver, element_id](
                                           fidl::Result<fad::PacketStream::Start>& result) {
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
    EXPECT_EQ(result.error_value().domain_error(), fad::PacketStreamStartError::kAlreadyPending)
        << result.error_value();
    EXPECT_TRUE(fake_driver->PacketStreamStarted(element_id));
    received_callback_2 = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback_1 && received_callback_2);
  EXPECT_TRUE(fake_driver->PacketStreamStarted(element_id));
  EXPECT_EQ(PacketStreamServer::count(), 1u);
  EXPECT_TRUE(control->client().is_valid());
}

// Test Start-Start, when the second Start occurs after the first has successfully completed.
TEST_F(PacketStreamServerCompositeWarningTest, StartWhileStarted) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());

  SetBuffers(packet_stream_client);

  received_callback = false;

  packet_stream_client->Start({}).Then([&received_callback, &fake_driver, element_id](
                                           fidl::Result<fad::PacketStream::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_TRUE(fake_driver->PacketStreamStarted(element_id));
    received_callback = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;
  packet_stream_client->Start({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Start>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::PacketStreamStartError::kAlreadyStarted)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_EQ(PacketStreamServer::count(), 1u);
  EXPECT_TRUE(control->client().is_valid());
}

// Test Stop when not yet Started.
TEST_F(PacketStreamServerCompositeWarningTest, StopBeforeStarted) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(packet_stream_client.is_valid());
  received_callback = false;

  packet_stream_client->Stop({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Stop>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::PacketStreamStopError::kAlreadyStopped)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());
  EXPECT_TRUE(control->client().is_valid());
}

// Test Start-Stop-Stop, when the second Stop is called before the first one completes.
TEST_F(PacketStreamServerCompositeWarningTest, StopWhilePending) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(packet_stream_client.is_valid());

  SetBuffers(packet_stream_client);

  received_callback = false;

  packet_stream_client->Start({}).Then([&received_callback, &fake_driver, element_id](
                                           fidl::Result<fad::PacketStream::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_TRUE(fake_driver->PacketStreamStarted(element_id));
    received_callback = true;
  });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());
  bool received_callback_1 = false, received_callback_2 = false;

  packet_stream_client->Stop({}).Then([&received_callback_1, &fake_driver,
                                       element_id](fidl::Result<fad::PacketStream::Stop>& result) {
    EXPECT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_FALSE(fake_driver->PacketStreamStarted(element_id));
    received_callback_1 = true;
  });
  packet_stream_client->Stop({}).Then(
      [&received_callback_2](fidl::Result<fad::PacketStream::Stop>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::PacketStreamStopError::kAlreadyPending)
            << result.error_value();
        received_callback_2 = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback_1 && received_callback_2);
  EXPECT_EQ(PacketStreamServer::count(), 1u);
  EXPECT_TRUE(control->client().is_valid());
}

// Test Start-Stop-Stop, when the first Stop successfully completed before the second is called.
TEST_F(PacketStreamServerCompositeWarningTest, StopAfterStopped) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  ASSERT_EQ(status, AudioDeviceRegistry::DevicePresence::Active);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        EXPECT_TRUE(result.is_ok()) << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(packet_stream_client.is_valid());

  SetBuffers(packet_stream_client);

  received_callback = false;

  packet_stream_client->Start({}).Then([&received_callback, &fake_driver, element_id](
                                           fidl::Result<fad::PacketStream::Start>& result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_TRUE(fake_driver->PacketStreamStarted(element_id));
    received_callback = true;
  });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);
  ASSERT_TRUE(packet_stream_client.is_valid());
  received_callback = false;

  packet_stream_client->Stop({}).Then([&received_callback, &fake_driver,
                                       element_id](fidl::Result<fad::PacketStream::Stop>& result) {
    EXPECT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_FALSE(fake_driver->PacketStreamStarted(element_id));
    received_callback = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  received_callback = false;

  packet_stream_client->Stop({}).Then(
      [&received_callback](fidl::Result<fad::PacketStream::Stop>& result) {
        ASSERT_TRUE(result.is_error());
        ASSERT_TRUE(result.error_value().is_domain_error()) << result.error_value();
        EXPECT_EQ(result.error_value().domain_error(), fad::PacketStreamStopError::kAlreadyStopped)
            << result.error_value();
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());
  EXPECT_TRUE(control->client().is_valid());
}

// Test SetBuffers when RegisterInfo is missing vmo_infos.
TEST_F(PacketStreamServerCompositeWarningTest, SetBuffersRegisterVmosMissingFields) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);

  received_callback = false;
  fad::PacketStreamSetBuffersRequest request;
  fha::RegisterVmosConfig register_vmos_config;
  // Missing vmo_infos
  request.vmo_info(
      fad::PacketStreamSetupVmoInfo::WithRegisterInfo(std::move(register_vmos_config)));

  packet_stream_client->SetBuffers(std::move(request))
      .Then([&received_callback](fidl::Result<fad::PacketStream::SetBuffers>& result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_TRUE(result.error_value().is_domain_error());
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::PacketStreamSetBufferError::kBadVmoConfig);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());
  EXPECT_TRUE(control->client().is_valid());
}

// Test SetBuffers when AllocateInfo is missing fields.
TEST_F(PacketStreamServerCompositeWarningTest, SetBuffersAllocateVmosMissingFields) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);

  received_callback = false;
  fad::PacketStreamSetBuffersRequest request;
  fha::AllocateVmosConfig allocate_vmos_config;
  // Missing vmo_count and min_vmo_size
  request.vmo_info(
      fad::PacketStreamSetupVmoInfo::WithAllocateInfo(std::move(allocate_vmos_config)));

  packet_stream_client->SetBuffers(std::move(request))
      .Then([&received_callback](fidl::Result<fad::PacketStream::SetBuffers>& result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_TRUE(result.error_value().is_domain_error());
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::PacketStreamSetBufferError::kBadVmoConfig);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());
  EXPECT_TRUE(control->client().is_valid());
}

// Test SetBuffers when RegisterInfo VMO lacks ZX_RIGHT_DUPLICATE
TEST_F(PacketStreamServerCompositeWarningTest, SetBuffersRegisterVmosMissingDuplicateRight) {
  auto fake_driver = CreateFakeComposite();
  auto element_id = FakeComposite::kMaxPacketStreamElementId;
  auto device = EnableDriverAndAddDevice(fake_driver);
  auto format = SafePcmPacketStreamFormat(element_id, device->packet_stream_format_sets());
  auto registry = CreateTestRegistryServer();

  auto token_id = WaitForAddedDeviceTokenId(registry->client());
  ASSERT_TRUE(token_id);
  auto [status, added_device] = adr_service()->FindDeviceByTokenId(*token_id);
  auto control = CreateTestControlServer(added_device);
  auto [packet_stream_client, packet_stream_server_end] = CreatePacketStreamClient();
  bool received_callback = false;

  control->client()
      ->CreatePacketStream({{
          element_id,
          fad::PacketStreamOptions{{.format = format}},
          std::move(packet_stream_server_end),
      }})
      .Then([&received_callback](fidl::Result<fad::Control::CreatePacketStream>& result) {
        received_callback = true;
      });

  RunLoopUntilIdle();
  ASSERT_TRUE(received_callback);

  received_callback = false;

  zx::vmo vmo, vmo_no_dup;
  ASSERT_EQ(ZX_OK, zx::vmo::create(8192, 0, &vmo));
  ASSERT_EQ(ZX_OK, vmo.replace(kRequiredVmoRightsForRead, &vmo_no_dup));

  fad::PacketStreamSetBuffersRequest request;
  fha::RegisterVmosConfig register_vmos_config;
  fha::VmoInfo vmo_info;
  vmo_info.id(0);
  vmo_info.vmo(std::move(vmo_no_dup));
  std::vector<fha::VmoInfo> vmo_infos;
  vmo_infos.push_back(std::move(vmo_info));
  register_vmos_config.vmo_infos(std::move(vmo_infos));
  request.vmo_info(
      fad::PacketStreamSetupVmoInfo::WithRegisterInfo(std::move(register_vmos_config)));

  packet_stream_client->SetBuffers(std::move(request))
      .Then([&received_callback](fidl::Result<fad::PacketStream::SetBuffers>& result) {
        ASSERT_TRUE(result.is_error());
        EXPECT_TRUE(result.error_value().is_domain_error());
        EXPECT_EQ(result.error_value().domain_error(),
                  fad::PacketStreamSetBufferError::kBadVmoConfig);
        received_callback = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(received_callback);
  EXPECT_TRUE(packet_stream_client.is_valid());
  EXPECT_TRUE(control->client().is_valid());
}

}  // namespace
}  // namespace media_audio
