// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/audio_device_registry.h"

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <utility>

#include "src/media/audio/services/common/fidl_thread.h"
#include "src/media/audio/services/device_registry/control_creator_server.h"
#include "src/media/audio/services/device_registry/control_notify.h"
#include "src/media/audio/services/device_registry/control_server.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/device_detector.h"
#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/observer_server.h"
#include "src/media/audio/services/device_registry/packet_stream_server.h"
#include "src/media/audio/services/device_registry/provider_server.h"
#include "src/media/audio/services/device_registry/registry_server.h"
#include "src/media/audio/services/device_registry/ring_buffer_server.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;

AudioDeviceRegistry::AudioDeviceRegistry(std::shared_ptr<FidlThread> server_thread)
    : thread_(std::move(server_thread)),
      outgoing_(component::OutgoingDirectory(thread_->dispatcher())) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
}

AudioDeviceRegistry::~AudioDeviceRegistry() { ADR_LOG_METHOD(kLogObjectLifetimes); }

zx_status_t AudioDeviceRegistry::StartDeviceDetection(bool ignore_devices) {
  auto detector_result = media_audio::DeviceDetector::Create(
      [this](std::string_view name, fad::DeviceType device_type, fad::DriverClient driver_client) {
        DeviceDetected(name, device_type, std::move(driver_client));
      },
      [this]() { InitialDeviceDetectionComplete(); }, thread_->dispatcher(), ignore_devices);

  if (detector_result.is_ok()) {
    device_detector_ = detector_result.value();
  }

  ADR_LOG_METHOD(kLogDeviceDetection) << "returning " << detector_result.status_string();
  return detector_result.status_value();
}

void AudioDeviceRegistry::DeviceDetected(std::string_view name, fad::DeviceType device_type,
                                         fad::DriverClient driver_client) {
  ADR_LOG_OBJECT(kLogDeviceDetection) << "detected Audio " << device_type << " '" << name << "'";

  switch (driver_client.Which()) {
    case fad::DriverClient::Tag::kCodec:
      FX_CHECK(driver_client.codec()->is_valid());
      break;
    case fad::DriverClient::Tag::kComposite:
      FX_CHECK(driver_client.composite()->is_valid());
      break;
    default:
      ADR_WARN_OBJECT() << "Device detected but not supported";
      return;
  }
  AddDevice(Device::Create(this->shared_from_this(), thread_->dispatcher(), name, device_type,
                           std::move(driver_client), "DevFS"));
}

void AudioDeviceRegistry::AddDevice(const std::shared_ptr<Device>& initializing_device) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods);
  if (initial_device_detection_complete_) {
    pending_devices_.insert(initializing_device);
  } else {
    initial_pending_devices_.insert(initializing_device);
  }
}

// The DeviceDetector has notified us of any devices that existed when device detection began.
// (It will continue to notify us if additional devices are detected.)
// These initially-detected devices might still be in the initialization process, but if not, then
// notify all Registry servers that they may begin responding to WatchDevicesAdded calls.
void AudioDeviceRegistry::InitialDeviceDetectionComplete() {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods);

  FX_DCHECK(!initial_device_detection_complete_);
  initial_device_detection_complete_ = true;

  MaybeNotifyInitialDevicesAvailable();
}

// A device has completed initialization and can be moved into the set of "ready" devices.
// If this was an initially-detected device, then check whether we can now allow Registry servers to
// respond to WatchDevicesAdded calls.
void AudioDeviceRegistry::DeviceIsReady(std::shared_ptr<media_audio::Device> ready_device) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods) << "for device " << ready_device;

  bool initial_device_is_now_ready = initial_pending_devices_.erase(ready_device);
  bool subsequent_device_is_now_ready = pending_devices_.erase(ready_device);
  FX_DCHECK(!(initial_device_is_now_ready && subsequent_device_is_now_ready))
      << __func__ << " inconsistency error: device " << ready_device
      << " found in both pending lists";
  if (!initial_device_is_now_ready && !subsequent_device_is_now_ready) {
    FX_LOGS(ERROR) << __func__ << ": device " << ready_device
                   << " not found in either pending list";
  }
  // We don't check the removed device list, because TokenIds are uint64_t created incrementally;
  // it is galactically improbable that TokenIds will ever rollover. We don't check the unhealthy
  // device list, because (for now) an unhealthy device cannot "get well"; it must be entirely
  // powered-down/removed before it can rejoin the party (thus see "removed device list" above).

  if (!devices_.insert(ready_device).second) {
    FX_LOGS(ERROR) << __func__ << ": device " << ready_device << " already in initialized list";
    return;
  }

  // Notify registry servers of this new device (even if they can't yet notify their clients).
  for (auto& weak_registry : registries_) {
    if (auto registry = weak_registry.lock(); registry) {
      registry->DeviceWasAdded(ready_device);
    }
  }

  // If this was one of the preexisting devices, then this may be the last one needed for us to
  // consider initial device discovery complete. Check for that (and if so, notify all existing
  // Registry servers that they may respond to WatchDevicesAdded now).
  if (initial_device_is_now_ready) {
    MaybeNotifyInitialDevicesAvailable();
  }
}

