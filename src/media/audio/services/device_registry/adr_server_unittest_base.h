// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_ADR_SERVER_UNITTEST_BASE_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_ADR_SERVER_UNITTEST_BASE_H_

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>

#include <memory>
#include <optional>
#include <string_view>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/media/audio/services/common/fidl_thread.h"
#include "src/media/audio/services/common/testing/test_server_and_async_client.h"
#include "src/media/audio/services/device_registry/audio_device_registry.h"
#include "src/media/audio/services/device_registry/control_creator_server.h"
#include "src/media/audio/services/device_registry/control_server.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/observer_server.h"
#include "src/media/audio/services/device_registry/provider_server.h"
#include "src/media/audio/services/device_registry/registry_server.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_codec.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"
#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {

inline void LogFidlClientError(fidl::UnbindInfo error, const std::string& tag = "") {
  if (error.status() != ZX_OK && error.status() != ZX_ERR_PEER_CLOSED) {
    FX_LOGS(WARNING) << tag << ":" << error;
  } else {
    FX_LOGS(DEBUG) << tag << ":" << error;
  }
}

// This provides shared unittest functions for AudioDeviceRegistry and the six FIDL server classes.
class AudioDeviceRegistryServerTestBase : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    // Use our production Inspector during device unittests.
    media_audio::Inspector::Initialize(dispatcher());
  }

 protected:
  static inline const std::string kClassName = "AudioDeviceRegistryServerTestBase";

  // Create a FakeCodec that can mock a real device that has been detected, using default settings.
  // From here, the fake Codec can be customized before it is enabled.
  std::shared_ptr<FakeCodec> CreateFakeCodecInput() { return CreateFakeCodec(true); }
  std::shared_ptr<FakeCodec> CreateFakeCodecOutput() { return CreateFakeCodec(false); }
  std::shared_ptr<FakeCodec> CreateFakeCodecNoDirection() { return CreateFakeCodec(std::nullopt); }

  // Create a FakeComposite that can mock a real device that has been detected, using default
  // settings. From here, the fake Composite can be customized before it is enabled.
  std::shared_ptr<FakeComposite> CreateFakeComposite() {
    EXPECT_EQ(dispatcher(), test_loop().dispatcher());
    auto composite_endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::Composite>();
    EXPECT_TRUE(composite_endpoints.is_ok());
    return std::make_shared<FakeComposite>(composite_endpoints->server.TakeChannel(),
                                           composite_endpoints->client.TakeChannel(), dispatcher());
  }

  // Device
  // Create a Device object (backed by a fake driver); insert it to ADR as if it had been detected.
  // Through the driver_client connection, this will communicate with the fake driver.
  void AddDeviceForDetection(std::string_view name, fuchsia_audio_device::DeviceType device_type,
                             fuchsia_audio_device::DriverClient driver_client) {
    ASSERT_TRUE(ClientIsValidForDeviceType(device_type, driver_client));
    adr_service_->AddDevice(Device::Create(adr_service_, dispatcher(), name, device_type,
                                           std::move(driver_client), kClassName));
  }

  static const std::unordered_map<TopologyId,
                                  std::vector<fuchsia_hardware_audio_signalprocessing::EdgePair>>&
  topology_map(const std::shared_ptr<Device>& device) {
    return device->sig_proc_topology_map_;
  }
  static const std::unordered_map<ElementId, ElementRecord>& element_map(
      const std::shared_ptr<Device>& device) {
    return device->sig_proc_element_map_;
  }

  class FidlHandler {
   public:
    explicit FidlHandler(AudioDeviceRegistryServerTestBase* parent) : parent_(parent) {}

   protected:
    AudioDeviceRegistryServerTestBase* parent() const { return parent_; }

   private:
    AudioDeviceRegistryServerTestBase* parent_;
  };

  // Provider support
  std::unique_ptr<TestServerAndNaturalAsyncClient<ProviderServer>> CreateTestProviderServer() {
    auto [client_end, server_end] = CreateNaturalAsyncClientOrDie<fuchsia_audio_device::Provider>();
    auto server = adr_service_->CreateProviderServer(std::move(server_end));
    provider_fidl_error_status().reset();
    auto client = fidl::Client<fuchsia_audio_device::Provider>(std::move(client_end), dispatcher(),
                                                               provider_fidl_handler_.get());
    return std::make_unique<TestServerAndNaturalAsyncClient<ProviderServer>>(
        test_loop(), std::move(server), std::move(client));
  }
  class ProviderFidlHandler final : public fidl::AsyncEventHandler<fuchsia_audio_device::Provider>,
                                    public FidlHandler {
   public:
    explicit ProviderFidlHandler(AudioDeviceRegistryServerTestBase* parent) : FidlHandler(parent) {}
    void on_fidl_error(fidl::UnbindInfo error) override {
      LogFidlClientError(error, "Provider");
      parent()->provider_fidl_error_status_ = error.status();
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_audio_device::Provider> metadata) override {
      FX_LOGS(WARNING) << "ProviderFidlHandler: unknown event (Provider) ordinal "
                       << metadata.event_ordinal;
    }
  };
  std::optional<zx_status_t>& provider_fidl_error_status() { return provider_fidl_error_status_; }

  // Registry support
  std::optional<TokenId> WaitForAddedDeviceTokenId(
      fidl::Client<fuchsia_audio_device::Registry>& reg_client) {
    std::optional<TokenId> added_device_id;
    reg_client->WatchDevicesAdded().Then(
        [&added_device_id](
            fidl::Result<fuchsia_audio_device::Registry::WatchDevicesAdded>& result) mutable {
          ASSERT_TRUE(result.is_ok()) << result.error_value();
          ASSERT_TRUE(result->devices());
          ASSERT_EQ(result->devices()->size(), 1u);
          ASSERT_TRUE(result->devices()->at(0).token_id());
          added_device_id = *result->devices()->at(0).token_id();
        });

    RunLoopUntilIdle();
    return added_device_id;
  }

  std::unique_ptr<TestServerAndNaturalAsyncClient<RegistryServer>>
  CreateTestRegistryServerNoDeviceDiscovery() {
    auto [client_end, server_end] = CreateNaturalAsyncClientOrDie<fuchsia_audio_device::Registry>();
    auto server = adr_service_->CreateRegistryServer(std::move(server_end));
    registry_fidl_error_status().reset();
    auto client = fidl::Client<fuchsia_audio_device::Registry>(std::move(client_end), dispatcher(),
                                                               registry_fidl_handler_.get());
    return std::make_unique<TestServerAndNaturalAsyncClient<RegistryServer>>(
        test_loop(), std::move(server), std::move(client));
  }
  std::unique_ptr<TestServerAndNaturalAsyncClient<RegistryServer>> CreateTestRegistryServer() {
    auto registry = CreateTestRegistryServerNoDeviceDiscovery();
    registry->server().InitialDeviceDiscoveryIsComplete();
    return registry;
  }
  class RegistryFidlHandler final : public fidl::AsyncEventHandler<fuchsia_audio_device::Registry>,
                                    public FidlHandler {
   public:
    explicit RegistryFidlHandler(AudioDeviceRegistryServerTestBase* parent) : FidlHandler(parent) {}
    void on_fidl_error(fidl::UnbindInfo error) override {
      LogFidlClientError(error, "Registry");
      parent()->registry_fidl_error_status_ = error.status();
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_audio_device::Registry> metadata) override {
      FX_LOGS(WARNING) << "RegistryFidlHandler: unknown event (Registry) ordinal "
                       << metadata.event_ordinal;
    }
  };
  std::optional<zx_status_t>& registry_fidl_error_status() { return registry_fidl_error_status_; }

  // ControlCreator support
  std::unique_ptr<TestServerAndNaturalAsyncClient<ControlCreatorServer>>
  CreateTestControlCreatorServer() {
    auto [client_end, server_end] =
        CreateNaturalAsyncClientOrDie<fuchsia_audio_device::ControlCreator>();
    auto server = adr_service_->CreateControlCreatorServer(std::move(server_end));
    control_creator_fidl_error_status().reset();
    auto client = fidl::Client<fuchsia_audio_device::ControlCreator>(
        std::move(client_end), dispatcher(), control_creator_fidl_handler_.get());
    return std::make_unique<TestServerAndNaturalAsyncClient<ControlCreatorServer>>(
        test_loop(), std::move(server), std::move(client));
  }
  class ControlCreatorFidlHandler final
      : public fidl::AsyncEventHandler<fuchsia_audio_device::ControlCreator>,
        public FidlHandler {
   public:
    explicit ControlCreatorFidlHandler(AudioDeviceRegistryServerTestBase* parent)
        : FidlHandler(parent) {}
    void on_fidl_error(fidl::UnbindInfo error) override {
      LogFidlClientError(error, "ControlCreator");
      parent()->control_creator_fidl_error_status_ = error.status();
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_audio_device::ControlCreator> metadata) override {
      FX_LOGS(WARNING) << "ControlCreatorFidlHandler: unknown event (ControlCreator) ordinal "
                       << metadata.event_ordinal;
    }
  };
  std::optional<zx_status_t>& control_creator_fidl_error_status() {
    return control_creator_fidl_error_status_;
  }

  // Observer support
  std::unique_ptr<TestServerAndNaturalAsyncClient<ObserverServer>> CreateTestObserverServer(
      const std::shared_ptr<Device>& observed_device) {
    auto [client_end, server_end] = CreateNaturalAsyncClientOrDie<fuchsia_audio_device::Observer>();
    auto server = adr_service_->CreateObserverServer(std::move(server_end), observed_device);
    observer_fidl_error_status().reset();
    auto client = fidl::Client<fuchsia_audio_device::Observer>(std::move(client_end), dispatcher(),
                                                               observer_fidl_handler().get());
    return std::make_unique<TestServerAndNaturalAsyncClient<ObserverServer>>(
        test_loop(), std::move(server), std::move(client));
  }
  class ObserverFidlHandler final : public fidl::AsyncEventHandler<fuchsia_audio_device::Observer>,
                                    public FidlHandler {
   public:
    explicit ObserverFidlHandler(AudioDeviceRegistryServerTestBase* parent) : FidlHandler(parent) {}
    // Invoked when the underlying driver disconnects its driver_client protocol.
    void on_fidl_error(fidl::UnbindInfo error) override {
      LogFidlClientError(error, "Observer");
      parent()->observer_fidl_error_status_ = error.status();
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_audio_device::Observer> metadata) override {
      FAIL() << "ObserverFidlHandler: unknown event (Observer) ordinal " << metadata.event_ordinal;
    }
  };
  const std::unique_ptr<ObserverFidlHandler>& observer_fidl_handler() {
    return observer_fidl_handler_;
  }
  std::optional<zx_status_t>& observer_fidl_error_status() { return observer_fidl_error_status_; }

  // Control support
  std::unique_ptr<TestServerAndNaturalAsyncClient<ControlServer>> CreateTestControlServer(
      const std::shared_ptr<Device>& device_to_control) {
    auto [client_end, server_end] = CreateNaturalAsyncClientOrDie<fuchsia_audio_device::Control>();
    auto server = adr_service_->CreateControlServer(std::move(server_end), device_to_control);
    FX_CHECK(server) << "ControlServer is NULL";
    control_fidl_error_status().reset();
    auto client = fidl::Client<fuchsia_audio_device::Control>(std::move(client_end), dispatcher(),
                                                              control_fidl_handler().get());
    return std::make_unique<TestServerAndNaturalAsyncClient<ControlServer>>(
        test_loop(), std::move(server), std::move(client));
  }
  class ControlFidlHandler final : public fidl::AsyncEventHandler<fuchsia_audio_device::Control>,
                                   public FidlHandler {
   public:
    explicit ControlFidlHandler(AudioDeviceRegistryServerTestBase* parent) : FidlHandler(parent) {}
    void on_fidl_error(fidl::UnbindInfo error) override {
      LogFidlClientError(error, "Control");
      parent()->control_fidl_error_status_ = error.status();
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_audio_device::Control> metadata) override {
      FAIL() << "ControlFidlHandler: unknown event (Control) ordinal " << metadata.event_ordinal;
    }
  };
  const std::unique_ptr<ControlFidlHandler>& control_fidl_handler() {
    return control_fidl_handler_;
  }
  std::optional<zx_status_t>& control_fidl_error_status() { return control_fidl_error_status_; }

  // RingBuffer support
  class RingBufferFidlHandler final
      : public fidl::AsyncEventHandler<fuchsia_audio_device::RingBuffer>,
        public FidlHandler {
   public:
    explicit RingBufferFidlHandler(AudioDeviceRegistryServerTestBase* parent)
        : FidlHandler(parent) {}
    void on_fidl_error(fidl::UnbindInfo error) override {
      LogFidlClientError(error, "RingBuffer");
      parent()->ring_buffer_fidl_error_status_ = error.status();
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_audio_device::RingBuffer> metadata) override {
      FX_LOGS(WARNING) << "RingBufferFidlHandler: unknown event (RingBuffer) ordinal "
                       << metadata.event_ordinal;
    }
  };
  std::optional<zx_status_t>& ring_buffer_fidl_error_status() {
    return ring_buffer_fidl_error_status_;
  }
  const std::unique_ptr<RingBufferFidlHandler>& ring_buffer_fidl_handler() {
    return ring_buffer_fidl_handler_;
  }

  // PacketStream support
  class PacketStreamFidlHandler final
      : public fidl::AsyncEventHandler<fuchsia_audio_device::PacketStream>,
        public FidlHandler {
   public:
    explicit PacketStreamFidlHandler(AudioDeviceRegistryServerTestBase* parent)
        : FidlHandler(parent) {}
    void on_fidl_error(fidl::UnbindInfo error) override {
      LogFidlClientError(error, "PacketStream");
      parent()->packet_stream_fidl_error_status_ = error.status();
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_audio_device::PacketStream> metadata) override {
      FX_LOGS(WARNING) << "PacketStreamFidlHandler: unknown event (PacketStream) ordinal "
                       << metadata.event_ordinal;
    }
  };
  std::optional<zx_status_t>& packet_stream_fidl_error_status() {
    return packet_stream_fidl_error_status_;
  }
  const std::unique_ptr<PacketStreamFidlHandler>& packet_stream_fidl_handler() {
    return packet_stream_fidl_handler_;
  }

  // General members
  std::shared_ptr<media_audio::AudioDeviceRegistry> adr_service() { return adr_service_; }
  void ClearRegistry() { adr_service_ = std::make_shared<AudioDeviceRegistry>(server_thread_); }

 private:
  std::unique_ptr<ProviderFidlHandler> provider_fidl_handler_ =
      std::make_unique<ProviderFidlHandler>(static_cast<AudioDeviceRegistryServerTestBase*>(this));
  std::optional<zx_status_t> provider_fidl_error_status_;

  std::unique_ptr<RegistryFidlHandler> registry_fidl_handler_ =
      std::make_unique<RegistryFidlHandler>(static_cast<AudioDeviceRegistryServerTestBase*>(this));
  std::optional<zx_status_t> registry_fidl_error_status_;

  std::unique_ptr<ControlCreatorFidlHandler> control_creator_fidl_handler_ =
      std::make_unique<ControlCreatorFidlHandler>(
          static_cast<AudioDeviceRegistryServerTestBase*>(this));
  std::optional<zx_status_t> control_creator_fidl_error_status_;

  std::unique_ptr<ObserverFidlHandler> observer_fidl_handler_ =
      std::make_unique<ObserverFidlHandler>(static_cast<AudioDeviceRegistryServerTestBase*>(this));
  std::optional<zx_status_t> observer_fidl_error_status_;

  std::unique_ptr<ControlFidlHandler> control_fidl_handler_ =
      std::make_unique<ControlFidlHandler>(static_cast<AudioDeviceRegistryServerTestBase*>(this));
  std::optional<zx_status_t> control_fidl_error_status_;

  std::unique_ptr<RingBufferFidlHandler> ring_buffer_fidl_handler_ =
      std::make_unique<RingBufferFidlHandler>(
          static_cast<AudioDeviceRegistryServerTestBase*>(this));
  std::optional<zx_status_t> ring_buffer_fidl_error_status_;

  std::unique_ptr<PacketStreamFidlHandler> packet_stream_fidl_handler_ =
      std::make_unique<PacketStreamFidlHandler>(
          static_cast<AudioDeviceRegistryServerTestBase*>(this));
  std::optional<zx_status_t> packet_stream_fidl_error_status_;

  std::shared_ptr<FidlThread> server_thread_ =
      FidlThread::CreateFromCurrentThread("test_server_thread", dispatcher());

  std::shared_ptr<media_audio::AudioDeviceRegistry> adr_service_ =
      std::make_shared<media_audio::AudioDeviceRegistry>(server_thread_);

  std::shared_ptr<FakeCodec> CreateFakeCodec(std::optional<bool> is_input = false) {
    EXPECT_EQ(dispatcher(), test_loop().dispatcher());
    auto codec_endpoints = fidl::Endpoints<fuchsia_hardware_audio::Codec>::Create();
    auto fake_codec = std::make_shared<FakeCodec>(
        codec_endpoints.server.TakeChannel(), codec_endpoints.client.TakeChannel(), dispatcher());
    fake_codec->set_is_input(is_input);
    return fake_codec;
  }
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_ADR_SERVER_UNITTEST_BASE_H_
