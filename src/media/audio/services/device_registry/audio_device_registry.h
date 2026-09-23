// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_AUDIO_DEVICE_REGISTRY_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_AUDIO_DEVICE_REGISTRY_H_

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>

#include <memory>
#include <unordered_set>

#include "src/media/audio/services/common/fidl_thread.h"
#include "src/media/audio/services/common/vector_of_weak_ptr.h"
#include "src/media/audio/services/device_registry/basic_types.h"
#include "src/media/audio/services/device_registry/control_notify.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/device_detector.h"
#include "src/media/audio/services/device_registry/device_presence_watcher.h"

namespace media_audio {

class ControlCreatorServer;
class ControlServer;
class ObserverServer;
class PacketStreamServer;
class ProviderServer;
class RegistryServer;
class RingBufferServer;

// This singleton coordinates device detection, serves the outgoing FIDL, and maintains lists of
// pending, active and unhealthy Devices. The object should live for the duration of the service.
class AudioDeviceRegistry : public std::enable_shared_from_this<AudioDeviceRegistry>,
                            public DevicePresenceWatcher {
 public:
  explicit AudioDeviceRegistry(std::shared_ptr<FidlThread> server_thread);
  ~AudioDeviceRegistry() override;

  // Kick off device-detection. After AddDevice, the device auto-initializes and then calls one of
  // DeviceIsReady, DeviceHasError or DeviceIsRemoved.
  zx_status_t StartDeviceDetection(bool ignore_devices);
  void InitialDeviceDetectionComplete();

  zx_status_t RegisterAndServeOutgoing();

  // Device support
  // The set of active (successfully initialized) devices.
  const std::unordered_set<std::shared_ptr<Device>>& devices() { return devices_; }
  // The set of devices that encountered an error. A device should never be in both sets.
  const std::unordered_set<std::shared_ptr<Device>>& unhealthy_devices() {
    return unhealthy_devices_;
  }

  // Add a newly-constructed Device object to our "initializing" list.
  void AddDevice(const std::shared_ptr<Device>& initializing_device);

  enum class DevicePresence : uint8_t { Unknown, Active, Error };
  std::pair<DevicePresence, std::shared_ptr<Device>> FindDeviceByTokenId(TokenId token_id);

  static bool ClaimDeviceForControl(const std::shared_ptr<Device>& device,
                                    std::shared_ptr<ControlNotify> notify);

  // DevicePresenceWatcher interface -- called by Devices when they change state.
  // Add a successfully-initialized Device to our active list.
  void DeviceIsReady(std::shared_ptr<Device> ready_device) final;
  void DeviceHasError(std::shared_ptr<Device> device_with_error) final;
  void DeviceIsRemoved(std::shared_ptr<Device> device_to_remove) final;

  // Provider support
  std::shared_ptr<ProviderServer> CreateProviderServer(
      fidl::ServerEnd<fuchsia_audio_device::Provider> server_end);

  // Registry support
  std::shared_ptr<RegistryServer> CreateRegistryServer(
      fidl::ServerEnd<fuchsia_audio_device::Registry> server_end);

  // ControlCreator support
  std::shared_ptr<ControlCreatorServer> CreateControlCreatorServer(
      fidl::ServerEnd<fuchsia_audio_device::ControlCreator> server_end);

  // Observer support
  std::shared_ptr<ObserverServer> CreateObserverServer(
      fidl::ServerEnd<fuchsia_audio_device::Observer> server_end,
      const std::shared_ptr<Device>& observed_device);

  // Control support
  std::shared_ptr<ControlServer> CreateControlServer(
      fidl::ServerEnd<fuchsia_audio_device::Control> server_end,
      const std::shared_ptr<Device>& device_to_control);

  // RingBuffer support
  std::shared_ptr<RingBufferServer> CreateRingBufferServer(
      fidl::ServerEnd<fuchsia_audio_device::RingBuffer> server_end,
      const std::shared_ptr<ControlServer>& parent,
      const std::shared_ptr<Device>& device_to_control, ElementId element_id);

  // PacketStream support
  std::shared_ptr<PacketStreamServer> CreatePacketStreamServer(
      fidl::ServerEnd<fuchsia_audio_device::PacketStream> server_end,
      const std::shared_ptr<ControlServer>& parent,
      const std::shared_ptr<Device>& device_to_control, ElementId element_id);

 private:
  static constexpr std::string_view kClassName = "AudioDeviceRegistry";

  void DeviceDetected(std::string_view name, fuchsia_audio_device::DeviceType device_type,
                      fuchsia_audio_device::DriverClient driver_client);
  void MaybeNotifyInitialDevicesAvailable();
  void NotifyRegistriesOfDeviceRemoval(TokenId removed_device_id);

  std::shared_ptr<DeviceDetector> device_detector_;

  // These devices are in the process of being initialized. A device should never be in both sets.
  std::unordered_set<std::shared_ptr<Device>> initial_pending_devices_;
  std::unordered_set<std::shared_ptr<Device>> pending_devices_;
  bool initial_device_detection_complete_ = false;
  bool all_initial_devices_ready_ = false;

  // The list of operational devices, provided to clients (via Registry/WatchDevicesAdded).
  std::unordered_set<std::shared_ptr<Device>> devices_;
  // These devices have encountered an error or have self-reported as unhealthy. They have been
  // reported (via Registry/WatchDeviceRemoved) as being removed, and are no longer in the device
  // list provided to clients (via Registry/WatchDevicesAdded).
  // Once one of these devices is actually removed, it is deleted from this set.
  std::unordered_set<std::shared_ptr<Device>> unhealthy_devices_;

  std::shared_ptr<FidlThread> thread_;
  component::OutgoingDirectory outgoing_;

  VectorOfWeakPtr<RegistryServer> registries_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_AUDIO_DEVICE_REGISTRY_H_