// Either initial device detection just completed, or a device is now ready. If initial detection is
// done, and all of the initially-detected devices are now ready, then (one-time) notify existing
// Registry servers that they can start responding to WatchDevicesAdded. We also set the flag
// all_initial_devices_ready_ so we inform subsequent Registry servers immediately on creation.
void AudioDeviceRegistry::MaybeNotifyInitialDevicesAvailable() {
  if (!all_initial_devices_ready_ && initial_device_detection_complete_ &&
      initial_pending_devices_.empty()) {
    all_initial_devices_ready_ = true;

    for (auto& weak_registry : registries_) {
      if (auto registry = weak_registry.lock(); registry) {
        registry->InitialDeviceDiscoveryIsComplete();
      }
    }
  }
}

void AudioDeviceRegistry::DeviceHasError(std::shared_ptr<media_audio::Device> device_with_error) {
  if (devices_.erase(device_with_error)) {
    ADR_WARN_METHOD() << "for previously-initialized device " << device_with_error;

    // Device should have already notified any other associated objects.
    NotifyRegistriesOfDeviceRemoval(device_with_error->token_id());
  }

  if (pending_devices_.erase(device_with_error)) {
    ADR_WARN_METHOD() << "for pending (initializing) device " << device_with_error;
  }

  if (!unhealthy_devices_.insert(device_with_error).second) {
    ADR_WARN_METHOD() << "device " << device_with_error
                      << " had previously encountered an error - no change";
    return;
  }

  ADR_WARN_METHOD() << "added device " << device_with_error << " to unhealthy list";
}

// Entirely remove the device, including from the unhealthy list.
void AudioDeviceRegistry::DeviceIsRemoved(std::shared_ptr<media_audio::Device> device_to_remove) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods) << "for device " << device_to_remove;

  if (devices_.erase(device_to_remove)) {
    // Device should have already notified any other associated objects.
    NotifyRegistriesOfDeviceRemoval(device_to_remove->token_id());

    ADR_LOG_METHOD(kLogObjectLifetimes)
        << "removed " << device_to_remove << " from active device list";
  }

  if (pending_devices_.erase(device_to_remove)) {
    ADR_LOG_METHOD(kLogObjectLifetimes)
        << "removed " << device_to_remove << " from pending (initializing) device list";
  }

  if (unhealthy_devices_.erase(device_to_remove)) {
    ADR_LOG_METHOD(kLogObjectLifetimes)
        << "removed " << device_to_remove << " from unhealthy device list";
  }
}

std::pair<AudioDeviceRegistry::DevicePresence, std::shared_ptr<Device>>
AudioDeviceRegistry::FindDeviceByTokenId(TokenId token_id) {
  for (auto& device : devices_) {
    if (device->token_id() == token_id) {
      ADR_LOG_STATIC(kLogAudioDeviceRegistryMethods) << "active (token_id " << token_id << ")";
      return std::make_pair(DevicePresence::Active, device);
    }
  }
  for (auto& device : unhealthy_devices_) {
    if (device->token_id() == token_id) {
      ADR_WARN_METHOD() << "device (token_id " << token_id << ") has an error";
      return std::make_pair(DevicePresence::Error, nullptr);
    }
  }
  // For initializing devices and completely unknown devices, we return the same error.
  for (auto& device : pending_devices_) {
    if (device->token_id() == token_id) {
      ADR_WARN_METHOD() << "device not yet ready (token_id " << token_id << ")";
      break;
    }
  }
  ADR_WARN_METHOD() << "no known device (token_id " << token_id << ")";
  return std::make_pair(DevicePresence::Unknown, nullptr);
}

bool AudioDeviceRegistry::ClaimDeviceForControl(const std::shared_ptr<Device>& device,
                                                std::shared_ptr<ControlNotify> notify) {
  return device->SetControl(std::move(notify));
}

// Notify registry clients of this device departure (whether from surprise-removal or error).
void AudioDeviceRegistry::NotifyRegistriesOfDeviceRemoval(TokenId removed_device_id) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods);

  for (auto& weak_registry : registries_) {
    if (auto registry = weak_registry.lock(); registry) {
      registry->DeviceWasRemoved(removed_device_id);
    }
  }
}

zx_status_t AudioDeviceRegistry::RegisterAndServeOutgoing() {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods);

  auto status = outgoing_.AddUnmanagedProtocol<fad::Provider>(
      [this](fidl::ServerEnd<fad::Provider> server_end) mutable {
        ADR_LOG_OBJECT(kLogProviderServerMethods)
            << "Incoming connection for fuchsia.audio.device.Provider";

        auto provider = CreateProviderServer(std::move(server_end));
      });

  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Provider protocol: " << status.error_value() << " ("
                   << status.status_string() << ")";
    return status.status_value();
  }

  status = outgoing_.AddUnmanagedProtocol<fad::Registry>(
      [this](fidl::ServerEnd<fad::Registry> server_end) mutable {
        ADR_LOG_OBJECT(kLogRegistryServerMethods)
            << "Incoming connection for fuchsia.audio.device.Registry";

        auto registry = CreateRegistryServer(std::move(server_end));
      });
  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Registry protocol: " << status.error_value() << " ("
                   << status.status_string() << ")";
    return status.status_value();
  }

  status = outgoing_.AddUnmanagedProtocol<fad::ControlCreator>(
      [this](fidl::ServerEnd<fad::ControlCreator> server_end) mutable {
        ADR_LOG_OBJECT(kLogControlCreatorServerMethods)
            << "Incoming connection for fuchsia.audio.device.ControlCreator";

        auto control_creator = CreateControlCreatorServer(std::move(server_end));
      });
  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to add ControlCreator protocol: " << status.error_value() << " ("
                   << status.status_string() << ")";
    return status.status_value();
  }

  // Set up an outgoing directory with the startup handle (provided by the system to components so
  // they can serve out FIDL protocols etc).
  auto result = outgoing_.ServeFromStartupInfo();

  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods) << "returning " << result.status_string();
  return result.status_value();
}

std::shared_ptr<RegistryServer> AudioDeviceRegistry::CreateRegistryServer(
    fidl::ServerEnd<fad::Registry> server_end) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods || kLogRegistryServerMethods);

  auto new_registry = RegistryServer::Create(thread_, std::move(server_end), shared_from_this());
  registries_.push_back(new_registry);

  for (const auto& device : devices()) {
    new_registry->DeviceWasAdded(device);
  }
  // Notify new_registry whether it can start responding to WatchDevicesAdded calls.
  if (all_initial_devices_ready_) {
    new_registry->InitialDeviceDiscoveryIsComplete();
  }
  return new_registry;
}

std::shared_ptr<ObserverServer> AudioDeviceRegistry::CreateObserverServer(
    fidl::ServerEnd<fad::Observer> server_end, const std::shared_ptr<Device>& observed_device) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods || kLogObserverServerMethods);

  auto observer = ObserverServer::Create(thread_, std::move(server_end), observed_device);
  observed_device->AddObserver(observer);
  return observer;
}

std::shared_ptr<ControlServer> AudioDeviceRegistry::CreateControlServer(
    fidl::ServerEnd<fad::Control> server_end, const std::shared_ptr<Device>& device_to_control) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods || kLogControlServerMethods);

  auto control =
      ControlServer::Create(thread_, std::move(server_end), shared_from_this(), device_to_control);

  if (device_to_control->SetControl(control)) {
    return control;
  }
  control->Shutdown();
  return nullptr;
}

// These subsequent methods simply call [Provider|ControlCreator|RingBuffer]Server::Create directly,
// but they mirror similar methods (above) for FIDL Server classes that do more.
std::shared_ptr<ProviderServer> AudioDeviceRegistry::CreateProviderServer(
    fidl::ServerEnd<fad::Provider> server_end) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods || kLogProviderServerMethods);
  return ProviderServer::Create(thread_, std::move(server_end), shared_from_this());
}

std::shared_ptr<ControlCreatorServer> AudioDeviceRegistry::CreateControlCreatorServer(
    fidl::ServerEnd<fad::ControlCreator> server_end) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods || kLogControlCreatorServerMethods);
  return ControlCreatorServer::Create(thread_, std::move(server_end), shared_from_this());
}

std::shared_ptr<RingBufferServer> AudioDeviceRegistry::CreateRingBufferServer(
    fidl::ServerEnd<fad::RingBuffer> server_end, const std::shared_ptr<ControlServer>& parent,
    const std::shared_ptr<Device>& device_to_control, ElementId element_id) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods || kLogRingBufferServerMethods);
  return RingBufferServer::Create(thread_, std::move(server_end), parent, device_to_control,
                                  element_id);
}

std::shared_ptr<PacketStreamServer> AudioDeviceRegistry::CreatePacketStreamServer(
    fidl::ServerEnd<fad::PacketStream> server_end, const std::shared_ptr<ControlServer>& parent,
    const std::shared_ptr<Device>& device_to_control, ElementId element_id) {
  ADR_LOG_METHOD(kLogAudioDeviceRegistryMethods || kLogPacketStreamServerMethods);
  return PacketStreamServer::Create(thread_, std::move(server_end), parent, device_to_control,
                                    element_id);
}

}  // namespace media_audio
